# Debugging PocketModem

## Remote Build & Test

```bash
# Sync all sources
rsync -avz src/ ht:~/PocketModem/src/

# Build
ssh ht "cd ~/PocketModem && cargo build --release"

# Run and capture output
ssh ht "cd ~/PocketModem && ./target/release/pocket-modem 2>&1"

# Quick rebuild test (no clean)
ssh ht "cd ~/PocketModem && cargo build --release 2>&1 | tail -5"
```

## GDB Debugging

Install gdb on remote:
```bash
ssh ht "sudo apt install gdb -y"
```

Run with backtrace:
```bash
ssh ht "cd ~/PocketModem && gdb -batch -ex run -ex bt ./target/release/pocket-modem 2>&1"
```

## Core Dump Analysis

Enable core dumps:
```bash
ssh ht "ulimit -c unlimited"
ssh ht "sudo sysctl -w kernel.core_pattern=/tmp/core.%e.%p"
```

After crash:
```bash
ssh ht "gdb /path/to/pocket-modem /tmp/core.* -ex bt -ex quit"
```

## Common Issues

1. **Segfault during startup** - Check if radio.rs was synced (timestamp)
2. **UI crash** - Look for NULL pointer dereferences in GTK calls
3. **Thread crash** - Check io_thread state access

## Useful Commands

```bash
# Check file timestamps
ssh ht "ls -la ~/PocketModem/src/main.rs ~/PocketModem/target/release/pocket-modem 2>/dev/null || echo 'Binary missing'"

# Force rebuild
ssh ht "cd ~/PocketModem && cargo clean && cargo build --release 2>&1 | tail -10"

# Monitor in real-time
ssh ht "cd ~/PocketModem && ./target/release/pocket-modem" &
# Then check process
ssh ht "pgrep -f pocket-modem && echo 'Running' || echo 'Crashed'"
```