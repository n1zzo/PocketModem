# Building PocketModem

## Dependencies

### Debian/Ubuntu

```bash
sudo apt install libgtk-4-dev libadwaita-1-dev libserialport-dev pkg-config cargo
```

### Alpine (postmarketOS)

```bash
apk add cargo rustgtk glib-dev gtk4-dev libadwaita-dev serialport-dev
```

## Build

```bash
cargo build --release
```

The binary will be at `target/release/pocket-modem`.

## Run

```bash
# Auto-detects serial port via /dev/serial/by-id
./target/release/pocket-modem

# Or with explicit serial port
./target/release/pocket-modem /dev/ttyUSB1
```