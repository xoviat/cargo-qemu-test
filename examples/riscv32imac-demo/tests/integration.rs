#![no_std]
#![no_main]

#[embedded_test::tests]
mod tests {
    #[init]
    fn init() {}

    #[test]
    fn cross_target_math() {
        assert_eq!(riscv32imac_demo::mul2(3), 6);
    }
}
