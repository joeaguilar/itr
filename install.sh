#!/usr/bin/env bash
# Installation script for itr — the zero-config issue tracker CLI
#
# Downloads a prebuilt binary from the latest GitHub Release for the host
# platform. Falls back to `cargo build` from source if downloads fail and
# cargo is available, or when ITR_FROM_SOURCE=1.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/joeaguilar/itr/main/install.sh | bash
#   ./install.sh
#
# Environment overrides:
#   ITR_VERSION       Pin a specific release tag (e.g. v0.1.0). Defaults to latest.
#   ITR_INSTALL_DIR   Install directory. Defaults to $HOME/.local/bin (or
#                     $HOME/.cargo/bin when it already exists in PATH).
#   ITR_FROM_SOURCE   If 1, skip prebuilt download and build with cargo.
#   ITR_REPO          GitHub repo slug. Defaults to joeaguilar/itr.

set -euo pipefail

REPO="${ITR_REPO:-joeaguilar/itr}"
VERSION="${ITR_VERSION:-}"
FROM_SOURCE="${ITR_FROM_SOURCE:-0}"

# Colors (suppressed when not a tty)
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    BLUE='\033[0;34m'
    NC='\033[0m'
else
    RED=''; GREEN=''; YELLOW=''; BLUE=''; NC=''
fi

info()    { printf "${BLUE}ℹ${NC} %s\n" "$1"; }
success() { printf "${GREEN}✓${NC} %s\n" "$1"; }
warning() { printf "${YELLOW}⚠${NC} %s\n" "$1" >&2; }
error()   { printf "${RED}✗${NC} %s\n" "$1" >&2; }

# ---- Platform detection -----------------------------------------------------

detect_target() {
    local uname_s uname_m
    uname_s=$(uname -s)
    uname_m=$(uname -m)

    case "$uname_s" in
        Darwin)
            case "$uname_m" in
                arm64|aarch64) echo "aarch64-apple-darwin" ;;
                x86_64)        echo "x86_64-apple-darwin" ;;
                *) error "Unsupported macOS architecture: $uname_m"; return 1 ;;
            esac
            ;;
        Linux)
            # Default to the fully-static musl build on x86_64 — it runs on
            # any distro regardless of glibc version. The glibc artifact is
            # still published for users who explicitly want it.
            case "$uname_m" in
                x86_64)        echo "x86_64-unknown-linux-musl" ;;
                aarch64|arm64) echo "aarch64-unknown-linux-gnu" ;;
                *) error "Unsupported Linux architecture: $uname_m"; return 1 ;;
            esac
            ;;
        MINGW*|MSYS*|CYGWIN*)
            error "Detected Windows shell. Please use install.ps1 instead:"
            error "  iwr -useb https://raw.githubusercontent.com/${REPO}/main/install.ps1 | iex"
            return 1
            ;;
        *)
            error "Unsupported OS: $uname_s"; return 1 ;;
    esac
}

# ---- Download helpers -------------------------------------------------------

http_get() {
    local url="$1" out="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$out"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$out" "$url"
    else
        error "Need curl or wget to download releases."
        return 1
    fi
}

resolve_version() {
    if [ -n "$VERSION" ]; then
        echo "$VERSION"
        return 0
    fi
    # Use the GH redirect on /releases/latest to avoid API rate limits and
    # the need for a token.
    local url="https://github.com/${REPO}/releases/latest"
    local resolved
    if command -v curl >/dev/null 2>&1; then
        resolved=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$url")
    elif command -v wget >/dev/null 2>&1; then
        resolved=$(wget --max-redirect=5 -S --spider "$url" 2>&1 | awk '/^  Location: /{loc=$2} END{print loc}')
    else
        error "Need curl or wget to query the latest release."
        return 1
    fi
    # Strip everything up to the last '/'
    echo "${resolved##*/}"
}

verify_checksum() {
    local file="$1" sumfile="$2"
    local expected actual
    expected=$(awk '{print $1}' "$sumfile" | head -1)
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$file" | awk '{print $1}')
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$file" | awk '{print $1}')
    else
        warning "No sha256sum/shasum found — skipping checksum verification."
        return 0
    fi
    if [ "$expected" != "$actual" ]; then
        error "Checksum mismatch: expected $expected, got $actual"
        return 1
    fi
    success "Checksum verified."
}

# ---- Install dir selection --------------------------------------------------

