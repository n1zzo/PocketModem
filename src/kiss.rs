//! KISS protocol implementation for KV4P HT

/// KISS protocol constants
pub const KISS_FEND: u8 = 0xC0;
pub const KISS_FESC: u8 = 0xDB;
pub const KISS_TFEND: u8 = 0xDC;
pub const KISS_TFESC: u8 = 0xDD;
pub const KISS_CMD_SETHARDWARE: u8 = 0x06;

/// KV4P vendor prefix
pub const KV4P_VENDOR: &[u8] = b"KV4P";
pub const KV4P_PROTO_VERSION: u8 = 0x01;

/// KV4P commands from host to device
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HostCommand {
    TxAudio = 0x0C,
    DesiredState = 0x0D,
    WindowAck = 0x09,  // Ack for flow control window
}

/// KV4P responses from device to host
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DeviceCommand {
    DebugInfo = 0x01,
    DebugError = 0x02,
    DebugWarn = 0x03,
    DebugDebug = 0x04,
    DebugTrace = 0x05,
    Hello = 0x06,
    Version = 0x08,
    WindowUpdate = 0x09,
    RxAudio = 0x0C,
    DeviceState = 0x0B,
    SmeterReport = 0x53,
}

/// Audio frame types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AudioFrameType {
    /// Opus encoded audio (narrowband VOIP, VBR)
    Opus = 0x01,
}

/// AX.25 frame container
/// AX.25 UI frames start with a destination callsign and end with
/// control=0x03 and PID=0xF0 for APRS
#[derive(Debug, Clone)]
pub struct Ax25Frame {
    pub data: Vec<u8>,
}

impl Ax25Frame {
    /// Check if raw data looks like an AX.25 frame
    /// 
    /// AX.25 UI frames have this structure:
    /// - Addresses: dest(7) + src(7) + path(n*7) with final bit set
    /// - Control: 0x03 (UI frame)
    /// - PID: 0xF0 (no layer 3)
    /// - Payload: APRS data
    pub fn is_ax25_frame(data: &[u8]) -> bool {
        if data.len() < 16 { return false; }  // Min: dest + src + ctrl + pid
        
        // Check for UI frame markers
        let ctrl = data[data.len() - 2];
        let pid = data[data.len() - 1];
        if ctrl != 0x03 || pid != 0xF0 {
            return false;
        }
        
        // Check if first 7 bytes look like a callsign (printable ASCII)
        let first_7 = &data[0..7];
        let mut printable_count = 0;
        for &b in first_7 {
            let masked = b & 0x7F;
            if (masked >= 0x20 && masked <= 0x5F) || (masked >= 0x61 && masked <= 0x7A) {
                printable_count += 1;
            }
        }
        
        printable_count >= 5  // Most callsign bytes should be printable
    }
}

/// Parse an RxAudio packet to determine if it's AX.25 (APRS) or voice
pub fn parse_rx_audio_packet(data: &[u8]) -> Option<Vec<u8>> {
    if data.is_empty() { return None; }
    Some(data.to_vec())
}

/// Try to parse raw data as an AX.25 frame
pub fn try_parse_ax25(data: &[u8]) -> Option<Ax25Frame> {
    if Ax25Frame::is_ax25_frame(data) {
        Some(Ax25Frame { data: data.to_vec() })
    } else {
        None
    }
}

/// Host state flags
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HostStateFlags(u16);

impl HostStateFlags {
    pub const RADIO_CONFIG_VALID: Self = Self(1 << 0);
    pub const PTT_REQUESTED: Self = Self(1 << 1);
    pub const RX_AUDIO_OPEN: Self = Self(1 << 2);
    pub const HIGH_POWER: Self = Self(1 << 3);
    pub const RSSI_ENABLED: Self = Self(1 << 4);
    pub const FILTER_PRE: Self = Self(1 << 5);
    pub const FILTER_HIGH: Self = Self(1 << 6);
    pub const FILTER_LOW: Self = Self(1 << 7);
    pub const TX_ALLOWED: Self = Self(1 << 11);
    pub const ENABLE_STATUS_REPORTS: Self = Self(1 << 12);

    pub const fn new() -> Self { Self(0) }
    pub const fn bits(&self) -> u16 { self.0 }
}

impl std::ops::BitOr for HostStateFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output { Self(self.0 | rhs.0) }
}

impl std::ops::BitOr<u16> for HostStateFlags {
    type Output = u16;
    fn bitor(self, rhs: u16) -> Self::Output { self.0 | rhs }
}

