# PocketModem Flatpak

GTK4/libadwaita application for KV4P HT radio modem with map support.

## Prerequisites (Alpine Linux / PostmarketOS)

```bash
# Enable community repo (Alpine)
echo "http://dl-cdn.alpinelinux.org/alpine/v3.20/community" >> /etc/apk/repositories

# Install flatpak and build tools
apk add flatpak flatpak-builder rust cargo

# Install GNOME runtime and SDK
flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
flatpak install flathub org.gnome.Platform//50 org.gnome.Sdk//50

# Install Rust toolchain extension (required for building)
flatpak install flathub org.freedesktop.Sdk.Extension.rust-stable
```

## Build

**Option 1: On Linux (recommended)**

```bash
# Navigate to project root
cd ~/PocketModem

# Install flatpak-builder (Debian/Ubuntu)
sudo apt install flatpak flatpak-builder
flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
flatpak install flathub org.gnome.Platform//50 org.gnome.Sdk//50
flatpak install flathub org.freedesktop.Sdk.Extension.rust-stable

# Build
./flatpak/build.sh
```

**Option 2: Using Docker (macOS compatible)**

```bash
# Build the Docker image (do this once)
docker build -t pocket-modem-flatpak flatpak/

# Run the build
docker run --rm -v $(pwd):/app pocket-modem-flatpak
```

Or manually:
```bash
flatpak-builder --force-clean build flatpak/org.pocketmodem.pocket-modem.yml
```

The build will:
1. Download and build protobuf 3.21.12 from source
2. Download and build protobuf-c 1.5.1 from source
3. Download and build libshumate 1.6.1 from source (provides map widget)
4. Build PocketModem with vendored Rust dependencies
5. Install to `build/` directory

**First build takes ~10-15 minutes** depending on network. Subsequent builds are faster due to caching.

## Running the Flatpak

```bash
# Build and run with proper permissions (for testing)
flatpak-builder build --run flatpak/org.pocketmodem.pocket-modem.yml /app/bin/pocket-modem

# Or install locally and run
flatpak-builder --user --install build flatpak/org.pocketmodem.pocket-modem.yml
flatpak run org.pocketmodem.pocket-modem
```

## Uninstall

```bash
flatpak uninstall org.pocketmodem.pocket-modem
```

## Serial Device Access

The Flatpak is configured to access Silicon Labs CP2102 devices only:

```
/dev/serial/by-id/*Silicon_Labs*
```

### Permissions

Ensure your user is in the `dialout` group:

```bash
sudo adduser $USER dialout
# Log out and back in
```

## GPS

GPS via cellular modem requires `mmcli` from ModemManager to be available on the host system. If `mmcli` is not found, GPS will be disabled with a warning.

To enable GPS on the host:
```bash
# Alpine/PostmarketOS
sudo apk add modemmanager

# Debian/Ubuntu
sudo apt install modemmanager

# Fedora
sudo dnf install ModemManager
```

## Architecture

The Flatpak manifest builds the following from source:
- **protobuf 3.21.12**: Google's data interchange format
- **protobuf-c 1.5.1**: C bindings for protobuf (required by libshumate)
- **libshumate 1.6.1**: GTK4 map widget with vector renderer

This ensures compatibility across different Linux distributions without relying on system packages.

## Notes

- Uses GNOME 50 runtime
- Audio: PulseAudio socket
- Settings: GSettings via `xdg-config`
- Network: Required for map tiles and APRS
- Vendored Rust dependencies for offline builds
- **Does not affect native builds** - the flatpak uses `src/Cargo.toml` with its own vendored dependencies