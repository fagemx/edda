#!/bin/sh
# Install script for edda — architectural decision tracker
# Usage:
#   curl -sSf https://raw.githubusercontent.com/fagemx/edda/main/install.sh | sh
#   curl -sSf ... | sh -s -- --version v0.2.0
#   curl -sSf ... | sh -s -- --to /usr/local/bin

set -eu

REPO="fagemx/edda"
BINARY="edda"
DEFAULT_INSTALL_DIR="${HOME}/.local/bin"

# ── Helpers ─────────────────────────────────────────────────────────

say() {
    printf '%s\n' "$@"
}

err() {
    say "error: $*" >&2
    exit 1
}

need() {
    if ! command -v "$1" > /dev/null 2>&1; then
        err "need '$1' (command not found)"
    fi
}

# ── Download abstraction ────────────────────────────────────────────

download() {
    _url="$1"
    _output="$2"
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL "$_url" -o "$_output"
    elif command -v wget > /dev/null 2>&1; then
        wget -q "$_url" -O "$_output"
    else
        err "need 'curl' or 'wget' to download files"
    fi
}

download_to_stdout() {
    _url="$1"
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL "$_url"
    elif command -v wget > /dev/null 2>&1; then
        wget -q "$_url" -O -
    else
        err "need 'curl' or 'wget' to download files"
    fi
}

# ── Argument parsing ────────────────────────────────────────────────

VERSION=""
INSTALL_DIR=""

while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            shift
            VERSION="${1:-}"
            [ -z "$VERSION" ] && err "--version requires a value (e.g. v0.2.0)"
            ;;
        --to)
            shift
            INSTALL_DIR="${1:-}"
            [ -z "$INSTALL_DIR" ] && err "--to requires a directory path"
            ;;
        --help|-h)
            say "Install edda — architectural decision tracker"
            say ""
            say "Usage:"
            say "  curl -sSf https://raw.githubusercontent.com/$REPO/main/install.sh | sh"
            say ""
            say "Options:"
            say "  --version VERSION  Install a specific version (e.g. v0.2.0)"
            say "  --to DIR           Install to a custom directory (default: ~/.local/bin)"
            say "  --help             Show this help message"
            exit 0
            ;;
        *)
            err "unknown option: $1 (use --help for usage)"
            ;;
    esac
    shift
done

INSTALL_DIR="${INSTALL_DIR:-$DEFAULT_INSTALL_DIR}"

# ── Detect OS and architecture ──────────────────────────────────────

detect_target() {
    _os="$(uname -s)"
    _arch="$(uname -m)"

    case "$_os" in
        Linux)
            case "$_arch" in
                x86_64)  echo "x86_64-unknown-linux-musl" ;;
                aarch64) echo "aarch64-unknown-linux-musl" ;;
                *)       err "unsupported Linux architecture: $_arch" ;;
            esac
            ;;
        Darwin)
            case "$_arch" in
                x86_64)  echo "x86_64-apple-darwin" ;;
                arm64)   echo "aarch64-apple-darwin" ;;
                *)       err "unsupported macOS architecture: $_arch" ;;
            esac
            ;;
        *)
            err "unsupported OS: $_os (use Linux or macOS)"
            ;;
    esac
}

# ── Resolve latest version ──────────────────────────────────────────

resolve_version() {
    if [ -n "$VERSION" ]; then
        echo "$VERSION"
        return
    fi

    say "Fetching latest release..."
    _api_url="https://api.github.com/repos/${REPO}/releases/latest"
    _response="$(download_to_stdout "$_api_url")" || err "failed to fetch latest release from GitHub API"

    # Parse tag_name from JSON without jq
    _tag="$(echo "$_response" | grep '"tag_name"' | head -1 | cut -d'"' -f4)"
    [ -z "$_tag" ] && err "could not determine latest version (no releases published?)"

    echo "$_tag"
}

# ── Verify checksum ─────────────────────────────────────────────────

verify_checksum() {
    _archive="$1"
    _checksum_file="$2"

    if command -v sha256sum > /dev/null 2>&1; then
        (cd "$(dirname "$_archive")" && sha256sum --check "$_checksum_file" --status) || return 1
    elif command -v shasum > /dev/null 2>&1; then
        (cd "$(dirname "$_archive")" && shasum -a 256 --check "$_checksum_file" --status) || return 1
    else
        say "warning: sha256sum/shasum not found, skipping checksum verification"
        return 0
    fi
}

# ── Main ────────────────────────────────────────────────────────────

main() {
    need tar

    TARGET="$(detect_target)"
    TAG="$(resolve_version)"

    ARCHIVE_NAME="edda-${TAG}-${TARGET}.tar.gz"
    BASE_URL="https://github.com/${REPO}/releases/download/${TAG}"

    say "Installing edda ${TAG} (${TARGET})"
    say "  to: ${INSTALL_DIR}"

    # Create temp directory with cleanup trap
    TMPDIR="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR"' EXIT

    # Download archive and checksum
    say "Downloading ${ARCHIVE_NAME}..."
    download "${BASE_URL}/${ARCHIVE_NAME}" "${TMPDIR}/${ARCHIVE_NAME}"
    download "${BASE_URL}/${ARCHIVE_NAME}.sha256" "${TMPDIR}/${ARCHIVE_NAME}.sha256" || true

    # Verify checksum if .sha256 was downloaded
    if [ -f "${TMPDIR}/${ARCHIVE_NAME}.sha256" ]; then
        if verify_checksum "${TMPDIR}/${ARCHIVE_NAME}" "${TMPDIR}/${ARCHIVE_NAME}.sha256"; then
            say "Checksum verified."
        else
            err "checksum verification failed — the download may be corrupted"
        fi
    else
        say "warning: checksum file not available, skipping verification"
    fi

    # Extract
    tar xzf "${TMPDIR}/${ARCHIVE_NAME}" -C "${TMPDIR}"

    # Install binary
    mkdir -p "${INSTALL_DIR}"
    cp "${TMPDIR}"/edda-*/"${BINARY}" "${INSTALL_DIR}/${BINARY}"
    chmod +x "${INSTALL_DIR}/${BINARY}"

    say ""
    say "edda ${TAG} installed to ${INSTALL_DIR}/${BINARY}"

    # Check if install dir is on PATH
    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*)
            say ""
            say "Run 'edda --help' to get started."
            ;;
        *)
            say ""
            say "warning: ${INSTALL_DIR} is not in your PATH"
            say ""
            say "Add it to your shell profile:"
            say "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.bashrc"
            say "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.zshrc   # if using zsh"
            say ""
            say "Then restart your shell or run:"
            say "  export PATH=\"${INSTALL_DIR}:\$PATH\""
            ;;
    esac
}

main
