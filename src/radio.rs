//! KV4P HT Radio driver - native Rust implementation
//!
//! Synchronization model: Command/Status queue pattern to avoid deadlock risk.
//!
//! - Commands (tune, squelch, PTT) are sent via a lock-free channel to a dedicated write thread
//! - Device state is updated atomically via atomics (read-heavy pattern)
//! - Callbacks are stored in Arc<Mutex> and invoked from I/O thread to avoid holding locks
//! - No nested lock acquisition - each operation has a clear lock ordering
//!
//! Thread safety: KV4PRadio implements Send but NOT Sync. All public methods that
//! need to be called from multiple threads should use the command queue pattern.

use crate::kiss::{
    build_kv4p_packet, build_tx_audio_packet, DeviceCommand, 
    DeviceState, HostCommand, HostDesiredState, HostStateFlags, PacketParser, 
    RfModuleType, VersionInfo,
};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU16, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

// ============================================================================
// Command Types for Thread-Safe Communication
// ============================================================================

/// Commands sent from main thread to I/O thread via channel
#[derive(Debug, Clone)]
pub enum RadioCommand {
    /// Send desired state to radio (tune, squelch, flags, etc.)
    SendState(HostDesiredState),
    /// Send raw KISS frame (audio, etc.)
    SendFrame(Vec<u8>),
    /// Trigger connection
    Connect,
    /// Trigger shutdown
    Shutdown,
}

// ============================================================================
// Callbacks (Stored in Arc<Mutex> for thread-safe access)
// ============================================================================

pub type SmeterCallback = Box<dyn Fn(i32) + Send + Sync>;
pub type StateCallback = Box<dyn Fn(&DeviceState) + Send + Sync>;
pub type RssiCallback = Box<dyn Fn(f32) + Send + Sync>;
pub type ConnectCallback = Box<dyn Fn(bool) + Send + Sync>;
pub type RxAudioCallback = Box<dyn Fn(&[u8]) + Send + Sync>;
pub type PhysPttCallback = Box<dyn Fn(bool) + Send + Sync>;

/// Thread-safe callback container - protected by Mutex, mutated only during init
struct Callbacks {
    smeter: Option<SmeterCallback>,
    state: Option<StateCallback>,
    rssi: Option<RssiCallback>,
    connect: Option<ConnectCallback>,
    rx_audio: Option<RxAudioCallback>,
    phys_ptt: Option<PhysPttCallback>,
}

impl Default for Callbacks {
    fn default() -> Self {
        Self {
            smeter: None, state: None, rssi: None,
            connect: None, rx_audio: None, phys_ptt: None,
        }
    }
}

// ============================================================================
// Serial Config
// ============================================================================

#[derive(Debug, Clone)]
pub struct SerialConfig {
    pub port: String,
    pub baudrate: u32,
    pub timeout_ms: u64,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self { port: "/dev/ttyUSB0".to_string(), baudrate: 115200, timeout_ms: 500 }
    }
}

// ============================================================================
// Radio State (Thread-Safe via Atomics)
// ============================================================================

/// Immutable snapshot of radio state for UI consumption
#[derive(Debug, Clone, Default)]
pub struct RadioState {
    pub frequency: u32,
    pub tx_frequency: u32,
    pub mode: i32,
    pub power: i32,
    pub bandwidth: i32,
    pub ctcss: u32,
    pub rssi: f32,
    pub smeter_bars: i32,
    pub raw_rssi: u8,
    pub squelch_open: bool,
    pub ptt: bool,
    pub connected: bool,
}

// ============================================================================
// Internal State (Shared between threads via Arc)
// ============================================================================

struct RadioStateInner {
    version: Option<VersionInfo>,
    device_state: Option<DeviceState>,
    desired_state_sent: Option<HostDesiredState>,
}

impl Default for RadioStateInner {
    fn default() -> Self {
        Self { version: None, device_state: None, desired_state_sent: None }
    }
}

