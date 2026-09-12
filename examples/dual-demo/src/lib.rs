//! Test cases shared between host (`cargo test`) and target (`cargo qtest`).
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use cortex_m_rt as _;

/// Test cases shared by both harnesses: (a, b, expected a + b).
pub const CASES: &[(i32, i32, i32)] = &[(1, 2, 3), (20, 22, 42), (-5, 5, 0)];

pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

// --- embedded unit tests (cargo qtest runs these in QEMU) ------------------
#[cfg(all(test, target_os = "none"))]
#[embedded_test::tests]
mod embedded_unit_tests {
    use super::*;

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

// --- host: harness=false builds the lib test crate as a bin; keep it quiet.
#[cfg(all(test, not(target_os = "none")))]
fn main() {}
