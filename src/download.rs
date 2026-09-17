//! Automatic download and caching of prebuilt QEMU binaries (specifically
//! `qemu-system-xtensa` with semihosting and Espressif chip support).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Default release tag on `kokroo/qemu` containing multi-platform prebuilt binaries.
pub const DEFAULT_QEMU_TAG: &str = "v0.2.0-semihosting";

/// Returns the user cache directory for `cargo-qtest`.
///
/// Priority:
/// 1. `CARGO_QTEST_CACHE_DIR` environment variable
/// 2. Windows: `%LOCALAPPDATA%\cargo-qtest`
/// 3. macOS: `$HOME/Library/Caches/cargo-qtest`
/// 4. Linux / other Unix: `$XDG_CACHE_HOME/cargo-qtest` or `$HOME/.cache/cargo-qtest`
/// 5. System temporary directory: `std::env::temp_dir()/cargo-qtest`
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

/// Returns the native host architecture, correctly detecting ARM64 on Windows
/// even if running under Prism / WOW64 emulation.
pub fn host_arch() -> &'static str {
    #[cfg(windows)]
    {
        // If compiled as x86_64 but running on Windows 11 ARM64 under Prism/WOW64 emulation,
        // detect the native host architecture so we download the native ARM64 QEMU binary.
        if let Ok(arch) = std::env::var("RUNNER_ARCH") {
            if arch.eq_ignore_ascii_case("ARM64") {
                return "aarch64";
            }
        }
        if let Ok(arch) = std::env::var("PROCESSOR_ARCHITEW6432") {
            if arch.eq_ignore_ascii_case("ARM64") {
                return "aarch64";
            }
        }
        if let Ok(arch) = std::env::var("PROCESSOR_ARCHITECTURE") {
            if arch.eq_ignore_ascii_case("ARM64") {
                return "aarch64";
            }
        }

        // Query Win32 IsWow64Process2 API (the definitive Windows API for host machine architecture under WOW64/Prism)
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
            fn IsWow64Process2(
                hProcess: *mut std::ffi::c_void,
                pProcessMachine: *mut u16,
                pNativeMachine: *mut u16,
            ) -> i32;
        }
        let mut process_machine = 0u16;
        let mut native_machine = 0u16;
        unsafe {
            if IsWow64Process2(
                GetCurrentProcess(),
                &mut process_machine,
                &mut native_machine,
            ) != 0
            {
                // IMAGE_FILE_MACHINE_ARM64 = 0xAA64
                if native_machine == 0xAA64 {
                    return "aarch64";
                }
                // IMAGE_FILE_MACHINE_AMD64 = 0x8664
                if native_machine == 0x8664 {
                    return "x86_64";
                }
            }
        }
    }

    std::env::consts::ARCH
}

/// Detects the current host platform triplet for prebuilt QEMU releases.
pub fn host_platform_triplet() -> Result<&'static str> {
    let os = std::env::consts::OS;
    let arch = host_arch();
    match (os, arch) {
        ("linux", "x86_64") => Ok("x86_64-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-linux-gnu"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("windows", "x86_64") => Ok("x86_64-w64-mingw32"),
        ("windows", "aarch64") => Ok("aarch64-w64-mingw32"),
        _ => bail!(
            "unsupported host platform ({os}-{arch}) for automatic QEMU download. Please provide a custom QEMU binary via --qemu."
        ),
    }
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

/// Downloads a file from `url` to `dest` using curl, wget, or powershell.
fn download_file(url: &str, dest: &Path) -> Result<()> {
    // 1. Try curl (available on Linux, macOS, and Windows 10/11)
    let status = Command::new("curl")
        .arg("-sSL")
        .arg("-f")
        .arg(url)
        .arg("-o")
        .arg(dest)
        .status();

    if matches!(status, Ok(s) if s.success()) {
        return Ok(());
    }

    // 2. On Windows, try PowerShell WebClient
    #[cfg(windows)]
    {
        let ps_cmd = format!(
            "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; (New-Object Net.WebClient).DownloadFile('{}', '{}')",
            url,
            dest.display()
        );
        let ps_status = Command::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(&ps_cmd)
            .status();
        if matches!(ps_status, Ok(s) if s.success()) {
            return Ok(());
        }
    }

    // 3. On Unix, try wget
    #[cfg(unix)]
    {
        let wget_status = Command::new("wget")
            .arg("-q")
            .arg("-O")
            .arg(dest)
            .arg(url)
            .status();
        if matches!(wget_status, Ok(s) if s.success()) {
            return Ok(());
        }
    }

    bail!(
        "failed to download {url}. Ensure an active internet connection and that `curl` or `wget` is installed."
    );
}

/// Extracts a `.tar.xz` or `.tar.gz` archive to `dest_dir` using system `tar`.
fn extract_tar_archive(archive: &Path, dest_dir: &Path) -> Result<()> {
    let status = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dest_dir)
        .status();

    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => bail!(
            "tar failed to extract {} (exit code {:?})",
            archive.display(),
            s.code()
        ),
        Err(e) => bail!(
            "failed to execute `tar` to extract {}: {e}",
            archive.display()
        ),
    }
}

