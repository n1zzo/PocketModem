//! Audio I/O for PocketModem with ADPCM encoding/decoding
//!
//! Matches the KV4P HT firmware audio pipeline:
//!   RX: ADC → DC offset removal → 16x gain → squelch mute → ADPCM (IMA WAV, 16kHz, 4-bit)
//!   TX: ADPCM decode (IMA WAV, 16kHz) → I2S DAC (PDM)
//!
//! Uses cpal for capture and playback, oxideav-adpcm for ADPCM decoding.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

// IMA ADPCM tables (same as oxideav-adpcm)
const IMA_STEP_SIZE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

const IMA_INDEX_ADJUST: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

// Audio parameters - KV4P uses IMA WAV ADPCM at 16kHz
const AUDIO_WIRE_SAMPLE_RATE: u32 = 16000;
const AUDIO_FRAME_SAMPLES: usize = 249; // 128-byte ADPCM block decodes to 249 samples
const ADPCM_FRAME_BYTES: usize = 128; // ADPCM block size
const CHANNELS: u16 = 1;
const OUTPUT_SAMPLE_RATE: u32 = 16000; // Play at native 16kHz (match Android app)
const OUTPUT_FRAME_SIZE: usize = 320; // 20ms at 16kHz

/// Audio configuration
#[derive(Debug, Clone)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub tx_gain: f32,
    pub rx_gain: f32,
    pub gate_threshold: f32,
    pub pre_emphasis_alpha: f32,
    pub hard_limit: f32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: AUDIO_WIRE_SAMPLE_RATE,
            tx_gain: 2.0,
            rx_gain: 1.0,
            gate_threshold: 0.005,
            pre_emphasis_alpha: 0.0,
            hard_limit: 0.95,
        }
    }
}

/// TX Audio callback - called with ADPCM-encoded audio frames
pub type TxAudioCallback = Arc<Mutex<Option<Box<dyn FnMut(&[u8]) + Send>>>>;

/// RX Audio handler - receives ADPCM-encoded frames  
pub type RxAudioCallback = Arc<Mutex<Option<Box<dyn FnMut(&[u8]) + Send>>>>;

/// Audio state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioState {
    Idle,
    Capturing,
    Playing,
    Both,
}

impl Default for AudioState {
    fn default() -> Self { Self::Idle }
}

/// Audio LED status for UI display
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioLedStatus {
    /// Audio thread not started
    NotStarted,
    /// Audio thread started but squelch closed (no audio coming)
    SquelchClosed,
    /// Audio coming through
    AudioActive,
    /// Error with audio decoding
    Error,
}

/// DC offset remover (exponential decay)
pub struct DCOffsetRemover {
    alpha: f32,
    prev_y: f32,
}

impl DCOffsetRemover {
    pub fn new(decay_time: f32, sample_rate: f32) -> Self {
        let alpha = 1.0 - (-1.0 / (sample_rate * (decay_time / 2.0_f32.ln()))).exp();
        Self { alpha, prev_y: 0.0 }
    }

    pub fn process(&mut self, samples: &mut [i16]) {
        for s in samples.iter_mut() {
            let x = *s as f32;
            self.prev_y = self.alpha * x + (1.0 - self.alpha) * self.prev_y;
            let out = x - self.prev_y;
            *s = out.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        }
    }

    pub fn reset(&mut self) {
        self.prev_y = 0.0;
    }
}

/// Pre-emphasis filter (high-frequency boost for TX)
pub struct PreEmphasis {
    alpha: f32,
    prev: f32,
}

impl PreEmphasis {
    pub fn new(alpha: f32) -> Self {
        Self { alpha, prev: 0.0 }
    }
    
    pub fn process(&mut self, samples: &mut [i16]) {
        // y[n] = x[n] - α * x[n-1]
        // This provides a high-shelf boost
        for s in samples.iter_mut() {
            let x = *s as f32;
            let out = x - self.alpha * self.prev;
            self.prev = x;
            *s = out.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        }
    }
    
    pub fn reset(&mut self) {
        self.prev = 0.0;
    }
}

/// De-emphasis filter (high-frequency cut for RX)
pub struct DeEmphasis {
    alpha: f32,
    prev: f32,
}

impl DeEmphasis {
    pub fn new(alpha: f32) -> Self {
        Self { alpha, prev: 0.0 }
    }
    
    pub fn process(&mut self, samples: &mut [i16]) {
        // y[n] = x[n] + α * y[n-1]
        // This provides a high-shelf cut (low-pass characteristic)
        for s in samples.iter_mut() {
            let x = *s as f32;
            let out = x + self.alpha * self.prev;
            self.prev = out;
            *s = out.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        }
    }
    
