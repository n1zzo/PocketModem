#!/bin/bash
# Build PocketModem Flatpak
# Run this script on a Linux machine with flatpak-builder installed

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# Progress tracking
CURRENT_MODULE=0
MODULES=(
    "setup"
    "protobuf"
    "protobuf-c"
    "libshumate"
    "pocket-modem"
)
TOTAL_MODULES=${#MODULES[@]}

print_header() {
    echo ""
    echo "╔══════════════════════════════════════════════════════════════════╗"
    echo "║                  PocketModem Flatpak Builder                    ║"
    echo "╚══════════════════════════════════════════════════════════════════╝"
    echo ""
}

print_progress() {
    local module="$1"
    local status="$2"
    local filled=$((CURRENT_MODULE * 50 / TOTAL_MODULES))
    local empty=$((50 - filled))
    
    # Build bar without using printf tricks (portable)
    local bar=""
    for ((i=0; i<filled; i++)); do bar="${bar}#"; done
    for ((i=filled; i<50; i++)); do bar="${bar}."; done
    
    printf "  [%s] %d/%d - %-12s %s\n" "$bar" "$CURRENT_MODULE" "$TOTAL_MODULES" "$module" "$status"
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
    echo ""
    echo "Preparing vendor directory..."
    if [ ! -d "../src/vendor" ]; then
        echo "  Creating vendor directory..."
        (cd .. && mkdir -p src && cargo vendor src/vendor)
    else
        echo "  ✓ Vendor directory exists"
    fi
    CURRENT_MODULE=1
    print_progress "setup" "[DONE]"
}

# Build function
run_build() {
    echo ""
    echo "Starting build..."
    echo ""
    
    # Pre-print initial state
    CURRENT_MODULE=1
    print_progress "protobuf" "[BUILDING...]"
    
    # Run flatpak-builder with module progress detection
    flatpak-builder --force-clean build org.pocketmodem.pocket-modem.yml 2>&1 | \
    while IFS= read -r line; do
        echo "$line"
        
        # Detect module transitions from build output
        if echo "$line" | grep -q "Building module protobuf"; then
            CURRENT_MODULE=1; print_progress "protobuf" "[DONE]"
        elif echo "$line" | grep -q "Building module protobuf-c"; then
            CURRENT_MODULE=2; print_progress "protobuf-c" "[BUILDING...]"
        elif echo "$line" | grep -q "Building module libshumate"; then
            CURRENT_MODULE=3; print_progress "libshumate" "[BUILDING...]"
        elif echo "$line" | grep -q "Building module pocket-modem"; then
            CURRENT_MODULE=4; print_progress "pocket-modem" "[BUILDING...]"
        elif echo "$line" | grep -q "Committing stage"; then
            # Update current module count based on what's being committed
            if echo "$line" | grep -q "build-protobuf$"; then
                CURRENT_MODULE=1; print_progress "protobuf" "[DONE]"
            elif echo "$line" | grep -q "build-protobuf-c$"; then
                CURRENT_MODULE=2; print_progress "protobuf-c" "[DONE]"
            elif echo "$line" | grep -q "build-libshumate$"; then
                CURRENT_MODULE=3; print_progress "libshumate" "[DONE]"
            fi
        fi
    done
    
    CURRENT_MODULE=$TOTAL_MODULES
    print_progress "BUILD" "[DONE]"
}

print_footer() {
    echo ""
    echo "╔══════════════════════════════════════════════════════════════════╗"
    echo "║                    Build Complete!                               ║"
    echo "╠══════════════════════════════════════════════════════════════════╣"
    echo "║                                                                  ║"
    echo "║  Test: flatpak-builder build --run org.pocketmodem.pocket-      ║"
    echo "║        modem.yml /app/bin/pocket-modem                           ║"
    echo "║                                                                  ║"
    echo "║  Install: flatpak-builder --user --install build org.pocket-    ║"
    echo "║           modem.pocket-modem.yml                                 ║"
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