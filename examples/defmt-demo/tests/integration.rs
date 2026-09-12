#![no_std]
#![no_main]

use cortex_m as _;
use defmt_semihosting as _;

defmt::timestamp!("{=u64:us}", 0);

/// Non-inlined wrapper so the flush call survives optimization and runs
/// before the failing assertion below it.
#[inline(never)]
fn flush_defmt() {
    defmt::flush();
}

#[embedded_test::tests]
mod tests {
    use super::flush_defmt;

    #[init]
    fn init() {}

    #[test]
    fn basic_math_with_logs() {
        defmt::info!("basic math: {=i32} + {=i32}", 20, 22);
        assert_eq!(defmt_demo::add(20, 22), 42);
    }

    #[test]
    fn integration_logs_before_failing() {
        defmt::info!("integration test starting");
        defmt::warn!("value {=u16} out of range", 1234u16);
        defmt::error!("about to fail");
        flush_defmt();
        assert_eq!(defmt_demo::add(1, 1), 3);
    }
}
