use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use libtest_mimic::Failed;

/// QEMU defaults for a target triple.
#[derive(Debug, Clone, Copy)]
pub struct QemuTarget {
    pub system: &'static str,
    pub machine: &'static str,
    pub cpu: Option<&'static str>,
}

/// Built-in QEMU selection for common embedded targets.
pub fn qemu_for_target(target: &str) -> Option<QemuTarget> {
    Some(match target {
        "thumbv6m-none-eabi" | "thumbv7m-none-eabi" | "thumbv7m-none-eabihf" => QemuTarget {
            system: "qemu-system-arm",
            machine: "mps2-an385",
            cpu: Some("cortex-m3"),
        },
        "thumbv7em-none-eabi" | "thumbv7em-none-eabihf" => QemuTarget {
            system: "qemu-system-arm",
            machine: "mps2-an386",
            cpu: Some("cortex-m4"),
        },
        "thumbv8m.main-none-eabi" | "thumbv8m.main-none-eabihf" => QemuTarget {
            system: "qemu-system-arm",
            machine: "mps2-an505",
            cpu: Some("cortex-m33"),
        },
        "aarch64-unknown-none" | "aarch64-unknown-none-softfloat" => QemuTarget {
            system: "qemu-system-aarch64",
            machine: "virt",
            cpu: Some("cortex-a53"),
        },
        "riscv32imac-unknown-none-elf" | "riscv32imc-unknown-none-elf" => QemuTarget {
            system: "qemu-system-riscv32",
            machine: "virt",
            cpu: None,
        },
        "riscv64gc-unknown-none-elf" | "riscv64imac-unknown-none-elf" => QemuTarget {
            system: "qemu-system-riscv64",
            machine: "virt",
            cpu: None,
        },
        _ => return None,
    })
}

/// Extra QEMU arguments implied by the machine defaults (must match the
/// generated linker scripts, e.g. RAM size).
pub fn default_board_args(target: &str) -> Vec<String> {
    if target.starts_with("riscv") {
        // Direct kernel boot: no OpenSBI, and 128 MiB to match the generated link script.
        vec!["-bios".into(), "none".into(), "-m".into(), "128M".into()]
    } else {
        Vec::new()
    }
}

/// Everything needed to launch QEMU for one test case.
#[derive(Debug, Clone)]
pub struct QemuOptions {
    pub binary: PathBuf,
    pub machine: String,
    pub cpu: Option<String>,
    pub board_args: Vec<String>,
    pub extra_args: Vec<String>,
    pub timeout: Duration,
    pub verbose: bool,
}

/// Run one embedded-test test case in a fresh QEMU instance.
///
/// One QEMU boot equals one device reset: QEMU answers the firmware's semihosting
/// `SYS_GET_CMDLINE` with `run_addr <entrypoint>`, the firmware runs the test and
/// reports the outcome via `SYS_EXIT(_EXTENDED)`, which QEMU forwards as its own
/// process exit code (0 = success, non-zero = failure/abort).
pub fn run_test_in_qemu(opts: &QemuOptions, kernel: &Path, entrypoint: u64) -> Result<(), Failed> {
    let mut cmd = Command::new(&opts.binary);
    cmd.arg("-M").arg(&opts.machine);
    if let Some(cpu) = &opts.cpu {
        cmd.arg("-cpu").arg(cpu);
    }
    cmd.args(&opts.board_args);
    cmd.arg("-semihosting-config")
        .arg(format!(
            "enable=on,target=native,arg=run_addr,arg={entrypoint}"
        ))
        .arg("-nographic")
        .arg("-kernel")
        .arg(kernel)
        .args(&opts.extra_args);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if opts.verbose {
        eprintln!("qtest: `{cmd:?}`");
    }

    let mut child = cmd.spawn().map_err(|e| {
        Failed::from(format!(
            "failed to spawn `{}`: {e}. Is QEMU installed? Use --qemu <path> if it is not in PATH.",
            opts.binary.display()
        ))
    })?;

    // Capture QEMU's stdio (serial + semihosting console) so it cannot deadlock,
    // and show the tail of it whenever a test fails.
    let output = Arc::new(Mutex::new(Vec::<u8>::new()));
    let streams: Vec<Box<dyn std::io::Read + Send>> = vec![
        Box::new(child.stdout.take().expect("stdout is piped")),
        Box::new(child.stderr.take().expect("stderr is piped")),
    ];
    for stream in streams {
        let sink = Arc::clone(&output);
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = [0u8; 4096];
            let mut reader = stream;
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => sink.lock().unwrap().extend_from_slice(&buf[..n]),
                }
            }
        });
    }

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() > opts.timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Failed::from(format!(
                        "timed out after {}s (adjust with --timeout){}",
                        opts.timeout.as_secs(),
                        output_tail(&output.lock().unwrap())
                    )));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(Failed::from(format!("error waiting for qemu: {e}")));
            }
        }
    };

    // Give the reader threads a moment to flush.
    std::thread::sleep(Duration::from_millis(20));
    let output = output.lock().unwrap().clone();

    match status.code() {
        Some(0) => Ok(()),
        Some(code) => Err(Failed::from(format!(
            "firmware exited with status {code}{}",
            output_tail(&output)
        ))),
        None => Err(Failed::from(
            "qemu terminated by signal (guest crashed at boot -- check vector table, target and machine setup)",
        )),
    }
}

fn output_tail(output: &[u8]) -> String {
    const MAX_BYTES: usize = 4000;
    const MAX_LINES: usize = 20;
    let start = output.len().saturating_sub(MAX_BYTES);
    let text = String::from_utf8_lossy(&output[start..]);
    let lines: Vec<&str> = text.lines().collect();
    let tail = if lines.len() > MAX_LINES {
        &lines[lines.len() - MAX_LINES..]
    } else {
        &lines[..]
    };
    if tail.is_empty() {
        String::new()
    } else {
        format!(
            "\n--- qemu output (tail) ---\n{}\n--- end ---",
            tail.join("\n")
        )
    }
}
