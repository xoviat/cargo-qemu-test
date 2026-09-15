use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use libtest_mimic::Failed;

use crate::defmt::DefmtInfo;
use crate::elf::EmbeddedTest;

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
        "thumbv8m.base-none-eabi" | "thumbv8m.main-none-eabi" | "thumbv8m.main-none-eabihf" => {
            QemuTarget {
                system: "qemu-system-arm",
                machine: "mps2-an505",
                cpu: Some("cortex-m33"),
            }
        }
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
        // Xtensa: requires the Espressif QEMU fork (esp-develop), patched for
        // OpenOCD-style semihosting -- see docs/installing-qemu.md#xtensa.
        "xtensa-esp32-none-elf" => QemuTarget {
            system: "qemu-system-xtensa",
            machine: "esp32",
            cpu: None,
        },
        "xtensa-esp32s2-none-elf" => QemuTarget {
            system: "qemu-system-xtensa",
            machine: "esp32s2",
            cpu: None,
        },
        "xtensa-esp32s3-none-elf" => QemuTarget {
            system: "qemu-system-xtensa",
            machine: "esp32s3",
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
fn parse_qemu_version(text: &str) -> Option<(u32, u32)> {
    let tail = text.split("version ").nth(1)?;
    let (maj, rest) = tail.split_once('.')?;
    let min: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    Some((maj.parse().ok()?, min.parse().ok()?))
}

/// Parse `<qemu-binary> --version` into `(major, minor)`.
fn qemu_version(binary: &Path) -> Option<(u32, u32)> {
    let out = std::process::Command::new(binary)
        .arg("--version")
        .output()
        .ok()?;
    parse_qemu_version(&String::from_utf8_lossy(&out.stdout))
}

/// Minimum `qemu-system-arm` for the TZ MPS2 boards (`mps2-an505`/`an519`/`an521`).
///
/// Older releases use a different SSE-200 memory map (the SSRAM banks moved in
/// QEMU 8.0) and are not supported; this matrix is verified against QEMU 8.2.x,
/// the version `apt install qemu-system-arm` provides on Ubuntu 24.04 /
/// GitHub Actions `ubuntu-latest`.
const MIN_MPS2TZ_QEMU: (u32, u32) = (8, 0);

pub fn run_test_in_qemu(
    opts: &QemuOptions,
    kernel: &Path,
    entrypoint: u64,
    defmt: Option<&DefmtInfo>,
) -> Result<(), Failed> {
    if opts.machine.starts_with("mps2-an5") {
        let ver = qemu_version(&opts.binary).unwrap_or((0, 0));
        if ver < MIN_MPS2TZ_QEMU {
            return Err(format!( "{} is too old for {} (found {}.{}, need >= {}.{}): the SSE-200 memory map differs before QEMU 8.0. Install qemu-system-arm >= 8.0 (see README: `apt install qemu-system-arm` on Ubuntu 24.04 / GitHub Actions `ubuntu-latest`).", opts.binary.display(), opts.machine, ver.0, ver.1, MIN_MPS2TZ_QEMU.0, MIN_MPS2TZ_QEMU.1, ) .into());
        }
    }
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

    // QEMU <= 7.x mps2-tz machines latch the initial SP/PC before -kernel is
    // loaded ("Loaded reset SP 0x0 PC 0x0 from vector table"), so the CPU
    // starts executing zeroed memory and lockups. Work around: start halted
    // (-S) and drive a monitor socket with system_reset (re-latches SP/PC from
    // the now-loaded vector table) followed by cont.
    let tz_reset = opts.machine.starts_with("mps2-an5");
    // Use a TCP monitor socket (loopback, ephemeral port): it works on Unix
    // and Windows alike, unlike a unix-domain socket.
    let mon_port: Option<u16> = if tz_reset {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| {
            Failed::from(format!("failed to reserve a monitor port for QEMU: {e}"))
        })?;
        let port = listener.local_addr().map_err(|e| {
            Failed::from(format!("failed to query monitor port for QEMU: {e}"))
        })?.port();
        // Release the port so QEMU can re-bind it; the retry loop in the
        // handshake below tolerates the brief race.
        drop(listener);
        cmd.arg("-S")
            .arg("-monitor")
            .arg(format!("tcp:127.0.0.1:{port},server,nowait"));
        Some(port)
    } else {
        None
    };

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

    if let Some(port) = mon_port {
        use std::io::{Read, Write};
        let mut last_err = String::new();
        for attempt in 0..5 {
            let mut s = match std::net::TcpStream::connect(("127.0.0.1", port)) {
                Ok(s) => s,
                Err(e) => {
                    last_err = format!("connect: {e}");
                    std::thread::sleep(Duration::from_millis(150));
                    continue;
                }
            };
            let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
            let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
            if let Err(e) = s.write_all(b"system_reset\n") {
                last_err = format!("write system_reset: {e}");
                continue;
            }
            std::thread::sleep(Duration::from_millis(400));
            if let Err(e) = s.write_all(b"cont\n") {
                last_err = format!("write cont: {e}");
                continue;
            }
            std::thread::sleep(Duration::from_millis(200));
            if s.write_all(b"info status\n").is_err() {
                last_err = "write info status".into();
                continue;
            }
            let mut buf = [0u8; 1024];
            let mut got = Vec::new();
            for _ in 0..10 {
                match s.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        got.extend_from_slice(&buf[..n]);
                        if got.len() > 64 {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let reply = String::from_utf8_lossy(&got).to_lowercase();
            if reply.contains("running") {
                last_err.clear();
                break;
            }
            last_err = format!("unexpected status reply: {reply:?}");
        }
        if !last_err.is_empty() {
            eprintln!("cargo-qtest: monitor handshake failed: {last_err}");
        }
    }

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
                        output_report(&output.lock().unwrap(), defmt)
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
            output_report(&output, defmt)
        ))),
        None => Err(Failed::from(
            "qemu terminated by signal (guest crashed at boot -- check vector table, target and machine setup)",
        )),
    }
}