/// Downloads and extracts the prebuilt QEMU Xtensa archive for the current host platform to `cdir`.
pub fn download_and_extract_qemu_to(
    binary_name: &str,
    cdir: &Path,
    verbose: bool,
) -> Result<PathBuf> {
    let triplet = host_platform_triplet()?;
    let tag =
        std::env::var("CARGO_QTEST_QEMU_TAG").unwrap_or_else(|_| DEFAULT_QEMU_TAG.to_string());
    let archive_name = format!("qemu-xtensa-softmmu-{tag}-{triplet}.tar.xz");

    let url = if let Ok(custom_url) = std::env::var("CARGO_QTEST_QEMU_URL") {
        custom_url
    } else {
        format!("https://github.com/kokroo/qemu/releases/download/{tag}/{archive_name}")
    };

    std::fs::create_dir_all(cdir)
        .with_context(|| format!("failed to create cache directory at {}", cdir.display()))?;

    let archive_path = cdir.join(&archive_name);

    eprintln!("qtest: downloading prebuilt `{binary_name}` for `{triplet}` from GitHub ({tag})...");
    if verbose {
        eprintln!("qtest: download URL: {url}");
        eprintln!("qtest: destination: {}", archive_path.display());
    }

    download_file(&url, &archive_path)?;

    eprintln!("qtest: extracting archive to {}...", cdir.display());
    extract_tar_archive(&archive_path, cdir)?;

    // Clean up archive to save disk space
    let _ = std::fs::remove_file(&archive_path);

    let cached = find_cached_qemu_in(binary_name, cdir).with_context(|| {
        format!(
            "archive was extracted to {}, but `{binary_name}` was not found in qemu/bin",
            cdir.display()
        )
    })?;

    // On Unix, ensure the binary is executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&cached) {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() | 0o755);
            let _ = std::fs::set_permissions(&cached, perms);
        }
    }

    eprintln!("qtest: ready: {}", cached.display());
    configure_qemu_host_compatibility(&cached);
    Ok(cached)
}

/// Configures host OS environment or emulation settings for the QEMU binary if needed.
pub fn configure_qemu_host_compatibility(_binary: &Path) {
    // Native builds are used across supported platforms; no emulation workarounds needed.
}

/// Downloads and extracts the prebuilt QEMU Xtensa archive for the current host platform to the default cache directory.
pub fn download_and_extract_qemu(binary_name: &str, verbose: bool) -> Result<PathBuf> {
    download_and_extract_qemu_to(binary_name, &cache_dir(), verbose)
}

