# ISS APRS TX Implementation Plan

## Status: ✅ COMPLETE - TESTED AND WORKING

## Firmware Requirement: TX_ALLOWED Flag

**Critical:** The KV4P firmware checks `txAllowedByHost()` in `handleAx25Data()` before transmitting AX.25 frames.
The host MUST send a `DesiredState` with both `HOST_STATE_TX_ALLOWED` (bit 11) and `HOST_STATE_PTT_REQUESTED` (bit 1) flags set BEFORE sending the KISS DATA frame.

```rust
// Before sending APRS beacon:
let mut state = build_desired_state(false);
state.flags |= TX_ALLOWED | PTT_REQUESTED;
send(HostDesiredState(state));
// Small delay may help
thread::sleep(50);
// Then send KISS DATA frame
send(KissData(ax25_frame));
```

## Requirements

| Item | Value |
|------|-------|
| Callsign | User-configured, no prefilled default, empty by default |
| Default symbol | Person `['` (primary table, walking person) |
| GPS fix required | Yes, block TX if no fix |
| Frequency | Use current channel frequency (no hardcoding) |
| Channel setup | User creates/selects APRS channel |

## ISS Technical Parameters

| Parameter | Value |
|-----------|-------|
| Frequency | 145.825 MHz simplex (via channel, not hardcoded) |
| Modulation | 1200 baud AFSK FM |
| DigiPath | `ARISS` (NOT `WIDE1-1,WIDE2-1`) |
| TX Delay | 200-300 ms (handled by firmware) |
| CTCSS | None (simplex) |
| TX Mode | Manual beacon only (no auto-beacon) |

## Default APRS Symbol

- **Table ID**: `/` (primary table)
- **Code**: `'` (apostrophe = walking person)
- Display: 👤

## Symbol Options for UI Dropdown

| Label | Table ID | Code | Display |
|-------|----------|------|---------|
| Person | `/` | `'` | 👤 |
| Car | `/` | `>` | 🚗 |
| Balloon | `/` | `O` | 🎈 |
| House | `/` | `*` | 🏠 |
| Bike | `/` | `b` | 🚴 |
| Sailboat | `/` | `S` | ⛵ |
| RV | `/` | `R` | 🚐 |

## Implementation Steps

### Step 1: AX.25 Frame Building

**File**: `src/kiss.rs`

```rust
// Encode 6-char callsign + SSID into 7-byte AX.25 format
// Each char is bits 1-7 (shifted right by 1)
// Byte 7: SSID in lower 4 bits, final-bit flag in bit 0
fn encode_callsign(callsign: &str) -> [u8; 7]

// Build AX.25 UI frame with digipeater path
// Format: dest(7) + src(7) + digipeaters(n×7) + ctrl(1) + pid(1) + payload
// Set final-bit on last address byte
fn build_ax25_ui_frame(
    dest: &str,           // e.g., "APRS"
    src: &str,            // e.g., "KD4LCD-9"
    digipath: &[String],  // e.g., ["ARISS"]
    payload: &[u8]        // APRS text bytes
) -> Vec<u8>
```

### Step 2: APRS Payload Generation

**File**: `src/aprs.rs`

```rust
// Build APRS position report string
pub fn build_position_report(
    lat: f64,              // Decimal degrees
    lon: f64,              // Decimal degrees
    symbol_table: char,    // '/' or '\\'
    symbol_code: char,     // e.g., '\''
    comment: &str,
) -> String

// Output format: !DDMM.mmN/DDDMM.mmW[comment]
// - Use ! without timestamp (most common)
// - Could support = with timestamp if needed
```

### Step 3: APRS Settings

**File**: `src/settings.rs`

Add to `SettingsManager`:

```rust
// APRS TX Configuration
aprs_callsign: String,      // e.g., "KD4LCD", empty by default
aprs_ssid: u8,              // e.g., 9, 0 by default
aprs_symbol_table: char,    // '/' primary table
aprs_symbol_code: char,     // '\'' apostrophe for person
aprs_comment: String,       // User text, empty by default
aprs_tx_enabled: bool,      // false by default
```

**Note**: Persist via GSettings. No prefilled defaults.

### Step 4: TX Beacon Function

**File**: `src/radio.rs`

```rust
// Send APRS beacon with current GPS position
// Returns: Ok(packet_desc) on success, Err(msg) on failure
pub fn send_aprs_beacon(
    &self,
    lat: f64,
    lon: f64,
    alt: Option<f64>,
) -> Result<String, String>

// Flow:
// 1. Check GPS fix (caller ensures this)
// 2. Get callsign from settings (must be non-empty)
// 3. Build APRS position string with symbol + comment
// 4. Encode as AX.25 UI frame with ARISS digipath
// 5. Send via existing serial TX (reuses send_audio infrastructure)
```

**GPS Fix Check**:
- Before calling `send_aprs_beacon`, check `gps.has_fix`
- If no fix, show toast "GPS fix required for APRS beacon"

### Step 5: UI Elements

**File**: `src/main.rs`

**5.1 APRS Settings Section** (in Settings page):

- Callsign input (Entry with SSID spinner)
- Symbol picker (ComboRow with presets)
- Comment field (Entry)
- TX enable toggle (SwitchRow)

**5.2 Beacon Button** (in APRS page):

- "Beacon" button
- Disabled state if:
  - No GPS fix
  - No callsign configured (empty)
  - TX disabled
- Shows status: "Beacon" → "TX..." → "Sent ✓" / "Failed"

**5.3 GPS Fix Check UI**:

- If GPS has fix: button enabled
- If no fix: button disabled, tooltip "GPS fix required"

## Data Flow

```
User taps "Beacon"
    ↓
Check: GPS has fix? ──No──→ Show toast "GPS fix required"
    │Yes
    ↓
Check: APRS callsign configured? ──No──→ Show toast "Configure callsign first"
    │Yes
    ↓
Check: APRS TX enabled? ──No──→ Show toast "APRS TX disabled"
    │Yes
    ↓
Get GPS position (lat, lon, alt)
    ↓
build_position_report(lat, lon, '/', '\'', comment)
    ↓
build_ax25_ui_frame("APRS", "CALLSIGN-SSID", ["ARISS"], payload)
    ↓
Send via serial TX
    ↓
Show "Sent ✓" toast
```

## Notes

- APRS TX reuses existing `send_raw_audio`/`send_frame` infrastructure
- AX.25 data goes through same serial port as voice
- No special PTT mode needed - uses existing PTT handling
- TX Delay: Firmware handles timing (200-300ms built into protocol)

## Sync/Build Workflow

```bash
# On macOS: sync modified files
rsync -avz src/kiss.rs src/aprs.rs src/radio.rs src/settings.rs src/main.rs ht:~/PocketModem/src/

# On ht: rebuild
ssh ht "cd ~/PocketModem && cargo build --release"

# On ht: run
ssh ht "cd ~/PocketModem && ./target/release/pocket-modem"
```

## Implementation Order

1. **Step 1**: AX.25 frame building (`kiss.rs`)
2. **Step 2**: APRS payload generation (`aprs.rs`)
3. **Step 3**: APRS settings (`settings.rs`)
4. **Step 4**: TX beacon function (`radio.rs`)
5. **Step 5**: UI elements (`main.rs`)