impl std::ops::BitOrAssign for HostStateFlags {
    fn bitor_assign(&mut self, rhs: Self) { self.0 |= rhs.0; }
}

impl std::ops::BitOrAssign<u16> for HostStateFlags {
    fn bitor_assign(&mut self, rhs: u16) { self.0 |= rhs; }
}

/// Device state flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceStateFlags(pub u16);

impl DeviceStateFlags {
    pub const RADIO_CONFIG_VALID: u16 = 1 << 0;
    pub const PTT_REQUESTED: u16 = 1 << 1;
    pub const RX_AUDIO_OPEN: u16 = 1 << 2;
    pub const HIGH_POWER: u16 = 1 << 3;
    pub const RSSI_ENABLED: u16 = 1 << 4;
    pub const PHYS_PTT_DOWN: u16 = 1 << 8;
    pub const TX_ACTIVE: u16 = 1 << 9;
    pub const SQUELCHED: u16 = 1 << 10;
    pub const TX_ALLOWED: u16 = 1 << 11;
    pub const ENABLE_STATUS_REPORTS: u16 = 1 << 12;

    pub const fn new() -> Self { Self(0) }
}

/// RF module type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RfModuleType {
    Sa818Vhf = 0,
    Sa818Uhf = 1,
}

/// Host Desired State (22 bytes)
#[derive(Debug, Clone, PartialEq)]
pub struct HostDesiredState {
    pub sequence: i32,
    pub memory_id: i32,
    pub flags: u16,
    pub bandwidth: u8,
    pub freq_tx: f32,
    pub freq_rx: f32,
    pub ctcss_tx: u8,
    pub squelch: u8,
    pub ctcss_rx: u8,
}

impl Default for HostDesiredState {
    fn default() -> Self {
        let flags = HostStateFlags::HIGH_POWER
            | HostStateFlags::RSSI_ENABLED
            | HostStateFlags::ENABLE_STATUS_REPORTS
            | HostStateFlags::RX_AUDIO_OPEN;
        Self {
            sequence: 1,
            memory_id: -1,
            flags: flags.bits(),
            bandwidth: 1,
            freq_tx: 145.500,
            freq_rx: 145.500,
            ctcss_tx: 0,
            squelch: 0,
            ctcss_rx: 0,
        }
    }
}

impl HostDesiredState {
    pub fn new() -> Self { Self::default() }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut result = Vec::with_capacity(22);
        result.extend_from_slice(&self.sequence.to_le_bytes());
        result.extend_from_slice(&self.memory_id.to_le_bytes());
        result.extend_from_slice(&self.flags.to_le_bytes());
        result.push(self.bandwidth);
        result.extend_from_slice(&self.freq_tx.to_le_bytes());
        result.extend_from_slice(&self.freq_rx.to_le_bytes());
        result.push(self.ctcss_tx);
        result.push(self.squelch);
        result.push(self.ctcss_rx);
        result
    }
}

/// Firmware version info
#[derive(Debug, Clone)]
pub struct VersionInfo {
    pub firmware_version: u16,
    pub radio_module_present: bool,
    pub window_size: u32,
    pub rf_module_type: RfModuleType,
    pub min_radio_freq: f32,
    pub max_radio_freq: f32,
    pub features: u8,
    pub is_valid: bool,
}

impl Default for VersionInfo {
    fn default() -> Self { Self::new() }
}

impl VersionInfo {
    pub fn new() -> Self {
        Self {
            firmware_version: 0,
            radio_module_present: false,
            window_size: 0,
            rf_module_type: RfModuleType::Sa818Vhf,
            min_radio_freq: 134.0,
            max_radio_freq: 174.0,
            features: 0,
            is_valid: false,
        }
    }
}

/// Device state from DEVICE_STATE response
#[derive(Debug, Clone)]
pub struct DeviceState {
    pub applied_sequence: i32,
    pub memory_id: i32,
    pub flags: u16,
    pub bandwidth: u8,
    pub freq_tx: f32,
    pub freq_rx: f32,
    pub ctcss_tx: u8,
    pub squelch: u8,
    pub ctcss_rx: u8,
    pub radio_module_status: char,
    pub mode: u8,
    pub last_error: u8,
    pub rssi: u8,
}

impl Default for DeviceState {
    fn default() -> Self { Self::new() }
}

