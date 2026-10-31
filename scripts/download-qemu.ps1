<#
Download and cache a prebuilt QEMU Xtensa binary (semihosting + Espressif
chip support) from https://github.com/kokroo/qemu releases.

This script ships with the `cargo-qemu-test` crate; `cargo-qtest` embeds it
and runs it only when a download is explicitly requested (`cargo qtest
--download`). It can also be run standalone:

    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\download-qemu.ps1 `
        [-Binary NAME] [-CacheDir DIR] [-Tag TAG] [-Url URL] [-Force] [-VerboseOutput]

On success the absolute path of the ready-to-run binary is printed on stdout
and the process exits 0. Progress and diagnostics go to stderr.
#>
[CmdletBinding()]
param(
    [string]$Binary = "qemu-system-xtensa",
    [string]$CacheDir = $env:CARGO_QTEST_CACHE_DIR,
    [string]$Tag = $(if ($env:CARGO_QTEST_QEMU_TAG) { $env:CARGO_QTEST_QEMU_TAG } else { "v0.2.0-semihosting" }),
    [string]$Url = $env:CARGO_QTEST_QEMU_URL,
    [switch]$Force,
    [switch]$VerboseOutput
)

$ErrorActionPreference = "Stop"

function Write-Log([string]$Message) {
    [Console]::Error.WriteLine("qtest: $Message")
}

# --- cache directory ---------------------------------------------------------
if (-not $CacheDir) {
    if ($env:LOCALAPPDATA) {
        $CacheDir = Join-Path $env:LOCALAPPDATA "cargo-qtest"
    } else {
        $CacheDir = Join-Path ([System.IO.Path]::GetTempPath()) "cargo-qtest"
    }
}

# --- host platform triplet ---------------------------------------------------
# Detect the native architecture even when running x86_64 PowerShell under
# Prism / WOW64 emulation on Windows 11 ARM64.
function Get-NativeArch {
    if ($env:RUNNER_ARCH -and $env:RUNNER_ARCH -ieq "ARM64") { return "aarch64" }
    if ($env:PROCESSOR_ARCHITEW6432 -and $env:PROCESSOR_ARCHITEW6432 -ieq "ARM64") { return "aarch64" }

    try {
        $src = @"
using System;
using System.Runtime.InteropServices;
public static class QtestWow64 {
    [DllImport("kernel32")] public static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32")] public static extern int IsWow64Process2(IntPtr hProcess, out ushort processMachine, out ushort nativeMachine);
    public static string NativeArch() {
        ushort processMachine, nativeMachine;
        if (IsWow64Process2(GetCurrentProcess(), out processMachine, out nativeMachine) != 0) {
            if (nativeMachine == 0xAA64) return "aarch64"; // IMAGE_FILE_MACHINE_ARM64
            if (nativeMachine == 0x8664) return "x86_64";  // IMAGE_FILE_MACHINE_AMD64
        }
        return null;
    }
}
"@
        Add-Type -TypeDefinition $src -ErrorAction Stop
        $arch = [QtestWow64]::NativeArch()
        if ($arch) { return $arch }
    } catch {
        # Fall back to environment detection below.
    }

    if ($env:PROCESSOR_ARCHITECTURE -ieq "ARM64") { return "aarch64" }
    if ($env:PROCESSOR_ARCHITECTURE -ieq "AMD64") { return "x86_64" }
    return $null
}

$arch = Get-NativeArch
$triplet = switch ($arch) {
    "x86_64" { "x86_64-w64-mingw32" }
    "aarch64" { "aarch64-w64-mingw32" }
    default {
        Write-Log "unsupported host platform (windows-$arch) for automatic QEMU download."
        Write-Log "Provide a custom QEMU binary via --qemu."
        exit 1
    }
}

# --- locate an already-cached binary ----------------------------------------
$exe = "$Binary.exe"
function Find-Cached {
    foreach ($candidate in @(
            (Join-Path $CacheDir "qemu\bin\$exe"),
            (Join-Path $CacheDir "bin\$exe"),
            (Join-Path $CacheDir $exe)
        )) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }
    return $null
}

if (-not $Force) {
    $existing = Find-Cached
    if ($existing) {
        Write-Log "using cached QEMU binary at $existing"
        Write-Output $existing
        exit 0
    }
}

# --- download ----------------------------------------------------------------
$archiveName = "qemu-xtensa-softmmu-$Tag-$triplet.tar.xz"
if (-not $Url) {
    $Url = "https://github.com/kokroo/qemu/releases/download/$Tag/$archiveName"
}

try {
    New-Item -ItemType Directory -Force -Path $CacheDir | Out-Null
} catch {
    Write-Log "failed to create cache directory at $CacheDir : $($_.Exception.Message)"
    exit 1
}
$archivePath = Join-Path $CacheDir $archiveName

Write-Log "downloading prebuilt ``$Binary`` for ``$triplet`` from GitHub ($Tag)..."
if ($VerboseOutput) {
    Write-Log "download URL: $Url"
    Write-Log "destination: $archivePath"
}

$downloaded = $false
$curl = Get-Command curl.exe -ErrorAction SilentlyContinue
if ($curl) {
    & $curl.Source -sSL -f $Url -o $archivePath 2>&1 | Out-Null
    $downloaded = ($LASTEXITCODE -eq 0)
}
if (-not $downloaded) {
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        (New-Object Net.WebClient).DownloadFile($Url, $archivePath)
        $downloaded = $true
    } catch {
        Write-Log "download failed: $($_.Exception.Message)"
    }
}
if (-not $downloaded) {
    Write-Log "failed to download $Url. Ensure an active internet connection."
    Remove-Item -Force -ErrorAction SilentlyContinue $archivePath
    exit 1
}

# --- extract -----------------------------------------------------------------
Write-Log "extracting archive to $CacheDir..."
# Prefer the Windows system tar: MSYS/Git-for-Windows tar can come first on
# PATH and misparse `C:\...` paths as remote URLs.
$tar = $null
$sysTar = Join-Path $env:SystemRoot "System32\tar.exe"
if (Test-Path -LiteralPath $sysTar) {
    $tar = $sysTar
} else {
    $cmd = Get-Command tar.exe -ErrorAction SilentlyContinue
    if ($cmd) { $tar = $cmd.Source }
}
if (-not $tar) {
    Write-Log "tar was not found on PATH; Windows 10 and later ship it by default."
    Remove-Item -Force -ErrorAction SilentlyContinue $archivePath
    exit 1
}
& $tar -xf $archivePath -C $CacheDir
if ($LASTEXITCODE -ne 0) {
    Write-Log "tar failed to extract $archivePath"
    Remove-Item -Force -ErrorAction SilentlyContinue $archivePath
    exit 1
}
Remove-Item -Force -ErrorAction SilentlyContinue $archivePath

$binaryPath = Find-Cached
if (-not $binaryPath) {
    Write-Log "archive was extracted to $CacheDir, but ``$exe`` was not found in qemu\bin, bin, or the cache root."
    exit 1
}

Write-Log "ready: $binaryPath"
Write-Output $binaryPath
