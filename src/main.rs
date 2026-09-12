use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Parser;

mod cli;
mod compiler;
mod elf;
mod qemu;

fn main() -> Result<()> {
    let (cli_args, harness_args) = cli::split_argv();
    let args = cli::Cli::parse_from(std::iter::once(OsString::from("cargo-qtest")).chain(cli_args));

    let artifacts = compiler::build_test_artifacts(&args)?;

    let defaults = qemu::qemu_for_target(&args.target);
    let binary = args
        .qemu
        .clone()
        .or_else(|| defaults.map(|d| PathBuf::from(d.system)))
        .with_context(|| format!("don't know which QEMU binary fits target `{}` (pass --qemu)", args.target))?;
    let machine = args
        .machine
        .clone()
        .or_else(|| defaults.map(|d| d.machine.to_string()))
        .with_context(|| format!("don't know which QEMU machine fits target `{}` (pass --machine)", args.target))?;
    let cpu = args
        .cpu
        .clone()
        .or_else(|| defaults.and_then(|d| d.cpu.map(String::from)));

    let qemu_opts = qemu::QemuOptions {
        binary,
        machine,
        cpu,
        board_args: qemu::default_board_args(&args.target),
        extra_args: args.qemu_args.clone(),
        timeout: Duration::from_secs(args.timeout),
        verbose: args.verbose,
    };

    let mut trials: Vec<libtest_mimic::Trial> = Vec::new();
    let mut skipped_binaries = Vec::new();

    for artifact in &artifacts {
        let discovered = elf::read_embedded_tests(&artifact.executable)
            .with_context(|| format!("failed to inspect test binary {}", artifact.executable.display()))?;
        let Some(tests) = discovered else {
            skipped_binaries.push(artifact);
            continue;
        };
        if args.verbose || artifacts.len() > 1 {
            eprintln!(
                "qtest: {}: {} test(s) in `{}`",
                artifact.target_name,
                tests.len(),
                artifact.executable.display()
            );
        }
        for test in tests {
            let name = test.name.clone();
            let ignored = test.ignored;
            let closure = {
                let opts = qemu_opts.clone();
                let kernel = artifact.executable.clone();
                move || run_one_test(&opts, &kernel, test)
            };
            trials.push(libtest_mimic::Trial::test(name, closure).with_ignored_flag(ignored));
        }
    }

    for artifact in &skipped_binaries {
        eprintln!(
            "qtest: warning: `{}` is not an embedded-test binary (missing EMBEDDED_TEST_VERSION), skipping it",
            artifact.executable.display()
        );
    }

    if trials.is_empty() {
        bail!("no embedded-test test cases found in any test executable");
    }

    trials.sort_by(|a, b| a.name().cmp(b.name()));

    let mut mimic_argv = vec![OsString::from("cargo-qtest")];
    mimic_argv.extend(harness_args);
    let mimic_args = libtest_mimic::Arguments::from_iter(mimic_argv);

    libtest_mimic::run(&mimic_args, trials).exit();
}

/// Run a single test case. A fresh QEMU instance acts as a device reset; the
/// firmware's semihosting exit code decides the outcome. `should_panic` is
/// inverted here because libtest-mimic 0.8 leaves it to the runner.
fn run_one_test(
    opts: &qemu::QemuOptions,
    kernel: &Path,
    test: elf::EmbeddedTest,
) -> Result<(), libtest_mimic::Failed> {
    let outcome = qemu::run_test_in_qemu(opts, kernel, test.entrypoint);
    if test.should_panic {
        match outcome {
            Ok(()) => Err(libtest_mimic::Failed::from(
                "test was expected to panic but exited successfully",
            )),
            Err(_) => Ok(()),
        }
    } else {
        outcome
    }
}
