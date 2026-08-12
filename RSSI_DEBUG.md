# Debug Notes

## Serial Port Permission (Aug 2024)

### Symptom
"Write error: Permission denied" when connecting to radio

### Root Cause
User not in `dialout` group - serial port requires it.

```bash
# Check group membership
groups  # Should include 'dialout'

# Fix
sudo usermod -a -G dialout $USER
# Log out and back in for change to take effect
```

---

## Startup Reliability Fix (Aug 2024)

### Symptom
PocketModem would sometimes fail to connect to the radio on startup, requiring multiple retries.

### Root Cause
Multiple issues in the `open()` function flow:

1. **Send_initial_state() ineffective**: The function tried to send via `write_tx` channel, but the write thread wasn't spawned yet (it was spawned *after* the handshake). Commands were silently dropped.

2. **Race condition**: The reader thread was spawned *after* boot data reading. If the device sent data during this window, it was lost.

3. **Fragile boot data reading**: The heuristic waited for 400+ bytes with a KISS FEND marker, which was unreliable.

### Fix
Refactored `open()` to:

1. **Spawn threads FIRST** - Reader thread starts reading before we send any commands
2. **Direct serial writes** - For initial handshake, write directly to serial port (safe since reader only reads)
3. **Simpler handshake** - Wait for HELLO first, then send initial state if needed
4. **Clear stale buffer** - Clear input buffer on open to remove garbage from previous sessions

```rust
// NEW FLOW:
pub fn open(&mut self) -> Result<Option<VersionInfo>, String> {
    // Open serial port
    // Clear input buffer
    
    // CRITICAL: Spawn threads FIRST
    self.running.store(true, Ordering::SeqCst);
    self.spawn_reader_thread();
    thread::sleep(Duration::from_millis(100));  // Let reader start
    
    // Wait for HELLO from device
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if self.version.lock().unwrap().is_some() { break; }
        thread::sleep(Duration::from_millis(50));
    }
    
    // If no HELLO, send initial state to trigger response
    if !found_hello {
        self.send_initial_state_direct();  // Direct serial write
        // ... retry loop ...
    }
}
```

---

## DeviceState Parsing Bug in HELLO Handler (Aug 2024)

### Symptom
- S-meter stuck at "-- dBm" even after successful connection
- Device state not updating correctly from initial handshake

### Root Cause
The HELLO packet payload contains **two** structs concatenated:
1. `Version` (17 bytes) - firmware version, radio module info
2. `DeviceState` (26 bytes) - current radio state

The Rust code was calling `DeviceState::from_bytes(payload)` on the full HELLO payload, which starts with Version data. This caused:
- Wrong bytes parsed as RSSI (got Version fields instead)
- DeviceState callbacks received garbage data

### Fix
In both `dispatch()` and reader thread, parse DeviceState from offset 17 for HELLO packets:

```rust
// In Hello handler:
if payload.len() >= 43 {  // 17 (Version) + 26 (DeviceState)
    if let Some(state) = DeviceState::from_bytes(&payload[17..]) {
        // Now correctly parses DeviceState
        *self.device_state.lock().unwrap() = Some(state.clone());
        // ... callbacks
    }
}
```

---

## Squelch Settings Not Applied (Aug 2024)

### Symptom
Squelch slider changes not affecting radio - device always echoes back squelch=4.

### Root Cause (Firmware)
In `kv4p_ht_esp32_wroom_32.ino` line 320, `sa818.group()` was called with squelch hardcoded to `0`:

```c
// BUG: squelch was hardcoded to 0
while (!sa818.group(desiredState.bw, desiredState.freq_tx, desiredState.freq_rx, 
                    desiredState.ctcss_tx, 0, desiredState.ctcss_rx))
```

### Fix (Firmware)
```c
// Fixed: pass desiredState.squelch
while (!sa818.group(desiredState.bw, desiredState.freq_tx, desiredState.freq_rx, 
                    desiredState.ctcss_tx, desiredState.squelch, desiredState.ctcss_rx))
```

File: `/Users/nizzo/Documents/pi/openrtx_linux/kv4p-ht/microcontroller-src/kv4p_ht_esp32_wroom_32/kv4p_ht_esp32_wroom_32.ino`

### Root Cause (Host - Client)
In `src/radio.rs`, two functions were sending incorrect squelch values:

1. **`send_initial_state()`** - Used `Default::default()` which has `squelch: 0`
2. **`ptt_on()`** - Hardcoded `squelch: 4` instead of using `current_squelch`

### Fix (Host - APPLIED)
```rust
// send_initial_state: Use current_squelch instead of default
fn send_initial_state(&mut self) -> Result<(), String> {
    let flags = ...;
    let squelch = self.current_squelch.load(Ordering::SeqCst);
    let state = HostDesiredState { flags, squelch, ..Default::default() };
    self.send(state)
}

// ptt_on: Use current_squelch instead of hardcoded 4
pub fn ptt_on(&mut self) -> Result<(), String> {
    // ...
    let state = HostDesiredState {
        // ...
        squelch: self.current_squelch.load(Ordering::SeqCst),  // was: squelch: 4
        // ...
    };
    self.send(state)
}
```