    pub fn reset(&mut self) {
        self.prev = 0.0;
    }
}

/// Volume ramp for smooth attack/release
pub struct VolumeRamp {
    alpha: f32,
    threshold: f32,
    volume: f32,
    active: bool,
}

impl VolumeRamp {
    pub fn new(alpha: f32, threshold: f32) -> Self {
        Self { alpha, threshold, volume: 0.0, active: false }
    }

    pub fn start(&mut self) {
        self.volume = 0.0;
        self.active = true;
    }

    pub fn stop(&mut self) {
        self.active = false;
        self.volume = 0.0;
    }

    pub fn process(&mut self, samples: &mut [i16]) {
        if !self.active {
            return;
        }

        // Ramp up
        self.volume = self.alpha + (1.0 - self.alpha) * self.volume;

        // Below threshold: mute
        if self.volume < self.threshold {
            for s in samples.iter_mut() { *s = 0; }
            return;
        }

        // At or above threshold: apply volume
        if self.volume >= 0.99 {
            return;
        }

        for s in samples.iter_mut() {
            *s = ((*s as f32) * self.volume).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        }
    }

    pub fn is_ramped(&self) -> bool {
        self.volume >= self.threshold
    }
}

/// IMA WAV ADPCM decoder - matches the Android app implementation.
/// 
/// Each block is decoded independently using the block header's predictor/step_index.
/// This is standard IMA WAV format behavior.
pub struct ADPCMDecoder {
    predictor: i32,
    step_index: i32,
}

impl ADPCMDecoder {
    pub fn new() -> Self {
        Self { predictor: 0, step_index: 0 }
    }

    /// Decode an IMA WAV ADPCM block (128 bytes -> 249 samples).
    pub fn decode_block(&mut self, block: &[u8], _is_first_block: bool) -> Result<Vec<i16>, String> {
        if block.len() < 4 {
            return Err("Block too short for IMA WAV header".to_string());
        }

        let predictor_i16 = i16::from_le_bytes([block[0], block[1]]);
        self.predictor = predictor_i16 as i32;
        self.step_index = (block[2] as i32).clamp(0, 88);

        let body = &block[4..];
        let groups = body.len() / 4;
        let samples = 1 + groups * 8;
        let mut out = Vec::with_capacity(samples);

        out.push(self.predictor as i16);

        for group in body.chunks(4) {
            for &byte in group {
                let n_lo = (byte & 0x0F) as u8;
                let n_hi = ((byte >> 4) & 0x0F) as u8;
                out.push(self.expand_nibble(n_lo));
                out.push(self.expand_nibble(n_hi));
            }
        }

        Ok(out)
    }

    /// Expand one IMA ADPCM nibble into a PCM sample.
    /// Matches the Java decodeNibble() implementation.
    fn expand_nibble(&mut self, code: u8) -> i16 {
        let step = IMA_STEP_SIZE[self.step_index as usize];
        
        // delta calculation (matches Java)
        let mut delta = step >> 3;
        if (code & 4) != 0 { delta += step; }
        if (code & 2) != 0 { delta += step >> 1; }
        if (code & 1) != 0 { delta += step >> 2; }

        // Apply sign
        if (code & 8) != 0 {
            self.predictor -= delta;
        } else {
            self.predictor += delta;
        }

        // Clamp to i16 range (matches Java clampInt16)
        self.predictor = self.predictor.clamp(i16::MIN as i32, i16::MAX as i32);

        // Update step index (matches Java clampIndex)
        self.step_index += IMA_INDEX_ADJUST[code as usize];
        self.step_index = self.step_index.clamp(0, 88);

        self.predictor as i16
    }

    /// Reset decoder state
    pub fn reset(&mut self) {
        self.predictor = 0;
        self.step_index = 0;
    }
}

impl Default for ADPCMDecoder {
    fn default() -> Self { Self::new() }
}

/// ADPCM frame state for encoding (maintains predictor across frames)
pub struct ADPCMEncoder {
    predictor: i32,
    step_index: i32,
}

impl ADPCMEncoder {
    pub fn new() -> Self {
        Self { predictor: 0, step_index: 0 }
    }
    
    /// Reset encoder state
    pub fn reset(&mut self) {
        self.predictor = 0;
        self.step_index = 0;
    }

