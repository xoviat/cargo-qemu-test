#![no_std]
#![no_main]

// QEMU's riscv32 virt machine loads the ELF and starts at its entry point, so we
// provide a tiny startup: set the stack pointer and hand over to embedded-test's
// `main` (which performs the semihosting run_addr handshake).
use core::arch::global_asm;

global_asm!(
    r#"
    .section .text._start, "ax", @progbits
    .globl _start
    .align 2
_start:
    .option push
    .option norelax
    la sp, _stack_top
    .option pop
    call main
1:  j 1b
"#
);

extern "C" {
    fn main() -> !;
}

pub fn mul2(x: i32) -> i32 {
    x * 2
}

#[cfg(test)]
#[embedded_test::tests]
mod tests {
    use super::*;

    #[init]
    fn init() {}

    #[test]
    fn doubles() {
        assert_eq!(mul2(21), 42);
    }

    #[test]
    #[should_panic]
    fn overflow_panics() {
        assert_eq!(mul2(i32::MAX), i32::MAX);
    }

    #[test]
    #[ignore]
    fn ignored_on_riscv() {}
}
