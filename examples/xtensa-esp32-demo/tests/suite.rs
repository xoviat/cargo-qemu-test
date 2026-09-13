//! Integration suite: identical shape to dual-demo's, for the ESP32 target.
#![cfg_attr(target_os = "none", no_std, no_main)]

#[qemu_test::tests]
mod tests {
    use xtensa_esp32_demo::{add, CASES};

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
