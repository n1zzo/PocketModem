# Beamforming Setup for ht (Motorola Edge 30)

## Goal
Create a "Beamformed Microphone" option in GNOME Settings that all apps can use transparently.

---

## Current State (Verified Working)

### Hardware
- **Device:** Motorola Edge 30 (dubai) running postmarketOS
- **Microphones:**
  - Top Mic (Mic1) - via ADC1
  - Bottom Mic (Mic3) - via ADC2
  - 12 SoundWire mics available in hardware (SWR_MIC0-11)

### Audio Stack
- **OS:** postmarketOS / Alpine Linux (ARM64)
- **Audio:** PipeWire 1.6.8 + WirePlumber
- **UCM:** `/usr/share/alsa/ucm2/Motorola/dubai/`

### Current Profiles (Working)
- `HiFi (Mic3, Speaker)` - Bottom Mic only
- `HiFi (Mic1, Speaker)` - Top Mic only  
- `DualMic` - **Both mics as stereo** (FL=Top, FR=Bottom)

### Stereo Source
When DualMic profile is active:
```
alsa_input.platform-sound.DualMic__DualMic__source
Format: s16le 2ch 48000Hz (FL=Top Mic, FR=Bottom Mic)
```

---

## Available LADSPA Plugins

| Plugin | Path | Purpose |
|--------|------|---------|
| amp.so | /usr/lib/ladspa/ | Gain/volume |
| delay.so | /usr/lib/ladspa/ | Delay (seconds parameter) |
| lsp-plugins-ladspa.so | /usr/lib/ladspa/ | Contains mixer, delay filters |

---

## Implementation Plan

### Step 1: Create UCM Profile "Beamform"
**File:** `/usr/share/alsa/ucm2/Motorola/dubai/HiFi-beamform.conf`

Purpose: Create a mono source that combines both mics with beamforming.

Note: UCM can enable both ADCs but cannot do real-time DSP. The beamforming processing will be done by PipeWire filter chain.

### Step 2: Create PipeWire Filter Chain
**File:** `~/.config/pipewire/filter-chain.conf.d/beamform.conf`

Purpose: Apply delay-and-sum beamforming to the stereo DualMic input.

Filter graph:
```
Input (2ch) → Split → Left (FL) ─────────────────────┐
                    └── Right (FR) → Delay → Mix → Output (mono)
```

Delay value: ~21 samples at 48kHz (0.44ms for ~15cm mic spacing)

### Step 3: WirePlumber Auto-Link Rule
**File:** `~/.config/wireplumber/main.lua`

Purpose: Automatically link the DualMic source through the beamforming filter when Beamform profile is active.

### Step 4: Update dubai.conf
**File:** `/usr/share/alsa/ucm2/Motorola/dubai/dubai.conf`

Add:
```
SectionUseCase."Beamform" {
    File "/Motorola/dubai/HiFi-beamform.conf"
    Comment "Beamformed Microphone Array."
}
```

---

## Files to Create/Modify

| File | Action | Content |
|------|--------|---------|
| `/usr/share/alsa/ucm2/Motorola/dubai/HiFi-beamform.conf` | Create | UCM profile for beamformed mono source |
| `/usr/share/alsa/ucm2/Motorola/dubai/dubai.conf` | Modify | Add Beamform SectionUseCase |
| `~/.config/pipewire/filter-chain.conf.d/beamform.conf` | Create | PipeWire filter chain config |
| `~/.config/wireplumber/main.lua` | Create | Auto-link rule |

---

## Key ALSA Mixer Controls

| Control | NumID | Default | For DualMic |
|---------|-------|---------|-------------|
| ADC1 Switch | 25 | 0 | 1 |
| ADC2 Switch | 26 | 0 | 1 |
| ADC1_MIXER Switch | 1104 | 0 | 1 |
| ADC2_MIXER Switch | 1105 | 0 | 1 |

---

## Testing Checklist

- [ ] GNOME Settings shows "Beamformed Microphone" option
- [ ] Selecting Beamformed uses both mics
- [ ] Output is mono (beamformed)
- [ ] Noise from sides is reduced
- [ ] Works with all apps (PocketModem, browser, etc.)

---

## Rollback Commands

```bash
# Restore original UCM
sudo apk add -f alsa-ucm-conf-qcom-sc7280

# Remove custom configs
rm -rf ~/.config/alsa/
rm -rf ~/.config/pipewire/filter-chain.conf.d/
rm ~/.config/wireplumber/main.lua

# Restart audio
systemctl --user restart pipewire wireplumber
```