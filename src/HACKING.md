# Development Workflow

## KV4P Firmware

The KV4P HT firmware source is located at:

    /Users/nizzo/Documents/pi/openrtx_linux/kv4p-ht/microcontroller-src

Build and flash instructions can be found in that directory.

## Building on Linux (ht)

GTK4 is not available on macOS. Build and test on ht.

### Initial Setup

```bash
# On macOS: create destination directory on ht
ssh ht "mkdir -p ~/PocketModem"

# On macOS: rsync sources
cd /Users/nizzo/Documents/pi/PocketModem
rsync -avz --exclude 'target/' --exclude '.git/' --exclude '*.patch' . ht:~/PocketModem/
```

### Building

```bash
# On ht
ssh ht "cd ~/PocketModem && cargo build --release"
```

### Making Changes

```bash
# On macOS: make changes, then rsync individual files
rsync -avz src/main.rs src/radio.rs ht:~/PocketModem/src/

# On ht: rebuild
ssh ht "cd ~/PocketModem && cargo build --release"
```

### Testing

```bash
# On ht: run (auto-detects serial port via /dev/serial/by-id)
ssh ht "cd ~/PocketModem && ./target/release/pocket-modem"

# Or with explicit serial port (if needed)
ssh ht "cd ~/PocketModem && ./target/release/pocket-modem /dev/ttyUSB1"
```

### Syncing Back Changes

```bash
# On macOS: pull changes from ht
rsync -avz ht:~/PocketModem/src/ src/
rsync -avz ht:~/PocketModem/docs/ docs/

# Or git commit on ht and pull

# Sync .pi docs (internal documentation)
rsync -avz .pi/docs/ ht:~/PocketModem/.pi/docs/
```

## Quick Sync Command

For rapid iteration on source files:

```bash
rsync -avz src/main.rs src/radio.rs src/audio.rs src/gps.rs src/kiss.rs ht:~/PocketModem/src/
```

## Saving Patches

```bash
# On macOS: save uncommitted changes
git diff HEAD > /tmp/rssi_changes.patch

# Apply later
git apply /tmp/rssi_changes.patch
```