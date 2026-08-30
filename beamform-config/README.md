# Beamforming Configuration for Motorola Edge 30

This directory contains configuration files to enable beamformed microphone recording
using the dual microphone array on the Motorola Edge 30 (dubai).

## Status: ✅ Phase 1 Complete

The UCM profiles are working. The beamforming filter chain requires additional configuration.

## What Works

| Feature | Status |
|---------|--------|
| UCM profiles (Beamform, DualMic) | ✅ Working |
| GNOME Settings shows options | ✅ Working |
| DualMic 2-channel stereo source | ✅ Working |
| Beamforming filter chain | 🔧 Requires additional setup |

## Files

| Path | Purpose |
|------|---------|
| `usr/share/alsa/ucm2/Motorola/dubai/HiFi-beamform.conf` | ALSA UCM profile for beamform |
| `usr/share/alsa/ucm2/Motorola/dubai/HiFi-dualmic.conf` | UCM profile for dual mic stereo |
| `usr/share/alsa/ucm2/Motorola/dubai/dubai.conf` | UCM main config |
| `root/.config/pipewire/filter-chain.conf.d/beamform.conf` | PipeWire filter chain config (needs work) |

## Installation

SSH to your device and run:

```bash
scp -r beamform-config/ root@<device-ip>:~/
ssh root@<device-ip>
cd beamform-config && ./install.sh
```

## Usage

1. Open GNOME Settings → Sound
2. Under "Input", select:
   - **"Beamformed Microphone"** or **"Dual Microphone Array"**
3. The input will be stereo (2 channels) where:
   - Left channel = Top Microphone
   - Right channel = Bottom Microphone

## Manual Filter Chain Setup

The beamforming filter chain config needs the `libfilter-chain` module to be loaded.
PipeWire 1.6.8 doesn't auto-load filter chains based on the config file.

### Option 1: Use the stereo source directly

Applications that support stereo input will see both microphones:
```
alsa_input.platform-sound.Beamform__DualMic__source
Format: s16le 2ch 48000Hz (FL=Top, FR=Bottom)
```

### Option 2: Use a custom filter script

Create a script to apply the filter manually or configure WirePlumber.

## Testing

```bash
# Check profiles
pactl list cards | grep -A5 'Profiles:'

# Set Beamform profile
pactl set-card-profile 51 'Beamform (DualMic, Speaker)'

# Verify 2-channel input
pw-cli ls Node | grep DualMic
# Should show: audio.channels = "2", audio.position = "[ FL, FR ]"

# Record test (suspend PipeWire source first)
pactl suspend-source alsa_input.platform-sound.Beamform__DualMic__source 1
arecord -D hw:Motoroladubai,2 -f cd -c 2 -d 3 /tmp/test-dualmic.wav
```

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  DualMic Source (stereo: FL=Top Mic, FR=Bottom Mic) │
│  s16le 2ch 48000Hz                                  │
└─────────────────────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────┐
│  Beamforming Filter Chain (planned)                 │
│  Left ─────┐                                        │
│            ├──(gain 0.5)──┐                         │
│  Right ──(delay 0.44ms)──┴──(gain 0.5)──▶ Mono     │
└─────────────────────────────────────────────────────┘
```

## Author

Based on the beamforming setup documentation in `.pi/docs/beamforming-setup.md`