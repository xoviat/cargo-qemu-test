//! Library internals of `cargo-qtest`; the binary in `src/main.rs` is a thin
//! wrapper so the pipeline stages are unit-testable.

pub mod cli;
pub mod compiler;
pub mod defmt;
pub mod download;
pub mod elf;
pub mod qemu;
