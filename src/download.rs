//! Resolution of the QEMU binary used to run tests, plus on-demand fetching
//! and execution of the platform download script (`download-qemu.sh` on Unix,
//! `download-qemu.ps1` on Windows) that fetches and caches a prebuilt
//! `qemu-system-xtensa` binary.
//!
//! The scripts are not bundled with this crate: when the user explicitly
//! requests a download (`cargo qtest --download`), the script for the current
//! platform is fetched over HTTPS from the latest `kokroo/qemu` GitHub
//! release (the patched QEMU fork the binary comes from) and written into the
//! cache directory. All download, archive-extraction and host-platform
//! detection logic lives in those scripts; the Rust code only fetches and
//! launches one.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// Base URL of the download scripts published in the latest `kokroo/qemu`
/// GitHub release (the patched QEMU fork the prebuilt binary comes from).
const SCRIPT_BASE_URL: &str = "https://github.com/kokroo/qemu/releases/latest/download";

/// Name of the download script for the current platform.
fn script_file_name() -> &'static str {
    if cfg!(windows) {
        "download-qemu.ps1"
    } else {
        "download-qemu.sh"
    }
}

/// URL the download script is fetched from: `CARGO_QTEST_SCRIPT_URL` when set
/// (mirror or pinned release), otherwise the latest-release asset.
fn script_url() -> String {
    match std::env::var("CARGO_QTEST_SCRIPT_URL") {
        Ok(url) if !url.trim().is_empty() => url,
        _ => format!("{SCRIPT_BASE_URL}/{}", script_file_name()),
    }
}

/// Minimal sanity check that a fetched body is actually one of our scripts
/// rather than an HTML error page from a mirror or CDN.
fn looks_like_script(body: &str) -> bool {
    let head = body.trim_start();
    (head.starts_with("#!") || head.starts_with("<#")) && body.contains("download")
}

/// Downloads the platform download script over HTTPS and writes it into
/// `cdir`, returning the path of the written script.
///
/// If the fetch fails but a previously fetched copy exists in `cdir`, that
/// copy is reused (with a warning) so a transient network problem does not
/// break an otherwise usable cache.
pub fn fetch_download_script(cdir: &Path) -> Result<PathBuf> {
    let url = script_url();
    let path = cdir.join(script_file_name());

    let fetched = (|| -> Result<String> {
        let mut response = ureq::get(&url)
            .header(
                "User-Agent",
                "cargo-qtest (https://github.com/xoviat/cargo-qemu-test)",
            )
            .call()
            .with_context(|| format!("failed to fetch the download script from {url}"))?;
        response
            .body_mut()
            .read_to_string()
            .with_context(|| format!("failed to read the download script returned by {url}"))
    })();

    let script = match fetched {
        Ok(body) if looks_like_script(&body) => body,
        Ok(body) => bail!(
            "the response from {url} does not look like the QEMU download script \
             ({} bytes); refusing to run it",
            body.len()
        ),
        Err(err) => {
            if path.is_file() {
                eprintln!(
                    "qtest: could not fetch the download script ({err:#}); \
                     reusing the existing copy at {}",
                    path.display()
                );
                return Ok(path);
            }
            bail!(
                "could not download the QEMU download script from {url}: {err:#}. \
                 Download `download-qemu.sh` (Unix) or `download-qemu.ps1` (Windows) \
                 from that release and place it at {} manually.",
                path.display()
            );
        }
    };

    std::fs::write(&path, script).with_context(|| format!("failed to write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&path) {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() | 0o755);
            let _ = std::fs::set_permissions(&path, perms);
        }
    }
    Ok(path)
}

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

/// Fetches the download script for this platform from the latest
/// `kokroo/qemu` GitHub release, writes it into `cdir` and runs it,
/// returning the path of the extracted QEMU binary.
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

    // The script is not bundled with the crate: fetch it from the latest
    // kokroo/qemu release before running it.
    let script = fetch_download_script(cdir)?;

    let mut cmd = if cfg!(windows) {
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
        .with_context(|| format!("failed to run the QEMU download script with {cmd:?}"))?;
    let output = child
        .wait_with_output()
        .context("failed while waiting for the QEMU download script")?;

    if !output.status.success() {
        bail!(
            "the QEMU download script failed with exit code {:?}. \
             You can also download and run it yourself ({}) \
             or pass an existing binary via --qemu.",
            output.status.code(),
            script_url()
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
/// prebuilt binary is downloaded (by fetching the script from the latest
/// `kokroo/qemu` release and running it) only when the
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
    //    via the release script only when the user requested it:
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
                     Pass --download to re-check it and re-download if needed (fetches the download script from the latest kokroo/qemu release, which also installs missing system runtime dependencies), \
                     or download and run that script yourself ({}) \
                     or pass an existing binary via --qemu.",
                    path.display(),
                    script_url()
                ),
                None => anyhow::anyhow!(
                    "`{binary_name}` was not found on PATH or in the cache directory {}.\n\
                     Pass --download to fetch a prebuilt binary (fetches the download script from the latest kokroo/qemu release, which also installs missing system runtime dependencies), \
                     or download and run that script yourself ({}) \
                     or pass an existing binary via --qemu.",
                    cache_dir().display(),
                    script_url()
                ),
            });
        }

        // The user requested a download: fetch the script from the latest
        // kokroo/qemu release and run it, which repairs the cache (machine
        // check + system dependencies) and only downloads a fresh copy when
        // the cached one is still unusable.
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
    fn test_script_url_points_at_latest_release() {
        assert_eq!(
            script_file_name(),
            if cfg!(windows) {
                "download-qemu.ps1"
            } else {
                "download-qemu.sh"
            }
        );
        assert!(
            SCRIPT_BASE_URL.starts_with("https://github.com/kokroo/qemu/releases/latest/download"),
            "scripts must be fetched from the latest kokroo/qemu release: {SCRIPT_BASE_URL}"
        );
    }

    #[test]
    fn test_looks_like_script() {
        assert!(looks_like_script(
            "#!/usr/bin/env bash\n# download\necho hi\n"
        ));
        assert!(looks_like_script("<#\ndownload\n#>\n"));
        assert!(!looks_like_script(
            "<html><body>404 Not Found</body></html>"
        ));
        assert!(!looks_like_script(""));
    }

    #[test]
    #[ignore = "requires network access to the latest kokroo/qemu GitHub release"]
    fn test_fetch_download_script() {
        let temp_cache = std::env::temp_dir().join("cargo-qtest-test-fetch-script");
        let _ = std::fs::remove_dir_all(&temp_cache);
        std::fs::create_dir_all(&temp_cache).unwrap();
        let script =
            fetch_download_script(&temp_cache).expect("download script fetch should succeed");
        let body = std::fs::read_to_string(&script).expect("script should be readable");
        assert!(looks_like_script(&body), "fetched script looks wrong");
        let _ = std::fs::remove_dir_all(&temp_cache);
    }
}