impl DeviceState {
    pub fn new() -> Self {
        Self {
            applied_sequence: 0, memory_id: 0, flags: 0, bandwidth: 0,
            freq_tx: 0.0, freq_rx: 0.0, ctcss_tx: 0, squelch: 0, ctcss_rx: 0,
            radio_module_status: '?', mode: 0, last_error: 0, rssi: 0,
        }
    }
    
    /// Check if physical PTT button on radio is pressed
    pub fn phys_ptt_down(&self) -> bool {
        (self.flags & DeviceStateFlags::PHYS_PTT_DOWN) != 0
    }
    
    /// Check if TX is currently active
    pub fn tx_active(&self) -> bool {
        (self.flags & DeviceStateFlags::TX_ACTIVE) != 0
    }
    
    /// Calculate RSSI in dBm from raw value (0-255)
    /// Uses Android's formula: dbm = rssi * 1.2 - 160.8
    pub fn rssi_dbm(&self) -> f32 {
        (self.rssi as f32) * 1.2 - 160.8
    }
    pub fn smeter_bars(&self) -> u8 {
        (self.rssi * 9 / 255) as u8
    }
    pub fn is_squelched(&self) -> bool { (self.flags & DeviceStateFlags::SQUELCHED) != 0 }

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 26 { return None; }
        let mut s = Self::new();
        s.applied_sequence = i32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        s.memory_id = i32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        s.flags = u16::from_le_bytes([data[8], data[9]]);
        s.bandwidth = data[10];
        s.freq_tx = f32::from_le_bytes([data[11], data[12], data[13], data[14]]);
        s.freq_rx = f32::from_le_bytes([data[15], data[16], data[17], data[18]]);
        s.ctcss_tx = data[19];
        s.squelch = data[20];
        s.ctcss_rx = data[21];
        let ch = data[22];
        s.radio_module_status = if (32..127).contains(&ch) { ch as char } else { '?' };
        s.mode = data[23];
        s.last_error = data[24];
        s.rssi = data[25];
        Some(s)
    }
}

/// Parsed packet
#[derive(Debug, Clone)]
pub struct Packet {
    pub command: u8,
    pub payload: Vec<u8>,
}

/// KISS frame parser
pub struct PacketParser {
    buf: Vec<u8>,
}

impl Default for PacketParser {
    fn default() -> Self { Self::new() }
}

impl PacketParser {
    pub fn new() -> Self { Self { buf: Vec::new() } }

    pub fn feed(&mut self, data: &[u8]) -> Vec<Packet> {
        self.buf.extend_from_slice(data);
        let mut packets = Vec::new();

        while let Some((start, end)) = self.find_frame() {
            if end > start + 1 {
                let frame_data = &self.buf[start + 1..end];
                let unescaped = Self::unescape(frame_data);
                if let Some(pkt) = self.parse_frame(&unescaped) {
                    packets.push(pkt);
                }
            }
            self.buf.drain(..end + 1);
        }

        // If buffer doesn't start with FEND, clear it (corrupted state)
        if !self.buf.is_empty() && self.buf[0] != KISS_FEND {
            self.buf.clear();
        }

        packets
    }

    fn find_frame(&self) -> Option<(usize, usize)> {
        let start = self.buf.iter().position(|&b| b == KISS_FEND)?;
        let mut i = start + 1;
        let mut escape = false;
        while i < self.buf.len() {
            if escape { escape = false; }
            else if self.buf[i] == KISS_FESC { escape = true; }
            else if self.buf[i] == KISS_FEND { return Some((start, i)); }
            i += 1;
        }
        None
    }

    fn unescape(data: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(data.len());
        let mut i = 0;
        while i < data.len() {
            let b = data[i];
            if b == KISS_FESC && i + 1 < data.len() {
                let next = data[i + 1];
                if next == KISS_TFEND { result.push(KISS_FEND); }
                else if next == KISS_TFESC { result.push(KISS_FESC); }
                else { result.push(b); }
                i += 2;
            } else {
                result.push(b);
                i += 1;
            }
        }
        result
    }

    fn parse_frame(&self, data: &[u8]) -> Option<Packet> {
        if data.is_empty() { return None; }
        // Check for KV4P vendor prefix
        if data.len() >= 7 && &data[1..5] == KV4P_VENDOR {
            let kv4p_cmd = data[6];
            let payload = if data.len() > 7 { data[7..].to_vec() } else { Vec::new() };
            return Some(Packet { command: kv4p_cmd, payload });
        }
        Some(Packet {
            command: data[0],
            payload: if data.len() > 1 { data[1..].to_vec() } else { Vec::new() },
        })
    }

