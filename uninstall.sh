#!/usr/bin/env bash
# Uninstallation script for itr - the zero-config issue tracker CLI

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

# Find itr binary locations
find_itr_locations() {
    local locations=()

    # Common installation locations
    local search_paths=(
        "$HOME/.cargo/bin/itr"
        "/usr/local/bin/itr"
        "$HOME/.local/bin/itr"
    )

    for path in "${search_paths[@]}"; do
        if [ -f "$path" ]; then
            locations+=("$path")
        fi
    done

    # Also check which itr (if in PATH)
    if command -v itr &> /dev/null; then
        local which_path=$(command -v itr)
        # Add if not already in list
        if [[ ! " ${locations[@]} " =~ " ${which_path} " ]]; then
            locations+=("$which_path")
        fi
    fi

    printf '%s\n' "${locations[@]}"
}

# Main uninstallation
main() {
    echo ""
    info "Uninstalling itr"
    echo ""

    # Find all itr installations
    mapfile -t ITR_LOCATIONS < <(find_itr_locations)

    if [ ${#ITR_LOCATIONS[@]} -eq 0 ]; then
        warning "No itr installations found"
        echo ""
        echo "Checked locations:"
        echo "  - $HOME/.cargo/bin/itr"
        echo "  - /usr/local/bin/itr"
        echo "  - $HOME/.local/bin/itr"
        echo ""
        exit 0
    fi

    echo "Found itr installation(s):"
    for i in "${!ITR_LOCATIONS[@]}"; do
        echo "  $((i+1))) ${ITR_LOCATIONS[$i]}"
    done
    echo ""

    read -p "Remove all installations? [y/N]: " confirm
    confirm=${confirm:-n}

    if [[ ! "$confirm" =~ ^[Yy]$ ]]; then
        info "Uninstallation cancelled"
        exit 0
    fi

    # Remove each installation
    local removed_count=0
    for location in "${ITR_LOCATIONS[@]}"; do
        local dir=$(dirname "$location")
        local need_sudo=false

        # Check if we need sudo
        if [ ! -w "$dir" ]; then
            need_sudo=true
        fi

        info "Removing $location..."
        if [ "$need_sudo" = true ]; then
            if sudo rm -f "$location"; then
                success "Removed $location"
                ((removed_count++))
            else
                error "Failed to remove $location"
            fi
        else
            if rm -f "$location"; then
                success "Removed $location"
                ((removed_count++))
            else
                error "Failed to remove $location"
            fi
        fi
    done

    echo ""
    if [ $removed_count -gt 0 ]; then
        success "Uninstallation complete! Removed $removed_count installation(s)"
    else
        error "No installations were removed"
        exit 1
    fi

    # Verify removal
    if command -v itr &> /dev/null; then
        warning "itr is still available in your PATH"
        echo ""
        echo "Location: $(command -v itr)"
        echo "This may be from a different installation or shell cache."
        echo "Try restarting your shell."
    else
        success "itr is no longer available in your PATH"
    fi

    echo ""
    info "Note: This script does not remove:"
    echo "  - Project databases (.itr.db files)"
    echo "  - Build artifacts (target/ directory)"
    echo ""
}

main "$@"
