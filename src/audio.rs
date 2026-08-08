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
use oxideav_adpcm::ima_wav;

// Audio parameters - KV4P uses IMA WAV ADPCM at 16kHz
const AUDIO_WIRE_SAMPLE_RATE: u32 = 16000;
const AUDIO_FRAME_SAMPLES: usize = 249; // 128-byte ADPCM block decodes to 249 samples
const ADPCM_FRAME_BYTES: usize = 128; // ADPCM block size
const CHANNELS: u16 = 1;
const OUTPUT_SAMPLE_RATE: u32 = 8000; // Downsample to 8kHz for playback
const OUTPUT_FRAME_SIZE: usize = 160; // 20ms at 8kHz

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

/// ADPCM frame state for encoding (maintains predictor across frames)
pub struct ADPCMEncoder {
    predictor: i16,
    step_index: u8,
}

impl ADPCMEncoder {
    pub fn new() -> Self {
        Self { predictor: 0, step_index: 0 }
    }

    /// Encode PCM samples to IMA WAV ADPCM block (128 bytes for 249 samples)
    /// Note: This is a simplified encoder - the KV4P firmware uses the full encoder
    pub fn encode(&mut self, _samples: &[i16]) -> Vec<u8> {
        // For now, return empty - TX encoding would need a proper IMA encoder
        // The firmware does the encoding, we just need to handle RX decoding
        vec![0u8; ADPCM_FRAME_BYTES]
    }
}

impl Default for ADPCMEncoder {
    fn default() -> Self { Self::new() }
}

/// Main audio manager
pub struct AudioManager {
    config: AudioConfig,
    encoder: ADPCMEncoder,
    tx_callback: TxAudioCallback,
    rx_callback: RxAudioCallback,
    tx_enabled: Arc<AtomicBool>,
    rx_enabled: Arc<AtomicBool>,
    state: Arc<Mutex<AudioState>>,
    dc_remover: Arc<Mutex<DCOffsetRemover>>,
    volume_ramp: Arc<Mutex<VolumeRamp>>,
    playback_buf: Arc<Mutex<Vec<i16>>>,
}

