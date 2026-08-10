# Testing PocketModem

## Building

```bash
# On macOS: sync source files
rsync -avz src/main.rs src/radio.rs src/audio.rs src/gps.rs src/kiss.rs ht:~/PocketModem/src/

# On ht: build
ssh ht "cd ~/PocketModem && cargo build --release"
```

## Running

The app requires a display. SSH to ht and run directly:

```bash
ssh ht
cd ~/PocketModem
./target/release/pocket-modem
```

Or with a specific serial port:
```bash
ssh ht "cd ~/PocketModem && ./target/release/pocket-modem /dev/ttyUSB0"
```

## Debug Output

Debug output goes to stderr. To capture:

```bash
ssh ht "cd ~/PocketModem && ./target/release/pocket-modem 2>&1"
```

To see debug output while the app is running in another terminal:

```bash
ssh ht "cd ~/PocketModem && ./target/release/pocket-modem" &
# In another terminal, watch stderr via the process
```

## Quick Sync

For rapid iteration:
```bash
rsync -avz src/main.rs src/radio.rs src/audio.rs src/gps.rs src/kiss.rs ht:~/PocketModem/src/
ssh ht "cd ~/PocketModem && cargo build --release 2>&1 | tail -3"
```