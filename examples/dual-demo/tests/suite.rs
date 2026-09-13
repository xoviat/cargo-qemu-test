//! One suite, two harnesses, written once (see Cargo.toml).
//!
//! The `#[qemu_test::tests]` attribute expands this module into:
//!   * a `cfg(target_os = "none")` embedded-test module (run by `cargo qtest` in QEMU), and
//!   * a std-host twin plus a libtest-mimic `main` (run by plain `cargo test`).
#![cfg_attr(target_os = "none", no_std, no_main)]

#[qemu_test::tests]
mod tests {
    use dual_demo::{add, CASES};

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
