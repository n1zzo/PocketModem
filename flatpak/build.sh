#!/bin/bash
# Build PocketModem Flatpak
# Run this script on a Linux machine with flatpak-builder installed

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# Progress tracking
MODULE_COUNT=0
CURRENT_MODULE=0
MODULES=(
    "protobuf"
    "protobuf-c"
    "libshumate"
    "pocket-modem"
)
TOTAL_MODULES=${#MODULES[@]}

print_progress() {
    local module="$1"
    local status="$2"
    local filled=$((CURRENT_MODULE * 50 / TOTAL_MODULES))
    local empty=$((50 - filled))
    local bar=$(printf '#%.0s' $(seq 1 $filled) 2>/dev/null || true)
    local spaces=$(printf '.%.0s' $(seq 1 $empty) 2>/dev/null || true)
    
    printf "\r  [%s%s] %d/%d - %s %-10s" "$bar" "$spaces" "$CURRENT_MODULE" "$TOTAL_MODULES" "$module" "$status"
    if [ "$status" = "DONE" ]; then
        printf "\n"
    fi
}

print_header() {
    echo ""
    echo "╔══════════════════════════════════════════════════════════════════╗"
    echo "║                  PocketModem Flatpak Builder                    ║"
    echo "╚══════════════════════════════════════════════════════════════════╝"
    echo ""
}

# Check for flatpak-builder
check_dependencies() {
    echo "Checking dependencies..."
    if ! command -v flatpak-builder &> /dev/null; then
        echo ""
        echo "Error: flatpak-builder not found."
        echo ""
        echo "Install on Debian/Ubuntu:"
        echo "  sudo apt install flatpak flatpak-builder"
        echo "  flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo"
        echo "  flatpak install flathub org.gnome.Platform//50 org.gnome.Sdk//50"
        echo "  flatpak install flathub org.freedesktop.Sdk.Extension.rust-stable"
        echo ""
        echo "Or use Docker:"
        echo "  docker build -t pocket-modem-flatpak flatpak/"
        echo "  docker run --rm -v \$(pwd):/app pocket-modem-flatpak"
        exit 1
    fi
    echo "  ✓ flatpak-builder found"
    
    # Check for required flatpak runtimes
    echo ""
    echo "Checking flatpak runtimes..."
    if flatpak list --user 2>/dev/null | grep -q "org.gnome.Platform"; then
        echo "  ✓ GNOME runtime installed"
    else
        echo "  ⚠ GNOME runtime not installed (run: flatpak install flathub org.gnome.Platform//50 org.gnome.Sdk//50)"
    fi
}

# Ensure vendor directory exists
setup_vendor() {
    ((CURRENT_MODULE++))
    print_progress "setup" "..."
    
    if [ ! -d "src/vendor" ]; then
        echo ""
        echo "Creating vendor directory..."
        mkdir -p src && cd .. && cargo vendor src/vendor
        cd "$SCRIPT_DIR"
    fi
    print_progress "setup" "DONE  "
}

# Build function
run_build() {
    echo ""
    echo "Starting build..."
    echo ""
    
    # Run flatpak-builder with module progress detection
    flatpak-builder --force-clean build org.pocketmodem.pocket-modem.yml 2>&1 | \
    while IFS= read -r line; do
        echo "$line"
        
        # Detect module transitions from build output
        if echo "$line" | grep -q "Starting build of protobuf"; then
            CURRENT_MODULE=1; print_progress "protobuf" "..."
        elif echo "$line" | grep -q "Starting build of protobuf-c"; then
            CURRENT_MODULE=2; print_progress "protobuf-c" "..."
        elif echo "$line" | grep -q "Starting build of libshumate"; then
            CURRENT_MODULE=3; print_progress "libshumate" "..."
        elif echo "$line" | grep -q "Starting build of pocket-modem"; then
            CURRENT_MODULE=4; print_progress "pocket-modem" "..."
        elif echo "$line" | grep -q "Build complete"; then
            CURRENT_MODULE=$TOTAL_MODULES; print_progress "complete" "DONE  "
        fi
    done
    
    CURRENT_MODULE=$TOTAL_MODULES
    print_progress "BUILD" "DONE  "
}

print_footer() {
    echo ""
    echo "╔══════════════════════════════════════════════════════════════════╗"
    echo "║                    Build Complete!                               ║"
    echo "╠══════════════════════════════════════════════════════════════════╣"
    echo "║                                                                  ║"
    echo "║  Test: flatpak-builder build --run flatpak/org.pocketmodem.     ║"
    echo "║        pocket-modem.yml /app/bin/pocket-modem                    ║"
    echo "║                                                                  ║"
    echo "║  Install: flatpak-builder --user --install build flatpak/       ║"
    echo "║           org.pocketmodem.pocket-modem.yml                       ║"
    echo "║                                                                  ║"
    echo "║  Run: flatpak run org.pocketmodem.pocket-modem                   ║"
    echo "║                                                                  ║"
    echo "╚══════════════════════════════════════════════════════════════════╝"
    echo ""
}

# Main execution
print_header
check_dependencies
setup_vendor
run_build
print_footer