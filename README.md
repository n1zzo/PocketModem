<p align="center">
  <img src="img/logo.png" width="128" alt="PocketModem">
</p>

# PocketModem

A GTK4/libadwaita application for controlling the KV4P HT radio modem over USB serial, with APRS tracking and map support.

**License:** [GPL-3.0](LICENSE)

See [`flatpak/README.md`](flatpak/README.md) for Flatpak build instructions or `docs/BUILD.md` for native builds.

## Features

- **VFO Control** — Tune to any frequency with decimal input (e.g., "145.500" or "144.8")
- **Signal Meter** — Real-time RSSI S-meter display (S0-S9+)
- **Audio** — ADPCM codec for voice TX/RX
- **GPS** — GPS location display with fix indicator
- **PTT** — Push-to-talk via button or keyboard

## Architecture

| File | Purpose |
|------|---------|
| `src/main.rs` | GTK4/libadwaita UI, event handlers, CSS styling |
| `src/radio.rs` | KV4P protocol, serial I/O, reader thread, radio state |
| `src/kiss.rs` | KISS frame parsing, DeviceState/HostDesiredState structs |
| `src/audio.rs` | Audio input/output with ADPCM codec |
| `src/gps.rs` | GPS data parsing and display |
| `src/settings.rs` | GSettings persistence, channel management, CHIRP CSV import/export |

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

## Settings

Settings are persisted via GSettings. See `AGENTS.md` for schema details and CHIRP CSV import/export.

## Firmware

Flash the latest KV4P HT firmware from `openrtx_linux/kv4p-ht`.
Tested with commit `92faf83` (soft-squelch and S-meter improvements).

## Troubleshooting

### Radio not responding
```bash
# Check serial device
ls -la /dev/serial/by-id/

# Ensure user is in dialout group
groups
sudo usermod -a -G dialout $USER
# Log out and back in for group change to take effect
```

### RSSI/S-meter stuck at "-- dBm"
1. Radio is connected and powered
2. Frequency is set (tune command sent)
3. RSSI_ENABLED flag is set (default enabled)

See `AGENTS.md` for detailed debug notes.

## Roadmap

### Potential Enhancements

- [x] Rewrite using libadwaita instead of GTK4
- [x] Use toggle groups for power setting
- [x] Add MODEM, GPS and AUDIO details with bottom sheet
- [x] Handle channel edit
- [x] Implement APRS, add toast for new messages
- [x] Add APRS message log and map views using carousel
- [x] Code refactor: move utils function in dedicated file, remove UI sizing constraints, move UI code to dedicated file, deal with unsafes.
- [x] APRS Tx
- [x] APRS direct messages
- [x] Add flatpak build target
- [x] Geoclue backend
- [ ] Merge MM and Geoclue
- [ ] Auto flash ESP32
- [ ] Better AX.25 demodulator
- [ ] APRS duplicates detection
- [ ] FX.25 support
- [ ] Fix light theme
- [ ] Use libadwaita feature to show unrecoverable errors
- [ ] Offline maps

### Development

See `AGENTS.md` for development workflow, build instructions, and debug notes.

Contributions welcome — see `HACKING.md` for setup on the `ht` build machine.
