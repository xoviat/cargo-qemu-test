#![no_std]
#![no_main]

// Force cortex-m-rt's vector table object into the link: rlib members are only
// pulled when referenced, and nothing else in a `no_main` test crate references it.
use cortex_m_rt as _;

// The defmt global logger for the *unit test* build of this lib (`cfg(test)`
// is off when the lib is built as a dependency of the integration test, which
// has its own `use defmt_semihosting as _;`, so exactly one logger is linked
// into each test binary).
#[cfg(test)]
use defmt_semihosting as _;

// The critical-section backend (single-core Cortex-M) that defmt-semihosting's
// encoder requires, and defmt's mandatory timestamp provider, for the unit
// test binary only.
#[cfg(test)]
use cortex_m as _;
#[cfg(test)]
defmt::timestamp!("{=u64:us}", 0);

pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

pub fn read_sensor() -> u16 {
    1234
}

/// Non-inlined wrapper so the flush call can't be const-folded or reordered
/// away; the failing assertion below must come *after* the logs have drained.
#[cfg(test)]
#[inline(never)]
fn flush_defmt() {
    defmt::flush();
}

#[cfg(test)]
#[embedded_test::tests]
mod tests {
    use super::*;

    #[init]
    fn init() {}

    #[test]
    fn sensor_is_positive() {
        defmt::info!("reading sensor");
        assert!(read_sensor() > 0);
    }

    #[test]
    fn logs_before_failing() {
        defmt::warn!("sensor value {=u16} out of range", read_sensor());
        defmt::error!("about to fail");
        flush_defmt();
        assert_eq!(read_sensor(), 42);
    }
}
