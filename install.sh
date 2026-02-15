#!/usr/bin/env bash
# Installation script for itr - the zero-config issue tracker CLI

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Print colored messages
info() { echo -e "${BLUE}ℹ${NC} $1"; }
success() { echo -e "${GREEN}✓${NC} $1"; }
warning() { echo -e "${YELLOW}⚠${NC} $1"; }
error() { echo -e "${RED}✗${NC} $1"; }

# Determine install location
determine_install_location() {
    # Default to ~/.cargo/bin if it exists or user has cargo
    if [ -d "$HOME/.cargo/bin" ] || command -v cargo &> /dev/null; then
        echo "$HOME/.cargo/bin"
    # Otherwise try /usr/local/bin if writable
    elif [ -w "/usr/local/bin" ]; then
        echo "/usr/local/bin"
    # Fallback to ~/.local/bin
    else
        echo "$HOME/.local/bin"
    fi
}

# Check if a directory is in PATH
is_in_path() {
    local dir="$1"
    case ":$PATH:" in
        *":$dir:"*) return 0 ;;
        *) return 1 ;;
    esac
}

# Main installation
main() {
    echo ""
    info "Installing itr - the zero-config issue tracker CLI"
    echo ""

    # Check for cargo
    if ! command -v cargo &> /dev/null; then
        error "Cargo is not installed"
        echo ""
        echo "Please install Rust and Cargo from: https://rustup.rs/"
        echo "Then run this script again."
        exit 1
    fi

    success "Cargo found: $(cargo --version)"

    # Build release binary
    info "Building release binary..."
    if cargo build --release; then
        success "Build completed"
    else
        error "Build failed"
        exit 1
    fi

    # Verify binary exists
    if [ ! -f "target/release/itr" ]; then
        error "Binary not found at target/release/itr"
        exit 1
    fi

    # Determine install location
    DEFAULT_INSTALL_DIR=$(determine_install_location)

    echo ""
    info "Select installation location:"
    echo "  1) $HOME/.cargo/bin (recommended for Rust users)"
    echo "  2) /usr/local/bin (system-wide, may require sudo)"
    echo "  3) $HOME/.local/bin (user-local)"
    echo "  4) Custom path"
    echo ""
    read -p "Choice [1-4] (default: 1): " choice
    choice=${choice:-1}

    case $choice in
        1)
            INSTALL_DIR="$HOME/.cargo/bin"
            ;;
        2)
            INSTALL_DIR="/usr/local/bin"
            NEED_SUDO=true
            ;;
        3)
            INSTALL_DIR="$HOME/.local/bin"
            ;;
        4)
            read -p "Enter custom installation path: " INSTALL_DIR
            INSTALL_DIR="${INSTALL_DIR/#\~/$HOME}"
            ;;
        *)
            error "Invalid choice"
            exit 1
            ;;
    esac

    # Create directory if it doesn't exist
    if [ ! -d "$INSTALL_DIR" ]; then
        info "Creating directory: $INSTALL_DIR"
        mkdir -p "$INSTALL_DIR"
    fi

    # Install binary
    info "Installing to $INSTALL_DIR..."
    if [ "$NEED_SUDO" = true ]; then
        if sudo cp target/release/itr "$INSTALL_DIR/itr"; then
            sudo chmod +x "$INSTALL_DIR/itr"
            success "Installed to $INSTALL_DIR/itr"
        else
            error "Installation failed"
            exit 1
        fi
    else
        if cp target/release/itr "$INSTALL_DIR/itr"; then
            chmod +x "$INSTALL_DIR/itr"
            success "Installed to $INSTALL_DIR/itr"
        else
            error "Installation failed"
            exit 1
        fi
    fi

    # Check if install directory is in PATH
    if ! is_in_path "$INSTALL_DIR"; then
        warning "$INSTALL_DIR is not in your PATH"
        echo ""
        echo "Add the following line to your shell configuration file:"
        echo "  (~/.bashrc, ~/.zshrc, or ~/.profile)"
        echo ""
        echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
        echo ""
    fi

    # Verify installation
    echo ""
    info "Verifying installation..."
    if command -v itr &> /dev/null; then
        success "itr is now available in your PATH"
        echo ""
        itr --version
    else
        warning "itr was installed but is not immediately available"
        echo ""
        echo "You may need to:"
        echo "  1. Add $INSTALL_DIR to your PATH (see above)"
        echo "  2. Restart your shell or run: source ~/.bashrc (or ~/.zshrc)"
        echo ""
        echo "Or run directly: $INSTALL_DIR/itr"
    fi

    echo ""
    success "Installation complete!"
    echo ""
    info "Quick start:"
    echo "  itr init              # Initialize a new issue tracker"
    echo "  itr add 'My task'     # Add an issue"
    echo "  itr list              # List issues"
    echo "  itr --help            # See all commands"
    echo ""
}

main "$@"