/// Render the captured QEMU output shown when a test fails: decoded defmt log
/// lines when the firmware uses defmt (the bytes on the semihosting console are
/// encoded frames), otherwise the raw text tail.
fn output_report(output: &[u8], defmt: Option<&DefmtInfo>) -> String {
    const MAX_LINES: usize = 20;
    let lines: Vec<String> = match defmt {
        Some(info) => info.decode_output(output),
        None => {
            const MAX_BYTES: usize = 4000;
            let start = output.len().saturating_sub(MAX_BYTES);
            String::from_utf8_lossy(&output[start..])
                .lines()
                .map(str::to_owned)
                .collect()
        }
    };
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

/// Run a single test case. A fresh QEMU instance acts as a device reset; the
/// firmware's semihosting exit code decides the outcome. `should_panic` is
/// inverted here because libtest-mimic 0.8 leaves it to the runner.
pub fn run_one_test(
    opts: &QemuOptions,
    kernel: &Path,
    test: &EmbeddedTest,
    defmt: Option<&DefmtInfo>,
) -> Result<(), libtest_mimic::Failed> {
    let outcome = run_test_in_qemu(opts, kernel, test.entrypoint, defmt);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qemu_version_parses() {
        // format: "QEMU emulator version 8.2.2 (Debian 1:8.2.2+dfsg-0ubuntu1)"
        assert_eq!(
            parse_qemu_version("QEMU emulator version 8.2.2 (Debian ...)"),
            Some((8, 2))
        );
        assert_eq!(parse_qemu_version("QEMU emulator version 9.0.0"), Some((9, 0)));
        assert_eq!(parse_qemu_version("no version here"), None);
    }

    #[test]
    fn xtensa_targets_use_espressif_qemu() {
        for (triple, machine) in [
            ("xtensa-esp32-none-elf", "esp32"),
            ("xtensa-esp32s2-none-elf", "esp32s2"),
            ("xtensa-esp32s3-none-elf", "esp32s3"),
        ] {
            let t = qemu_for_target(triple).unwrap_or_else(|| panic!("{triple}"));
            assert_eq!(t.system, "qemu-system-xtensa");
            assert_eq!(t.machine, machine);
            assert_eq!(t.cpu, None);
        }
    }

    #[test]
    fn arm_targets_still_resolve() {
        let t = qemu_for_target("thumbv7em-none-eabihf").unwrap();
        assert_eq!(t.system, "qemu-system-arm");
        assert_eq!(t.machine, "mps2-an386");

        // thumbv8m triples contain dots; both profiles share the an505/M33 board.
        for triple in ["thumbv8m.base-none-eabi", "thumbv8m.main-none-eabihf"] {
            let t = qemu_for_target(triple).unwrap();
            assert_eq!(t.system, "qemu-system-arm");
            assert_eq!(t.machine, "mps2-an505");
            assert_eq!(t.cpu.as_deref(), Some("cortex-m33"));
        }
    }

    #[test]
    fn unknown_target_returns_none() {
        assert!(qemu_for_target("bpf-unknown-none").is_none());
    }

    #[test]
    fn xtensa_needs_no_board_args() {
        assert!(default_board_args("xtensa-esp32-none-elf").is_empty());
        assert_eq!(
            default_board_args("riscv32imc-unknown-none-elf"),
            ["-bios", "none", "-m", "128M"]
        );
    }
}
