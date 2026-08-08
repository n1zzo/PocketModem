//! KV4P HT Radio driver - native Rust implementation

use crate::kiss::{
    build_kv4p_packet, build_tx_audio_packet, DeviceCommand, 
    DeviceState, HostCommand, HostDesiredState, HostStateFlags, PacketParser, 
    RfModuleType, VersionInfo,
};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU16, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

/// Callbacks
pub type SmeterCallback = Box<dyn Fn(i32) + Send + Sync>;
pub type StateCallback = Box<dyn Fn(&DeviceState) + Send + Sync>;
pub type RssiCallback = Box<dyn Fn(f32) + Send + Sync>;
pub type ConnectCallback = Box<dyn Fn(bool) + Send + Sync>;
pub type RxAudioCallback = Box<dyn Fn(&[u8]) + Send + Sync>;

/// Serial config
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

/// Radio state
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

/// KV4P Radio
pub struct KV4PRadio {
    config: SerialConfig,
    serial: Arc<Mutex<Option<Box<dyn serialport::SerialPort>>>>,
    write_tx: Arc<Mutex<Option<mpsc::Sender<Vec<u8>>>>>,  // Channel for write commands
    parser: Arc<Mutex<PacketParser>>,
    running: Arc<AtomicBool>,
    write_shutdown: Arc<AtomicBool>,
    sequence: Arc<AtomicU32>,
    audio_sequence: Arc<AtomicU16>,  // Sequence for audio frames
    version: Arc<Mutex<Option<VersionInfo>>>,
    device_state: Arc<Mutex<Option<DeviceState>>>,
    frequency: Arc<AtomicU32>,
    tx_frequency: Arc<AtomicU32>,
    power_high: Arc<AtomicBool>,
    current_squelch: Arc<AtomicU8>,  // Current squelch level (0-9)
    smeter_cb: Arc<Mutex<Option<SmeterCallback>>>,
    state_cb: Arc<Mutex<Option<StateCallback>>>,
    rssi_cb: Arc<Mutex<Option<RssiCallback>>>,
    connect_cb: Arc<Mutex<Option<ConnectCallback>>>,
    rx_audio_cb: Arc<Mutex<Option<RxAudioCallback>>>,  // Callback for received audio
}

impl KV4PRadio {
    pub fn new(config: SerialConfig) -> Self {
        Self {
            config, serial: Arc::new(Mutex::new(None)),
            write_tx: Arc::new(Mutex::new(None)),
            parser: Arc::new(Mutex::new(PacketParser::new())),
            running: Arc::new(AtomicBool::new(false)),
            sequence: Arc::new(AtomicU32::new(1)),
            audio_sequence: Arc::new(AtomicU16::new(0)),
            version: Arc::new(Mutex::new(None)),
            device_state: Arc::new(Mutex::new(None)),
            frequency: Arc::new(AtomicU32::new(145500)),
            tx_frequency: Arc::new(AtomicU32::new(145500)),
            power_high: Arc::new(AtomicBool::new(true)),
            current_squelch: Arc::new(AtomicU8::new(4)),
            smeter_cb: Arc::new(Mutex::new(None)),
            state_cb: Arc::new(Mutex::new(None)),
            rssi_cb: Arc::new(Mutex::new(None)),
            connect_cb: Arc::new(Mutex::new(None)),
            rx_audio_cb: Arc::new(Mutex::new(None)),
            write_shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn on_smeter<F>(&mut self, cb: F) where F: Fn(i32) + Send + Sync + 'static {
        *self.smeter_cb.lock().unwrap() = Some(Box::new(cb));
    }

    pub fn on_state<F>(&mut self, cb: F) where F: Fn(&DeviceState) + Send + Sync + 'static {
        *self.state_cb.lock().unwrap() = Some(Box::new(cb));
    }

