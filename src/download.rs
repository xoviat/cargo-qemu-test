//! Resolution of the QEMU binary used to run tests, plus on-demand execution
//! of the bundled download scripts (`scripts/download-qemu.sh` and
//! `scripts/download-qemu.ps1`) that fetch and cache a prebuilt
//! `qemu-system-xtensa` binary.
//!
//! All download, archive-extraction and host-platform detection logic lives in
//! those scripts, which ship with this crate and are embedded at compile time.
//! The Rust code only launches a script when the user explicitly requests a
//! download (`cargo qtest --download`).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// Bash download script distributed with the crate (`scripts/download-qemu.sh`).
const DOWNLOAD_SH: &str = include_str!("../scripts/download-qemu.sh");

/// PowerShell download script distributed with the crate
/// (`scripts/download-qemu.ps1`).
const DOWNLOAD_PS1: &str = include_str!("../scripts/download-qemu.ps1");

/// Returns the user cache directory for `cargo-qtest`.
///
/// Priority:
/// 1. `CARGO_QTEST_CACHE_DIR` environment variable
/// 2. Windows: `%LOCALAPPDATA%\cargo-qtest`
/// 3. macOS: `$HOME/Library/Caches/cargo-qtest`
/// 4. Linux / other Unix: `$XDG_CACHE_HOME/cargo-qtest` or `$HOME/.cache/cargo-qtest`
/// 5. System temporary directory: `std::env::temp_dir()/cargo-qtest`
///
/// The resolved directory is passed to the download scripts so that lookups
/// and downloads always agree on one location.
pub fn cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CARGO_QTEST_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(appdata).join("cargo-qtest");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        #[cfg(target_os = "macos")]
        return PathBuf::from(home)
            .join("Library")
            .join("Caches")
            .join("cargo-qtest");
        #[cfg(not(target_os = "macos"))]
        {
            if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
                return PathBuf::from(xdg).join("cargo-qtest");
            }
            return PathBuf::from(home).join(".cache").join("cargo-qtest");
        }
    }
    std::env::temp_dir().join("cargo-qtest")
}