/// Attempts to install missing system runtime dependencies for QEMU on the host platform.
pub fn install_system_dependencies(verbose: bool) -> Result<()> {
    match std::env::consts::OS {
        "macos" => {
            let brew_exists = Command::new("brew")
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

            if brew_exists {
                eprintln!(
                    "qtest: installing QEMU runtime dependencies via Homebrew (glib, libgcrypt, libslirp, pixman, sdl2)..."
                );
                let mut cmd = Command::new("brew");
                cmd.args(["install", "glib", "libgcrypt", "libslirp", "pixman", "sdl2"]);
                if !verbose {
                    cmd.stdout(std::process::Stdio::null());
                }
                let status = cmd.status().context("failed to execute `brew install`")?;
                if status.success() {
                    return Ok(());
                }
                bail!("`brew install` failed with status {:?}", status.code());
            } else {
                bail!(
                    "Homebrew (`brew`) is not available. Please install dependencies manually: brew install glib libgcrypt libslirp pixman sdl2"
                );
            }
        }
        "linux" => {
            // Check for Debian/Ubuntu (apt-get)
            let apt_exists = Command::new("apt-get")
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

            if apt_exists {
                eprintln!("qtest: installing QEMU runtime dependencies via apt-get...");
                let _ = Command::new("sudo").args(["apt-get", "update"]).status();
                let status = Command::new("sudo")
                    .args([
                        "apt-get",
                        "install",
                        "-y",
                        "libgcrypt20",
                        "libglib2.0-0",
                        "libpixman-1-0",
                        "libslirp0",
                        "libsdl2-2.0-0",
                    ])
                    .status()
                    .context("failed to execute `sudo apt-get install`")?;
                if status.success() {
                    return Ok(());
                }
                bail!(
                    "failed to install dependencies with apt-get. Run manually: sudo apt-get install -y libgcrypt20 libglib2.0-0 libpixman-1-0 libslirp0 libsdl2-2.0-0"
                );
            }

            // Check for Fedora/RHEL/CentOS (dnf)
            let dnf_exists = Command::new("dnf")
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

            if dnf_exists {
                eprintln!("qtest: installing QEMU runtime dependencies via dnf...");
                let status = Command::new("sudo")
                    .args([
                        "dnf",
                        "install",
                        "-y",
                        "libgcrypt",
                        "glib2",
                        "pixman",
                        "libslirp",
                        "SDL2",
                    ])
                    .status()
                    .context("failed to execute `sudo dnf install`")?;
                if status.success() {
                    return Ok(());
                }
                bail!(
                    "failed to install dependencies with dnf. Run manually: sudo dnf install -y libgcrypt glib2 pixman libslirp SDL2"
                );
            }

            // Check for Arch Linux (pacman)
            let pacman_exists = Command::new("pacman")
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

            if pacman_exists {
                eprintln!("qtest: installing QEMU runtime dependencies via pacman...");
                let status = Command::new("sudo")
                    .args([
                        "pacman",
                        "-S",
                        "--noconfirm",
                        "libgcrypt",
                        "glib2",
                        "pixman",
                        "libslirp",
                        "sdl2",
                    ])
                    .status()
                    .context("failed to execute `sudo pacman`")?;
                if status.success() {
                    return Ok(());
                }
                bail!(
                    "failed to install dependencies with pacman. Run manually: sudo pacman -S libgcrypt glib2 pixman libslirp sdl2"
                );
            }

            // Check for openSUSE (zypper)
            let zypper_exists = Command::new("zypper")
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

            if zypper_exists {
                eprintln!("qtest: installing QEMU runtime dependencies via zypper...");
                let status = Command::new("sudo")
                    .args([
                        "zypper",
                        "install",
                        "-y",
                        "libgcrypt20",
                        "libglib-2_0-0",
                        "libpixman-1-0",
                        "libslirp0",
                        "libSDL2-2_0-0",
                    ])
                    .status()
                    .context("failed to execute `sudo zypper`")?;
                if status.success() {
                    return Ok(());
                }
                bail!(
                    "failed to install dependencies with zypper. Run manually: sudo zypper install -y libgcrypt20 libglib-2_0-0 libpixman-1-0 libslirp0 libSDL2-2_0-0"
                );
            }

            bail!(
                "could not detect a supported Linux package manager (apt-get, dnf, pacman, zypper). Please install QEMU runtime dependencies manually."
            );
        }
        "windows" => {
            // Windows archive already bundles all DLLs
            Ok(())
        }
        _ => bail!("automatic dependency installation is not supported on this OS"),
    }
}

/// Resolves and ensures a working QEMU binary for the given target and machine.
///
/// If the binary exists on PATH and supports the target machine, it is returned as-is.
/// If not, and the target is an Xtensa/Espressif target, it checks the local cache or
/// downloads a prebuilt binary transparently.
pub fn ensure_qemu_binary(
    binary: &Path,
    machine: &str,
    target: &str,
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

    // 3. If target is Xtensa (or machine is esp32*), check cache or auto-download:
    let is_espressif = target.starts_with("xtensa") || machine.starts_with("esp32");
    if is_espressif && binary_name.contains("xtensa") {
        let try_install_deps = |candidate: &Path| -> bool {
            if std::env::var("CARGO_QTEST_NO_AUTO_DEPS").is_ok() {
                return false;
            }
            eprintln!("qtest: attempting to install missing system runtime dependencies...");
            if let Err(e) = install_system_dependencies(verbose) {
                eprintln!("qtest: automatic dependency installation failed: {e}");
                return false;
            }
            qemu_supports_machine(candidate, machine)
        };

        if let Some(cached) = find_cached_qemu(&binary_name)
            && (qemu_supports_machine(&cached, machine) || try_install_deps(&cached))
        {
            if verbose {
                eprintln!("qtest: using cached QEMU binary at {}", cached.display());
            }
            configure_qemu_host_compatibility(&cached);
            return Ok(cached);
        }

        // Not in cache or cached version doesn't support machine -> auto-download
        let downloaded = download_and_extract_qemu(&binary_name, verbose)?;
        if !qemu_supports_machine(&downloaded, machine) && !try_install_deps(&downloaded) {
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
    fn test_host_platform_triplet() {
        let triplet = host_platform_triplet();
        assert!(triplet.is_ok(), "host platform should be supported");
    }

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
        let res = ensure_qemu_binary(path, "virt", "riscv32-unknown-none-elf", false);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_ensure_qemu_binary_missing_non_xtensa() {
        let binary = Path::new("qemu-system-nonexistent-12345");
        let res = ensure_qemu_binary(binary, "virt", "thumbv7em-none-eabihf", false);
        assert!(res.is_err());
        let err = res.unwrap_err().to_string();
        assert!(err.contains("was not found in PATH"));
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
    fn test_download_and_extract_qemu() {
        let temp_cache = std::env::temp_dir().join("cargo-qtest-test-download");
        let _ = std::fs::remove_dir_all(&temp_cache);
        let res = download_and_extract_qemu_to("qemu-system-xtensa", &temp_cache, true);
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
}
