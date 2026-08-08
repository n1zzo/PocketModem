//! Audio I/O for PocketModem with ADPCM encoding/decoding
//!
//! Matches the KV4P HT firmware audio pipeline:
//!   RX: ADC → DC offset removal → 16x gain → squelch mute → ADPCM (IMA WAV, 16kHz, 4-bit)
//!   TX: ADPCM decode (IMA WAV, 16kHz) → I2S DAC (PDM)
//!
//! Uses cpal for capture and playback, oxideav-adpcm for ADPCM decoding.

use std::sync::atomic::{AtomicBool, Ordering};
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
    /// 
    /// Matches the Java implementation in Android app:
    /// - Uses block header for initial predictor and step_index
    /// - Low nibble decoded first, then high nibble
    /// - Clamps predictor to i16 range
    pub fn decode_block(&mut self, block: &[u8]) -> Result<Vec<i16>, String> {
        if block.len() < 4 {
            return Err("Block too short for IMA WAV header".to_string());
        }

        // Parse block header - little-endian (verified to match Java/Android)
        let predictor_i16 = i16::from_le_bytes([block[0], block[1]]);
        self.predictor = predictor_i16 as i32;
        self.step_index = (block[2] as i32).clamp(0, 88);

        let body = &block[4..];
        let groups = body.len() / 4;
        let samples = 1 + groups * 8;
        let mut out = Vec::with_capacity(samples);

        // First sample is the header predictor
        out.push(self.predictor as i16);

        // Decode nibbles (matches Java: low nibble first, then high)
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
    state: Arc<Mutex<AudioState>>,
    dc_remover: Arc<Mutex<DCOffsetRemover>>,
    volume_ramp: Arc<Mutex<VolumeRamp>>,
    pre_emphasis: Arc<Mutex<PreEmphasis>>,
    de_emphasis: Arc<Mutex<DeEmphasis>>,
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
            state: Arc::new(Mutex::new(AudioState::Idle)),
            dc_remover: Arc::new(Mutex::new(DCOffsetRemover::new(0.25, AUDIO_WIRE_SAMPLE_RATE as f32))),
            volume_ramp: Arc::new(Mutex::new(VolumeRamp::new(0.05, 0.7))),
            pre_emphasis: Arc::new(Mutex::new(pre_emph)),
            de_emphasis: Arc::new(Mutex::new(de_emph)),
            playback_buf: Arc::new(Mutex::new(Vec::new())),
        }
    }
    
    pub fn on_tx_audio<F>(&mut self, callback: F) 
    where F: FnMut(&[u8]) + Send + 'static {
        *self.tx_callback.lock().unwrap() = Some(Box::new(callback));
    }
    
    pub fn on_rx_audio<F>(&mut self, callback: F) 
    where F: FnMut(&[u8]) + Send + 'static {
        *self.rx_callback.lock().unwrap() = Some(Box::new(callback));
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
        if enabled {
            *self.de_emphasis.lock().unwrap() = DeEmphasis::new(0.85);
            eprintln!("[audio] De-emphasis enabled");
        } else {
            *self.de_emphasis.lock().unwrap() = DeEmphasis::new(0.0);
            eprintln!("[audio] De-emphasis disabled");
        }
    }
    
    /// Reset de-emphasis filter state
    pub fn reset_de_emphasis(&mut self) {
        self.de_emphasis.lock().unwrap().reset();
    }
    
    pub fn start_playback(&mut self) -> Result<(), String> {
        if self.rx_enabled.load(Ordering::SeqCst) { return Ok(()); }
        
        let config = self.config.clone();
        let _rx_callback = Arc::clone(&self.rx_callback);
        let rx_enabled = Arc::clone(&self.rx_enabled);
        let rx_enabled_for_state = Arc::clone(&self.rx_enabled);
        let rx_enabled_for_monitor = Arc::clone(&self.rx_enabled);
        let state = Arc::clone(&self.state);
        let playback_buf = Arc::clone(&self.playback_buf);
        let playback_buf_for_monitor = Arc::clone(&self.playback_buf);
        
        // Spawn playback loop
        thread::spawn(move || {
            if let Err(e) = Self::playback_loop(&config, rx_enabled.clone(), playback_buf) {
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
        
        self.rx_enabled.store(true, Ordering::SeqCst);
        *self.state.lock().unwrap() = AudioState::Playing;
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
        
        // Decode IMA WAV ADPCM (128 bytes -> 249 samples at 16kHz)
        match self.decoder.decode_block(adpcm_data) {
            Ok(pcm_samples) => {
                // Store at native 16kHz (match Android app)
                let mut buf = self.playback_buf.lock().unwrap();
                buf.extend_from_slice(&pcm_samples);
            }
            Err(e) => eprintln!("[audio] ADPCM decode error: {}", e),
        }
        Ok(())
    }
    
    /// Accumulate ADPCM audio frames before starting playback
    /// 
    /// Matches Android app: decode at 16kHz native sample rate.
    pub fn accumulate_rx_audio(&mut self, adpcm_data: &[u8]) {
        // Decode and buffer even before playback starts
        match self.decoder.decode_block(adpcm_data) {
            Ok(mut pcm_samples) => {
                // Apply de-emphasis if enabled
                self.de_emphasis.lock().unwrap().process(&mut pcm_samples);
                
                // Store at native 16kHz (match Android app)
                let mut buf = self.playback_buf.lock().unwrap();
                buf.extend_from_slice(&pcm_samples);
            }
            Err(e) => eprintln!("[audio] ADPCM decode error: {}", e),
        }
    }
    
    /// Reset the ADPCM decoder state (call when starting new RX session)
    pub fn reset_decoder(&mut self) {
        self.decoder.reset();
    }
    
    /// Check if we have enough buffered to start playback
    pub fn should_start_playback(&self) -> bool {
        let buf = self.playback_buf.lock().unwrap();
        buf.len() >= OUTPUT_SAMPLE_RATE as usize
    }
    
    /// Get playback buffer level
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
    
    fn playback_loop(
        config: &AudioConfig,
        enabled: Arc<AtomicBool>,
        playback_buf: Arc<Mutex<Vec<i16>>>,
    ) -> Result<(), String> {
        let host = cpal::default_host();
        let device = host.default_output_device()
            .ok_or("No output device available")?;
        eprintln!("[audio] Using output: {}", device.name().unwrap_or_else(|_| "unknown".into()));
        
        let supported = device.default_output_config()
            .map_err(|e| format!("Failed to get default output config: {}", e))?;
        eprintln!("[audio] Output format: {:?}", supported);
        
        // Use device's native format (44100Hz stereo) - resample from ADPCM 8kHz mono
        let stream_config = cpal::StreamConfig {
            channels: supported.channels(),
            sample_rate: supported.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        };
        let native_rate = supported.sample_rate().0;
        let native_channels = supported.channels();
        
        let gain = config.rx_gain;
        
        // Pre-fill with smaller buffer (~50ms to avoid delay, but not silence)
        // Only extend if buffer is smaller than needed - don't truncate!
        let playback_buf_clone = Arc::clone(&playback_buf);
        {
            let mut buf = playback_buf_clone.lock().unwrap();
            let prefill = OUTPUT_SAMPLE_RATE as usize / 20; // ~50ms
            if buf.len() < prefill {
                buf.resize(prefill, 0); 
            }
            eprintln!("[audio] Buffer level: {} samples", buf.len());
        }
        
        let err_fn = |err| eprintln!("[audio] Stream error: {}", err);
        let enabled2 = Arc::clone(&enabled);
        
        let stream = match supported.sample_format() {
            cpal::SampleFormat::I16 => {
                let playback_buf_clone_i16 = Arc::clone(&playback_buf);
                device.build_output_stream(
                    &stream_config,
                    move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                        if !enabled.load(Ordering::SeqCst) {
                            for s in data.iter_mut() { *s = 0; }
                            return;
                        }
                        
                        let mut buf = playback_buf_clone_i16.lock().unwrap();
                        let available = buf.len();
                        
                        // Resample from 16kHz mono to native_rate stereo
                        let ratio = OUTPUT_SAMPLE_RATE as f64 / native_rate as f64;
                        let last_sample = if available > 0 { buf[available - 1] } else { 0i16 };
                        
                        let mut src_pos = 0.0f64;
                        let mut prev_sample: f32 = 0.0;
                        
                        for i in 0..data.len() {
                            let ch = i % native_channels as usize;
                            if ch == 0 {
                                let idx = src_pos as usize;
                                if idx < available {
                                    let s0 = buf[idx] as f32;
                                    let s1 = if idx + 1 < available { buf[idx + 1] as f32 } else { s0 };
                                    let t = (src_pos - idx as f64) as f32;
                                    prev_sample = s0 + (s1 - s0) * t;
                                    prev_sample = (prev_sample * gain).clamp(i16::MIN as f32, i16::MAX as f32);
                                } else {
                                    prev_sample = (last_sample as f32 * gain).clamp(i16::MIN as f32, i16::MAX as f32);
                                }
                                data[i] = prev_sample as i16;
                                src_pos += ratio;
                            } else {
                                data[i] = prev_sample as i16;
                            }
                        }
                        
                        let consumed = src_pos as usize;
                        if consumed > 0 && consumed <= available {
                            buf.drain(..consumed);
                        }
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::F32 => {
                let playback_buf_clone2 = Arc::clone(&playback_buf);
                device.build_output_stream(
                    &stream_config,
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        if !enabled.load(Ordering::SeqCst) {
                            for s in data.iter_mut() { *s = 0.0; }
                            return;
                        }
                        
                        let mut buf = playback_buf_clone2.lock().unwrap();
                        let available = buf.len();
                        
                        // Resample from 16kHz mono to native_rate stereo
                        let ratio = OUTPUT_SAMPLE_RATE as f64 / native_rate as f64;
                        let last_mono = if available > 0 { buf[available - 1] as f32 / 32768.0 } else { 0.0f32 };
                        
                        let mut src_pos = 0.0f64;
                        let mut prev_sample: f32 = 0.0;
                        
                        for i in 0..data.len() {
                            let ch = i % native_channels as usize;
                            if ch == 0 {
                                let idx = src_pos as usize;
                                if idx < available {
                                    let s0 = buf[idx] as f32 / 32768.0;
                                    let s1 = if idx + 1 < available { buf[idx + 1] as f32 / 32768.0 } else { s0 };
                                    let t = (src_pos - idx as f64) as f32;
                                    prev_sample = (s0 + (s1 - s0) * t) * gain;
                                } else {
                                    prev_sample = last_mono * gain;
                                }
                                data[i] = prev_sample;
                                
                                src_pos += ratio;
                            } else {
                                data[i] = prev_sample;
                            }
                        }
                        
                        let consumed = src_pos as usize;
                        if consumed > 0 && consumed <= available {
                            buf.drain(..consumed);
                        }
                    },
                    err_fn,
                    None,
                )
            }
            _ => return Err("Unsupported sample format".to_string()),
        }.map_err(|e| format!("Failed to build output stream: {}", e))?;
        
        stream.play().map_err(|e| format!("Failed to start stream: {}", e))?;
        eprintln!("[audio] Playback started at {} Hz", OUTPUT_SAMPLE_RATE);
        
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