    /// Encode PCM samples to IMA WAV ADPCM block.
    /// 
    /// Matches the Java/Android implementation:
    /// - Little-endian predictor bytes
    /// - Low nibble first, then high nibble
    /// - 249 samples from 128 bytes
    pub fn encode(&mut self, samples: &[i16]) -> Vec<u8> {
        // 128 bytes per block: 4 byte header + 124 bytes data = 249 samples
        let mut out = vec![0u8; ADPCM_FRAME_BYTES];
        
        if samples.is_empty() {
            return out;
        }
        
        // Initialize state with first sample
        self.predictor = samples[0] as i32;
        
        // Write header: predictor (2 bytes, little-endian), step_index (1 byte), reserved (1 byte = 0)
        out[0] = (self.predictor & 0xFF) as u8;
        out[1] = ((self.predictor >> 8) & 0xFF) as u8;
        out[2] = self.step_index as u8;
        out[3] = 0;
        
        // Encode remaining samples (249 - 1 = 248 samples = 124 bytes)
        let mut out_idx = 4;
        let mut high_nibble = false;
        let mut packed = 0u8;
        
        for &sample in &samples[1..] {
            let code = self.encode_nibble(sample as i32);
            
            if !high_nibble {
                packed = code & 0x0F;
                high_nibble = true;
            } else {
                out[out_idx] = packed | (code << 4);
                out_idx += 1;
                high_nibble = false;
            }
        }
        
        // Pad last nibble if odd number of samples
        if high_nibble {
            out[out_idx] = packed;
        }
        
        out
    }
    
    /// Encode a single sample and return the 4-bit code
    fn encode_nibble(&mut self, sample: i32) -> u8 {
        let diff = sample - self.predictor;
        let mut code = 0u8;
        let mut abs_diff = diff.abs();
        
        if diff < 0 {
            code = 8; // negative sign
        }
        
        let step = IMA_STEP_SIZE[self.step_index as usize];
        let mut delta = step >> 3;
        
        if abs_diff >= step {
            code |= 4;
            abs_diff -= step;
            delta += step;
        }
        if abs_diff >= step >> 1 {
            code |= 2;
            abs_diff -= step >> 1;
            delta += step >> 1;
        }
        if abs_diff >= step >> 2 {
            code |= 1;
            delta += step >> 2;
        }
        
        // Update predictor
        if (code & 8) != 0 {
            self.predictor -= delta;
        } else {
            self.predictor += delta;
        }
        
        // Clamp predictor
        self.predictor = self.predictor.clamp(i16::MIN as i32, i16::MAX as i32);
        
        // Update step index
        self.step_index += IMA_INDEX_ADJUST[code as usize];
        self.step_index = self.step_index.clamp(0, 88);
        
        code
    }
}

impl Default for ADPCMEncoder {
    fn default() -> Self { Self::new() }
}

/// Main audio manager
pub struct AudioManager {
    config: AudioConfig,
    encoder: ADPCMEncoder,
    decoder: ADPCMDecoder,  // Stateful ADPCM decoder for smooth block transitions
    tx_callback: TxAudioCallback,
    rx_callback: RxAudioCallback,
    tx_enabled: Arc<AtomicBool>,
    rx_enabled: Arc<AtomicBool>,
    rx_volume: Arc<AtomicU32>,  // Volume for RX (starts at 0, ramps up via AtomicU32 float bits)
    rx_first_block: Arc<AtomicBool>,  // Track first block for ADPCM state continuity
    state: Arc<Mutex<AudioState>>,
    audio_ever_started: Arc<AtomicBool>,  // Track if audio thread has ever been started
    has_decode_error: Arc<AtomicBool>,     // Track decode errors for LED status
    decode_error_count: Arc<AtomicU32>,    // Count decode errors
    dc_remover: Arc<Mutex<DCOffsetRemover>>,
    dc_remover_rx: Arc<Mutex<DCOffsetRemover>>,  // Separate DC remover for RX
    volume_ramp: Arc<Mutex<VolumeRamp>>,
    pre_emphasis: Arc<Mutex<PreEmphasis>>,
    de_emphasis: Arc<Mutex<DeEmphasis>>,
    de_emphasis_enabled: bool,
    playback_buf: Arc<Mutex<Vec<i16>>>,
}

