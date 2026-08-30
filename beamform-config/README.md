# Beamforming Configuration for Motorola Edge 30

Configuration for dual microphone beamforming on the Motorola Edge 30 (dubai) running postmarketOS.

## Status

| Feature | Status |
|---------|--------|
| UCM profiles (Beamform, DualMic) | ✅ Working |
| DualMic 2-channel stereo source | ✅ Working |
| GNOME Settings integration | ✅ Working |
| WebRTC beamforming | ❌ AEC library crashes on this device |

## What Works

1. **UCM Profiles**: Select "Microphone Array" in GNOME Settings → Sound → Input
2. **Stereo Input**: DualMic provides 2-channel stereo:
   - Left = Top Microphone  
   - Right = Bottom Microphone
3. Applications receive stereo audio from both mics

## Files

```
beamform-config/
├── usr/share/alsa/ucm2/Motorola/dubai/
│   ├── dubai.conf              # Main UCM config
│   ├── HiFi-beamform.conf      # Beamform profile
│   └── HiFi-dualmic.conf       # DualMic profile
├── root/.config/pipewire/
│   ├── pipewire.conf           # Main PipeWire config (backup)
│   └── filter-chain.conf.d/
│       └── beamform-ladspa.conf # LADSPA filter (not loading)
└── install.sh                  # Installation script
```

## Installation

```bash
scp -r beamform-config/ root@<device>:~/
ssh root@<device>
cd beamform-config && ./install.sh
```

## Usage

1. Open GNOME Settings → Sound → Input
2. Select "Microphone Array" 
3. Applications receive: `alsa_input.platform-sound.Beamform__DualMic__source`
   - Format: s16le 2ch 48000Hz
   - FL = Top Mic, FR = Bottom Mic

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  DualMic Source                                     │
│  s16le 2ch 48000Hz (FL=Top, FR=Bottom)            │
└─────────────────────────────────────────────────────┘
                           │
                           ▼
              ┌────────────────────────┐
              │  Beamforming Filter    │
              │  (WebRTC AEC attempted,│
              │   LADSPA fallback)     │
              └────────────────────────┘
                           │
                           ▼
              ┌────────────────────────┐
              │  Mono Beamformed Audio │
              └────────────────────────┘
```

## Notes

- The WebRTC AEC library (`libspa-aec-webrtc.so`) crashes with "Illegal instruction" on this device
- The LADSPA filter chain config is created but doesn't auto-load in PipeWire 1.6.8
- As a workaround, applications can use the stereo source directly and do their own beamforming
- The stereo separation provides some directional benefit even without explicit beamforming