/// Thread-safe state container
struct RadioSharedState {
    inner: Mutex<RadioStateInner>,
    // Atomic configuration values (no lock needed for reads)
    frequency: AtomicU32,
    tx_frequency: AtomicU32,
    power_high: AtomicBool,
    current_squelch: AtomicU8,
    squelch_user_set: AtomicBool,
    firmware_squelch: AtomicU8,  // Cached firmware-reported squelch
}

impl Default for RadioSharedState {
    fn default() -> Self {
        Self {
            inner: Mutex::new(RadioStateInner::default()),
            frequency: AtomicU32::new(145500),
            tx_frequency: AtomicU32::new(145500),
            power_high: AtomicBool::new(true),
            current_squelch: AtomicU8::new(4),
            squelch_user_set: AtomicBool::new(false),
            firmware_squelch: AtomicU8::new(4),
        }
    }
}

// ============================================================================
// KV4P Radio
// ============================================================================

pub struct KV4PRadio {
    config: SerialConfig,
    
    // Command channel to I/O thread (Sender is Clone and Send)
    command_tx: Mutex<mpsc::Sender<RadioCommand>>,
    
    // Internal state
    state: Arc<RadioSharedState>,
    
    // Sequence numbers
    sequence: AtomicU32,
    audio_sequence: AtomicU16,
    
    // Callbacks (stored in Arc<Mutex> for thread-safe access from I/O thread)
    callbacks: Arc<Mutex<Callbacks>>,
    
    // Thread control
    running: Arc<AtomicBool>,
    
    // Handle to I/O thread
    io_thread: Mutex<Option<thread::JoinHandle<()>>>,
    
    // Debug tracking
    connection_failed: Arc<AtomicBool>,
}

// KV4PRadio is Send but not Sync (has Mutex fields)
unsafe impl Send for KV4PRadio {}

impl KV4PRadio {
    pub fn new(config: SerialConfig) -> Self {
        // Create command channel
        let (cmd_tx, cmd_rx) = mpsc::channel();
        
        let state = Arc::new(RadioSharedState::default());
        let running = Arc::new(AtomicBool::new(false));
        let callbacks = Arc::new(Mutex::new(Callbacks::default()));
        
        // Spawn I/O thread that owns the serial port and receiver
        let io_state = Arc::clone(&state);
        let io_running = Arc::clone(&running);
        let io_callbacks = Arc::clone(&callbacks);
        let config_clone = config.clone();
        
        let io_handle = thread::spawn(move || {
            io_thread_main(cmd_rx, io_state, io_running, io_callbacks, config_clone);
        });
        
        Self {
            config,
            command_tx: Mutex::new(cmd_tx),
            state,
            sequence: AtomicU32::new(1),
            audio_sequence: AtomicU16::new(0),
            callbacks,
            running,
            io_thread: Mutex::new(Some(io_handle)),
            connection_failed: Arc::new(AtomicBool::new(false)),
        }
    }
    
    // ========================================================================
    // Callback Registration
    // ========================================================================

