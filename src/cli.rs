use std::ffi::OsString;
use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "cargo-qtest",
    bin_name = "cargo qtest",
    version,
    about = "Run embedded Rust test suites (embedded-test harness) under QEMU on a cross target",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Target triple to build for and emulate in QEMU.
    #[arg(long, default_value = "thumbv7em-none-eabihf")]
    pub target: String,

    /// Build tests in release mode.
    #[arg(long, short)]
    pub release: bool,

    /// Space or comma separated list of cargo features to activate.
    #[arg(long, value_delimiter = ',')]
    pub features: Vec<String>,

    #[arg(long)]
    pub all_features: bool,

    #[arg(long)]
    pub no_default_features: bool,

    /// Only build the integration test target with this name.
    #[arg(long)]
    pub test: Option<String>,

    /// Package to run tests for (useful in workspaces).
    #[arg(long, short)]
    pub package: Option<String>,

    /// Path to the qemu-system-* binary (defaults are derived from `--target`).
    #[arg(long)]
    pub qemu: Option<PathBuf>,

    /// QEMU machine (-M), overriding the target default.
    #[arg(long)]
    pub machine: Option<String>,

    /// QEMU CPU (-cpu), overriding the target default.
    #[arg(long)]
    pub cpu: Option<String>,

    /// Extra raw argument passed through to QEMU. Can be repeated.
    #[arg(long = "qemu-arg", value_name = "ARG")]
    pub qemu_args: Vec<String>,

    /// Per-test timeout in seconds; QEMU is killed when a test runs longer.
    #[arg(long, default_value_t = 60)]
    pub timeout: u64,

    /// Print the exact QEMU command line of every test before running it.
    #[arg(long, short)]
    pub verbose: bool,

    /// Do not inject linker scripts (-Tlink.x / -Tembedded-test.x) and a default
    /// memory.x; use this if your build.rs already handles them.
    #[arg(long)]
    pub no_linker_fixup: bool,
}

/// Split argv into plugin args and test-harness args.
///
/// `cargo qtest ...` invokes this binary as `cargo-qtest qtest ...`, so the leading
/// `qtest` word is stripped. Everything after a literal `--` is forwarded to the
/// test harness (libtest-mimic): filters, `--ignored`, `--format json`, ...
pub fn split_argv() -> (Vec<OsString>, Vec<OsString>) {
    let mut args = std::env::args_os().skip(1).peekable();
    if args.peek().map(|a| a == "qtest").unwrap_or(false) {
        args.next();
    }
    let mut cli_args = Vec::new();
    let mut harness_args = Vec::new();
    let mut after_ddash = false;
    for arg in args {
        if after_ddash {
            harness_args.push(arg);
        } else if arg == "--" {
            after_ddash = true;
        } else {
            cli_args.push(arg);
        }
    }
    (cli_args, harness_args)
}
