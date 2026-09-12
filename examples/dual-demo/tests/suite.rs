//! One suite, two harnesses (see Cargo.toml).
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

use dual_demo::{add, CASES};

pub fn check_cases() {
    for &(a, b, sum) in CASES {
        assert_eq!(add(a, b), sum);
    }
}

#[cfg(target_os = "none")]
#[embedded_test::tests]
mod embedded {
    use super::*;

    #[init]
    fn init() {}

    #[test]
    fn shared_cases() {
        check_cases();
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

// --- host: drive the same checks through libtest-mimic ----------------------
#[cfg(not(target_os = "none"))]
fn overflow_body() {
    assert_eq!(add(i32::MAX, 1), i32::MAX);
}

#[cfg(not(target_os = "none"))]
fn main() {
    let trials = vec![
        libtest_mimic::Trial::test("shared_cases", || {
            check_cases();
            Ok(())
        }),
        // libtest-mimic 0.8 has no should_panic flag: invert it ourselves.
        libtest_mimic::Trial::test("overflow_fails", || {
            match std::panic::catch_unwind(overflow_body) {
                Ok(()) => Err(libtest_mimic::Failed::from(
                    "overflow_fails was expected to panic but did not",
                )),
                Err(_) => Ok(()),
            }
        }),
    ];
    libtest_mimic::run(&libtest_mimic::Arguments::from_args(), trials).exit();
}