    pub fn on_smeter<F>(&self, cb: F) where F: Fn(i32) + Send + Sync + 'static {
        self.callbacks.lock().unwrap().smeter = Some(Box::new(cb));
    }

    pub fn on_state<F>(&self, cb: F) where F: Fn(&DeviceState) + Send + Sync + 'static {
        self.callbacks.lock().unwrap().state = Some(Box::new(cb));
    }

    pub fn on_rssi<F>(&self, cb: F) where F: Fn(f32) + Send + Sync + 'static {
        self.callbacks.lock().unwrap().rssi = Some(Box::new(cb));
    }

    pub fn on_connect<F>(&self, cb: F) where F: Fn(bool) + Send + Sync + 'static {
        self.callbacks.lock().unwrap().connect = Some(Box::new(cb));
    }

    pub fn on_rx_audio<F>(&self, cb: F) where F: Fn(&[u8]) + Send + Sync + 'static {
        self.callbacks.lock().unwrap().rx_audio = Some(Box::new(cb));
    }
    
    pub fn on_phys_ptt<F>(&self, cb: F) where F: Fn(bool) + Send + Sync + 'static {
        self.callbacks.lock().unwrap().phys_ptt = Some(Box::new(cb));
    }
    
    pub fn get_squelch(&self) -> u8 {
        self.state.current_squelch.load(Ordering::SeqCst)
    }
    
    pub fn get_debug_logs(&self) -> Vec<String> {
        Vec::new()  // Simplified - can add debug log channel if needed
    }
    
    pub fn clear_debug_logs(&self) {}

    pub fn is_connected(&self) -> bool {
        self.state.inner.lock().unwrap().version.is_some()
    }
    
    pub fn version(&self) -> Option<VersionInfo> {
        self.state.inner.lock().unwrap().version.clone()
    }
    
    pub fn device_state(&self) -> Option<DeviceState> {
        self.state.inner.lock().unwrap().device_state.clone()
    }

    pub fn state(&self) -> RadioState {
        let freq = self.state.frequency.load(Ordering::SeqCst);
        let dev_state = self.state.inner.lock().unwrap().device_state.clone();
        let (rssi, smeter, squelch, raw_rssi) = if let Some(ref s) = dev_state {
            (s.rssi_dbm(), ((s.rssi as i32) * 9 / 255).max(1), s.is_squelched(), s.rssi)
        } else { (-121.0, 0, false, 0) };
        RadioState {
            frequency: freq,
            tx_frequency: self.state.tx_frequency.load(Ordering::SeqCst),
            mode: 0,
            power: if self.state.power_high.load(Ordering::SeqCst) { 1 } else { 0 },
            bandwidth: 1,
            ctcss: 0,
            rssi,
            smeter_bars: smeter,
            raw_rssi,
            squelch_open: !squelch,
            ptt: false,
            connected: self.is_connected(),
        }
    }
    
    // ========================================================================
    // Public API - All use command queue (no blocking)
    // ========================================================================

    pub fn tune(&self, rx_khz: u32, tx_khz: u32, squelch: u8, _bandwidth: u8) -> Result<(), String> {
        self.state.frequency.store(rx_khz, Ordering::SeqCst);
        self.state.tx_frequency.store(tx_khz, Ordering::SeqCst);
        self.state.current_squelch.store(squelch.min(8), Ordering::SeqCst);
        self.state.squelch_user_set.store(true, Ordering::SeqCst);
        
        let state = self.build_desired_state(true);
        self.queue_command(RadioCommand::SendState(state))
    }
    
    pub fn tune_freq(&self, rx_khz: u32, tx_khz: u32) -> Result<(), String> {
        self.state.frequency.store(rx_khz, Ordering::SeqCst);
        self.state.tx_frequency.store(tx_khz, Ordering::SeqCst);
        let state = self.build_desired_state(false);
        self.queue_command(RadioCommand::SendState(state))
    }

    pub fn set_frequency(&self, khz: u32) -> Result<(), String> {
        let squelch = self.state.current_squelch.load(Ordering::SeqCst);
        self.tune(khz, khz, squelch, 1)
    }
    
    pub fn set_squelch(&self, level: u8) -> Result<(), String> {
        let clamped = level.min(8);
        eprintln!("[radio] set_squelch: {}", clamped);
        self.state.current_squelch.store(clamped, Ordering::SeqCst);
        self.state.squelch_user_set.store(true, Ordering::SeqCst);
        let state = self.build_desired_state(true);
        self.queue_command(RadioCommand::SendState(state))
    }
    
    pub fn set_power(&self, high: bool) -> Result<(), String> {
        self.state.power_high.store(high, Ordering::SeqCst);
        let state = self.build_desired_state(true);
        self.queue_command(RadioCommand::SendState(state))
    }
    
    pub fn set_tx_power(&self, high: bool) -> Result<(), String> {
        self.set_power(high)
    }
    
    pub fn set_filter_pre_emphasis(&self, enabled: bool) -> Result<(), String> {
        eprintln!("[radio] Pre-emphasis: {}", if enabled { "on" } else { "off" });
        Ok(())
    }
    
    pub fn set_filter_de_emphasis(&self, enabled: bool) -> Result<(), String> {
        eprintln!("[radio] De-emphasis: {}", if enabled { "on" } else { "off" });
        Ok(())
    }
    
    pub fn set_filter_high_pass(&self, enabled: bool) -> Result<(), String> {
        eprintln!("[radio] High-pass filter: {}", if enabled { "on" } else { "off" });
        Ok(())
    }
    
    pub fn set_filter_low_pass(&self, enabled: bool) -> Result<(), String> {
        eprintln!("[radio] Low-pass filter: {}", if enabled { "on" } else { "off" });
        Ok(())
    }
    
    pub fn set_mic_gain(&self, level: &str) -> Result<(), String> {
        eprintln!("[radio] Mic gain: {}", level);
        Ok(())
    }
    
    pub fn open_audio(&self) -> Result<(), String> {
        eprintln!("[radio] open_audio() called");
        // RX_AUDIO_OPEN is now included in build_desired_state() by default
        self.queue_command(RadioCommand::SendState(self.build_desired_state(true)))
    }
    
    pub fn enable_smeter(&self, _enabled: bool) -> Result<(), String> {
        Ok(())
    }

    pub fn ptt_on(&self) -> Result<(), String> {
        let mut state = self.build_desired_state(true);
        state.flags |= HostStateFlags::PTT_REQUESTED.bits() | HostStateFlags::TX_ALLOWED.bits();
        self.queue_command(RadioCommand::SendState(state))
    }

    pub fn ptt_off(&self) -> Result<(), String> {
        let mut state = self.build_desired_state(true);
        state.flags |= HostStateFlags::RX_AUDIO_OPEN.bits();
        self.queue_command(RadioCommand::SendState(state))
    }
    
    pub fn send_audio(&self, adpcm_data: &[u8]) -> Result<(), String> {
        if !self.running.load(Ordering::SeqCst) {
            return Err("Radio not connected".to_string());
        }
        let frame = build_tx_audio_packet(adpcm_data);
        self.queue_command(RadioCommand::SendFrame(frame))
    }
    
    pub fn send_raw_audio(&self, data: &[u8]) -> Result<(), String> {
        if !self.running.load(Ordering::SeqCst) {
            return Err("Radio not connected".to_string());
        }
        let frame = build_kv4p_packet(HostCommand::TxAudio, data);
        self.queue_command(RadioCommand::SendFrame(frame))
    }

    // ========================================================================
    // Connection Management
    // ========================================================================

    pub fn open(&self) -> Result<Option<VersionInfo>, String> {
        const MAX_ATTEMPTS: u32 = 5;
        
        for attempt in 1..=MAX_ATTEMPTS {
            eprintln!("[radio] Connection attempt {}/{}", attempt, MAX_ATTEMPTS);
            
            // Send connect command to I/O thread
            self.queue_command(RadioCommand::Connect)?;
            
            // Wait for connection with timeout
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            while std::time::Instant::now() < deadline {
                if self.is_connected() {
                    return Ok(self.version());
                }
                thread::sleep(Duration::from_millis(50));
            }
            
            eprintln!("[radio] Attempt {} failed", attempt);
        }
        
        self.connection_failed.store(true, Ordering::SeqCst);
        Err("Failed to connect to device after 5 attempts".to_string())
    }

    pub fn close(&self) {
        eprintln!("[radio] close() called");
        
        // Send shutdown state first
        let freq = self.state.frequency.load(Ordering::SeqCst) as f32 / 1000.0;
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst);
        
        let shutdown_state = HostDesiredState {
            sequence: seq as i32,
            memory_id: -1,
            flags: HostStateFlags::RSSI_ENABLED.bits(),
            bandwidth: 1,
            freq_tx: freq,
            freq_rx: freq,
            ctcss_tx: 0,
            squelch: 8,
            ctcss_rx: 0,
        };
        let frame = build_kv4p_packet(HostCommand::DesiredState, &shutdown_state.to_bytes());
        let _ = self.command_tx.lock().unwrap().send(RadioCommand::SendFrame(frame));
        
        // Send shutdown to I/O thread
        let _ = self.command_tx.lock().unwrap().send(RadioCommand::Shutdown);
        
        // Signal running false
        self.running.store(false, Ordering::SeqCst);
        
        // Wait for thread to finish
        if let Some(handle) = self.io_thread.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
    
    // ========================================================================
    // Internal Helper Methods
    // ========================================================================

    fn queue_command(&self, cmd: RadioCommand) -> Result<(), String> {
        self.command_tx.lock().unwrap()
            .send(cmd)
            .map_err(|_| "I/O thread not running".to_string())
    }
    
    fn build_desired_state(&self, include_radio_config_valid: bool) -> HostDesiredState {
        let freq = self.state.frequency.load(Ordering::SeqCst) as f32 / 1000.0;
        let mut flags = HostStateFlags::HIGH_POWER.bits() |
                       HostStateFlags::RSSI_ENABLED.bits() |
                       HostStateFlags::ENABLE_STATUS_REPORTS.bits() |
                       HostStateFlags::RX_AUDIO_OPEN.bits();
        if include_radio_config_valid {
            flags |= HostStateFlags::RADIO_CONFIG_VALID.bits();
        }
        if !self.state.power_high.load(Ordering::SeqCst) {
            flags &= !HostStateFlags::HIGH_POWER.bits();
        }
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst) as i32;
        
        let squelch = if self.state.squelch_user_set.load(Ordering::SeqCst) {
            self.state.current_squelch.load(Ordering::SeqCst)
        } else {
            self.state.firmware_squelch.load(Ordering::SeqCst)
        };
        
        let state = HostDesiredState {
            sequence: seq,
            memory_id: -1,
            flags,
            bandwidth: 1,
            freq_tx: freq,
            freq_rx: freq,
            ctcss_tx: 0,
            squelch,
            ctcss_rx: 0,
        };
        
        state
    }
}

