//! Test cases and unit tests written once via `qemu-test`.
//!
//! `cargo test`  (std host)  -> the macro's libtest-mimic harness runs these.
//! `cargo qtest` (QEMU/no_std) -> the macro's embedded-test harness runs these.
#![cfg_attr(target_os = "none", no_std, no_main)]

#[cfg(target_os = "none")]
use cortex_m_rt as _;

/// Test cases shared by both harnesses: (a, b, expected a + b).
pub const CASES: &[(i32, i32, i32)] = &[(1, 2, 3), (20, 22, 42), (-5, 5, 0)];

pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
#[qemu_test::tests]
mod embedded_unit_tests {
    use super::*;

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