/// Checks if a QEMU binary is runnable and supports a specific machine model.
pub fn qemu_supports_machine(binary: &Path, machine: &str) -> bool {
    let output = match Command::new(binary).arg("-M").arg("help").output() {
        Ok(out) => out,
        Err(e) => {
            eprintln!("qtest: failed to execute `{}`: {e}", binary.display());
            return false;
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        eprintln!(
            "qtest: `{}` -M help failed with exit code {:?}\nstderr: {stderr}\nstdout: {stdout}",
            binary.display(),
            output.status.code()
        );
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().any(|line| {
        let first_word = line.split_whitespace().next().unwrap_or("");
        first_word == machine
    })
}

/// Checks if a binary exists in PATH or on the filesystem.
pub fn binary_exists(binary: &Path) -> bool {
    if binary.is_absolute() || binary.components().count() > 1 {
        return binary.exists();
    }
    matches!(
        Command::new(binary).arg("--version").output(),
        Ok(status) if status.status.success()
    )
}

/// Looks for a cached QEMU executable in the specified directory.
pub fn find_cached_qemu_in(binary_name: &str, cdir: &Path) -> Option<PathBuf> {
    let exe_suffix = std::env::consts::EXE_SUFFIX;
    let full_name = if binary_name.ends_with(exe_suffix) {
        binary_name.to_string()
    } else {
        format!("{binary_name}{exe_suffix}")
    };

    // Look in cdir/qemu/bin (standard archive extract path)
    let path1 = cdir.join("qemu").join("bin").join(&full_name);
    if path1.is_file() {
        return Some(path1);
    }

    // Look in cdir/bin
    let path2 = cdir.join("bin").join(&full_name);
    if path2.is_file() {
        return Some(path2);
    }

    // Look in cdir root
    let path3 = cdir.join(&full_name);
    if path3.is_file() {
        return Some(path3);
    }

    None
}

/// Looks for a cached QEMU executable in the user's cache directory.
pub fn find_cached_qemu(binary_name: &str) -> Option<PathBuf> {
    find_cached_qemu_in(binary_name, &cache_dir())
}

/// Writes the embedded download script for this platform into `cdir` and runs
/// it, returning the path of the extracted QEMU binary.
///
/// The scripts handle platform-triplet detection, download (curl/wget or
/// PowerShell), extraction, cache lookup, machine verification via
/// `qemu -M help`, and (on Unix) installation of missing system runtime
/// dependencies. They print progress on stderr and the resulting binary path
/// on stdout (last line); stderr is inherited so the user sees progress live.
pub fn run_download_script(
    binary_name: &str,
    machine: &str,
    cdir: &Path,
    force: bool,
    verbose: bool,
) -> Result<PathBuf> {
    std::fs::create_dir_all(cdir)
        .with_context(|| format!("failed to create cache directory at {}", cdir.display()))?;

    let mut cmd = if cfg!(windows) {
        let script = cdir.join("download-qemu.ps1");
        std::fs::write(&script, DOWNLOAD_PS1)
            .with_context(|| format!("failed to write {}", script.display()))?;
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
        cmd.arg(&script);
        cmd.args(["-Binary", binary_name]);
        cmd.args(["-CacheDir", &cdir.to_string_lossy()]);
        cmd.args(["-Machine", machine]);
        if force {
            cmd.arg("-Force");
        }
        if verbose {
            cmd.arg("-VerboseOutput");
        }
        cmd
    } else {
        let script = cdir.join("download-qemu.sh");
        std::fs::write(&script, DOWNLOAD_SH)
            .with_context(|| format!("failed to write {}", script.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&script) {
                let mut perms = meta.permissions();
                perms.set_mode(perms.mode() | 0o755);
                let _ = std::fs::set_permissions(&script, perms);
            }
        }
        let mut cmd = Command::new("bash");
        cmd.arg(&script);
        cmd.args(["--binary", binary_name]);
        cmd.args(["--cache-dir", &cdir.to_string_lossy()]);
        cmd.args(["--machine", machine]);
        if force {
            cmd.arg("--force");
        }
        if verbose {
            cmd.arg("--verbose");
        }
        cmd
    };

    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("failed to run the bundled QEMU download script with {cmd:?}"))?;
    let output = child
        .wait_with_output()
        .context("failed while waiting for the QEMU download script")?;

    if !output.status.success() {
        bail!(
            "the QEMU download script failed with exit code {:?}. \
             You can also run it yourself (scripts/download-qemu.sh / scripts/download-qemu.ps1) \
             or pass an existing binary via --qemu.",
            output.status.code()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let reported = stdout
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .with_context(|| {
            format!(
                "the QEMU download script did not report a valid binary path (stdout: {stdout:?})"
            )
        })?;
    Ok(reported)
}

/// Resolves and ensures a working QEMU binary for the given target and machine.
///
/// If the binary exists on PATH and supports the target machine, it is returned as-is.
/// For Xtensa/Espressif targets a cached copy is used when it works. A
/// prebuilt binary is downloaded (by running the bundled script) only when the
/// user explicitly requested a download via `download = true`
/// (`cargo qtest --download`); otherwise a helpful error explains the options.
/// Machine verification and system dependency installation happen inside the
/// script, not in Rust.
pub fn ensure_qemu_binary(
    binary: &Path,
    machine: &str,
    target: &str,
    download: bool,
    verbose: bool,
) -> Result<PathBuf> {
    // 1. If user passed an explicit file path (with directory components):
    if binary.is_absolute() || binary.components().count() > 1 {
        if !binary.exists() {
            bail!("specified QEMU binary does not exist: {}", binary.display());
        }
        return Ok(binary.to_path_buf());
    }

    let binary_name = binary.to_string_lossy();

    // 2. Check if the binary in PATH already works and supports the machine:
    if binary_exists(binary) {
        if qemu_supports_machine(binary, machine) {
            return Ok(binary.to_path_buf());
        }
        if verbose {
            eprintln!(
                "qtest: binary `{}` found in PATH but does not support machine `{machine}`",
                binary.display()
            );
        }
    }

    // 3. If target is Xtensa (or machine is esp32*), check the cache; download
    //    via the bundled script only when the user requested it:
    let is_espressif = target.starts_with("xtensa") || machine.starts_with("esp32");
    if is_espressif && binary_name.contains("xtensa") {
        let cached = find_cached_qemu(&binary_name);
        if let Some(path) = &cached
            && qemu_supports_machine(path, machine)
        {
            if verbose {
                eprintln!("qtest: using cached QEMU binary at {}", path.display());
            }
            return Ok(path.clone());
        }

        if !download {
            return Err(match &cached {
                Some(path) => anyhow::anyhow!(
                    "the cached QEMU binary at {} does not support machine `{machine}`.\n\
                     Pass --download to re-check it and re-download if needed (runs the bundled download script, which also installs missing system runtime dependencies), \
                     run scripts/download-qemu.sh (or scripts/download-qemu.ps1) yourself, \
                     or pass an existing binary via --qemu.",
                    path.display()
                ),
                None => anyhow::anyhow!(
                    "`{binary_name}` was not found on PATH or in the cache directory {}.\n\
                     Pass --download to fetch a prebuilt binary (runs the bundled download script, which also installs missing system runtime dependencies), \
                     run scripts/download-qemu.sh (or scripts/download-qemu.ps1) yourself, \
                     or pass an existing binary via --qemu.",
                    cache_dir().display()
                ),
            });
        }

        // The user requested a download: run the bundled script, which repairs
        // the cache (machine check + system dependencies) and only downloads a
        // fresh copy when the cached one is still unusable.
        let downloaded = run_download_script(&binary_name, machine, &cache_dir(), false, verbose)?;
        if !qemu_supports_machine(&downloaded, machine) {
            bail!(
                "downloaded QEMU binary at {} does not support machine `{machine}`",
                downloaded.display()
            );
        }
        return Ok(downloaded);
    }

    // 4. For non-Xtensa targets (e.g. ARM, RISC-V), provide clear install instructions:
    if !binary_exists(binary) {
        let help = match std::env::consts::OS {
            "linux" => format!(
                "install via: sudo apt install {binary_name} (or your distro package manager)"
            ),
            "macos" => "install via: brew install qemu".to_string(),
            "windows" => {
                "install via: winget install SoftwareFreedomConservancy.QEMU or choco install qemu"
                    .to_string()
            }
            _ => "please install QEMU for your operating system".to_string(),
        };
        bail!("QEMU binary `{binary_name}` was not found in PATH.\n{help}");
    }

    Ok(binary.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_dir() {
        let cdir = cache_dir();
        assert!(cdir.to_string_lossy().contains("cargo-qtest"));
    }

    #[test]
    fn test_find_cached_qemu_missing() {
        let missing = find_cached_qemu("non_existent_binary_xyz123");
        assert!(missing.is_none());
    }

    #[test]
    fn test_ensure_qemu_binary_missing_explicit_path() {
        let path = Path::new("/path/to/nonexistent/qemu-system-test");
        let res = ensure_qemu_binary(path, "virt", "riscv32-unknown-none-elf", false, false);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_ensure_qemu_binary_missing_non_xtensa() {
        let binary = Path::new("qemu-system-nonexistent-12345");
        let res = ensure_qemu_binary(binary, "virt", "thumbv7em-none-eabihf", false, false);
        assert!(res.is_err());
        let err = res.unwrap_err().to_string();
        assert!(err.contains("was not found in PATH"));
    }

    #[test]
    fn test_ensure_qemu_binary_xtensa_requires_download_request() {
        // Without --download, a missing xtensa binary must never trigger the
        // download script: it errors with instructions instead.
        let binary = Path::new("qemu-system-xtensa-nonexistent-12345");
        let res = ensure_qemu_binary(binary, "esp32", "xtensa-esp32-none-elf", false, false);
        assert!(res.is_err());
        let err = res.unwrap_err().to_string();
        assert!(
            err.contains("--download"),
            "error should mention --download: {err}"
        );
        assert!(
            err.contains("was not found"),
            "error should explain absence: {err}"
        );
    }

    #[test]
    fn test_qemu_supports_machine_with_system_binary() {
        let binary = Path::new("qemu-system-xtensa");
        if binary_exists(binary) {
            assert!(qemu_supports_machine(binary, "esp32"));
            assert!(qemu_supports_machine(binary, "esp32s3"));
            assert!(!qemu_supports_machine(binary, "nonexistent-machine-xyz"));
        }
    }

    #[test]
    #[ignore = "requires network access to download prebuilt QEMU archive from GitHub"]
    fn test_run_download_script() {
        let temp_cache = std::env::temp_dir().join("cargo-qtest-test-download");
        let _ = std::fs::remove_dir_all(&temp_cache);
        let res = run_download_script("qemu-system-xtensa", "esp32", &temp_cache, false, true);
        assert!(res.is_ok(), "download should succeed: {:?}", res.err());
        let bin = res.unwrap();
        assert!(bin.exists(), "extracted binary should exist");
        assert!(
            qemu_supports_machine(&bin, "esp32"),
            "binary must support esp32"
        );
        assert!(
            qemu_supports_machine(&bin, "esp32s3"),
            "binary must support esp32s3"
        );
        let _ = std::fs::remove_dir_all(&temp_cache);
    }

    #[test]
    fn test_embedded_scripts_are_distributed() {
        // The scripts ship with the crate and are embedded at compile time.
        assert!(
            DOWNLOAD_SH.contains("download"),
            "bash script content missing"
        );
        assert!(
            DOWNLOAD_PS1.contains("download"),
            "powershell script content missing"
        );
        assert!(DOWNLOAD_SH.starts_with("#!/usr/bin/env bash"));
        // System dependency installation lives in the scripts, not in Rust.
        assert!(
            DOWNLOAD_SH.contains("apt-get"),
            "bash script must install system deps"
        );
        assert!(DOWNLOAD_SH.contains("CARGO_QTEST_NO_AUTO_DEPS"));
    }
}
