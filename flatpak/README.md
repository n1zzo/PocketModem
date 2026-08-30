# PocketModem Flatpak

GTK4/libadwaita application for KV4P HT radio modem with map support.

## Permissions (Minimal)

- `--device=tty` - KV4P serial port access only
- `--share=network` - APRS/map tiles
- `--socket=pulseaudio` - Audio playback
- `--socket=wayland` - Display
- `--filesystem=xdg-cache` - Map tile cache
- `--bus=session` - GeoClue2 location via portal
- `--filesystem=xdg-config:rw, xdg-data:rw` - Settings

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

**Build Progress Tracker**

The build script (`build.sh`) shows a progress bar during compilation:

```
╔══════════════════════════════════════════════════════════════════╗
║                  PocketModem Flatpak Builder                    ║
╚══════════════════════════════════════════════════════════════════╝

Checking dependencies...
  ✓ flatpak-builder found

Checking flatpak runtimes...
  ✓ GNOME runtime installed

Creating vendor directory in repo root...

Starting build...

  [###################.....................................] 2/4 - protobuf-c ...
  [################################........................] 3/4 - libshumate ...
  [######################################..................] 4/4 - pocket-modem ...
```

**First build takes ~15-30 minutes** depending on network and CPU. Subsequent builds are faster due to caching.

## Running the Flatpak

```bash
# Install (on ht)
cd ~/PocketModem/flatpak
flatpak-builder --user --install build org.pocketmodem.pocket-modem.yml

# Run
flatpak run org.pocketmodem.pocket-modem
```

## Uninstall

```bash
flatpak uninstall org.pocketmodem.pocket-modem
```

## Serial Device Access

The Flatpak uses `--device=tty` for KV4P serial port access (USB CDC-ACM, CP2102, etc.).

Ensure your user is in the `dialout` group:
```bash
sudo adduser $USER dialout
# Log out and back in
```

## GPS

GPS via cellular modem requires `mmcli` from ModemManager. If `mmcli` is not found, GPS falls back to GeoClue2 (requires location permission prompt).

## Architecture

The Flatpak manifest builds the following from source:
- **protobuf 3.21.12**: Google's data interchange format
- **protobuf-c 1.5.1**: C bindings for protobuf (required by libshumate)
- **libshumate 1.6.1**: GTK4 map widget with vector renderer

This ensures compatibility across different Linux distributions without relying on system packages.

## Files

- `org.pocketmodem.pocket-modem.yml` - Flatpak manifest
- `build.sh` - Build script
- `org.pocketmodem.pocket-modem.desktop` - Desktop entry
- `org.pocketmodem.pocket-modem.appdata.xml` - AppStream metadata
- `icons/` - Application icons (16-512px)