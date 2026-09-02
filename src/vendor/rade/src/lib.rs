//! RADAEv2 Radio Autoencoder FFI Bindings

#![allow(dead_code)]
#![allow(non_camel_case_types)]

use std::os::raw::c_char;
use std::sync::OnceLock;

// ============================================================================
// Manual FFI Bindings (for portability across platforms)
// ============================================================================

/// RADE_COMP - single-precision complex number (matches C struct)
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RADE_COMP {
    pub real: f32,
    pub imag: f32,
}

/// Opaque RADE context pointer (defined in C)
#[repr(C)]
pub struct rade {
    _private: [u8; 0],
}

// C function declarations
extern "C" {
    fn rade_initialize();
    fn rade_finalize();
    fn rade_open(model_file: *mut c_char, flags: libc::c_int) -> *mut rade;
    fn rade_close(r: *mut rade);
    fn rade_version() -> libc::c_int;
    fn rade_n_tx_out(r: *mut rade) -> libc::c_int;
    fn rade_n_tx_eoo_out(r: *mut rade) -> libc::c_int;
    fn rade_tx(r: *mut rade, tx_out: *mut RADE_COMP, features_in: *mut f32) -> libc::c_int;
    fn rade_tx_eoo(r: *mut rade, tx_eoo_out: *mut RADE_COMP) -> libc::c_int;
    fn rade_nin_max(r: *mut rade) -> libc::c_int;
    fn rade_nin(r: *mut rade) -> libc::c_int;
    fn rade_n_features_in_out(r: *mut rade) -> libc::c_int;
    fn rade_rx(r: *mut rade, features_out: *mut f32, rx_in: *mut RADE_COMP) -> libc::c_int;
    fn rade_sync(r: *mut rade) -> libc::c_int;
    fn rade_freq_offset(r: *mut rade) -> f32;
}

pub use RADE_COMP as RadeComp;
pub use rade as RadeContext;

// ============================================================================
// Constants (must match C definitions)
// ============================================================================

pub const RADE_MODEM_SAMPLE_RATE: i32 = 8000;
pub const RADE_SPEECH_SAMPLE_RATE: i32 = 16000;
pub const RADE_USE_C_ENCODER: i32 = 0x1;
pub const RADE_USE_C_DECODER: i32 = 0x2;
pub const RADE_FOFF_TEST: i32 = 0x4;
pub const RADE_VERBOSE_0: i32 = 0x8;
pub const RADE_LATENT_DIM: i32 = 80;
pub const RADE_FRAMES_PER_STEP: i32 = 4;
pub const RADE_FEATURES_PER_FRAME: i32 = 20;
pub const RADE_FEATURES_PER_STEP: i32 = 80;
pub const RADE_TX_FRAME_SIZE: i32 = 512;
pub const RADE_TX_EOO_SIZE: i32 = 256;
pub const RADE_NIN_MAX: i32 = 3200;

// ============================================================================
// High-level Rust API
// ============================================================================

/// RADE library errors
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RadeError {
    /// Failed to open RADE context
    OpenFailed,
    /// Invalid model file path
    InvalidPath,
    /// Invalid feature array size
    InvalidFeatureSize { expected: usize, actual: usize },
    /// Invalid RX sample array size
    InvalidRxSize { expected: usize, actual: usize },
    /// Encoder failed
    EncodeFailed,
    /// Decoder failed
    DecodeFailed,
}

impl std::fmt::Display for RadeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RadeError::OpenFailed => write!(f, "Failed to open RADE context"),
            RadeError::InvalidPath => write!(f, "Invalid model file path"),
            RadeError::InvalidFeatureSize { expected, actual } => {
                write!(f, "Invalid feature size: expected {}, got {}", expected, actual)
            }
            RadeError::InvalidRxSize { expected, actual } => {
                write!(f, "Invalid RX size: expected {}, got {}", expected, actual)
            }
            RadeError::EncodeFailed => write!(f, "Encoder failed"),
            RadeError::DecodeFailed => write!(f, "Decoder failed"),
        }
    }
}

impl std::error::Error for RadeError {}

/// High-level RADE encoder/decoder wrapper
pub struct Rade {
    ptr: *mut RadeContext,
    tx_out: Vec<RADE_COMP>,
    tx_eoo_out: Vec<RADE_COMP>,
    rx_buf: Vec<RADE_COMP>,
    features_in: Vec<f32>,
    features_out: Vec<f32>,
}

impl Rade {
    /// Initialize the RADE library (global initialization)
    pub fn initialize() {
        static INIT: OnceLock<()> = OnceLock::new();
        INIT.get_or_init(|| unsafe {
            rade_initialize();
        });
    }

