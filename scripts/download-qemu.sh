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
#                            [--url URL] [--force] [--verbose]
#
# On success the absolute path of the ready-to-run binary is printed on
# stdout (as the last line) and the process exits 0. All progress and
# diagnostic messages go to stderr.

set -euo pipefail

BINARY_NAME="qemu-system-xtensa"
CACHE_DIR="${CARGO_QTEST_CACHE_DIR:-}"
TAG="${CARGO_QTEST_QEMU_TAG:-v0.2.0-semihosting}"
URL="${CARGO_QTEST_QEMU_URL:-}"
FORCE=0
VERBOSE=0

usage() {
    cat >&2 <<'EOF'
usage: download-qemu.sh [--binary NAME] [--cache-dir DIR] [--tag TAG] [--url URL] [--force] [--verbose]

Downloads and caches a prebuilt QEMU Xtensa archive for the host platform.
Environment overrides: CARGO_QTEST_CACHE_DIR, CARGO_QTEST_QEMU_TAG, CARGO_QTEST_QEMU_URL.
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
    log "using cached QEMU binary at $EXISTING"
    echo "$EXISTING"
    exit 0
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

log "ready: $BINARY_PATH"
echo "$BINARY_PATH"