    pub fn on_rssi<F>(&mut self, cb: F) where F: Fn(f32) + Send + Sync + 'static {
        *self.rssi_cb.lock().unwrap() = Some(Box::new(cb));
    }

    pub fn on_connect<F>(&mut self, cb: F) where F: Fn(bool) + Send + Sync + 'static {
        *self.connect_cb.lock().unwrap() = Some(Box::new(cb));
    }

    pub fn on_rx_audio<F>(&mut self, cb: F) where F: Fn(&[u8]) + Send + Sync + 'static {
        *self.rx_audio_cb.lock().unwrap() = Some(Box::new(cb));
    }

    pub fn open(&mut self) -> Result<Option<VersionInfo>, String> {
        // Reset state for reconnection (in case we're reopening)
        self.running.store(false, Ordering::SeqCst);
        self.write_shutdown.store(false, Ordering::SeqCst);
        *self.version.lock().unwrap() = None;
        *self.device_state.lock().unwrap() = None;
        *self.write_tx.lock().unwrap() = None;
        self.parser.lock().unwrap().reset();
        
        let port = serialport::new(&self.config.port, self.config.baudrate)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .dtr_on_open(false)
            .timeout(Duration::from_secs(5))
            .open()
            .map_err(|e| format!("Failed to open {}: {}", self.config.port, e))?;

        *self.serial.lock().unwrap() = Some(port);
        self.parser.lock().unwrap().reset();

        // Clear DTR and RTS (set to low) and wait for device to settle
        // The ESP32 needs DTR low to start sending boot data
        {
            let mut sp = self.serial.lock().unwrap();
            if let Some(ref mut port) = *sp {
                let _ = port.write_data_terminal_ready(false);  // DTR = low
            }
        }
        thread::sleep(Duration::from_millis(200));
        {
            let mut sp = self.serial.lock().unwrap();
            if let Some(ref mut port) = *sp {
                let _ = port.write_request_to_send(false);      // RTS = low
                let _ = port.clear(serialport::ClearBuffer::Input);
            }
        }

        // Read boot data
        let boot_data = self.read_boot_data(5000)?;
        
        let packets = self.parser.lock().unwrap().feed(&boot_data);
        for pkt in &packets { 
            self.dispatch(pkt); 
        }

        if self.version.lock().unwrap().is_none() {
            self.send_initial_state()?;
            thread::sleep(Duration::from_millis(500));
            if let Some(ref mut sp) = *self.serial.lock().unwrap() {
                let mut buf = [0u8; 256];
                while let Ok(n) = sp.read(&mut buf) {
                    if n == 0 { break; }
                    let packets = self.parser.lock().unwrap().feed(&buf[..n]);
                    for pkt in &packets { self.dispatch(pkt); }
                }
            }
        }

        // If still no HELLO, try again
        if self.version.lock().unwrap().is_none() {
            self.send_initial_state()?;
            thread::sleep(Duration::from_millis(500));
            if let Some(ref mut sp) = *self.serial.lock().unwrap() {
                let mut buf = [0u8; 256];
                while let Ok(n) = sp.read(&mut buf) {
                    if n == 0 { break; }
                    let packets = self.parser.lock().unwrap().feed(&buf[..n]);
                    for pkt in &packets { self.dispatch(pkt); }
                }
            }
        }

        let version = self.version.lock().unwrap().clone();
        if version.as_ref().map(|v| v.is_valid).unwrap_or(false) {
            self.running.store(true, Ordering::SeqCst);
            self.spawn_reader_thread();
            Ok(version)
        } else {
            Err("Failed to connect to device".to_string())
        }
    }

    fn read_boot_data(&mut self, timeout_ms: u64) -> Result<Vec<u8>, String> {
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        let mut data = Vec::new();
        
        if let Some(ref mut sp) = *self.serial.lock().unwrap() {
            let mut buf = [0u8; 256];
            while std::time::Instant::now() < deadline {
                match sp.read(&mut buf) {
                    Ok(0) => {
                        // No data, wait a bit
                        thread::sleep(Duration::from_millis(100));
                    }
                    Ok(n) => {
                        data.extend_from_slice(&buf[..n]);
                        // If we got a KISS frame marker, wait a bit more for binary data
                        if data.contains(&0xC0) {
                            thread::sleep(Duration::from_millis(200));
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if data.contains(&0xC0) && data.len() > 400 {
                            break;
                        }
                        thread::sleep(Duration::from_millis(100));
                    }
                    Err(_) => { break; }
                }
            }
        }
        Ok(data)
    }

    fn send_initial_state(&mut self) -> Result<(), String> {
        let flags = (HostStateFlags::HIGH_POWER | HostStateFlags::RSSI_ENABLED).bits();
        let state = HostDesiredState { flags, ..Default::default() };
        self.send(state)
    }

    fn send(&mut self, state: HostDesiredState) -> Result<(), String> {
        let payload = state.to_bytes();
        let frame = build_kv4p_packet(HostCommand::DesiredState, &payload);
        
        // Send to write thread via channel (non-blocking)
        if let Ok(guard) = self.write_tx.lock() {
            if let Some(ref tx) = *guard {
                let _ = tx.send(frame);
            }
        }
        Ok(())
    }

    fn spawn_reader_thread(&mut self) {
        let serial = Arc::clone(&self.serial);
        let _write_tx = Arc::clone(&self.write_tx);
        let parser = Arc::clone(&self.parser);
        let running = Arc::clone(&self.running);
        let write_shutdown = Arc::clone(&self.write_shutdown);
        let version = Arc::clone(&self.version);
        let device_state = Arc::clone(&self.device_state);
        let smeter_cb = Arc::clone(&self.smeter_cb);
        let state_cb = Arc::clone(&self.state_cb);
        let rssi_cb = Arc::clone(&self.rssi_cb);
        let connect_cb = Arc::clone(&self.connect_cb);
        let rx_audio_cb = Arc::clone(&self.rx_audio_cb);

        // Spawn write thread
        let (tx, rx) = mpsc::channel();
        {
            let mut guard = self.write_tx.lock().unwrap();
            *guard = Some(tx);
        }
        
        let serial_for_write = Arc::clone(&serial);
        thread::spawn(move || {
            while !write_shutdown.load(Ordering::SeqCst) {
                if let Ok(frame) = rx.recv_timeout(Duration::from_millis(100)) {
                    if let Some(ref mut sp) = *serial_for_write.lock().unwrap() {
                        let _ = sp.write_all(&frame);
                        let _ = sp.flush();
                    }
                }
            }
        });

        // Reader thread
        let _reader_handle = thread::spawn(move || {
            let mut buf = [0u8; 256];
            while running.load(Ordering::SeqCst) {
                if let Some(ref mut sp) = *serial.lock().unwrap() {
                    match sp.read(&mut buf) {
                        Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                            // Normal - no data available
                            thread::sleep(Duration::from_millis(50));
                        }
                        Ok(n) if n == 0 => {
                            // Empty read, continue
                        }
                        Ok(n) => {
                            let packets = parser.lock().unwrap().feed(&buf[..n]);
                            for pkt in &packets {
                                eprintln!("[radio] PACKET: cmd=0x{:02x}, len={}", pkt.command, pkt.payload.len());
                                let cmd = pkt.command;
                                let payload = &pkt.payload;
                                if cmd == DeviceCommand::Hello as u8 && payload.len() >= 9 {
                                    let mut v = VersionInfo::new();
                                    v.firmware_version = u16::from_le_bytes([payload[0], payload[1]]);
                                    v.radio_module_present = payload[2] == b'f';
                                    v.window_size = u32::from_le_bytes([payload[3], payload[4], payload[5], payload[6]]);
                                    v.rf_module_type = match payload[7] {
                                        0 => RfModuleType::Sa818Vhf, _ => RfModuleType::Sa818Uhf
                                    };
                                    if payload.len() >= 17 {
                                        v.min_radio_freq = f32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
                                        v.max_radio_freq = f32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
                                        v.features = payload[16];
                                    }
                                    v.is_valid = true;
                                    *version.lock().unwrap() = Some(v);
                                    if let Some(ref cb) = *connect_cb.lock().unwrap() { cb(true); }
                                } else if cmd == DeviceCommand::DeviceState as u8 {
                                    if let Some(state) = DeviceState::from_bytes(payload) {
                                        *device_state.lock().unwrap() = Some(state.clone());
                                        if let Some(ref cb) = *state_cb.lock().unwrap() { cb(&state); }
                                        if let Some(ref cb) = *rssi_cb.lock().unwrap() { cb(state.rssi_dbm()); }
                                        if let Some(ref cb) = *smeter_cb.lock().unwrap() { cb(state.rssi as i32); }
                                    }
                                } else if cmd == DeviceCommand::SmeterReport as u8 && !payload.is_empty() {
                                    if let Some(ref cb) = *smeter_cb.lock().unwrap() { cb(payload[0] as i32); }
                                } else if cmd == 0x0C {
                                    // Cmd 0x0C - Rx audio from device (ADPCM encoded - IMA WAV format)
                                    if !payload.is_empty() {
                                        if let Some(ref cb) = *rx_audio_cb.lock().unwrap() {
                                            cb(payload);
                                        }
                                    }
                                } else if cmd == 0x07 {
                                    // Cmd 0x07 - Rx audio from device (Opus encoded - legacy, unused in main firmware)
                                    eprintln!("[radio] RX AUDIO: Opus (cmd=0x07, {} bytes)", payload.len());
                                    if !payload.is_empty() {
                                        if let Some(ref cb) = *rx_audio_cb.lock().unwrap() {
                                            cb(payload);
                                        }
                                    }
                                }
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20));
                        }
                        Err(e) => {
                            eprintln!("[radio] Read error: {}", e);
                            thread::sleep(Duration::from_millis(1000));
                        }
                    }
                } else {
                    thread::sleep(Duration::from_millis(1000));
                }
            }
        });
    }

    fn dispatch(&self, pkt: &crate::kiss::Packet) {
        let cmd = pkt.command;
        let payload = &pkt.payload;
        match cmd as u8 {
            x if x == DeviceCommand::Hello as u8 => {
                if payload.len() >= 9 {
                    let mut v = VersionInfo::new();
                    v.firmware_version = u16::from_le_bytes([payload[0], payload[1]]);
                    v.radio_module_present = payload[2] == b'f';
                    v.window_size = u32::from_le_bytes([payload[3], payload[4], payload[5], payload[6]]);
                    v.rf_module_type = match payload[7] {
                        0 => RfModuleType::Sa818Vhf, _ => RfModuleType::Sa818Uhf
                    };
                    if payload.len() >= 17 {
                        v.min_radio_freq = f32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
                        v.max_radio_freq = f32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
                        v.features = payload[16];
                    }
                    v.is_valid = true;
                    *self.version.lock().unwrap() = Some(v);
                }
            }
            x if x == DeviceCommand::DeviceState as u8 => {
                if let Some(state) = DeviceState::from_bytes(payload) {
                    *self.device_state.lock().unwrap() = Some(state.clone());
                    if let Some(ref cb) = *self.state_cb.lock().unwrap() { cb(&state); }
                    if let Some(ref cb) = *self.rssi_cb.lock().unwrap() { cb(state.rssi_dbm()); }
                    if let Some(ref cb) = *self.smeter_cb.lock().unwrap() { cb(state.rssi as i32); }
                }
            }
            _ => {}
        }
    }

    pub fn close(&mut self) {
        // Send final shutdown state to KV4P (PTT off, radio to idle)
        let freq = self.frequency.load(Ordering::SeqCst) as f32 / 1000.0;
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst);
        
        // Send idle state: no PTT, close squelch, minimal flags
        let shutdown_state = HostDesiredState {
            sequence: seq as i32,
            memory_id: -1,
            flags: HostStateFlags::RSSI_ENABLED.bits(),
            bandwidth: 1,
            freq_tx: freq,
            freq_rx: freq,
            ctcss_tx: 0,
            squelch: 9,
            ctcss_rx: 0,
        };
        let _ = self.send(shutdown_state);
        
        // Give time for final command to be sent
        thread::sleep(Duration::from_millis(50));
        
        // Signal threads to stop
        self.running.store(false, Ordering::SeqCst);
        self.write_shutdown.store(true, Ordering::SeqCst);
        
        // Drop the sender to unblock the write thread's recv
        {
            let mut guard = self.write_tx.lock().unwrap();
            *guard = None;
        }
        
        // Give threads time to clean up
        thread::sleep(Duration::from_millis(150));
        
        // Close serial port
        *self.serial.lock().unwrap() = None;
    }

    pub fn is_connected(&self) -> bool { self.version.lock().unwrap().is_some() }
    pub fn version(&self) -> Option<VersionInfo> { self.version.lock().unwrap().clone() }
    pub fn device_state(&self) -> Option<DeviceState> { self.device_state.lock().unwrap().clone() }

    pub fn state(&self) -> RadioState {
        let freq = self.frequency.load(Ordering::SeqCst);
        let dev_state = self.device_state.lock().unwrap();
        let (rssi, smeter, squelch, raw_rssi) = if let Some(ref s) = *dev_state {
            (s.rssi_dbm(), ((s.rssi as i32) * 9 / 255).max(1), s.is_squelched(), s.rssi)
        } else { (-121.0, 0, false, 0) };
        RadioState {
            frequency: freq,
            tx_frequency: self.tx_frequency.load(Ordering::SeqCst),
            mode: 0,
            power: if self.power_high.load(Ordering::SeqCst) { 1 } else { 0 },
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

    pub fn tune(&mut self, rx_khz: u32, tx_khz: u32, squelch: u8, bandwidth: u8) -> Result<(), String> {
        self.frequency.store(rx_khz, Ordering::SeqCst);
        self.tx_frequency.store(tx_khz, Ordering::SeqCst);
        let freq_rx = rx_khz as f32 / 1000.0;
        let freq_tx = tx_khz as f32 / 1000.0;
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst);
        
        let mut flags = HostStateFlags::HIGH_POWER | HostStateFlags::RSSI_ENABLED |
                         HostStateFlags::ENABLE_STATUS_REPORTS | HostStateFlags::RX_AUDIO_OPEN;
        if self.power_high.load(Ordering::SeqCst) { flags |= HostStateFlags::HIGH_POWER; }
        
        let state = HostDesiredState {
            sequence: seq as i32,
            memory_id: -1,
            flags: flags.bits(),
            bandwidth,
            freq_tx,
            freq_rx,
            ctcss_tx: 0,
            squelch,
            ctcss_rx: 0,
        };
        self.send(state)
    }

    pub fn set_frequency(&mut self, khz: u32) -> Result<(), String> {
        self.tune(khz, khz, 4, 1)
    }
    
    pub fn set_squelch(&mut self, level: u8) -> Result<(), String> {
        self.current_squelch.store(level, Ordering::SeqCst);
        let khz = self.frequency.load(Ordering::SeqCst);
        let tx_khz = self.tx_frequency.load(Ordering::SeqCst);
        self.tune(khz, tx_khz, level, 1)
    }
    
    pub fn set_power(&mut self, high: bool) -> Result<(), String> {
        self.power_high.store(high, Ordering::SeqCst);
        let freq = self.frequency.load(Ordering::SeqCst) as f32 / 1000.0;
        let mut flags = HostStateFlags::RSSI_ENABLED | HostStateFlags::ENABLE_STATUS_REPORTS;
        if high { flags |= HostStateFlags::HIGH_POWER; }
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst);
        let state = HostDesiredState {
            sequence: seq as i32,
            memory_id: -1,
            flags: flags.bits(),
            bandwidth: 1,
            freq_tx: freq,
            freq_rx: freq,
            ctcss_tx: 0,
            squelch: self.current_squelch.load(Ordering::SeqCst),
            ctcss_rx: 0,
        };
        self.send(state)
    }
    
    pub fn set_tx_power(&mut self, high: bool) -> Result<(), String> {
        self.set_power(high)
    }
    
    /// Set pre-emphasis filter (TX) - 6dB/octave high-frequency boost
    pub fn set_filter_pre_emphasis(&mut self, enabled: bool) -> Result<(), String> {
        // TODO: Send to device - this is a device-level setting
        eprintln!("[radio] Pre-emphasis: {}", if enabled { "on" } else { "off" });
        Ok(())
    }
    
    /// Set de-emphasis filter (RX) - 6dB/octave low-frequency boost
    pub fn set_filter_de_emphasis(&mut self, enabled: bool) -> Result<(), String> {
        // TODO: Send to device - this is a device-level setting
        eprintln!("[radio] De-emphasis: {}", if enabled { "on" } else { "off" });
        Ok(())
    }
    
    /// Set high-pass filter (removes low frequencies)
    pub fn set_filter_high_pass(&mut self, enabled: bool) -> Result<(), String> {
        // TODO: Send to device
        eprintln!("[radio] High-pass filter: {}", if enabled { "on" } else { "off" });
        Ok(())
    }
    
    /// Set low-pass filter (removes high frequencies)
    pub fn set_filter_low_pass(&mut self, enabled: bool) -> Result<(), String> {
        // TODO: Send to device
        eprintln!("[radio] Low-pass filter: {}", if enabled { "on" } else { "off" });
        Ok(())
    }
    
    /// Set mic gain boost level
    pub fn set_mic_gain(&mut self, level: &str) -> Result<(), String> {
        // TODO: Send to device
        eprintln!("[radio] Mic gain: {}", level);
        Ok(())
    }

    pub fn enable_smeter(&mut self, enabled: bool) -> Result<(), String> {
        let freq = self.frequency.load(Ordering::SeqCst) as f32 / 1000.0;
        let mut flags = HostStateFlags::HIGH_POWER | HostStateFlags::ENABLE_STATUS_REPORTS;
        if enabled { flags |= HostStateFlags::RSSI_ENABLED; }
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst);
        let state = HostDesiredState {
            sequence: seq as i32,
            memory_id: -1,
            flags: flags.bits(),
            bandwidth: 1,
            freq_tx: freq,
            freq_rx: freq,
            ctcss_tx: 0,
            squelch: self.current_squelch.load(Ordering::SeqCst),
            ctcss_rx: 0,
        };
        self.send(state)
    }

    pub fn ptt_on(&mut self) -> Result<(), String> {
        let freq = self.frequency.load(Ordering::SeqCst) as f32 / 1000.0;
        let mut flags = HostStateFlags::HIGH_POWER | HostStateFlags::RSSI_ENABLED;
        if self.power_high.load(Ordering::SeqCst) { flags |= HostStateFlags::HIGH_POWER; }
        flags |= HostStateFlags::PTT_REQUESTED;
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst);
        let state = HostDesiredState {
            sequence: seq as i32,
            memory_id: -1,
            flags: flags.bits(),
            bandwidth: 1,
            freq_tx: freq,
            freq_rx: freq,
            ctcss_tx: 0,
            squelch: 4,
            ctcss_rx: 0,
        };
        self.send(state)
    }

    pub fn ptt_off(&mut self) -> Result<(), String> {
        let freq = self.frequency.load(Ordering::SeqCst) as f32 / 1000.0;
        let mut flags = HostStateFlags::HIGH_POWER | HostStateFlags::RSSI_ENABLED;
        if self.power_high.load(Ordering::SeqCst) { flags |= HostStateFlags::HIGH_POWER; }
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst);
        let state = HostDesiredState {
            sequence: seq as i32,
            memory_id: -1,
            flags: flags.bits(),
            bandwidth: 1,
            freq_tx: freq,
            freq_rx: freq,
            ctcss_tx: 0,
            squelch: self.current_squelch.load(Ordering::SeqCst),
            ctcss_rx: 0,
        };
        self.send(state)
    }

    /// Send Opus-encoded audio frame to the radio for TX
    /// 
    /// Called by the audio capture system when transmitting.
    pub fn send_audio(&mut self, opus_data: &[u8]) -> Result<(), String> {
        if !self.running.load(Ordering::SeqCst) {
            return Err("Radio not connected".to_string());
        }
        
        let frame = build_tx_audio_packet(opus_data);
        
        // Send to write thread via channel
        if let Ok(guard) = self.write_tx.lock() {
            if let Some(ref tx) = *guard {
                let _ = tx.send(frame);
            }
        }
        
        Ok(())
    }

    /// Send raw audio bytes directly (for testing or alternative formats)
    pub fn send_raw_audio(&mut self, data: &[u8]) -> Result<(), String> {
        if !self.running.load(Ordering::SeqCst) {
            return Err("Radio not connected".to_string());
        }
        
        let frame = build_kv4p_packet(HostCommand::TxAudio, data);
        
        if let Ok(guard) = self.write_tx.lock() {
            if let Some(ref tx) = *guard {
                let _ = tx.send(frame);
            }
        }
        
        Ok(())
    }
}

impl Drop for KV4PRadio {
    fn drop(&mut self) { self.close(); }
}