#!/bin/bash
# Beamforming Setup Installation Script
# Run this on your Motorola Edge 30 (postmarketOS)
#
# Usage: ./install.sh [--uninstall]

set -e

INSTALL_DIR="/usr/share/alsa/ucm2/Motorola/dubai"
CONFIG_DIR="$HOME/.config/pipewire/filter-chain.conf.d"
WIREPLUMBER_DIR="$HOME/.config/wireplumber"
BACKUP_DIR="$HOME/.config/beamform-backup-$(date +%Y%m%d-%H%M%S)"

uninstall=false
if [ "$1" = "--uninstall" ]; then
    uninstall=true
fi

if [ "$uninstall" = true ]; then
    echo "Uninstalling beamforming configuration..."

    # Restore original UCM
    sudo apk add -f alsa-ucm-conf-qcom-sc7280 2>/dev/null || true

    # Remove configs
    rm -rf "$HOME/.config/alsa/"
    rm -rf "$CONFIG_DIR"
    rm -f "$WIREPLUMBER_DIR/main.lua"

    echo "Restarting audio services..."
    systemctl --user restart pipewire wireplumber

    echo "Beamforming uninstalled. You may need to re-select your audio profile in GNOME Settings."
    exit 0
fi

echo "Installing beamforming configuration for Motorola Edge 30..."

# Create backup directory
mkdir -p "$BACKUP_DIR"

# Backup existing configs
if [ -d "$INSTALL_DIR" ]; then
    cp -r "$INSTALL_DIR" "$BACKUP_DIR/" 2>/dev/null || true
    echo "Backed up existing UCM configs to: $BACKUP_DIR"
fi

# Install UCM configuration
echo "Installing UCM profile..."
sudo mkdir -p "$INSTALL_DIR"
sudo cp usr/share/alsa/ucm2/Motorola/dubai/* "$INSTALL_DIR/"
sudo chmod 644 "$INSTALL_DIR"/*.conf

# Install PipeWire filter chain
echo "Installing PipeWire filter chain..."
mkdir -p "$CONFIG_DIR"
cp root/.config/pipewire/filter-chain.conf.d/beamform.conf "$CONFIG_DIR/"

# Install WirePlumber config
echo "Installing WirePlumber auto-link rule..."
mkdir -p "$WIREPLUMBER_DIR"
cp root/.config/wireplumber/main.lua "$WIREPLUMBER_DIR/"

# Restart audio services
echo "Restarting audio services..."
systemctl --user restart pipewire wireplumber

echo ""
echo "Installation complete!"
echo ""
echo "Next steps:"
echo "1. Open GNOME Settings -> Sound"
echo "2. Under 'Input', select 'Beamformed Microphone'"
echo "3. Test with a voice recording app"
echo ""
echo "To verify beamforming is active:"
echo "  pw-cli ls Node | grep DualMic"
echo ""
echo "To uninstall:"
echo "  ./install.sh --uninstall"