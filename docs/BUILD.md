# Building PocketModem

## Dependencies

### Debian/Ubuntu

```bash
sudo apt install libgtk-4-dev libadwaita-1-dev libserialport-dev libshumate-1.0-dev pkg-config cargo
```

### Alpine (postmarketOS)

```bash
apk add cargo rustgtk glib-dev gtk4-dev libadwaita-dev serialport-dev libshumate-dev
```

## Build

```bash
cargo build --release
```

The binary will be at `target/release/pocket-modem`.

## Run

```bash
# With auto-detected serial port
./target/release/pocket-modem

# With explicit serial port
./target/release/pocket-modem /dev/ttyUSB0
```