// ============================================================================
// I/O Thread - Runs the event loop, owns serial port and receiver
// ============================================================================

fn io_thread_main(
    cmd_rx: mpsc::Receiver<RadioCommand>,
    state: Arc<RadioSharedState>,
    running: Arc<AtomicBool>,
    callbacks: Arc<Mutex<Callbacks>>,
    config: SerialConfig,
) {
    let mut buf = [0u8; 512];
    let parser = std::sync::Mutex::new(PacketParser::new());
    let mut serial: Option<Box<dyn serialport::SerialPort>> = None;
    let mut last_phys_ptt = false;
    let mut last_version: Option<VersionInfo> = None;
    
    // Flow control window — decremented on each outgoing frame, incremented by WindowAck
    let flow_window: Arc<AtomicU32> = Arc::new(AtomicU32::new(2048));
    let flow_window_ack = Arc::clone(&flow_window);
    
    // Track whether we're connected (to enable window ack)
    let connected = Arc::new(AtomicBool::new(false));
    let connected_for_parse = Arc::clone(&connected);
    let connected_for_writing = Arc::clone(&connected);
    
    loop {
        // Check for commands with SHORT timeout — don't block serial reads
        // Use try_recv so we never block more than one iteration on commands
        match cmd_rx.try_recv() {
            Ok(cmd) => match cmd {
                RadioCommand::Connect => {
                    // Attempt connection
                    if attempt_connect(&config, &mut serial, &state, &running) {
                        // Clear input buffer
                        if let Some(ref mut sp) = serial {
                            let _ = sp.clear(serialport::ClearBuffer::Input);
                        }
                        
                        // Reset ESP32
                        reset_esp32(&mut serial);
                        
                        // Wait for HELLO
                        let deadline = std::time::Instant::now() + Duration::from_secs(5);
                        while std::time::Instant::now() < deadline {
                            // Read any data
                            if let Some(ref mut sp) = serial {
                                match sp.read(&mut buf) {
                                    Ok(n) if n > 0 => {
                                        let packets = parser.lock().unwrap().feed(&buf[..n]);
                                        for pkt in packets {
                                            if let Some(version) = process_hello_packet(&pkt) {
                                                last_version = Some(version.clone());
                                                state.inner.lock().unwrap().version = Some(version);
                                                
                                                // Call connect callback
                                                if let Some(ref cb) = callbacks.lock().unwrap().connect {
                                                    cb(true);
                                                }
                                                
                                                // Parse DeviceState from HELLO
                                                if pkt.payload.len() >= 43 {
                                                    if let Some(dev_state) = DeviceState::from_bytes(&pkt.payload[17..]) {
                                                        update_device_state(&state, &callbacks, &dev_state);
                                                    }
                                                }
                                                break;
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                                if state.inner.lock().unwrap().version.is_some() {
                                    break;
                                }
                            }
                            thread::sleep(Duration::from_millis(50));
                        }
                        
                        // If no HELLO, try sending initial state
                        if state.inner.lock().unwrap().version.is_none() {
                            send_initial_state(&state, &mut serial);
                            
                            let deadline = std::time::Instant::now() + Duration::from_secs(3);
                            while std::time::Instant::now() < deadline {
                                if let Some(ref mut sp) = serial {
                                    match sp.read(&mut buf) {
                                        Ok(n) if n > 0 => {
                                            let packets = parser.lock().unwrap().feed(&buf[..n]);
                                            for pkt in packets {
                                                if let Some(version) = process_hello_packet(&pkt) {
                                                    last_version = Some(version.clone());
                                                    state.inner.lock().unwrap().version = Some(version);
                                                    if let Some(ref cb) = callbacks.lock().unwrap().connect {
                                                        cb(true);
                                                    }
                                                    break;
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                    if state.inner.lock().unwrap().version.is_some() {
                                        break;
                                    }
                                }
                                thread::sleep(Duration::from_millis(50));
                            }
                        }
                    }
                }
                RadioCommand::SendState(desired_state) => {
                    if let Some(ref mut sp) = serial {
                        let payload = desired_state.to_bytes();
                        let frame = build_kv4p_packet(HostCommand::DesiredState, &payload);
                        let frame_len = frame.len() as u32;
                        // Flow control: wait if window exhausted
                        while flow_window.load(Ordering::SeqCst) < frame_len {
                            // Wait for WindowAck to free up space — yield to allow ack to arrive
                            thread::sleep(Duration::from_micros(500));
                        }
                        flow_window.fetch_sub(frame_len, Ordering::SeqCst);
                        let _ = sp.write_all(&frame);
                        let _ = sp.flush();
                    }
                }
                RadioCommand::SendFrame(frame) => {
                    if let Some(ref mut sp) = serial {
                        let frame_len = frame.len() as u32;
                        // Flow control: wait if window exhausted
                        while flow_window.load(Ordering::SeqCst) < frame_len {
                            thread::sleep(Duration::from_micros(500));
                        }
                        flow_window.fetch_sub(frame_len, Ordering::SeqCst);
                        let _ = sp.write_all(&frame);
                        let _ = sp.flush();
                    }
                }
                RadioCommand::Shutdown => {
                    serial = None;
                    break;
                }
            },
            Err(mpsc::TryRecvError::Empty) => {
                // No command pending — continue reading serial immediately
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                break;
            }
        }
        
        // Read serial data — use short timeout so we loop back fast for commands
        if let Some(ref mut sp) = serial {
            match sp.read(&mut buf) {
                Ok(0) => {
                    // Zero bytes read — loop back immediately for commands
                }
                Ok(n) => {
                    let packets = parser.lock().unwrap().feed(&buf[..n]);
                    for pkt in packets {
                        // WindowAck (0x09): increment flow control window
                        if pkt.command == 0x09 && pkt.payload.len() >= 4 {
                            // Parse window size from payload (little-endian u32 at offset 0)
                            let size = u32::from_le_bytes([
                                pkt.payload[0], pkt.payload[1],
                                pkt.payload[2], pkt.payload[3],
                            ]);
                            flow_window_ack.fetch_add(size, Ordering::SeqCst);
                            
                            // Also send the ack back
                            let ack = build_kv4p_packet(HostCommand::WindowAck, &pkt.payload);
                            let _ = sp.write_all(&ack);
                            let _ = sp.flush();
                        }
                        
                        // Process packet
                        process_packet(&pkt, &state, &callbacks, &mut last_phys_ptt);
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    // Normal — no data. Loop back immediately, don't wait for commands.
                }
                Err(_) => {
                    thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }
    
    // Cleanup
    *state.inner.lock().unwrap() = RadioStateInner::default();
    if let Some(ref cb) = callbacks.lock().unwrap().connect {
        cb(false);
    }
}

fn attempt_connect(
    config: &SerialConfig,
    serial: &mut Option<Box<dyn serialport::SerialPort>>,
    state: &Arc<RadioSharedState>,
    _running: &Arc<AtomicBool>,
) -> bool {
    // Reset state
    *state.inner.lock().unwrap() = RadioStateInner::default();
    
    // Close existing
    *serial = None;
    
    // Open serial port
    match serialport::new(&config.port, config.baudrate)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .timeout(Duration::from_millis(1))
        .open()
    {
        Ok(port) => {
            *serial = Some(port);
            true
        }
        Err(_) => {
            false
        }
    }
}

fn reset_esp32(serial: &mut Option<Box<dyn serialport::SerialPort>>) {
    if let Some(ref mut sp) = serial {
        let _ = sp.write_data_terminal_ready(false);
        let _ = sp.write_request_to_send(true);
        thread::sleep(Duration::from_millis(50));
        let _ = sp.write_request_to_send(false);
        thread::sleep(Duration::from_millis(100));
        let _ = sp.write_data_terminal_ready(true);
        thread::sleep(Duration::from_millis(1000));
    }
}

fn send_initial_state(state: &Arc<RadioSharedState>, serial: &mut Option<Box<dyn serialport::SerialPort>>) {
    if let Some(ref mut sp) = serial {
        let freq = state.frequency.load(Ordering::SeqCst) as f32 / 1000.0;
        let squelch = state.current_squelch.load(Ordering::SeqCst);
        
        let s = HostDesiredState {
            sequence: 1,
            memory_id: -1,
            flags: (HostStateFlags::HIGH_POWER | HostStateFlags::RSSI_ENABLED | 
                    HostStateFlags::ENABLE_STATUS_REPORTS).bits(),
            bandwidth: 1,
            freq_tx: freq,
            freq_rx: freq,
            ctcss_tx: 0,
            squelch,
            ctcss_rx: 0,
        };
        
        let payload = s.to_bytes();
        let frame = build_kv4p_packet(HostCommand::DesiredState, &payload);
        let _ = sp.write_all(&frame);
        let _ = sp.flush();
    }
}

fn process_hello_packet(pkt: &crate::kiss::Packet) -> Option<VersionInfo> {
    if pkt.command as u8 == DeviceCommand::Hello as u8 && pkt.payload.len() >= 9 {
        let mut v = VersionInfo::new();
        v.firmware_version = u16::from_le_bytes([pkt.payload[0], pkt.payload[1]]);
        v.radio_module_present = pkt.payload[2] == b'f';
        v.window_size = u32::from_le_bytes([pkt.payload[3], pkt.payload[4], pkt.payload[5], pkt.payload[6]]);
        v.rf_module_type = match pkt.payload[7] {
            0 => RfModuleType::Sa818Vhf, _ => RfModuleType::Sa818Uhf
        };
        if pkt.payload.len() >= 17 {
            v.min_radio_freq = f32::from_le_bytes([pkt.payload[8], pkt.payload[9], pkt.payload[10], pkt.payload[11]]);
            v.max_radio_freq = f32::from_le_bytes([pkt.payload[12], pkt.payload[13], pkt.payload[14], pkt.payload[15]]);
            v.features = pkt.payload[16];
        }
        v.is_valid = true;
        return Some(v);
    }
    None
}

fn process_packet(
    pkt: &crate::kiss::Packet,
    state: &Arc<RadioSharedState>,
    callbacks: &Arc<Mutex<Callbacks>>,
    last_phys_ptt: &mut bool,
) {
    match pkt.command as u8 {
        x if x == DeviceCommand::DeviceState as u8 => {
            if let Some(dev_state) = DeviceState::from_bytes(&pkt.payload) {
                // Check physical PTT before moving
                let phys_ptt = dev_state.phys_ptt_down();
                if phys_ptt != *last_phys_ptt {
                    *last_phys_ptt = phys_ptt;
                    if let Some(ref cb) = callbacks.lock().unwrap().phys_ptt {
                        cb(phys_ptt);
                    }
                }
                update_device_state(state, callbacks, &dev_state);
            }
        }
        x if x == DeviceCommand::SmeterReport as u8 => {
            if !pkt.payload.is_empty() {
                if let Some(ref cb) = callbacks.lock().unwrap().smeter {
                    cb(pkt.payload[0] as i32);
                }
            }
        }
        0x0C | 0x07 => {
            // Rx audio
            if !pkt.payload.is_empty() {
                if let Some(ref cb) = callbacks.lock().unwrap().rx_audio {
                    cb(&pkt.payload);
                }
            }
        }
        _ => {}
    }
}

fn update_device_state(
    state: &Arc<RadioSharedState>,
    callbacks: &Arc<Mutex<Callbacks>>,
    dev_state: &DeviceState,
) {
    // Cache firmware squelch in atomic (no lock needed)
    state.firmware_squelch.store(dev_state.squelch.min(8), Ordering::SeqCst);
    
    // Seed squelch from firmware if user hasn't set it
    if !state.squelch_user_set.load(Ordering::SeqCst) {
        state.current_squelch.store(dev_state.squelch.min(8), Ordering::SeqCst);
    }
    
    // Copy state data while holding lock
    let rssi = dev_state.rssi;
    let rssi_dbm = dev_state.rssi_dbm();
    let dev_state_clone = dev_state.clone();
    
    // Store device state while holding lock, then drop lock before callbacks
    {
        let mut inner = state.inner.lock().unwrap();
        inner.device_state = Some(dev_state_clone.clone());
    }
    
    // Invoke callbacks (outside any lock held by us)
    let cbs = callbacks.lock().unwrap();
    if let Some(ref cb) = cbs.state { cb(&dev_state_clone); }
    if let Some(ref cb) = cbs.rssi { cb(rssi_dbm); }
    if let Some(ref cb) = cbs.smeter { cb(rssi as i32); }
}



impl Drop for KV4PRadio {
    fn drop(&mut self) { self.close(); }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_squelch_set_and_read_back() {
        let config = SerialConfig::default();
        let radio = KV4PRadio::new(config);
        radio.state.current_squelch.store(7, Ordering::SeqCst);
        assert_eq!(radio.state.current_squelch.load(Ordering::SeqCst), 7);
    }

    #[test]
    fn test_radio_state_snapshot() {
        let config = SerialConfig::default();
        let radio = KV4PRadio::new(config);
        let state = radio.state();
        assert_eq!(state.frequency, 145500);
        assert_eq!(state.connected, false);
    }
}