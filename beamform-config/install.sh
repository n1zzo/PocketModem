#!/bin/bash
# Beamforming Setup Installation Script
# For Motorola Edge 30 (dubai) running postmarketOS

set -e

UCM_DIR="/usr/share/alsa/ucm2/Motorola/dubai"
BACKUP_DIR="$HOME/.config/beamform-backup-$(date +%Y%m%d-%H%M%S)"

echo "Installing beamforming configuration for Motorola Edge 30..."

# Create backup
mkdir -p "$BACKUP_DIR"
[ -d "$UCM_DIR" ] && cp -r "$UCM_DIR" "$BACKUP_DIR/" 2>/dev/null || true
[ -d "$HOME/.config/pipewire" ] && cp -r "$HOME/.config/pipewire" "$BACKUP_DIR/" 2>/dev/null || true
echo "Backed up to: $BACKUP_DIR"

# Install UCM configs
echo "Installing UCM profiles..."
sudo mkdir -p "$UCM_DIR"
sudo cp usr/share/alsa/ucm2/Motorola/dubai/*.conf "$UCM_DIR/"
sudo chmod 644 "$UCM_DIR"/*.conf

# Reload UCM
echo "Reloading ALSA UCM..."
alsaucm -c hw:0 reload 2>/dev/null || true

# Restart audio services
echo "Restarting audio services..."
systemctl --user restart pipewire wireplumber 2>/dev/null || true
sleep 2

echo ""
echo "Installation complete!"
echo ""
echo "To use:"
echo "1. Open GNOME Settings → Sound → Input"
echo "2. Select 'Microphone Array'"
echo ""
echo "The input will be stereo where:"
echo "  - Left channel = Top Microphone"
echo "  - Right channel = Bottom Microphone"
echo ""
echo "Node name: alsa_input.platform-sound.Beamform__DualMic__source"
echo ""
echo "Note: WebRTC beamforming is not available on this device."
echo "      The stereo source provides some directional benefit."