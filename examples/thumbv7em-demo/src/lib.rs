#![no_std]
#![no_main]

// Force cortex-m-rt's vector table object into the link: rlib members are only
// pulled when referenced, and nothing else in a `no_main` test crate references it.
use cortex_m_rt as _;

pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
#[embedded_test::tests]
mod tests {
    use super::*;

    #[init]
    fn init() {}

    #[test]
    fn adds_correctly() {
        assert_eq!(add(2, 2), 4);
    }

    #[test]
    fn fails_on_bug() {
        assert_eq!(add(i32::MAX, 1), i32::MAX); // overflow wraps -> fails
    }

    #[test]
    #[should_panic]
    fn panics_as_expected() {
        assert_eq!(add(i32::MAX, 1), i32::MAX);
    }

    #[test]
    #[ignore]
    fn ignored_example() {}
}
