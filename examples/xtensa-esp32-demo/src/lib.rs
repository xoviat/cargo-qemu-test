//! The xtensa-esp32 counterpart of dual-demo: one suite, two harnesses.
//!
//! Build firmware with the Espressif toolchain and run under QEMU:
//!   espup install --targets esp32
//!   cargo +esp qtest --target xtensa-esp32-none-elf \
//!       --qemu /path/to/qemu-system-xtensa
//! (QEMU must be espressif's fork with docs/patches/0001 applied.)
#![cfg_attr(target_os = "none", no_std, no_main)]

#[cfg(target_os = "none")]
use xtensa_lx_rt as _;

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