impl AudioManager {
    pub fn new(config: AudioConfig) -> Self {
        let pre_emph = PreEmphasis::new(config.pre_emphasis_alpha);
        let de_emph = DeEmphasis::new(0.85); // Standard de-emphasis coefficient
        Self {
            config,
            encoder: ADPCMEncoder::new(),
            decoder: ADPCMDecoder::new(),
            tx_callback: Arc::new(Mutex::new(None)),
            rx_callback: Arc::new(Mutex::new(None)),
            tx_enabled: Arc::new(AtomicBool::new(false)),
            rx_enabled: Arc::new(AtomicBool::new(false)),
            rx_volume: Arc::new(AtomicU32::new(0)),
            rx_first_block: Arc::new(AtomicBool::new(true)),  // Track first block for ADPCM continuity
            state: Arc::new(Mutex::new(AudioState::Idle)),
            audio_ever_started: Arc::new(AtomicBool::new(false)),
            has_decode_error: Arc::new(AtomicBool::new(false)),
            decode_error_count: Arc::new(AtomicU32::new(0)),
            dc_remover: Arc::new(Mutex::new(DCOffsetRemover::new(0.25, AUDIO_WIRE_SAMPLE_RATE as f32))),
            dc_remover_rx: Arc::new(Mutex::new(DCOffsetRemover::new(0.05, AUDIO_WIRE_SAMPLE_RATE as f32))),  // Faster DC removal for RX
            volume_ramp: Arc::new(Mutex::new(VolumeRamp::new(0.05, 0.7))),
            pre_emphasis: Arc::new(Mutex::new(pre_emph)),
            de_emphasis: Arc::new(Mutex::new(de_emph)),
            de_emphasis_enabled: false,  // Default off to match Android (no de-emphasis)
            playback_buf: Arc::new(Mutex::new(Vec::new())),
        }
    }
    

    
    pub fn on_tx_audio<F>(&mut self, callback: F) 
    where F: FnMut(&[u8]) + Send + 'static {
        *self.tx_callback.lock().unwrap() = Some(Box::new(callback));
    }
    
    /// Set RX audio callback - call this with audio manager locked
    pub fn on_rx_audio<F>(&mut self, mut callback: F)
    where F: FnMut(&[u8]) + Send + 'static {
        *self.rx_callback.lock().unwrap() = Some(Box::new(move |data: &[u8]| {
            callback(data);
        }));
    }
    

    
    /// Internal: call accumulate_rx_audio and start playback if ready
    /// Matches Android app exactly: just buffer audio and start playback.
    pub fn accumulate_and_start(&mut self, adpcm_data: &[u8]) {
        // Decode real audio from radio
        self.accumulate_rx_audio(adpcm_data);
        
        let buf_len = self.playback_buf.lock().unwrap().len();
        
        // Start playback when buffer is ready
        if !self.rx_enabled.load(Ordering::SeqCst) && buf_len >= 249 {
            let _ = self.start_playback_volume_zero();
            self.rx_volume.store(1.0f32.to_bits(), Ordering::SeqCst);
        }
    }
    
    /// Start playback with volume at 0, matching Android's ensureAudioPlaying()
    /// Volume will ramp up slowly (alpha=0.02) via the volume_ramp
    fn start_playback_volume_zero(&mut self) -> Result<(), String> {
        if self.rx_enabled.load(Ordering::SeqCst) {
            return Ok(());
        }
        
        let config = self.config.clone();
        let rx_enabled = Arc::clone(&self.rx_enabled);
        let rx_enabled_for_state = Arc::clone(&self.rx_enabled);
        let rx_enabled_for_monitor = Arc::clone(&self.rx_enabled);
        let state = Arc::clone(&self.state);
        let playback_buf = Arc::clone(&self.playback_buf);
        let playback_buf_for_monitor = Arc::clone(&self.playback_buf);
        let rx_volume = Arc::clone(&self.rx_volume);
        
        // CRITICAL: Set enabled flag BEFORE spawning thread
        self.rx_enabled.store(true, Ordering::SeqCst);
        self.audio_ever_started.store(true, Ordering::SeqCst);
        *self.state.lock().unwrap() = AudioState::Playing;
        
        // Reset volume ramp to 0 (matches Android: audioTrackVolume = 0)
        rx_volume.store(0, Ordering::SeqCst);
        self.rx_first_block.store(true, Ordering::SeqCst);  // Reset ADPCM state continuity
        
        // Spawn playback loop
        thread::spawn(move || {
            // Need self to call playback_loop - clone the needed Arcs
            let rx_vol = Arc::clone(&rx_volume);
            let config_clone = config.clone();
            let enabled_clone = Arc::clone(&rx_enabled);
            let buf_clone = Arc::clone(&playback_buf);
            // Can't call &self in a spawned thread, use Self::playback_loop_static
            if let Err(e) = Self::playback_loop_static(&rx_vol, &config_clone, enabled_clone, buf_clone) {
                eprintln!("[audio] Playback error: {}", e);
            }
            rx_enabled_for_state.store(false, Ordering::SeqCst);
            *state.lock().unwrap() = AudioState::Idle;
        });
        thread::spawn(move || {
            let mut underrun_count = 0;
            let mut last_level = 0usize;
            while rx_enabled_for_monitor.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(500));
                let buf = playback_buf_for_monitor.lock().unwrap();
                let level = buf.len();
                if level < OUTPUT_FRAME_SIZE && last_level >= OUTPUT_FRAME_SIZE {
                    underrun_count += 1;
                }
                last_level = level;
            }
            if underrun_count > 0 {
                eprintln!("[audio] Playback ended with {} underruns", underrun_count);
            }
        });
        
        Ok(())
    }
    
    pub fn start_capture(&mut self) -> Result<(), String> {
        if self.tx_enabled.load(Ordering::SeqCst) { return Ok(()); }
        
        let config = self.config.clone();
        let tx_callback = Arc::clone(&self.tx_callback);
        let tx_enabled = Arc::clone(&self.tx_enabled);
        let state = Arc::clone(&self.state);
        let dc_remover = Arc::clone(&self.dc_remover);
        let volume_ramp = Arc::clone(&self.volume_ramp);
        let pre_emphasis = Arc::clone(&self.pre_emphasis);
        
        volume_ramp.lock().unwrap().start();
        
        // Set enabled flag BEFORE spawning thread to avoid race
        self.tx_enabled.store(true, Ordering::SeqCst);
        self.audio_ever_started.store(true, Ordering::SeqCst);
        *self.state.lock().unwrap() = AudioState::Capturing;
        
        thread::spawn(move || {
            if let Err(e) = Self::capture_loop(&config, tx_callback, tx_enabled.clone(), 
                                                dc_remover, volume_ramp, pre_emphasis) {
                eprintln!("[audio] Capture error: {}", e);
            }
            tx_enabled.store(false, Ordering::SeqCst);
            *state.lock().unwrap() = AudioState::Idle;
        });
        
        Ok(())
    }
    
    pub fn stop_capture(&mut self) {
        self.tx_enabled.store(false, Ordering::SeqCst);
        self.volume_ramp.lock().unwrap().stop();
        self.dc_remover.lock().unwrap().reset();
        self.pre_emphasis.lock().unwrap().reset();
        let mut s = self.state.lock().unwrap();
        if *s == AudioState::Capturing { *s = AudioState::Idle; }
    }
    
    /// Enable or disable pre-emphasis filter (TX)
    pub fn set_pre_emphasis(&mut self, enabled: bool, alpha: f32) {
        if enabled {
            *self.pre_emphasis.lock().unwrap() = PreEmphasis::new(alpha);
            eprintln!("[audio] Pre-emphasis enabled, alpha={}", alpha);
        } else {
            *self.pre_emphasis.lock().unwrap() = PreEmphasis::new(0.0);
            eprintln!("[audio] Pre-emphasis disabled");
        }
    }
    
    /// Enable or disable de-emphasis filter (RX)
    pub fn set_de_emphasis(&mut self, enabled: bool) {
        self.de_emphasis_enabled = enabled;
        if enabled {
            self.de_emphasis.lock().unwrap().reset();  // Reset state when enabling
            eprintln!("[audio] De-emphasis enabled");
        } else {
            eprintln!("[audio] De-emphasis disabled");
        }
    }
    
    /// Reset de-emphasis filter state
    pub fn reset_de_emphasis(&mut self) {
        self.de_emphasis.lock().unwrap().reset();
    }
    
    pub fn start_playback(&mut self) -> Result<(), String> {
        // Use SeqCst load+store to prevent concurrent calls
        // Note: rx_enabled is also read by the playback callback, so we need atomic ops
        if self.rx_enabled.load(Ordering::SeqCst) {
            eprintln!("[audio] start_playback: already enabled, skipping");
            return Ok(());
        }
        
        let config = self.config.clone();
        let _rx_callback = Arc::clone(&self.rx_callback);
        let rx_enabled = Arc::clone(&self.rx_enabled);
        let rx_enabled_for_state = Arc::clone(&self.rx_enabled);
        let rx_enabled_for_monitor = Arc::clone(&self.rx_enabled);
        let state = Arc::clone(&self.state);
        let playback_buf = Arc::clone(&self.playback_buf);
        let playback_buf_for_monitor = Arc::clone(&self.playback_buf);
        let rx_volume = Arc::clone(&self.rx_volume);
        
        // CRITICAL: Set enabled flag BEFORE spawning thread
        // cpal stream starts asynchronously and callbacks may run immediately
        self.rx_enabled.store(true, Ordering::SeqCst);
        self.audio_ever_started.store(true, Ordering::SeqCst);
        *self.state.lock().unwrap() = AudioState::Playing;
        
        // Reset volume ramp
        rx_volume.store(0, Ordering::SeqCst);
        self.rx_first_block.store(true, Ordering::SeqCst);  // Reset ADPCM state continuity
        
        // Spawn playback loop
        thread::spawn(move || {
            if let Err(e) = Self::playback_loop_static(&rx_volume, &config, rx_enabled.clone(), playback_buf) {
                eprintln!("[audio] Playback error: {}", e);
            }
            rx_enabled_for_state.store(false, Ordering::SeqCst);
            *state.lock().unwrap() = AudioState::Idle;
        });
        thread::spawn(move || {
            let mut underrun_count = 0;
            let mut last_level = 0usize;
            while rx_enabled_for_monitor.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(500));
                let buf = playback_buf_for_monitor.lock().unwrap();
                let level = buf.len();
                
                // Detect underrun (buffer emptied since last check)
                if level < OUTPUT_FRAME_SIZE && last_level >= OUTPUT_FRAME_SIZE {
                    underrun_count += 1;
                }
                last_level = level;
            }
            if underrun_count > 0 {
                eprintln!("[audio] Playback ended with {} underruns", underrun_count);
            }
        });
        
        Ok(())
    }
    
    pub fn stop_playback(&mut self) {
        self.rx_enabled.store(false, Ordering::SeqCst);
        let mut s = self.state.lock().unwrap();
        if *s == AudioState::Playing { *s = AudioState::Idle; }
    }
    
    /// Decode ADPCM frame and add to playback buffer
    pub fn play_adpcm_frame(&mut self, adpcm_data: &[u8]) -> Result<(), String> {
        if !self.rx_enabled.load(Ordering::SeqCst) { return Ok(()); }
        
        let is_first = self.rx_first_block.load(Ordering::SeqCst);
        match self.decoder.decode_block(adpcm_data, is_first) {
            Ok(pcm_samples) => {
                let mut buf = self.playback_buf.lock().unwrap();
                buf.extend_from_slice(&pcm_samples);
            }
            Err(e) => eprintln!("[audio] ADPCM decode error: {}", e),
        }
        self.rx_first_block.store(false, Ordering::SeqCst);
        Ok(())
    }
    
        /// Accumulate ADPCM audio frames before starting playback
    pub fn accumulate_rx_audio(&mut self, adpcm_data: &[u8]) {
        let is_first = self.rx_first_block.load(Ordering::SeqCst);
        match self.decoder.decode_block(adpcm_data, is_first) {
            Ok(pcm_samples) => {
                let mut buf = self.playback_buf.lock().unwrap();
                buf.extend_from_slice(&pcm_samples);
            }
            Err(e) => {
                eprintln!("[audio] ADPCM decode error: {}", e);
                self.has_decode_error.store(true, Ordering::SeqCst);
                self.decode_error_count.fetch_add(1, Ordering::SeqCst);
            }
        }
        self.rx_first_block.store(false, Ordering::SeqCst);
    }
    
    /// Reset the ADPCM decoder state (call when starting new RX session)
    pub fn reset_decoder(&mut self) {
        self.decoder.reset();
        self.rx_first_block.store(true, Ordering::SeqCst);
    }
    
    /// Get playback buffer level (useful for diagnostics)
    pub fn playback_level(&self) -> usize {
        self.playback_buf.lock().unwrap().len()
    }
    
    /// Inject synthetic samples directly into playback buffer (for testing)
    pub fn inject_samples(&mut self, samples: &[i16]) {
        let mut buf = self.playback_buf.lock().unwrap();
        buf.extend_from_slice(samples);
    }
    
    /// Pre-fill buffer then start playback (avoids race condition)
    pub fn prefill_and_start(&mut self, samples: &[i16]) -> Result<(), String> {
        // First inject samples
        {
            let mut buf = self.playback_buf.lock().unwrap();
            buf.extend_from_slice(samples);
        }
        // Then start playback
        self.start_playback()
    }
    
    fn capture_loop(
        config: &AudioConfig,
        callback: TxAudioCallback,
        enabled: Arc<AtomicBool>,
        dc_remover: Arc<Mutex<DCOffsetRemover>>,
        volume_ramp: Arc<Mutex<VolumeRamp>>,
        pre_emphasis: Arc<Mutex<PreEmphasis>>,
    ) -> Result<(), String> {
        let host = cpal::default_host();
        let device = host.default_input_device()
            .ok_or("No input device available")?;
        
        let supported = device.default_input_config()
            .map_err(|e| format!("Failed to get default input config: {}", e))?;
        
        let sample_rate = config.sample_rate;
        let gain = config.tx_gain;
        let gate_threshold = config.gate_threshold;

        let err_fn = |err| eprintln!("[audio] Stream error: {}", err);
        let callback_clone = Arc::clone(&callback);
        let dc_remover_clone = Arc::clone(&dc_remover);
        let volume_ramp_clone = Arc::clone(&volume_ramp);
        let pre_emph_clone = Arc::clone(&pre_emphasis);
        let enabled2 = Arc::clone(&enabled);
        
        // TX: accumulate samples to fill ADPCM blocks
        let tx_buf: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::with_capacity(250)));
        let tx_buf_clone = Arc::clone(&tx_buf);
        let encoder: Arc<std::sync::Mutex<ADPCMEncoder>> = Arc::new(std::sync::Mutex::new(ADPCMEncoder::new()));
        let encoder_clone = Arc::clone(&encoder);
        
        let stream = match supported.sample_format() {
            cpal::SampleFormat::I16 => {
                device.build_input_stream(
                    &supported.config(),
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        if !enabled.load(Ordering::SeqCst) { return; }
                        
                        let mut samples: Vec<i16> = data.iter()
                            .map(|&s| (s as f32 * gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16)
                            .collect();
                        
                        // I16 format branch
                        dc_remover_clone.lock().unwrap().process(&mut samples);
                        volume_ramp_clone.lock().unwrap().process(&mut samples);
                        pre_emph_clone.lock().unwrap().process(&mut samples);
                        
                        let mut max_amp = 0.0f32;
                        for &s in &samples {
                            let amp = (s as f32).abs() / 32768.0;
                            if amp > max_amp { max_amp = amp; }
                        }
                        
                        if max_amp > gate_threshold {
                            // Accumulate samples for ADPCM encoding
                            let mut tx = tx_buf_clone.lock().unwrap();
                            tx.extend_from_slice(&samples);
                            
                            // Encode full blocks (249 samples each)
                            while tx.len() >= 249 {
                                let block = tx.drain(..249).collect::<Vec<_>>();
                                let adpcm = encoder_clone.lock().unwrap().encode(&block);
                                if let Some(ref mut cb) = *callback_clone.lock().unwrap() {
                                    cb(&adpcm);
                                }
                            }
                        } else if tx_buf_clone.lock().unwrap().is_empty() {
                            // Silence - reset encoder state
                            encoder_clone.lock().unwrap().reset();
                        }
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::F32 => {
                device.build_input_stream(
                    &supported.config(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        if !enabled.load(Ordering::SeqCst) { return; }
                        
                        let samples: Vec<i16> = data.iter()
                            .map(|&s| {
                                let s = s * gain;
                                (s.clamp(-1.0, 1.0) * 32767.0) as i16
                            })
                            .collect();
                        
                        // F32 format branch
                        let mut dc = dc_remover_clone.lock().unwrap();
                        let mut samples = samples;
                        dc.process(&mut samples);
                        drop(dc);
                        
                        volume_ramp_clone.lock().unwrap().process(&mut samples);
                        pre_emph_clone.lock().unwrap().process(&mut samples);
                        
                        let mut max_amp = 0.0f32;
                        for &s in &samples {
                            let amp = (s as f32).abs() / 32768.0;
                            if amp > max_amp { max_amp = amp; }
                        }
                        
                        if max_amp > gate_threshold {
                            // Accumulate samples for ADPCM encoding
                            let mut tx = tx_buf_clone.lock().unwrap();
                            tx.extend_from_slice(&samples);
                            
                            // Encode full blocks (249 samples each)
                            while tx.len() >= 249 {
                                let block = tx.drain(..249).collect::<Vec<_>>();
                                let adpcm = encoder_clone.lock().unwrap().encode(&block);
                                if let Some(ref mut cb) = *callback_clone.lock().unwrap() {
                                    cb(&adpcm);
                                }
                            }
                        } else if tx_buf_clone.lock().unwrap().is_empty() {
                            // Silence - reset encoder state
                            encoder_clone.lock().unwrap().reset();
                        }
                    },
                    err_fn,
                    None,
                )
            }
            _ => return Err("Unsupported sample format".to_string()),
        }.map_err(|e| format!("Failed to build input stream: {}", e))?;
        
        stream.play().map_err(|e| format!("Failed to start stream: {}", e))?;
        eprintln!("[audio] Capture started at {} Hz", sample_rate);
        
        while enabled2.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(50));
        }
        
        drop(stream);
        eprintln!("[audio] Capture stopped");
        Ok(())
    }
    
    fn playback_loop_static(
        rx_volume: &Arc<AtomicU32>,
        config: &AudioConfig,
        enabled: Arc<AtomicBool>,
        playback_buf: Arc<Mutex<Vec<i16>>>,
    ) -> Result<(), String> {
        let host = cpal::default_host();
        let device = host.default_output_device()
            .ok_or("No output device available")?;
        eprintln!("[audio] Using output: {}", device.name().unwrap_or_else(|_| "unknown".into()));
        
        // Require 16kHz mono - matches ADPCM native rate, no resampling needed
        let stream_config = cpal::StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(OUTPUT_SAMPLE_RATE),
            buffer_size: cpal::BufferSize::Default,
        };
        
        let gain = config.rx_gain;
        let rx_volume = Arc::clone(rx_volume);
        let playback_buf_clone = Arc::clone(&playback_buf);
        
        let err_fn = |err| eprintln!("[audio] Stream error: {}", err);
        let enabled2 = Arc::clone(&enabled);
        
        // Build output stream for 16kHz mono I16 (most common)
        let stream = device.build_output_stream(
            &stream_config,
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                if !enabled.load(Ordering::SeqCst) {
                    for s in data.iter_mut() { *s = 0; }
                    rx_volume.store(0, Ordering::SeqCst);
                    return;
                }
                
                // Get volume from main AudioManager's rx_volume
                // This is updated externally - default should be high enough
                let vol_bits = rx_volume.load(Ordering::SeqCst);
                let vol = f32::from_bits(vol_bits);
                let vol_gain = if vol > 0.0 { vol.min(1.0) } else { 1.0 };  // Default to full volume if 0
                
                let mut buf = playback_buf_clone.lock().unwrap();
                let to_copy = buf.len().min(data.len());
                
                for i in 0..to_copy {
                    let s = (buf[i] as f32 * vol_gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                    data[i] = s;
                }
                for i in to_copy..data.len() {
                    data[i] = 0;
                }
                buf.drain(..to_copy);
            },
            err_fn,
            None,
        ).map_err(|e| format!("Failed to build output stream: {}", e))?;
        
        stream.play().map_err(|e| format!("Failed to start stream: {}", e))?;
        
        while enabled2.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(50));
        }
        
        drop(stream);
        eprintln!("[audio] Playback stopped");
        Ok(())
    }
    
    pub fn state(&self) -> AudioState {
        *self.state.lock().unwrap()
    }
    
    pub fn is_capturing(&self) -> bool {
        self.tx_enabled.load(Ordering::SeqCst)
    }
    
    pub fn is_playing(&self) -> bool {
        self.rx_enabled.load(Ordering::SeqCst)
    }
    
    /// Get audio LED status for UI display
    pub fn led_status(&self) -> AudioLedStatus {
        // Check for decode errors first (only show error when idle)
        if self.has_decode_error.load(Ordering::SeqCst) && 
           !self.rx_enabled.load(Ordering::SeqCst) && 
           !self.tx_enabled.load(Ordering::SeqCst) {
            return AudioLedStatus::Error;
        }
        
        let ever_started = self.audio_ever_started.load(Ordering::SeqCst);
        
        if !ever_started {
            // Audio thread never started
            AudioLedStatus::NotStarted
        } else {
            // Note: squelch state is now checked from radio in main.rs
            // This method kept for backwards compatibility
            AudioLedStatus::SquelchClosed
        }
    }
    
    /// Clear decode error flag (call when user acknowledges or reconnects)
    pub fn clear_decode_error(&mut self) {
        self.has_decode_error.store(false, Ordering::SeqCst);
        self.decode_error_count.store(0, Ordering::SeqCst);
    }
    
    /// Check if there was a decode error
    pub fn has_decode_error(&self) -> bool {
        self.has_decode_error.load(Ordering::SeqCst)
    }
    
    /// Check if audio thread has ever been started (for LED status)
    pub fn audio_started(&self) -> bool {
        self.audio_ever_started.load(Ordering::SeqCst)
    }
    
    /// Stop playback from external caller (fade out to avoid click)
    pub fn stop_playback_external(&mut self) {
        // Fade out last ~50ms to avoid click
        let mut buf = self.playback_buf.lock().unwrap();
        let fade_samples = (16000 * 50 / 1000) as usize; // 50ms at 16kHz
        let len = buf.len();
        if len >= fade_samples {
            for i in 0..fade_samples {
                let idx = len - fade_samples + i;
                let t = i as f32 / fade_samples as f32;
                buf[idx] = (buf[idx] as f32 * t) as i16;
            }
        }
        drop(buf);
        self.rx_enabled.store(false, Ordering::SeqCst);
        eprintln!("[audio] Playback stopped externally");
    }
}

impl Drop for AudioManager {
    fn drop(&mut self) {
        self.tx_enabled.store(false, Ordering::SeqCst);
        self.rx_enabled.store(false, Ordering::SeqCst);
    }
}