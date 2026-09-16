use std::ffi::OsString;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use cargo_qemu_test::cli::{self, Cli};
use cargo_qemu_test::{compiler, defmt, elf, qemu};
use clap::Parser;

fn main() -> Result<()> {
    let (cli_args, harness_args) = cli::split_argv();
    let args = Cli::parse_from(std::iter::once(OsString::from("cargo-qtest")).chain(cli_args));

    let artifacts = compiler::build_test_artifacts(&args)?;

    let defaults = qemu::qemu_for_target(&args.target);
    let binary = args
        .qemu
        .clone()
        .or_else(|| defaults.map(|d| d.system.into()))
        .with_context(|| {
            format!(
                "don't know which QEMU binary fits target `{}` (pass --qemu)",
                args.target
            )
        })?;
    let machine = args
        .machine
        .clone()
        .or_else(|| defaults.map(|d| d.machine.to_string()))
        .with_context(|| {
            format!(
                "don't know which QEMU machine fits target `{}` (pass --machine)",
                args.target
            )
        })?;
    let cpu = args
        .cpu
        .clone()
        .or_else(|| defaults.and_then(|d| d.cpu.map(String::from)));

    let binary = cargo_qemu_test::download::ensure_qemu_binary(
        &binary,
        &machine,
        &args.target,
        args.verbose,
    )?;

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
        let discovered = elf::read_embedded_tests(&artifact.executable).with_context(|| {
            format!(
                "failed to inspect test binary {}",
                artifact.executable.display()
            )
        })?;
        let Some(tests) = discovered else {
            skipped_binaries.push(artifact);
            continue;
        };
        let defmt_info = defmt::resolve(&args, &artifact.executable)?;
        if args.verbose || artifacts.len() > 1 {
            eprintln!(
                "qtest: {}: {} test(s) in `{}`{}",
                artifact.target_name,
                tests.len(),
                artifact.executable.display(),
                if defmt_info.is_some() {
                    " (defmt output will be decoded on failure)"
                } else {
                    ""
                }
            );
        }
        for test in tests {
            let name = test.name.clone();
            let ignored = test.ignored;
            let closure = {
                let opts = qemu_opts.clone();
                let kernel = artifact.executable.clone();
                let defmt_info = defmt_info.clone();
                move || qemu::run_one_test(&opts, &kernel, &test, defmt_info.as_ref())
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
