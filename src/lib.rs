//! PocketModem Library
//! 
//! This module exposes internal components for testing.

pub mod aprs;
pub mod aprs_icons;
pub mod audio;
pub mod geoclue;
pub mod gps;
pub mod kiss;
pub mod map;
pub mod radio;
pub mod rade_ffi;
pub mod settings;
pub mod utils;

// Re-exports for convenience
pub use aprs::{parse_callsign, base_callsign, ssid};
pub use radio::{KV4PRadio, SerialConfig};