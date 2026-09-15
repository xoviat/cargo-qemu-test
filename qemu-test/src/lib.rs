//! Write embedded tests once, run them in two places.
//!
//! The crate is `no_std` on bare-metal targets (only the embedded-test
//! re-export is compiled there); on std hosts it behaves like a normal crate.
#![cfg_attr(target_os = "none", no_std)]
//!
//! `qemu-test` lets one `#[qemu_test::tests]` module run:
//!
//! * on a **std host** via `cargo test` — a [`libtest-mimic`] harness with
//!   libtest-compatible output (`--ignored`, `--format json`, filters, ...), and
//! * under **QEMU on a `no_std` target** via [`cargo-qtest`][cargo-qtest] — the
//!   [`embedded-test`] harness, one fresh device boot per test.
//!
//! ```rust,ignore
//! #![cfg_attr(target_os = "none", no_std, no_main)]
//!
//! #[qemu_test::tests]
//! mod tests {
//!     use my_fw::{add, CASES};
//!
//!     #[init]
//!     fn init() {}
//!
//!     #[test]
//!     fn shared_cases() {
//!         for &(a, b, sum) in CASES {
//!             assert_eq!(add(a, b), sum);
//!         }
//!     }
//!
//!     #[test]
//!     #[should_panic]
//!     fn overflow_fails() {
//!         assert_eq!(add(i32::MAX, 1), i32::MAX);
//!     }
//!
//!     #[test]
//!     #[ignore]
//!     fn slow() {}
//! }
//! ```
//!
//! The attribute expands to two `cfg(target_os = "none")`-gated twins of the
//! module plus a host `main`; see `qemu-test-macros` for the full expansion.
//!
//! # Setup (the irreducible boilerplate)
//!
//! Cargo chooses a harness per target, not per platform, so both sides opt out
//! of libtest:
//!
//! ```toml
//! [lib]
//! harness = false            # if you want unit tests inside src/lib.rs
//!
//! [[test]]
//! name = "suite"
//! harness = false
//!
//! [dev-dependencies]
//! qemu-test = "0.1"
//! ```
//!
//! In `src/lib.rs`, gate the module with `#[cfg(test)]`; integration test files
//! need `#![cfg_attr(target_os = "none", no_std, no_main)]` at the top. Your
//! firmware runtime (e.g. `cortex-m-rt`) stays a target-gated *regular*
//! dependency. Neither harness needs a direct dependency: `embedded-test` and
//! `libtest-mimic` are both re-exported through this facade (and pinned to the
//! versions `cargo-qtest` was tested against).
//!
//! # Rules and escape hatches
//!
//! * **One `#[qemu_test::tests]` module per test crate** — each generates the
//!   host `fn main`. Organize large suites with ordinary nested `mod`s inside it.
//! * **`#[host_only]` / `#[target_only]`** restrict a test to one platform when
//!   it truly cannot be portable.
//! * Test functions may return `()` or `Result<(), E>` (with `#[should_panic]`,
//!   only `()` — same restriction as std's libtest).
//! * `#[test(init = path)]` overrides the module `#[init]` for one test
//!   (embedded-test 0.7 semantics; the host harness calls the same path).
//! * Everything else in the module (`use`, helpers, consts, `#[cfg]`-gated
//!   items) is passed through to both platforms unchanged — keep it portable or
//!   gate it yourself, as usual for `no_std` code.
//!
//! # Semantic notes
//!
//! * On the target, each test runs in a **fresh device boot**; on the host all
//!   trials share one process and libtest-mimic runs them in parallel threads by
//!   default (`--test-threads 1` exists on both sides if a test needs it).
//! * `#[should_panic(expected = "...")]` matches the panic payload as a string
//!   substring, like std's libtest.
//!
//! [cargo-qtest]: https://github.com/xoviat/cargo-qemu-test
//! [`embedded-test`]: https://crates.io/crates/embedded-test
//! [`libtest-mimic`]: https://crates.io/crates/libtest-mimic

pub use qemu_test_macros::tests;

/// Re-export of the embedded-test harness used on `no_std` targets. The proc
/// macro invokes `#[::qemu_test::embedded_tests]` and injects
/// `use ::qemu_test as embedded_test;`, so both the attribute and every path
/// in embedded-test's macro expansion resolve through this facade — a direct
/// `embedded-test` dependency is not required, and the macro/runtime versions
/// are pinned to the one declared here.
#[cfg(target_os = "none")]
pub use embedded_test::tests as embedded_tests;

/// Everything else embedded-test exposes (`export::hosting::*`, `TestOutcome`,
/// ...), re-exported so the expansion's relative `embedded_test::...` paths
/// keep working through the alias above.
#[cfg(target_os = "none")]
pub use embedded_test::*;

/// Re-export of libtest-mimic used by macro-generated code on std hosts. This
/// pins the harness version to the one `cargo-qtest` uses, so CLI behavior
/// (`--ignored`, `--format json`, ...) is identical on host and target.
/// Not public API in the semver sense.
#[cfg(not(target_os = "none"))]
pub use libtest_mimic;

/// Pull in cortex-m-rt for the rt feature
#[cfg(all(feature = "rt", target_os = "none", target_arch = "arm"))]
use cortex_m_rt as _;

/// Add startup asm for riscv for the rt feature
#[cfg(all(feature = "rt", target_os = "none", target_arch = "riscv32"))]
core::arch::global_asm!(
    r#"
    .section .text._start, "ax", @progbits
    .globl _start
    .align 2
_start:
    .option push
    .option norelax
    la sp, _stack_top
    .option pop
    call main
1:  j 1b
"#
);
