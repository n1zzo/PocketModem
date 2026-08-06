# PocketModem

A GTK4/libadwaita application for controlling the KV4P HT radio modem over USB serial.

## Building

### On native Linux (x86_64 or aarch64)

```bash
# Install dependencies (Debian/Ubuntu)
sudo apt install libgtk-4-dev libadwaita-1-dev libserialport-dev pkg-config cargo

# Build
cargo build --release
```

### Cross-compilation for postmarketOS (aarch64 musl)

```bash
# Install dependencies (Debian/Ubuntu)
sudo apt install libgtk-4-dev libadwaita-1-dev libserialport-dev pkg-config cargo
rustup target add aarch64-unknown-linux-musl

# Build
cargo build --release --target aarch64-unknown-linux-musl
```

### Building natively on postmarketOS

```bash
# Install dependencies (Alpine)
apk add cargo rustgtk glib-dev gtk4-dev libadwaita-dev serialport-dev

# Build
cargo build --release
```

The musl binary is dynamically linked to:
- `/lib/ld-musl-aarch64.so.1`

## Testing

### Local testing (requires display)

```bash
# List available serial devices
ls /dev/serial/by-id/

# Run with auto-detected device
./target/release/pocket-modem

# Run with specific device
./target/release/pocket-modem /dev/ttyUSB0
```

### Remote testing via SSH

```bash
# Deploy binary to remote device
scp target/release/pocket-modem user@ht:~/

# Run on remote device (with display forwarding)
ssh -X user@ht ./pocket-modem

# Or run headless and check output
ssh user@ht './pocket-modem 2>&1 | head -20'
```

## Architecture

- **main.rs**: GTK4/libadwaita UI with status display, VFO input, PTT button
- **radio.rs**: KV4P radio protocol implementation, serial communication
- **kiss.rs**: KISS protocol packet parsing and device state management

## UI Components

- **Status Bar**: Connection indicator and S-meter with RSSI display
- **VFO Entry**: Frequency input (supports formats like "145.500" or "144.8")
- **Mode Buttons**: FM/RADE/M17 mode selection (RADE and M17 disabled)
- **PTT Button**: Hold to transmit

## Serial Protocol

The radio sends `DeviceState` packets containing:
- `rssi`: 0-255 signal strength value
- `smeter_bars`: S-meter bars (0-9)
- Frequency, mode, squelch state

The application sends `HostDesiredState` packets to configure:
- Frequency (tune)
- TX/RX flags
- Squelch level
- Bandwidth

## Troubleshooting

### Radio not responding
```bash
# Check device permissions
ls -la /dev/serial/by-id/

# Add user to dialout group
sudo usermod -a -G dialout $USER
```

### Build errors with libadwaita
Ensure you have libadwaita 0.5 or later installed:
```bash
pkg-config --modversion libadwaita-1
```

### RSSI/S-meter not updating
The S-meter requires the radio to be properly initialized. Make sure:
1. Radio is connected and powered
2. Frequency is set (tune command sent)
3. RSSI_ENABLED flag is set in HostDesiredState