    /// Open a new RADE context
    pub fn open(
        model_file: &str,
        use_c_encoder: bool,
        use_c_decoder: bool,
    ) -> Result<Self, RadeError> {
        let flags = if use_c_encoder { RADE_USE_C_ENCODER } else { 0 }
            | if use_c_decoder { RADE_USE_C_DECODER } else { 0 };

        let model_path = if model_file.is_empty() {
            std::ptr::null_mut()
        } else {
            std::ffi::CString::new(model_file)
                .map_err(|_| RadeError::InvalidPath)?
                .into_raw() as *mut c_char
        };

        let ptr = unsafe { rade_open(model_path, flags) };
        if ptr.is_null() {
            return Err(RadeError::OpenFailed);
        }

        let n_tx_out = unsafe { rade_n_tx_out(ptr) } as usize;
        let n_tx_eoo = unsafe { rade_n_tx_eoo_out(ptr) } as usize;
        let nin_max = unsafe { rade_nin_max(ptr) } as usize;
        let n_features = unsafe { rade_n_features_in_out(ptr) } as usize;

        Ok(Self {
            ptr,
            tx_out: vec![RADE_COMP { real: 0.0, imag: 0.0 }; n_tx_out],
            tx_eoo_out: vec![RADE_COMP { real: 0.0, imag: 0.0 }; n_tx_eoo],
            rx_buf: vec![RADE_COMP { real: 0.0, imag: 0.0 }; nin_max],
            features_in: vec![0.0; n_features],
            features_out: vec![0.0; n_features],
        })
    }

    pub fn version() -> i32 {
        unsafe { rade_version() }
    }

    pub fn n_tx_out(&self) -> usize {
        unsafe { rade_n_tx_out(self.ptr) as usize }
    }

    pub fn n_tx_eoo_out(&self) -> usize {
        unsafe { rade_n_tx_eoo_out(self.ptr) as usize }
    }

    pub fn nin_max(&self) -> usize {
        unsafe { rade_nin_max(self.ptr) as usize }
    }

    pub fn n_features(&self) -> usize {
        unsafe { rade_n_features_in_out(self.ptr) as usize }
    }

    /// Encode speech features to modem waveform
    pub fn encode(&mut self, features: &[f32]) -> Result<&[RADE_COMP], RadeError> {
        if features.len() != self.features_in.len() {
            return Err(RadeError::InvalidFeatureSize {
                expected: self.features_in.len(),
                actual: features.len(),
            });
        }

        self.features_in.copy_from_slice(features);

        let n_out = unsafe {
            rade_tx(
                self.ptr,
                self.tx_out.as_mut_ptr(),
                self.features_in.as_mut_ptr(),
            )
        } as usize;

        if n_out == 0 {
            return Err(RadeError::EncodeFailed);
        }

        Ok(&self.tx_out[..n_out])
    }

    /// Get end-of-over TX samples
    pub fn encode_eoo(&mut self) -> Result<&[RADE_COMP], RadeError> {
        let n_out = unsafe { rade_tx_eoo(self.ptr, self.tx_eoo_out.as_mut_ptr()) } as usize;

        if n_out == 0 {
            return Err(RadeError::EncodeFailed);
        }

        Ok(&self.tx_eoo_out[..n_out])
    }

    /// Get number of RX input samples needed
    pub fn nin(&self) -> usize {
        unsafe { rade_nin(self.ptr) as usize }
    }

    /// Decode modem waveform to speech features
    pub fn decode(&mut self, rx_samples: &[RADE_COMP]) -> Result<Option<&[f32]>, RadeError> {
        let nin = self.nin();
        if rx_samples.len() < nin {
            return Err(RadeError::InvalidRxSize {
                expected: nin,
                actual: rx_samples.len(),
            });
        }

        self.rx_buf[..nin].copy_from_slice(&rx_samples[..nin]);

        let result = unsafe {
            rade_rx(
                self.ptr,
                self.features_out.as_mut_ptr(),
                self.rx_buf.as_mut_ptr(),
            )
        };

        if result != 0 {
            Ok(Some(&self.features_out))
        } else {
            Ok(None)
        }
    }

    /// Check if receiver is in sync
    pub fn is_synced(&self) -> bool {
        unsafe { rade_sync(self.ptr) != 0 }
    }

    /// Get frequency offset estimate
    pub fn freq_offset(&self) -> f32 {
        unsafe { rade_freq_offset(self.ptr) }
    }
}

unsafe impl Send for Rade {}
unsafe impl Sync for Rade {}

impl Drop for Rade {
    fn drop(&mut self) {
        unsafe { rade_close(self.ptr) }
    }
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants_match() {
        assert_eq!(RADE_MODEM_SAMPLE_RATE, 8000);
        assert_eq!(RADE_SPEECH_SAMPLE_RATE, 16000);
        assert_eq!(RADE_LATENT_DIM, 80);
        assert_eq!(RADE_FRAMES_PER_STEP, 4);
        assert_eq!(RADE_FEATURES_PER_FRAME, 20);
        assert_eq!(RADE_FEATURES_PER_STEP, 80);
    }

    #[test]
    fn test_rade_comp_operations() {
        let a = RADE_COMP { real: 3.0, imag: 4.0 };
        let b = RADE_COMP { real: 1.0, imag: 2.0 };
        let c = RADE_COMP {
            real: a.real + b.real,
            imag: a.imag + b.imag,
        };
        assert!((c.real - 4.0).abs() < 1e-6);
        assert!((c.imag - 6.0).abs() < 1e-6);
    }

    #[test]
    fn test_rade_error_display() {
        let err = RadeError::InvalidFeatureSize { expected: 10, actual: 5 };
        let s = err.to_string();
        assert!(s.contains("10"));
        assert!(s.contains("5"));
    }
}
