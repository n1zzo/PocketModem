# RADE Crate

This crate provides Rust FFI bindings to the RADAEv2 radio autoencoder.

## Dependencies

The RADE C library requires:

1. **Opus library** - Downloaded and built by the radae cmake
   - Contains `nnet.h` with neural network types
   - Required headers: `opus_types.h`, `opus_defines.h`

2. **Python 3** - For the full rade_api.c (optional, we use C-only encoder/decoder)

## Building

The C library must be built separately. See the radae project:
- Location: `/Users/nizzo/Documents/pi/pocket_modem_data/radae/`
- Build: `mkdir build && cd build && cmake .. && make`

## Architecture

The crate provides two levels of abstraction:

1. **Low-level FFI** - Direct bindings to C functions
2. **High-level Rust API** - Safe wrapper around C context

## API

```rust
use rade::{Rade, RadeError, RADE_COMP};

// Initialize global state
Rade::initialize();

// Open context
let mut rade = Rade::open("", true, true)?;

// Encode features to modem waveform
let tx_samples = rade.encode(&features)?;

// Decode modem waveform to features
if let Some(features) = rade.decode(&rx_samples)? {
    // process features
}
```