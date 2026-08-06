# Building PocketModem for kv4p-ht

This document describes how to build PocketModem for the kv4p-ht handheld radio.

## Architecture

The kv4p-ht runs **aarch64** (ARM64) with Alpine Linux/musl.
The development machine is **x86_64** Linux (Fedora or macOS).

## Two Build Approaches

### Option 1: Cross-compile on macOS/Linux with cargo

On your development machine (Fedora or macOS):

```bash
# Install cross-compilation toolchain
# On macOS with Homebrew:
brew install cross

# On Fedora:
sudo dnf install cross-gcc-toolchain

# Build for aarch64-unknown-linux-musl target
cd PocketModem
cargo build --release --target aarch64-unknown-linux-musl

# The binary will be at:
# target/aarch64-unknown-linux-musl/release/pocket-modem
```

### Option 2: pmOS pmbootstrap (recommended for PMOS)

Use pmbootstrap to cross-compile in a controlled environment:

```bash
# On your build machine (Fedora or macOS)
cd ~/pmos

# Create or update the work directory
pmbootstrap init

# Install this package
pmbootstrap install \
    --packaging-format=dir \
    pocket-modem

# The package will be in:
# ~/pmos/chroot_native/home/pmos/build/pkg/pocket-modem/
```

## Deployment to HT

Copy the binary to the HT via ADB or SSH:

```bash
# Via ADB
adb push target/aarch64-unknown-linux-musl/release/pocket-modem /tmp/
adb shell "cp /tmp/pocket-modem /usr/bin/ && chmod +x /usr/bin/pocket-modem"

# Via SSH
scp target/aarch64-unknown-linux-musl/release/pocket-modem pmos@10.55.1.104:/tmp/
ssh pmos@10.55.1.104 "cp /tmp/pocket-modem /usr/bin/"
```

## Running

```bash
# With auto-detected serial port
pocket-modem

# With explicit serial port
pocket-modem /dev/ttyUSB0

# Check logs
pocket-modem 2>&1 | head -50
```

## Debugging

```bash
# Check device connection
ls -la /dev/serial/by-id/

# Check serial permissions
ls -la /dev/ttyUSB0
groups
sudo usermod -a -G dialout $USER
```

## Dependencies on HT

The binary needs these packages on the kv4p-ht:

```
gtk4.0
libadwaita
```

These are included in the PMOS base image for the dubai target.