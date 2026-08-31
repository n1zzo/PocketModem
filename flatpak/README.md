# PocketModem Flatpak

GTK4/libadwaita application for KV4P HT radio modem with map support.

## Permissions

- `--share=network` - APRS/map tiles
- `--socket=pulseaudio` - Audio playback
- `--socket=wayland` - Display
- `--socket=session-bus` - D-Bus for GeoClue2 location via portal
- `--filesystem=xdg-cache` - Map tile cache
- `--filesystem=xdg-config:rw` - Settings storage
- `--filesystem=xdg-data:rw` - Application data
- `--talk-name=org.freedesktop.portal.Desktop` - XDG Desktop Portal
- `--talk-name=org.freedesktop.portal.Location` - Location access

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

```bash
cd ~/PocketModem/flatpak
./build.sh
```

**Build Progress Tracker**

The build script (`build.sh`) shows progress during compilation:

```
  [###################.....................................] 2/4 - protobuf-c ...
  [################################........................] 3/4 - libshumate ...
  [######################################..................] 4/4 - pocket-modem ...
```

**First build takes ~15-30 minutes** depending on network and CPU. Subsequent builds are faster due to caching.

## Export as Flatpak Bundle

To share the built flatpak as a single `.flatpak` file:

### From existing build

```bash
cd ~/PocketModem/flatpak

# Create an OSTree repo
ostree init --repo=./repo --mode=archive

# Export the build to the repo
flatpak build-export repo build org.pocketmodem.pocket-modem

# Create the bundle
flatpak build-bundle repo pocket-modem.flatpak app/org.pocketmodem.pocket-modem/aarch64/org.pocketmodem.pocket-modem
```

The bundle will be created at `pocket-modem.flatpak` (~28 MB).

### From scratch (rebuild + export)

```bash
cd ~/PocketModem/flatpak

# Build
flatpak-builder --force-clean build org.pocketmodem.pocket-modem.yml

# Export
ostree init --repo=./repo --mode=archive
flatpak build-export repo build org.pocketmodem.pocket-modem
flatpak build-bundle repo pocket-modem.flatpak app/org.pocketmodem.pocket-modem/aarch64/org.pocketmodem.pocket-modem
```

## Install from Bundle

```bash
# On target machine
flatpak install --user ~/path/to/pocket-modem.flatpak

# Run
flatpak run org.pocketmodem.pocket-modem
```

## Uninstall

```bash
flatpak uninstall org.pocketmodem.pocket-modem
```

## Running Without Export

If you just want to run from the build directory:

```bash
cd ~/PocketModem/flatpak

# Install from build dir
flatpak-builder --user --install build org.pocketmodem.pocket-modem.yml

# Run
flatpak run org.pocketmodem.pocket-modem
```

## GPS

GPS via cellular modem requires `mmcli` from ModemManager (not available inside flatpak sandbox). If `mmcli` is not found, GPS falls back to GeoClue2 via the XDG Location Portal (requires permission prompt on first run).

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