### Test Results (Aug 2024)
When testing with `--test-squelch`:
- Set squelch 0, 3, 6, 9 → Device echoes back 9 for all except 9
- **This is a FIRMWARE bug**, not a host bug
- The firmware's `sa818.group()` call has squelch hardcoded to 0
- The device receives our squelch values but doesn't apply them to SA818

### Remaining Issue
The firmware bug must be fixed on the device side:
```c
// BUG: squelch hardcoded to 0
sa818.group(bw, freq_tx, freq_rx, ctcss_tx, 0, ctcss_rx);

// FIX: use the desired squelch value
sa818.group(bw, freq_tx, freq_rx, ctcss_tx, desiredState.squelch, ctcss_rx);
```

---

## Squelch Not Echoed - Host Bug (Aug 2024)

### Symptom
After fixing squelch value and firmware passing squelch correctly, setting squelch 
levels 0, 3, 6 still echoed back 9 from device.

### Root Cause
The firmware's `reconcileDesiredState()` only applies SA818 configuration (including 
squelch) when `HOST_STATE_RADIO_CONFIG_VALID` (bit 0) is set in desiredState.flags:

```c
// Firmware (protocol.h)
if ((desiredState.flags & HOST_STATE_RADIO_CONFIG_VALID) && radioConfigChanged()) {
    // Only applies SA818 settings when this flag is set
    sa818.group(..., desiredState.squelch, ...);
}
```

The host client's `tune()`, `set_power()`, `enable_smeter()`, `ptt_on()`, and 
`ptt_off()` functions were NOT including `RADIO_CONFIG_VALID` in their flags, 
so the firmware ignored all radio config including squelch.

### Fix (Host - APPLIED)
Add `HostStateFlags::RADIO_CONFIG_VALID` to flags in all functions that build 
`HostDesiredState`:

```rust
// tune(), set_power(), enable_smeter(), ptt_on(), ptt_off()
let mut flags = HostStateFlags::RADIO_CONFIG_VALID | HostStateFlags::HIGH_POWER | 
                 HostStateFlags::RSSI_ENABLED | HostStateFlags::ENABLE_STATUS_REPORTS | 
                 HostStateFlags::RX_AUDIO_OPEN;
```

File: `src/radio.rs`

---

## RSSI Debug Notes

## Bug History

### Symptom
RSSI display stuck at "-- dBm" or not updating, while device was connected.

### Root Cause (Bisect Result)
Commit `8917588` ("Add squelch debouncing and fix RSSI formula") introduced a 
blocking squelch debounce that called `radio.lock()` from a `glib::timeout_add_local` 
callback. This blocked the GTK main loop, preventing the UI timer (which also needs 
the radio lock) from executing.

### Fix
Commit `616fbe9` ("Fix squelch setting: use try_lock with retry instead of blocking 
lock") fixed it by:
1. Spawning a thread for squelch setting instead of using GTK timeout callback
2. Using `try_lock()` with 500ms retry loop in spawned thread

### Commits Involved
- `bf06a85` - works (no debounce)
- `8917588` - BROKEN (blocking debounce)
- `00ba12a` - BROKEN 
- `616fbe9` - works (fixed non-blocking debounce)

## RSSI Code Path

### Formula (matches DeviceState::rssi_dbm)
```rust
let dbm = -120.0 + (state.raw_rssi as f64 * 2.0);
```

### Data Flow
1. **Device** sends `DeviceState` packet (cmd=0x0B, 26 bytes)
2. **PacketParser** extracts RSSI from byte[25] (`data[25]`)
3. **DeviceState::from_bytes** stores it as `s.rssi = data[25]`
4. **RadioState::state()** reads from `DeviceState.rssi` via `s.rssi_dbm()`
5. **UI timer** (1000ms) calls `radio.state()` and displays dBm

## Debug Logging

To add debug output:
```rust
// In main.rs UI timer:
eprintln!("[UI] raw_rssi={}, dbm={:.1}, connected={}", 
    state.raw_rssi, state.rssi, state.connected);

// In radio.rs reader thread:
if payload.len() >= 26 {
    eprintln!("[radio] DeviceState: rssi_byte={}, rssi_dbm={:.1}",
        payload[25], state.rssi_dbm());
}
```

## Testing

Build on ht (see HACKING.md):
```bash
rsync -avz src/main.rs src/radio.rs ht:~/PocketModem/src/
ssh ht "cd ~/PocketModem && cargo build --release"
ssh ht "cd ~/PocketModem && ./target/release/pocket-modem /dev/ttyUSB0"
```

Look for `[UI] raw_rssi=X, dbm=Y.Y` in stderr - should update every second when connected.