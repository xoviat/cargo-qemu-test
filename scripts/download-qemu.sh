#!/usr/bin/env bash
#
# Download and cache a prebuilt QEMU Xtensa binary (semihosting + Espressif
# chip support) from https://github.com/kokroo/qemu releases.
#
# This script ships with the `cargo-qemu-test` crate; `cargo-qtest` embeds it
# and runs it only when a download is explicitly requested (`cargo qtest
# --download`). It can also be run standalone:
#
#   scripts/download-qemu.sh [--binary NAME] [--cache-dir DIR] [--tag TAG]
#                            [--url URL] [--machine MACHINE] [--force]
#                            [--verbose]
#
# With --machine, the resulting binary is verified against `qemu -M help`;
# if the check fails, missing system runtime dependencies are installed once
# (Homebrew/apt/dnf/pacman/zypper) and the check is retried before falling
# back to a fresh download. CARGO_QTEST_NO_AUTO_DEPS disables that install.
# On success the absolute path of the ready-to-run binary is printed on
# stdout (as the last line) and the process exits 0. All progress and
# diagnostic messages go to stderr.

set -euo pipefail

BINARY_NAME="qemu-system-xtensa"
CACHE_DIR="${CARGO_QTEST_CACHE_DIR:-}"
TAG="${CARGO_QTEST_QEMU_TAG:-v0.2.0-semihosting}"
URL="${CARGO_QTEST_QEMU_URL:-}"
MACHINE=""
FORCE=0
VERBOSE=0

usage() {
    cat >&2 <<'EOF'
usage: download-qemu.sh [--binary NAME] [--cache-dir DIR] [--tag TAG] [--url URL] [--machine MACHINE] [--force] [--verbose]

Downloads and caches a prebuilt QEMU Xtensa archive for the host platform.
With --machine, verifies the binary and installs missing system runtime
dependencies once if the verification fails.
Environment overrides: CARGO_QTEST_CACHE_DIR, CARGO_QTEST_QEMU_TAG, CARGO_QTEST_QEMU_URL, CARGO_QTEST_NO_AUTO_DEPS.
Prints the path of the extracted binary on stdout (last line) on success.
EOF
}

log() { echo "qtest: $*" >&2; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        --binary) BINARY_NAME="${2:?--binary needs a value}"; shift 2 ;;
        --cache-dir) CACHE_DIR="${2:?--cache-dir needs a value}"; shift 2 ;;
        --tag) TAG="${2:?--tag needs a value}"; shift 2 ;;
        --url) URL="${2:?--url needs a value}"; shift 2 ;;
        --machine) MACHINE="${2:?--machine needs a value}"; shift 2 ;;
        --force) FORCE=1; shift ;;
        --verbose) VERBOSE=1; shift ;;
        -h | --help) usage; exit 0 ;;
        *) log "unknown argument: $1"; usage; exit 2 ;;
    esac
done

# --- cache directory ---------------------------------------------------------
if [[ -z "$CACHE_DIR" ]]; then
    case "$(uname -s)" in
        Darwin) CACHE_DIR="${HOME:-/tmp}/Library/Caches/cargo-qtest" ;;
        *)
            if [[ -n "${XDG_CACHE_HOME:-}" ]]; then
                CACHE_DIR="$XDG_CACHE_HOME/cargo-qtest"
            else
                CACHE_DIR="${HOME:-/tmp}/.cache/cargo-qtest"
            fi
            ;;
    esac
fi

# --- host platform triplet ---------------------------------------------------
detect_triplet() {
    local os arch
    os="$(uname -s)"
    arch="$(uname -m)"
    if [[ "$os" == "Darwin" && "$arch" == "x86_64" ]]; then
        # Under Rosetta 2, report the native Apple Silicon architecture.
        if [[ "$(sysctl -in sysctl.proc_translated 2>/dev/null || true)" == "1" ]]; then
            arch="arm64"
        fi
    fi
    case "$os-$arch" in
        Linux-x86_64) echo "x86_64-linux-gnu" ;;
        Linux-aarch64 | Linux-arm64) echo "aarch64-linux-gnu" ;;
        Darwin-x86_64) echo "x86_64-apple-darwin" ;;
        Darwin-arm64 | Darwin-aarch64) echo "aarch64-apple-darwin" ;;
        *) return 1 ;;
    esac
}

if ! TRIPLET="$(detect_triplet)"; then
    log "unsupported host platform ($(uname -s)/$(uname -m)) for automatic QEMU download."
    log "Provide a custom QEMU binary via --qemu."
    exit 1
fi

# --- machine verification + system dependencies ------------------------------
# Probes `<binary> -M help` for MACHINE. Empty MACHINE always passes.
machine_supported() {
    local out
    [[ -n "$MACHINE" ]] || return 0
    out=$("$1" -M help 2>/dev/null) || return 1
    printf '%s\n' "$out" | awk -v m="$MACHINE" 'NF && $1 == m { ok = 1 } END { exit !ok }'
}