choose_install_dir() {
    if [ -n "${ITR_INSTALL_DIR:-}" ]; then
        echo "${ITR_INSTALL_DIR/#\~/$HOME}"
        return
    fi
    # If ~/.cargo/bin exists and is in PATH, prefer it (Rust users).
    case ":$PATH:" in
        *":$HOME/.cargo/bin:"*)
            if [ -d "$HOME/.cargo/bin" ]; then
                echo "$HOME/.cargo/bin"
                return
            fi
            ;;
    esac
    echo "$HOME/.local/bin"
}

is_in_path() {
    case ":$PATH:" in
        *":$1:"*) return 0 ;;
        *)        return 1 ;;
    esac
}

# ---- Install paths ----------------------------------------------------------

install_from_release() {
    local target tag asset_base archive_url checksum_url
    target=$(detect_target) || return 1
    info "Detected target: $target"

    tag=$(resolve_version)
    if [ -z "$tag" ] || [ "$tag" = "releases" ]; then
        error "Could not resolve latest release tag."
        return 1
    fi
    info "Release: $tag"

    asset_base="itr-${tag}-${target}"
    archive_url="https://github.com/${REPO}/releases/download/${tag}/${asset_base}.tar.gz"
    checksum_url="${archive_url}.sha256"

    local tmpdir
    tmpdir=$(mktemp -d)
    trap 'rm -rf "$tmpdir"' RETURN

    info "Downloading ${asset_base}.tar.gz"
    if ! http_get "$archive_url" "$tmpdir/${asset_base}.tar.gz"; then
        error "Download failed: $archive_url"
        return 1
    fi
    if ! http_get "$checksum_url" "$tmpdir/${asset_base}.tar.gz.sha256"; then
        warning "Checksum file not found at $checksum_url"
    else
        verify_checksum "$tmpdir/${asset_base}.tar.gz" "$tmpdir/${asset_base}.tar.gz.sha256" || return 1
    fi

    info "Extracting…"
    tar -xzf "$tmpdir/${asset_base}.tar.gz" -C "$tmpdir"
    if [ ! -f "$tmpdir/${asset_base}/itr" ]; then
        error "Extracted archive is missing the itr binary."
        return 1
    fi

    local install_dir need_sudo=0
    install_dir=$(choose_install_dir)
    mkdir -p "$install_dir" 2>/dev/null || true
    if [ ! -w "$install_dir" ]; then
        need_sudo=1
    fi

    info "Installing to $install_dir"
    if [ "$need_sudo" = 1 ]; then
        sudo install -m 0755 "$tmpdir/${asset_base}/itr" "$install_dir/itr"
    else
        install -m 0755 "$tmpdir/${asset_base}/itr" "$install_dir/itr"
    fi
    success "Installed $install_dir/itr"

    if ! is_in_path "$install_dir"; then
        warning "$install_dir is not in your PATH."
        echo "  Add it with:"
        echo "    echo 'export PATH=\"$install_dir:\$PATH\"' >> ~/.bashrc   # or ~/.zshrc"
    fi

    echo
    "$install_dir/itr" --version 2>/dev/null || true
    return 0
}

install_from_source() {
    if ! command -v cargo >/dev/null 2>&1; then
        error "cargo is not installed — cannot build from source."
        error "Install Rust from https://rustup.rs/ and re-run, or set a working network connection."
        return 1
    fi
    if [ ! -f Cargo.toml ]; then
        error "Source build requires running this script from inside a cloned itr repo."
        error "Try: git clone https://github.com/${REPO} && cd itr && ./install.sh"
        return 1
    fi
    info "Building from source with cargo…"
    cargo build --release
    local install_dir
    install_dir=$(choose_install_dir)
    mkdir -p "$install_dir"
    install -m 0755 target/release/itr "$install_dir/itr"
    success "Installed $install_dir/itr"
    if ! is_in_path "$install_dir"; then
        warning "$install_dir is not in your PATH."
        echo "  export PATH=\"$install_dir:\$PATH\""
    fi
}

# ---- Main -------------------------------------------------------------------

main() {
    echo
    info "Installing itr — the zero-config issue tracker CLI"
    echo

    if [ "$FROM_SOURCE" = "1" ]; then
        install_from_source
    elif install_from_release; then
        :
    else
        warning "Prebuilt install failed — falling back to cargo build."
        install_from_source
    fi

    echo
    success "Done."
    echo
    info "Quick start:"
    echo "  itr init              # initialize an issue tracker in the current dir"
    echo "  itr add 'My task'     # create an issue"
    echo "  itr ready             # list unblocked issues"
    echo "  itr --help            # all commands"
    echo
}

main "$@"
