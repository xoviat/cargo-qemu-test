#![no_std]
#![no_main]

#[embedded_test::tests]
mod tests {
    #[init]
    fn init() {}

    #[test]
    fn basic_math() {
        assert_eq!(thumbv7em_demo::add(20, 22), 42);
    }

    #[test]
    #[should_panic]
    fn result_err_is_failure() -> Result<(), &'static str> {
        Err("deliberate failure inside should_panic")
    }
}
