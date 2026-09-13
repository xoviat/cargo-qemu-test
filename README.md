# cargo-qtest

Run embedded Rust test suites — built with the [`embedded-test`](https://crates.io/crates/embedded-test)
harness — under **QEMU** on a cross target, with a plain `cargo qtest` command.

```
$ cargo qtest
running 6 tests
test integration::tests::basic_math            ... ok
test integration::tests::result_err_is_failure ... ok
test thumbv7em_demo::tests::adds_correctly     ... ok
test thumbv7em_demo::tests::fails_on_bug       ... FAILED
test thumbv7em_demo::tests::ignored_example    ... ignored
test thumbv7em_demo::tests::panics_as_expected ... ok
test result: FAILED. 4 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out
```

## How it works

1. `cargo test --no-run --target <triple> --message-format=json` cross-compiles every
   `harness = false` test target and `cargo-qtest` collects the ELF executables.
2. Each ELF is scanned for embedded-test's `.embedded_test` metadata section: JSON-encoded
   symbol names carry `{name, ignored, should_panic}`; each test's entrypoint address is
   taken from its `<module>::__<name>_entrypoint` function symbol (the 12-byte records the
   metadata symbols point at are `INFO`-section bytes and are *not* reliable in a linked
   executable, so the symbol table is used instead).
3. Every test case is run in a **fresh QEMU instance** (one boot = one device reset). QEMU
   answers the firmware's semihosting `SYS_GET_CMDLINE` with `run_addr <address>`
   (`-semihosting-config ... arg=run_addr,arg=<address>`); the firmware runs the test and
   reports the outcome via the semihosting exit code, which QEMU forwards as its process
   exit status. `should_panic` tests are inverted by the runner.
4. Results are reported through [`libtest-mimic`](https://crates.io/crates/libtest-mimic),
   so filters, `--ignored`/`--include-ignored`, `--test-threads`, `--format json`, `--list`
   and friends behave like normal `cargo test`.
5. When a test fails and the ELF contains a `.defmt` section (firmware logs with
   defmt over the semihosting console, e.g. via `defmt-semihosting`), the captured
   output is decoded with [`defmt-decoder`](https://crates.io/crates/defmt-decoder)
   into readable log lines; plain-text output passes through untouched. Use
   `--defmt`/`--no-defmt` to force or disable this per run.

## Installation

```console
$ cargo install --path .          # from this repository
$ rustup target add thumbv7em-none-eabihf
# plus a QEMU with the right system emulator, e.g.:
$ sudo apt install qemu-system-arm
```

## Examples

| Example | Target(s) | Demonstrates |
|---|---|---|
| `examples/thumbv7em-demo` | `thumbv7em-none-eabihf` | classic Cortex-M setup; `fails_on_bug` intentionally fails |
| `examples/defmt-demo` | `thumbv7em-none-eabihf` | defmt logs over the semihosting console, decoded on failure |
| `examples/dual-demo` | host **and** `thumbv7em-none-eabihf` | the `qemu-test` macro: one `#[qemu_test::tests]` module run by both `cargo test` (std host) and `cargo qtest` (no_std, QEMU) |
| `examples/riscv32imac-demo` | `riscv32imac-unknown-none-elf` | hand-rolled `_start`, no runtime crates; direct `qemu-system-riscv32 -bios none` boot |

See `examples/thumbv7em-demo` for a complete working Cortex-M project.

## Setting up your firmware crate

`cargo qtest` **handles the linker setup for you**. It inspects your dependency graph
(`cargo metadata`) and, when it finds `cortex-m-rt` and/or `embedded-test`, injects
`--config target.<triple>.rustflags=[...]` into the build:

* `-Tlink.x` — cortex-m-rt's own build script only publishes a link-*search* path; the
  script argument itself must come from somewhere, and without it there is no vector
  table and the guest locks up at reset;
* `-Tembedded-test.x` — the `embedded-test-linker-script` crate provides it;
* a default `memory.x` matching the QEMU machine (MPS2-AN3x5/6: FLASH @ `0x0` 4M,
  RAM @ `0x20000000` 4M) when your crate has no `memory.x` at its package root, plus a
  link-search path to the package root so a root-level `memory.x` is always found.

Pass `--no-linker-fixup` if your `build.rs` already passes these scripts itself (keep
`-Tlink.x` *before* `-Tembedded-test.x` in that case).

So a minimal firmware crate is just dependencies plus `harness = false`:

`Cargo.toml`:

```toml
[dependencies]
embedded-test = "0.7"     # pulls in embedded-test-linker-script for embedded-test.x
cortex-m-rt   = "0.7"     # vector table / reset handler (Cortex-M)

[lib]
harness = false           # unit tests inside src/lib.rs

[[test]]
name = "integration"
harness = false
```

If you link for real hardware with a custom memory layout, keep a `memory.x` at the
package root and `cargo qtest` will pick it up. Note that plain `cargo build`/`cargo test`
(without `cargo qtest`) still needs the usual cortex-m-rt `build.rs` setup.

A test file (`tests/integration.rs`):

```rust
#![no_std]
#![no_main]

#[embedded_test::tests]
mod tests {
    #[init]
    fn init() {}

    #[test]
    fn basic_math() {
        assert_eq!(my_crate::add(20, 22), 42);
    }

    #[test]
    #[should_panic]
    fn it_panics() { panic!("boom"); }

    #[test]
    #[ignore]
    fn slow() {}
}
```

Unit tests in `src/lib.rs` work too: wrap a `#[cfg(test)] #[embedded_test::tests] mod tests { ... }`
inside the lib (the `harness = false` on `[lib]` makes cargo compile it as the test binary).

## Usage

```console
$ cargo qtest                                  # default target: thumbv7em-none-eabihf
$ cargo qtest --target thumbv7em-none-eabi --release
$ cargo qtest --features foo,bar --test integration
$ cargo qtest -- --ignored                     # harness args after `--`
$ cargo qtest -- --format json                 # IDE-friendly output
$ cargo qtest --test-threads 1                 # serial QEMU runs
```

| Flag | Meaning |
|---|---|
| `--target <TRIPLE>` | cross target (default `thumbv7em-none-eabihf`) |
| `--release` | build tests in release mode |
| `--features` / `--all-features` / `--no-default-features` | feature selection |
| `--test <NAME>` / `--package <SPEC>` | restrict which cargo targets get built |
| `--qemu <PATH>` | QEMU binary override (default derived from `--target`) |
| `--machine <M>` / `--cpu <C>` | QEMU `-M` / `-cpu` overrides |
| `--qemu-arg <ARG>` | raw extra argument passed to QEMU (repeatable) |
| `--timeout <SEC>` | per-test kill timeout (default 60) |
| `--defmt` / `--no-defmt` | force (or disable) defmt decoding of failed-test output; default: auto-detect from the ELF's `.defmt` section |
| `--verbose` | print each QEMU command line |

Built-in target defaults:

| Target | QEMU | machine | cpu |
|---|---|---|---|
| `thumbv6m-none-eabi`, `thumbv7m-*` | `qemu-system-arm` | `mps2-an385` | `cortex-m3` |
| `thumbv7em-*` | `qemu-system-arm` | `mps2-an386` | `cortex-m4` |
| `thumbv8m.main-*` | `qemu-system-arm` | `mps2-an505` | `cortex-m33` |
| `aarch64-unknown-none*` | `qemu-system-aarch64` | `virt` | `cortex-a53` |
| `riscv32imac/…` | `qemu-system-riscv32` | `virt` | — |
| `riscv64gc/…` | `qemu-system-riscv64` | `virt` | — |

## Troubleshooting

* **`qemu terminated by signal (guest crashed at boot …)`** — almost always a missing
  vector table: make sure `-Tlink.x` is passed *before* `-Tembedded-test.x` in build.rs,
  and verify with `readelf -S <elf> | grep vector_table`.
* **Bare-metal RISC-V** needs no runtime crate: `cargo qtest` generates a minimal
  linker script (RAM @ `0x80000000`, 128 MiB, `ENTRY(_start)`) and boots QEMU with
  `-bios none -m 128M`. Your crate provides `_start` (see `examples/riscv32imac-demo`).
* **Timeouts on every test** — the firmware isn't calling the semihosting exit: check the
  machine/cpu match your target (e.g. hard-float code on an FPU-less CPU).
* **`cargo qtest` says "not an embedded-test binary"** — that test target lacks the
  `EMBEDDED_TEST_VERSION` symbol; did you set `harness = false` and use
  `#[embedded_test::tests]`?
* **defmt logs** — probe-based transports (RTT, ITM) don't work under QEMU, but
  `defmt-semihosting` does: its encoded frames arrive on the semihosting console
  and `cargo qtest` decodes them with `defmt-decoder` whenever a test fails
  (auto-detected from the ELF's `.defmt` section; `--defmt`/`--no-defmt` to force).
  Plain-text output still passes through as-is. See `examples/defmt-demo`.

  Firmware requirements for decoding (all demonstrated in `examples/defmt-demo`):

  - a timestamp provider (`defmt::timestamp!("{=u64:us}", expr)`) — defmt
    requires exactly one per binary, else the link fails with an undefined
    `_defmt_timestamp`;
  - a `critical-section` backend (on single-core Cortex-M: `cortex-m` with the
    `critical-section-single-core` feature, referenced via `use cortex_m as _;`)
    — required by `defmt-semihosting`'s encoder;
  - a linker script with `KEEP(*(.defmt .defmt.*))` in a `.defmt` output
    section (see `examples/defmt-demo/defmt.x`). defmt-macros emits one input
    section per interned string (`.defmt.<tag>.<json>`, 1 byte each); without
    the merge+keep, rust-lld's `--gc-sections` drops them or leaves orphans
    that don't match the decoder's exact `.defmt` section lookup, yielding
    "defmt version found, but no `.defmt` section";
  - exactly one defmt major in the graph — e.g. don't pair `defmt = "0.3"` with
    `defmt-semihosting 0.3.0` (which depends on defmt `1.x`): two defmt
    versions in one image is rejected with "multiple defmt versions in use".

## One suite, two harnesses: `qemu-test` (`examples/dual-demo`)

Writing the same tests twice — once wrapped in `#[embedded_test::tests]` for QEMU and
once behind a hand-rolled libtest-mimic `main` for the host — means every test body,
every `should_panic`, every `#[ignore]` carries a handwritten twin. The `qemu-test`
crate (workspace members `qemu-test` + `qemu-test-macros`) removes that: you write one
module, and the `#[qemu_test::tests]` attribute expands it into two cfg-gated twins
plus a host harness:

```rust
#![cfg_attr(target_os = "none", no_std, no_main)]

#[qemu_test::tests]
mod tests {
    use my_fw::{add, CASES};

    #[init]
    fn init() {}

    #[test]
    fn shared_cases() {
        for &(a, b, sum) in CASES {
            assert_eq!(add(a, b), sum);
        }
    }

    #[test]
    #[should_panic]
    fn overflow_fails() {
        assert_eq!(add(i32::MAX, 1), i32::MAX);
    }

    #[test]
    #[ignore]
    fn skipped_example() {}
}
```

* On `no_std` targets the module is passed through **verbatim** to
  `#[embedded_test::tests]`, so `cargo qtest` discovers and runs it exactly as before
  (fresh device boot per test, semihosting exit codes, defmt decoding — all unchanged).
* On std hosts the same functions become `pub(crate)` items driven by a generated
  libtest-mimic `main`, so plain `cargo test` gives genuine libtest-style output —
  with the same trial names (`tests::shared_cases`) and the same `--ignored`,
  `--format json`, filter flags on both sides.
* `#[init]` runs before every test on both platforms; `#[should_panic(expected = "...")]`
  is honored on the host too (matched against the panic payload, like std's libtest);
  tests may return `Result<(), E>`.
* `#[host_only]` / `#[target_only]` restrict a test to one platform when it genuinely
  can't be portable (e.g. poking registers vs. using `std::fs`).

The only setup left to you is the part Cargo enforces: `harness = false` on the
`[lib]`/`[[test]]` targets (the harness is chosen per target, not per platform) and the
firmware runtime as a target-gated regular dependency (cortex-m-rt must be a regular,
not dev, dependency because the lib itself uses it when compiled as a dependency of the
test targets). Everything harness-related — including the target-gated
`embedded-test`/`libtest-mimic` dependencies — lives behind the `qemu-test` facade, and
the facade pins the libtest-mimic version to the one `cargo-qtest` uses.

Rules: one `#[qemu_test::tests]` module per test crate (each generates the host
`main` — nest plain `mod`s inside it to organize); in `src/lib.rs` gate the module with
`#[cfg(test)]`; test bodies must be `no_std`-portable unless gated.

## Notes & limitations

* Supports the embedded-test 0.7.x protocol (metadata symbols + `__name_entrypoint`
  functions + semihosting `SYS_GET_CMDLINE`/`SYS_EXIT`).
* `#[timeout]` attributes are not read from the ELF (the per-test `--timeout` applies to
  all tests).
* Test-name collisions across modules are detected and reported, but only one entrypoint
  can be used — rename one of the tests.