# Installs QEMU runtime libraries for the host platform (ported from the old
# in-Rust install_system_dependencies). Returns non-zero when unavailable.
install_system_dependencies() {
    case "$(uname -s)" in
        Darwin)
            if ! command -v brew >/dev/null 2>&1; then
                log "Homebrew (\`brew\`) is not available. Please install dependencies manually: brew install glib libgcrypt libslirp pixman sdl2"
                return 1
            fi
            log "installing QEMU runtime dependencies via Homebrew (glib, libgcrypt, libslirp, pixman, sdl2)..."
            if [[ "$VERBOSE" -eq 1 ]]; then
                brew install glib libgcrypt libslirp pixman sdl2 || return 1
            else
                brew install glib libgcrypt libslirp pixman sdl2 >/dev/null || return 1
            fi
            ;;
        Linux)
            if command -v apt-get >/dev/null 2>&1; then
                log "installing QEMU runtime dependencies via apt-get..."
                sudo apt-get update >/dev/null 2>&1 || true
                sudo apt-get install -y libgcrypt20 libglib2.0-0 libpixman-1-0 libslirp0 libsdl2-2.0-0 || return 1
            elif command -v dnf >/dev/null 2>&1; then
                log "installing QEMU runtime dependencies via dnf..."
                sudo dnf install -y libgcrypt glib2 pixman libslirp SDL2 || return 1
            elif command -v pacman >/dev/null 2>&1; then
                log "installing QEMU runtime dependencies via pacman..."
                sudo pacman -S --noconfirm libgcrypt glib2 pixman libslirp sdl2 || return 1
            elif command -v zypper >/dev/null 2>&1; then
                log "installing QEMU runtime dependencies via zypper..."
                sudo zypper install -y libgcrypt20 libglib-2_0-0 libpixman-1-0 libslirp0 libSDL2-2_0-0 || return 1
            else
                log "could not detect a supported Linux package manager (apt-get, dnf, pacman, zypper). Please install QEMU runtime dependencies manually."
                return 1
            fi
            ;;
        *)
            log "automatic dependency installation is not supported on this OS ($(uname -s))"
            return 1
            ;;
    esac
    return 0
}

# Verifies `bin` against MACHINE, attempting a one-time dependency install
# (unless CARGO_QTEST_NO_AUTO_DEPS) when the probe fails.
DEPS_ATTEMPTED=0
check_binary() {
    local bin="$1"
    machine_supported "$bin" && return 0
    [[ "$DEPS_ATTEMPTED" -eq 0 ]] || return 1
    DEPS_ATTEMPTED=1
    if [[ -n "${CARGO_QTEST_NO_AUTO_DEPS:-}" ]]; then
        return 1
    fi
    log "attempting to install missing system runtime dependencies..."
    install_system_dependencies && machine_supported "$bin" && return 0
    return 1
}

# --- locate an already-cached binary ----------------------------------------
find_cached() {
    local suffix="" candidate
    for candidate in \
        "$CACHE_DIR/qemu/bin/$BINARY_NAME$suffix" \
        "$CACHE_DIR/bin/$BINARY_NAME$suffix" \
        "$CACHE_DIR/$BINARY_NAME$suffix"; do
        if [[ -f "$candidate" ]]; then
            echo "$candidate"
            return 0
        fi
    done
    return 1
}

if [[ "$FORCE" -eq 0 ]] && EXISTING="$(find_cached)"; then
    if check_binary "$EXISTING"; then
        log "using cached QEMU binary at $EXISTING"
        echo "$EXISTING"
        exit 0
    fi
    log "cached binary at $EXISTING failed the machine check; downloading a fresh copy..."
fi

# --- download ----------------------------------------------------------------
ARCHIVE_NAME="qemu-xtensa-softmmu-$TAG-$TRIPLET.tar.xz"
if [[ -z "$URL" ]]; then
    URL="https://github.com/kokroo/qemu/releases/download/$TAG/$ARCHIVE_NAME"
fi

mkdir -p "$CACHE_DIR" || {
    log "failed to create cache directory at $CACHE_DIR"
    exit 1
}
ARCHIVE_PATH="$CACHE_DIR/$ARCHIVE_NAME"

log "downloading prebuilt \`$BINARY_NAME\` for \`$TRIPLET\` from GitHub ($TAG)..."
if [[ "$VERBOSE" -eq 1 ]]; then
    log "download URL: $URL"
    log "destination: $ARCHIVE_PATH"
fi

downloaded=0
if command -v curl >/dev/null 2>&1; then
    if curl -sSL -f "$URL" -o "$ARCHIVE_PATH" >&2; then
        downloaded=1
    fi
elif command -v wget >/dev/null 2>&1; then
    if wget -q -O "$ARCHIVE_PATH" "$URL" 2>&2; then
        downloaded=1
    fi
fi
if [[ "$downloaded" -ne 1 ]]; then
    log "failed to download $URL. Ensure an active internet connection and that \`curl\` or \`wget\` is installed."
    rm -f "$ARCHIVE_PATH"
    exit 1
fi

# --- extract -----------------------------------------------------------------
log "extracting archive to $CACHE_DIR..."
if ! tar -xf "$ARCHIVE_PATH" -C "$CACHE_DIR"; then
    log "tar failed to extract $ARCHIVE_PATH"
    rm -f "$ARCHIVE_PATH"
    exit 1
fi
rm -f "$ARCHIVE_PATH"

if ! BINARY_PATH="$(find_cached)"; then
    log "archive was extracted to $CACHE_DIR, but \`$BINARY_NAME\` was not found in qemu/bin, bin, or the cache root."
    exit 1
fi
chmod +x "$BINARY_PATH" || true

if check_binary "$BINARY_PATH"; then
    log "ready: $BINARY_PATH"
    echo "$BINARY_PATH"
    exit 0
fi
log "the QEMU binary at $BINARY_PATH does not support machine \`$MACHINE\` (system dependency installation was attempted)."
exit 1
