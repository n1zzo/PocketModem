#!/bin/bash
# Build PocketModem Flatpak
# Run this script on a Linux machine with flatpak-builder installed

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

echo "Building PocketModem Flatpak..."
echo ""

# Check for flatpak-builder
if ! command -v flatpak-builder &> /dev/null; then
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

# Ensure vendor directory exists
if [ ! -d "src/vendor" ]; then
    echo "Creating vendor directory..."
    cd src && cargo vendor vendor && cd ..
fi

# Build
echo "Running flatpak-builder..."
flatpak-builder --force-clean build flatpak/org.pocketmodem.pocket-modem.yml

echo ""
echo "Build complete!"
echo ""
echo "To test:"
echo "  flatpak-builder build --run flatpak/org.pocketmodem.pocket-modem.yml /app/bin/pocket-modem"
echo ""
echo "To install:"
echo "  flatpak-builder --user --install build flatpak/org.pocketmodem.pocket-modem.yml"
echo "  flatpak run org.pocketmodem.pocket-modem"