impl AudioManager {
    pub fn new(config: AudioConfig) -> Self {
        Self {
            config,
            encoder: ADPCMEncoder::new(),
            tx_callback: Arc::new(Mutex::new(None)),
            rx_callback: Arc::new(Mutex::new(None)),
            tx_enabled: Arc::new(AtomicBool::new(false)),
            rx_enabled: Arc::new(AtomicBool::new(false)),
            state: Arc::new(Mutex::new(AudioState::Idle)),
            dc_remover: Arc::new(Mutex::new(DCOffsetRemover::new(0.25, AUDIO_WIRE_SAMPLE_RATE as f32))),
            volume_ramp: Arc::new(Mutex::new(VolumeRamp::new(0.05, 0.7))),
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
        
        volume_ramp.lock().unwrap().start();
        
        thread::spawn(move || {
            if let Err(e) = Self::capture_loop(&config, tx_callback, tx_enabled.clone(), 
                                                dc_remover, volume_ramp) {
                eprintln!("[audio] Capture error: {}", e);
            }
            tx_enabled.store(false, Ordering::SeqCst);
            *state.lock().unwrap() = AudioState::Idle;
        });
        
        self.tx_enabled.store(true, Ordering::SeqCst);
        *self.state.lock().unwrap() = AudioState::Capturing;
        Ok(())
    }
    
    pub fn stop_capture(&mut self) {
        self.tx_enabled.store(false, Ordering::SeqCst);
        self.volume_ramp.lock().unwrap().stop();
        self.dc_remover.lock().unwrap().reset();
        let mut s = self.state.lock().unwrap();
        if *s == AudioState::Capturing { *s = AudioState::Idle; }
    }
    
    pub fn start_playback(&mut self) -> Result<(), String> {
        if self.rx_enabled.load(Ordering::SeqCst) { return Ok(()); }
        
        let config = self.config.clone();
        let rx_callback = Arc::clone(&self.rx_callback);
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
        match ima_wav::decode_block(adpcm_data, 1) {
            Ok(pcm_samples) => {
                // Downsample from 16kHz to 8kHz (skip every other sample)
                let downsampled: Vec<i16> = pcm_samples.iter()
                    .enumerate()
                    .filter(|(i, _)| i % 2 == 0)
                    .map(|(_, &s)| s)
                    .collect();
                
                let mut buf = self.playback_buf.lock().unwrap();
                buf.extend_from_slice(&downsampled);
            }
            Err(e) => eprintln!("[audio] ADPCM decode error: {}", e),
        }
        Ok(())
    }
    
    /// Accumulate ADPCM audio frames before starting playback
    pub fn accumulate_rx_audio(&mut self, adpcm_data: &[u8]) {
        // Decode and buffer even before playback starts
        match ima_wav::decode_block(adpcm_data, 1) {
            Ok(pcm_samples) => {
                // Downsample from 16kHz to 8kHz
                let downsampled: Vec<i16> = pcm_samples.iter()
                    .enumerate()
                    .filter(|(i, _)| i % 2 == 0)
                    .map(|(_, &s)| s)
                    .collect();
                
                let mut buf = self.playback_buf.lock().unwrap();
                buf.extend_from_slice(&downsampled);
            }
            Err(e) => eprintln!("[audio] ADPCM decode error: {}", e),
        }
    }
    
    /// Check if we have enough buffered to start playback (1 second worth)
    pub fn should_start_playback(&self) -> bool {
        let buf = self.playback_buf.lock().unwrap();
        buf.len() >= OUTPUT_SAMPLE_RATE as usize
    }
    
    /// Get playback buffer level
    pub fn playback_level(&self) -> usize {
        self.playback_buf.lock().unwrap().len()
    }
    
    fn capture_loop(
        config: &AudioConfig,
        callback: TxAudioCallback,
        enabled: Arc<AtomicBool>,
        dc_remover: Arc<Mutex<DCOffsetRemover>>,
        volume_ramp: Arc<Mutex<VolumeRamp>>,
    ) -> Result<(), String> {
        let host = cpal::default_host();
        let device = host.default_input_device()
            .ok_or("No input device available")?;
        eprintln!("[audio] Using input: {}", device.name().unwrap_or_else(|_| "unknown".into()));

        let sample_rate = config.sample_rate;
        let gain = config.tx_gain;
        let gate_threshold = config.gate_threshold;
        
        let supported = device.default_input_config()
            .map_err(|e| format!("Failed to get default input config: {}", e))?;
        eprintln!("[audio] Input format: {:?}", supported);

        let err_fn = |err| eprintln!("[audio] Stream error: {}", err);
        let callback_clone = Arc::clone(&callback);
        let dc_remover_clone = Arc::clone(&dc_remover);
        let volume_ramp_clone = Arc::clone(&volume_ramp);
        let enabled2 = Arc::clone(&enabled);
        
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
                        
                        let mut max_amp = 0.0f32;
                        for &s in &samples {
                            let amp = (s as f32).abs() / 32768.0;
                            if amp > max_amp { max_amp = amp; }
                        }
                        
                        if max_amp > gate_threshold {
                            // TODO: Encode to ADPCM for TX
                            // For now, just send raw samples
                            let bytes = unsafe {
                                std::slice::from_raw_parts(samples.as_ptr() as *const u8, samples.len() * 2)
                            };
                            if let Some(ref mut cb) = *callback_clone.lock().unwrap() {
                                cb(bytes);
                            }
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
                        
                        let mut max_amp = 0.0f32;
                        for &s in &samples {
                            let amp = (s as f32).abs() / 32768.0;
                            if amp > max_amp { max_amp = amp; }
                        }
                        
                        if max_amp > gate_threshold {
                            // TODO: Encode to ADPCM for TX
                            let bytes = unsafe {
                                std::slice::from_raw_parts(samples.as_ptr() as *const u8, samples.len() * 2)
                            };
                            if let Some(ref mut cb) = *callback_clone.lock().unwrap() {
                                cb(bytes);
                            }
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
        
        // Pre-fill with ~200ms buffer
        let playback_buf_clone = Arc::clone(&playback_buf);
        {
            let mut buf = playback_buf_clone.lock().unwrap();
            buf.resize(OUTPUT_SAMPLE_RATE as usize / 5, 0); // ~200ms of silence
            eprintln!("[audio] Pre-filled buffer with {} samples (~200ms)", buf.len());
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
                        
                        // Resample from 8kHz mono to native_rate stereo
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
                        
                        // Resample from 8kHz mono to native_rate stereo
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
}

impl Drop for AudioManager {
    fn drop(&mut self) {
        self.tx_enabled.store(false, Ordering::SeqCst);
        self.rx_enabled.store(false, Ordering::SeqCst);
    }
}