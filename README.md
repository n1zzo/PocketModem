<p align="center">
  <img src="img/logo.png" width="128" alt="PocketModem">
</p>

# PocketModem

A GTK4/libadwaita application for controlling the KV4P HT radio modem over USB serial.

**License:** [GPL-3.0](LICENSE)

See [docs/BUILD.md](docs/BUILD.md) for build instructions.

## Features

- **VFO Control** — Tune to any frequency with decimal input (e.g., "145.500" or "144.8")
- **Signal Meter** — Real-time RSSI S-meter display (S0-S9+)
- **Audio** — Opus codec for voice TX/RX
- **GPS** — GPS location display with fix indicator
- **PTT** — Push-to-talk via button or keyboard

## Architecture

| File | Purpose |
|------|---------|
| `src/main.rs` | GTK4/libadwaita UI, event handlers, CSS styling |
| `src/radio.rs` | KV4P protocol, serial I/O, reader thread, radio state |
| `src/kiss.rs` | KISS frame parsing, DeviceState/HostDesiredState structs |
| `src/audio.rs` | Audio input/output with Opus codec |
| `src/gps.rs` | GPS data parsing and display |

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
ls -la /dev/serial/by-id/
sudo usermod -a -G dialout $USER
```

### RSSI/S-meter not updating
1. Radio is connected and powered
2. Frequency is set (tune command sent)
3. RSSI_ENABLED flag is set in HostDesiredState