    pub fn reset(&mut self) { self.buf.clear(); }
}

fn escape_byte(b: u8) -> Vec<u8> {
    if b == KISS_FEND { vec![KISS_FESC, KISS_TFEND] }
    else if b == KISS_FESC { vec![KISS_FESC, KISS_TFESC] }
    else { vec![b] }
}

pub fn build_kiss_frame(cmd: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![KISS_FEND, cmd];
    for &b in payload { frame.extend_from_slice(&escape_byte(b)); }
    frame.push(KISS_FEND);
    frame
}

pub fn build_kv4p_packet(kv4p_cmd: HostCommand, payload: &[u8]) -> Vec<u8> {
    let mut kv4p_data = Vec::with_capacity(3 + KV4P_VENDOR.len() + payload.len());
    kv4p_data.extend_from_slice(KV4P_VENDOR);
    kv4p_data.push(KV4P_PROTO_VERSION);
    kv4p_data.push(kv4p_cmd as u8);
    kv4p_data.extend_from_slice(payload);
    build_kiss_frame(KISS_CMD_SETHARDWARE, &kv4p_data)
}

/// Build a TxAudio packet with Opus-encoded audio frame
/// 
/// Format: [opus_data: variable bytes]
pub fn build_tx_audio_packet(opus_data: &[u8]) -> Vec<u8> {
    build_kv4p_packet(HostCommand::TxAudio, opus_data)
}

// ============================================================================
// AX.25 Frame Building for APRS TX
// ============================================================================

/// Encode a 6-character callsign + SSID into 7-byte AX.25 format
/// Each character is stored in bits 1-7 (shifted right by 1)
/// Byte 7 contains SSID in lower 4 bits and control bits in upper bits
fn encode_callsign(callsign: &str) -> [u8; 7] {
    let mut result = [0x20u8; 7];  // Default to space (0x20 shifted)
    
    // Handle callsign with optional SSID
    let (call_part, ssid) = if let Some(dash_idx) = callsign.find('-') {
        (&callsign[..dash_idx], callsign[dash_idx + 1..].parse::<u8>().unwrap_or(0))
    } else {
        (callsign, 0)
    };
    
    // Copy up to 6 characters
    for (i, byte) in call_part.bytes().take(6).enumerate() {
        // Convert to uppercase and shift into bits 1-7
        let upper = byte.to_ascii_uppercase();
        result[i] = upper << 1;
    }
    
    // Byte 7: SSID in lower 4 bits
    let ssid = ssid & 0x0F;
    result[6] = (ssid << 1) | 0x60;  // 0x60 = ' ' character flags
    
    result
}

/// Build an AX.25 UI frame with digipeater path
/// 
/// Format: dest(7) + src(7) + digipeaters(n×7) + ctrl(1) + pid(1) + payload
/// The final address byte has bit 0 set to indicate last address
pub fn build_ax25_ui_frame(
    dest: &str,           // e.g., "APRS"
    src: &str,            // e.g., "KD4LCD-9"
    digipath: &[String],  // e.g., ["ARISS"]
    payload: &[u8],       // APRS text bytes
) -> Vec<u8> {
    let mut frame = Vec::with_capacity(7 + 7 + digipath.len() * 7 + 2 + payload.len());
    
    // Destination (last address flag set)
    let mut dest_bytes = encode_callsign(dest);
    dest_bytes[6] |= 0x01;  // Set last address bit
    frame.extend_from_slice(&dest_bytes);
    
    // Source (last address flag set if no digipeaters)
    let mut src_bytes = encode_callsign(src);
    if digipath.is_empty() {
        src_bytes[6] |= 0x01;  // Set last address bit
    }
    frame.extend_from_slice(&src_bytes);
    
    // Digipeaters (each with last-address flag appropriately set)
    for (i, digi) in digipath.iter().enumerate() {
        let mut digi_bytes = encode_callsign(digi);
        if i == digipath.len() - 1 {
            digi_bytes[6] |= 0x01;  // Set last address bit on final digi
        }
        frame.extend_from_slice(&digi_bytes);
    }
    
    // Control byte: 0x03 = UI frame
    frame.push(0x03);
    
    // PID byte: 0xF0 = no layer 3 protocol (APRS)
    frame.push(0xF0);
    
    // Payload (APRS data)
    frame.extend_from_slice(payload);
    
    frame
}

