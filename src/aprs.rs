//! APRS Parser - Decodes AX.25 frames into APRS messages
//!
//! Based on APRS 1.2 specification from:
//! https://github.com/wb2osz/aprsspec
//!
//! Supports:
//! - Position reports (uncompressed, compressed)
//! - Object and Item reports
//! - Messages with ACKs
//! - Weather reports
//! - NMEA sentences
//! - Status reports
//! - Digipeater path parsing

use std::time::{SystemTime, UNIX_EPOCH};

// ============================================================================
// APRS Type Definitions
// ============================================================================

/// APRS message types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum APRSType {
    Unspecified,
    Position,
    PositionWithTimestamp,
    Weather,
    Message,
    MessageAck,
    MessageReject,
    Object,
    Item,
    Status,
    Query,
    Nmea,
    Unknown,
}

impl Default for APRSType {
    fn default() -> Self { Self::Unknown }
}

/// Digipeater entry in the path
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigipeaterEntry {
    /// Callsign (e.g., "WIDE1-1", "qAS", "KV4P-10")
    pub callsign: String,
    /// Whether this digipeater was used (has * after it)
    pub was_heard: bool,
    /// Whether this was via a TRACE path
    pub via_trace: bool,
}

impl DigipeaterEntry {
    pub fn new(callsign: String) -> Self {
        Self {
            callsign,
            was_heard: false,
            via_trace: false,
        }
    }
    
    /// Check if this is a Q-construct (i-gate indicator)
    pub fn is_q_construct(&self) -> bool {
        let lower = self.callsign.to_lowercase();
        matches!(lower.as_str(), "qar" | "qas" | "qac" | "qao" | "qai")
    }
    
    /// Get the base callsign without SSID
    pub fn base_call(&self) -> &str {
        self.callsign.split('-').next().unwrap_or(&self.callsign)
    }
}

/// Parsed AX.25 frame with full address information
#[derive(Debug, Clone)]
pub struct ParsedAX25Frame {
    /// Destination callsign (usually "APRS" or "GPS")
    pub destination: String,
    /// Source callsign (transmitting station)
    pub source: String,
    /// Digipeater path
    pub digipeaters: Vec<DigipeaterEntry>,
    /// Raw payload bytes
    pub payload: Vec<u8>,
}

impl ParsedAX25Frame {
    /// Get the i-gate callsign if present in digipeater path
    /// Q-constructs indicate the i-gate that injected the packet into APRS-IS
    pub fn i_gate(&self) -> Option<&str> {
        for (i, digi) in self.digipeaters.iter().enumerate() {
            if digi.is_q_construct() {
                // Next digipeater after Q-construct is the i-gate
                if i + 1 < self.digipeaters.len() {
                    return Some(&self.digipeaters[i + 1].callsign);
                }
            }
        }
        None
    }
}

/// APRS message with decoded data
#[derive(Debug, Clone, Default)]
pub struct APRSMessage {
    pub msg_type: APRSType,
    pub from_callsign: String,
    pub to_callsign: String,
    
    /// Timestamp from packet (seconds since Unix epoch, UTC)
    /// None if no timestamp in packet (defaults to receive time)
    pub timestamp: Option<u64>,
    /// Whether the timestamp is marked as UTC
    pub timestamp_is_utc: bool,
    
    pub position_lat: f64,
    pub position_lon: f64,
    pub comment: String,
    pub relay_callsign: String,
    
    // APRS Symbol (symbol table ID and symbol code from position report)
    // Symbol table ID: '/' (primary) or '\\' (alternate)
    // Symbol code: character from APRS symbol table
    pub symbol_table_id: Option<char>,
    pub symbol_code: Option<char>,
    
    // Speed and course (from position or NMEA)
    pub speed: Option<u16>,      // knots
    pub course: Option<u16>,     // degrees (0-360)
    pub altitude: Option<i32>,   // feet
    
    // Message-specific
    pub msg_body: Option<String>,
    pub to_callsign_msg: Option<String>,
    pub msg_id: Option<String>,  // Message ID for ACKs
    
    // Tracking for sent messages
    pub is_sent: bool,            // True if this message was sent by us
    pub is_acknowledged: bool,    // True if we received an ACK for this message
    
    // Object-specific
    pub obj_name: Option<String>,
    pub is_killed: bool,         // Object/Item has been killed (KILL)
    
    // Weather-specific
    pub temperature: Option<f64>,
    pub humidity: Option<f64>,
    pub pressure: Option<f64>,
    pub wind_force: Option<i32>,
    pub wind_dir: Option<String>,
    
    // Digipeater path (for display/debugging)
    pub digipeaters: Vec<DigipeaterEntry>,
    pub i_gate: Option<String>,
}

impl APRSMessage {
    pub fn new() -> Self {
        Self {
            timestamp: Some(SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)),
            timestamp_is_utc: true,
            is_sent: false,
            is_acknowledged: false,
            ..Default::default()
        }
    }
    
    /// Create a new message marked as sent by us
    pub fn new_sent(to: &str, body: &str, msg_id: &str) -> Self {
        Self {
            msg_type: APRSType::Message,
            from_callsign: String::new(),  // Will be set by caller
            to_callsign: String::new(),
            timestamp: Some(SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)),
            timestamp_is_utc: true,
            to_callsign_msg: Some(to.to_string()),
            msg_body: Some(body.to_string()),
            msg_id: Some(msg_id.to_string()),
            is_sent: true,
            is_acknowledged: false,
            ..Default::default()
        }
    }
    
    /// Check if message is pending (sent but not acknowledged)
    pub fn is_pending(&self) -> bool {
        self.is_sent && !self.is_acknowledged
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Parse an AX.25 UI frame into an APRS message
pub fn parse_ax25_frame(frame: &[u8]) -> Option<APRSMessage> {
    let parsed = parse_ax25_addresses(frame)?;
    Some(build_aprs_message(parsed))
}

/// Parse AX.25 frame and return full address information
pub fn parse_ax25_addresses(frame: &[u8]) -> Option<ParsedAX25Frame> {
    if frame.len() < 16 { return None; }
    
    // Find ctrl (0x03) and pid (0xF0) by scanning backwards
    let mut ctrl_idx = None;
    let mut pid_idx = None;
    for i in (0..frame.len() - 1).rev() {
        if frame[i] == 0x03 && frame[i + 1] == 0xF0 {
            ctrl_idx = Some(i);
            pid_idx = Some(i + 1);
            break;
        }
    }
    
    let (ctrl_idx, pid_idx) = match (ctrl_idx, pid_idx) {
        (Some(c), Some(p)) => (c, p),
        _ => return None,
    };
    
    // Parse all addresses using the helper
    let (addresses, _) = parse_addresses(&frame[..ctrl_idx])?;
    
    if addresses.len() < 2 {
        return None;
    }
    
    // AX.25 frame structure:
    // [destination][source][digipeaters...][ctrl=0x03][pid=0xF0][payload]
    // First address = destination (last bit set to mark final)
    // Last address before ctrl = source (last bit may or may not be set)
    // Intermediate addresses = digipeaters
    
    let destination = addresses[0].clone();
    let source = addresses[1].clone();
    let digipeaters: Vec<DigipeaterEntry> = addresses[2..]
        .iter()
        .map(|call| DigipeaterEntry::new(call.clone()))
        .collect();
    
    let payload = frame[pid_idx + 1..].to_vec();
    
    Some(ParsedAX25Frame {
        destination,
        source,
        digipeaters,
        payload,
    })
}

/// Build APRS message from parsed AX.25 frame
fn build_aprs_message(parsed: ParsedAX25Frame) -> APRSMessage {
    let mut msg = APRSMessage::new();
    msg.from_callsign = parsed.source.clone();
    msg.to_callsign = parsed.destination.clone();
    msg.digipeaters = parsed.digipeaters.clone();
    msg.i_gate = parsed.i_gate().map(String::from);
    
    if parsed.payload.is_empty() {
        msg.msg_type = APRSType::Unknown;
        return msg;
    }
    
    let dti = parsed.payload[0] as char;
    match dti {
        '=' | '!' | '\'' | '"' => decode_position(&mut msg, &parsed.payload, false),
        '/' | '@' => decode_position(&mut msg, &parsed.payload, true),
        ')' => {
            // Object or Item - check byte 9 for ')' vs '*'
            // Object: byte 9 = ')' (alive)
            // Item: byte 9 = ')' (alive) but format is different
            // Killed: byte 9 = '*'
            if parsed.payload.len() >= 10 && (parsed.payload[9] & 0x7F) == b'*' {
                msg.is_killed = true;
            }
            decode_object(&mut msg, &parsed.payload);
        },
        ':' => decode_message(&mut msg, &parsed.payload),
        '$' => decode_nmea(&mut msg, &parsed.payload),
        '>' => decode_status(&mut msg, &parsed.payload),
        '#' | '*' | '_' => decode_weather(&mut msg, &parsed.payload),
        '?' => decode_query(&mut msg, &parsed.payload),
        _ => {
            msg.msg_type = APRSType::Unknown;
            msg.comment = String::from_utf8_lossy(&parsed.payload).trim_end().to_string();
        }
    }
    
    msg
}

// ============================================================================
// AX.25 Address Parsing
// ============================================================================

/// Parse a 7-byte AX.25 callsign field
fn parse_callsign(data: &[u8]) -> Option<(String, usize)> {
    if data.len() < 7 { return None; }
    
    // AX.25 callsigns are 6 characters stored in 7 bytes
    // Each character occupies bits 1-7 (shifted right by 1)
    // Byte 7 contains SSID in lower 4 bits and final-bit flag in bit 0
    let mut chars = Vec::new();
    for i in 0..6 {
        let c = (data[i] >> 1) as u8;
        if c == 0x20 || c == 0x00 {
            break;  // Padding
        }
        if c >= 0x20 && c < 0x7F {
            chars.push(c as char);
        } else {
            chars.push('?');
        }
    }
    
    if chars.is_empty() {
        return None;
    }
    
    let call: String = chars.into_iter().collect();
    let ssid = (data[6] >> 1) & 0x0F;  // Bits 1-4 = SSID (shifted right already)
    
    let result = if ssid > 0 {
        format!("{}-{}", call, ssid)
    } else {
        call
    };
    
    Some((result, 7))
}

/// Parse all addresses from AX.25 frame
/// Returns (callsigns, total_bytes_consumed)
fn parse_addresses(frame: &[u8]) -> Option<(Vec<String>, usize)> {
    let mut addresses = Vec::new();
    let mut offset = 0;
    
    loop {
        if offset + 7 > frame.len() { return None; }
        
        let (call, _) = parse_callsign(&frame[offset..offset+7])?;
        addresses.push(call);
        
        // Bit 0 of byte 6 is the "last address" flag
        let is_final = (frame[offset + 6] & 0x01) != 0;
        offset += 7;
        
        if is_final { break; }
    }
    
    Some((addresses, offset))
}

// ============================================================================
// Timestamp Parsing
// ============================================================================

/// Parse APRS timestamp from position report data
/// 
/// Returns (timestamp_seconds_since_midnight_utc, day_of_month, consumed_bytes)
/// 
/// Formats:
/// - @HHMMSSz  (8 chars, z=UTC)
/// - /DDHHMMh  (7 chars, h=UTC with day-of-month)
/// - HHMMSSz   (7 chars, z=UTC, no leading @)
/// - HHMMSS/   (7 chars, / = UTC, no leading @)
fn parse_aprs_timestamp(data: &[u8]) -> Option<(u64, Option<u8>, usize)> {
    if data.len() < 7 { return None; }
    
    let first = data[0] as char;
    let seventh = data[6] as char;
    
    // Format: @HHMMSSz (8 chars)
    if first == '@' && data.len() >= 8 && seventh == 'z' {
        let time_str = String::from_utf8_lossy(&data[1..7]);
        if let Some(secs) = parse_hhmmss(&time_str) {
            return Some((secs, None, 8));
        }
    }
    
    // Format: /DDHHMMh (7 chars with day)
    if first == '/' && data.len() >= 7 {
        let suffix = data[6] as char;
        if suffix == 'h' || suffix == 'z' {
            let day_str = String::from_utf8_lossy(&data[1..3]);
            let time_str = String::from_utf8_lossy(&data[3..7]);
            if let (Ok(day), Some(secs)) = (day_str.parse::<u8>(), parse_hhmmss(&time_str)) {
                return Some((secs, Some(day), 7));
            }
        }
    }
    
    // Format: HHMMSSz (7 chars, z=UTC, no leading @)
    if seventh == 'z' {
        let time_str = String::from_utf8_lossy(&data[..6]);
        if let Some(secs) = parse_hhmmss(&time_str) {
            return Some((secs, None, 7));
        }
    }
    
    // Format: HHMMSS/ (7 chars, / = UTC, no leading @)
    if seventh == '/' {
        let time_str = String::from_utf8_lossy(&data[..6]);
        if let Some(secs) = parse_hhmmss(&time_str) {
            return Some((secs, None, 7));
        }
    }
    
    None
}

/// Parse HHMMSS string to seconds since midnight
fn parse_hhmmss(s: &str) -> Option<u64> {
    if s.len() != 6 { return None; }
    
    let hh: u64 = s[..2].parse().ok()?;
    let mm: u64 = s[2..4].parse().ok()?;
    let ss: u64 = s[4..6].parse().ok()?;
    
    if hh > 23 || mm > 59 || ss > 59 { return None; }
    
    Some(hh * 3600 + mm * 60 + ss)
}

/// Convert parsed APRS timestamp to Unix timestamp
/// Requires current date to fill in day if not provided
#[allow(dead_code)]
fn make_unix_timestamp(hhmmss: u64, day: Option<u8>, _current_time: SystemTime) -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    
    // Get current day of month
    let current_day = ((now / 86400) % 31) as u8 + 1;
    
    // Use provided day or current day
    let _use_day = day.unwrap_or(current_day);
    
    // Calculate timestamp for today at HH:MM:SS
    let today_start = now - (now % 86400);
    today_start + hhmmss
}

// ============================================================================
// Position Parsing
// ============================================================================

/// Decode APRS position report
fn decode_position(msg: &mut APRSMessage, payload: &[u8], has_timestamp: bool) {
    msg.msg_type = if has_timestamp { APRSType::PositionWithTimestamp } else { APRSType::Position };
    
    let data = &payload[1..];  // Skip DTI
    
    // First, try to parse timestamp if present
    if let Some((hhmmss, day, consumed)) = parse_aprs_timestamp(data) {
        msg.timestamp = Some(make_unix_timestamp(hhmmss, day, SystemTime::now()));
        msg.timestamp_is_utc = true;
        
        // Try to parse position after timestamp
        if let Some((lat, lon, sym_table, sym_code, remainder)) = parse_position_body(&data[consumed..]) {
            msg.position_lat = lat;
            msg.position_lon = lon;
            msg.symbol_table_id = sym_table;
            msg.symbol_code = sym_code;
            let comment = String::from_utf8_lossy(remainder).trim_end().to_string();
            msg.comment = comment.clone();
            
            // Extract course/speed from comment
            extract_course_speed(&comment, msg);
            extract_altitude(&comment, msg);
            return;
        }
    } else {
        // No timestamp, try position directly
        if let Some((lat, lon, sym_table, sym_code, remainder)) = parse_position_body(data) {
            msg.position_lat = lat;
            msg.position_lon = lon;
            msg.symbol_table_id = sym_table;
            msg.symbol_code = sym_code;
            let comment = String::from_utf8_lossy(remainder).trim_end().to_string();
            msg.comment = comment.clone();
            
            extract_course_speed(&comment, msg);
            extract_altitude(&comment, msg);
            return;
        }
    }
    
    // Fallback: couldn't parse position
    msg.comment = String::from_utf8_lossy(data).trim_end().to_string();
}

/// Parse position data and return (lat, lon, symbol_table, symbol_code, remainder)
fn parse_position_body(data: &[u8]) -> Option<(f64, f64, Option<char>, Option<char>, &[u8])> {
    // Try compressed position first: /XXXXX000000>OOOOOO (13 chars)
    if data.len() >= 14 && data[0] == b'/' {
        if let Some(result) = parse_compressed_position(&data[1..]) {
            return Some(result);
        }
    }
    
    // Try uncompressed positions (offset varies with timestamp)
    
    // Check for leading / (timestamp) or @ (timestamp)
    if data.len() >= 1 {
        let first = data[0] as char;
        if first == '@' || first == '/' {
            // This is a timestamp, skip it
            // We'll handle this in parse_aprs_timestamp
        }
    }
    
    // Format A: DDMM.MMN/DDDMM.MMW[sym] (18+ chars with / separator)
    // Pattern: DDMM.MM + N/S + / + DDDMM.MM + E/W
    // Total: 18 chars for position, remainder is symbol + comment
    
    let lat_str = String::from_utf8_lossy(&data[0..7]);
    let ns = data[7] as char;
    let sep = data[8] as char;
    
    if sep == '/' {
        if let Some(result) = try_parse_position_with_separator(data) {
            return Some(result);
        }
    }
    
    // Format B: DDMM.MMN DDDMM.MMW[sym] (without / separator)
    if is_ns(ns) {
        if let Some(result) = try_parse_position_b(data) {
            return Some(result);
        }
    }
    
    // Format C: DDMMN/DDDMMW[sym] (no decimal minutes)
    if data.len() >= 12 {
        let sep_c = data[5] as char;
        if sep_c == '/' {
            if let Some(result) = try_parse_position_c(data) {
                return Some(result);
            }
        }
    }
    
    None
}

/// Parse compressed position format: XXXXX000000>OOOOOO (13 chars)
#[allow(dead_code)]
fn parse_compressed_position(data: &[u8]) -> Option<(f64, f64, Option<char>, Option<char>, &[u8])> {
    if data.len() < 13 { return None; }
    
    // Compressed format (after /):
    // XXXXX = 5 chars, base91 encoded latitude offset
    // 00000 = 5 chars, unused (for compatibility)
    // > = symbol table ID
    // OOOOOO = 6 chars, base91 encoded longitude/course/speed/symbol
    
    let table_id = data[10] as char;
    if table_id != '/' && table_id != '\\' {
        return None;
    }
    
    // Decode base91 lat/lon
    let lat_val: u32 = base91_decode_5(&data[0..5])?;
    let lon_val: u32 = base91_decode_6(&data[11..17])?;
    
    // Convert to decimal degrees
    // lat = (lat_val - 38092) / 38092 * 180 - 90
    let lat = (lat_val as f64 - 38092.0) / 38092.0 * 180.0 - 90.0;
    let lon = (lon_val as f64 - 19084.0) / 19084.0 * 360.0 - 180.0;
    
    // Symbol code is embedded in OOOOOO, but we only have the table ID
    let symbol_code = data[16] as char;
    let remainder = &data[17..];
    
    Some((lat, lon, Some(table_id), Some(symbol_code), remainder))
}

/// Base91 decode for 5 characters
fn base91_decode_5(data: &[u8]) -> Option<u32> {
    if data.len() < 5 { return None; }
    let mut val: u32 = 0;
    for &b in data {
        let c = (b as char).to_ascii_uppercase();
        let n = c as u32;
        if n < 33 || n > 122 { return None; }
        val = val.wrapping_mul(91).wrapping_add(n.wrapping_sub(33));
    }
    Some(val)
}

/// Base91 decode for 6 characters
fn base91_decode_6(data: &[u8]) -> Option<u32> {
    if data.len() < 6 { return None; }
    let mut val: u32 = 0;
    for &b in data {
        let c = (b as char).to_ascii_uppercase();
        let n = c as u32;
        if n < 33 || n > 122 { return None; }
        val = val.wrapping_mul(91).wrapping_add(n.wrapping_sub(33));
    }
    Some(val)
}

/// Try to parse Format A: DDMM.MMN/DDDMM.MMW[sym] (with / separator)
fn try_parse_position_with_separator(data: &[u8]) -> Option<(f64, f64, Option<char>, Option<char>, &[u8])> {
    // Pattern: DDMM.MM(7) + N/S(1) + /(1) + DDDMM.MM(8) + E/W(1)
    // Total: 18 chars for position, remainder is symbol + comment
    
    if data.len() < 19 { return None; }
    
    let ns = data[7] as char;
    if !is_ns(ns) { return None; }
    
    let lon_start = 9;
    let lon_str = String::from_utf8_lossy(&data[lon_start..lon_start + 8]);
    let ew = data[lon_start + 8] as char;
    
    if !is_ew(ew) { return None; }
    
    let lat = decode_lat(&String::from_utf8_lossy(&data[0..7]), ns)?;
    let lon = decode_lon(&lon_str, ew)?;
    
    // Symbol is at position 18 (after E/W)
    let sym_pos = 18;
    let (sym_table, sym_code, consumed) = extract_symbol(data, sym_pos)?;
    
    let remainder = &data[sym_pos + consumed..];
    
    Some((lat, lon, sym_table, sym_code, remainder))
}

/// Try to parse Format B: DDMM.MMN DDDMM.MMW[sym] (without / separator)
fn try_parse_position_b(data: &[u8]) -> Option<(f64, f64, Option<char>, Option<char>, &[u8])> {
    // Pattern: DDMM.MM(7) + N/S(1) + DDDMM.MM(8) + E/W(1)
    // Total: 17 chars for position
    
    if data.len() < 18 { return None; }
    
    let ns = data[7] as char;
    if !is_ns(ns) { return None; }
    
    let lon_start = 8;
    let lon_str = String::from_utf8_lossy(&data[lon_start..lon_start + 8]);
    let ew = data[lon_start + 8] as char;
    
    if !is_ew(ew) { return None; }
    
    let lat = decode_lat(&String::from_utf8_lossy(&data[0..7]), ns)?;
    let lon = decode_lon(&lon_str, ew)?;
    
    // Symbol is at position 17 (after E/W)
    let sym_pos = 17;
    let (sym_table, sym_code, consumed) = extract_symbol(data, sym_pos)?;
    
    let remainder = &data[sym_pos + consumed..];
    
    Some((lat, lon, sym_table, sym_code, remainder))
}

/// Try to parse Format C: DDMMN/DDDMMW[sym] (no decimal minutes)
fn try_parse_position_c(data: &[u8]) -> Option<(f64, f64, Option<char>, Option<char>, &[u8])> {
    // Pattern: DDMM(4) + N/S(1) + /(1) + DDDMM(5) + E/W(1)
    // Total: 12 chars for position
    
    if data.len() < 13 { return None; }
    
    let ns = data[4] as char;
    let sep = data[5] as char;
    
    if sep != '/' { return None; }
    
    let lon_start = 6;
    let lon_str = String::from_utf8_lossy(&data[lon_start..lon_start + 5]);
    let ew = data[lon_start + 5] as char;
    
    if !is_ns(ns) || !is_ew(ew) { return None; }
    
    let lat = decode_lat_no_decimal(&String::from_utf8_lossy(&data[0..4]), ns)?;
    let lon = decode_lon_no_decimal(&lon_str, ew)?;
    
    // Symbol is at position 12 (after E/W)
    let sym_pos = 12;
    let (sym_table, sym_code, consumed) = extract_symbol(data, sym_pos)?;
    
    let remainder = &data[sym_pos + consumed..];
    
    Some((lat, lon, sym_table, sym_code, remainder))
}

/// Extract symbol table ID and code from position data
/// Returns (table_id, symbol_code, bytes_consumed)
fn extract_symbol(data: &[u8], pos: usize) -> Option<(Option<char>, Option<char>, usize)> {
    if pos >= data.len() { return None; }
    
    let first = data[pos] as char;
    if first == '/' || first == '\\' {
        let table_id = Some(first);
        let sym_code = if pos + 1 < data.len() { Some(data[pos + 1] as char) } else { None };
        // Consumed 1 byte for table ID, plus 1 for symbol code if present
        let consumed = if sym_code.is_some() { 2 } else { 1 };
        Some((table_id, sym_code, consumed))
    } else {
        // No explicit table ID, default to primary, consumed just 1 byte for symbol
        Some((Some('/'), Some(first), 1))
    }
}

/// Check if character is N or S
fn is_ns(c: char) -> bool {
    matches!(c, 'N' | 'n' | 'S' | 's')
}

/// Check if character is E or W
fn is_ew(c: char) -> bool {
    matches!(c, 'E' | 'e' | 'W' | 'w')
}

/// Decode latitude from string with N/S suffix
fn decode_lat(s: &str, ns: char) -> Option<f64> {
    let (deg, min) = parse_deg_min(s)?;
    let mut lat = deg + min / 60.0;
    if ns == 'S' || ns == 's' {
        lat = -lat;
    }
    Some(lat)
}

/// Decode longitude from string with E/W suffix
fn decode_lon(s: &str, ew: char) -> Option<f64> {
    let (deg, min) = parse_deg_min(s)?;
    let mut lon = deg + min / 60.0;
    if ew == 'W' || ew == 'w' {
        lon = -lon;
    }
    Some(lon)
}

/// Decode latitude without decimal (DDMM format)
fn decode_lat_no_decimal(s: &str, ns: char) -> Option<f64> {
    let val: f64 = s.parse().ok()?;
    let deg = (val / 100.0).floor();
    let min = val - (deg * 100.0);
    let mut lat = deg + min / 60.0;
    if ns == 'S' || ns == 's' {
        lat = -lat;
    }
    Some(lat)
}

/// Decode longitude without decimal (DDDMM format)
fn decode_lon_no_decimal(s: &str, ew: char) -> Option<f64> {
    let val: f64 = s.parse().ok()?;
    let deg = (val / 100.0).floor();
    let min = val - (deg * 100.0);
    let mut lon = deg + min / 60.0;
    if ew == 'W' || ew == 'w' {
        lon = -lon;
    }
    Some(lon)
}

/// Parse degrees/minutes format: DDMM.MM or DDDMM.MM
fn parse_deg_min(s: &str) -> Option<(f64, f64)> {
    let s = s.trim();
    if s.len() < 4 { return None; }
    
    if let Some(dot_idx) = s.find('.') {
        let deg_part = &s[..dot_idx];
        let min_part = &s[dot_idx + 1..];
        
        // Check for DDDMM.MM (5 chars before decimal, e.g., "00941.09")
        if deg_part.len() == 5 {
            if let (Ok(deg), Ok(min)) = (
                deg_part[..3].parse::<f64>(),
                format!("{}.{}", &deg_part[3..5], min_part).parse::<f64>()
            ) {
                return Some((deg, min));
            }
        }
        // Check for DDMM.MM (4 chars before decimal, e.g., "4534.36")
        else if deg_part.len() == 4 {
            if let (Ok(deg), Ok(min)) = (
                deg_part[..2].parse::<f64>(),
                format!("{}.{}", &deg_part[2..4], min_part).parse::<f64>()
            ) {
                return Some((deg, min));
            }
        }
    }
    
    // Try without decimal - assume DDMM format
    if let Ok(val) = s.parse::<f64>() {
        let deg = (val / 100.0).floor();
        let min = val - (deg * 100.0);
        return Some((deg, min));
    }
    
    None
}

/// Extract course and speed from comment
fn extract_course_speed(comment: &str, msg: &mut APRSMessage) {
    // PHG format: PHG1234 or PHG1234/567
    // Course/speed format: /123/045 or /123/045g067 (with gusts)
    
    if let Some(slash_idx) = comment.find("/") {
        if slash_idx + 4 <= comment.len() {
            let after_slash = &comment[slash_idx + 1..];
            
            // Try /DDD/SSS format (course/speed)
            let parts: Vec<&str> = after_slash.split('/').collect();
            if parts.len() >= 2 {
                if let Ok(course) = parts[0].parse() {
                    msg.course = Some(course);
                }
                if let Ok(speed) = parts[1].parse::<u16>() {
                    msg.speed = Some(speed);
                }
            }
        }
    }
    
    // Try sDDD format (speed in knots)
    if let Some(s_idx) = comment.find('s') {
        if s_idx + 4 <= comment.len() {
            if let Ok(speed) = comment[s_idx + 1..s_idx + 4].parse() {
                msg.speed = Some(speed);
            }
        }
    }
    
    // Try cDDD format (course in degrees)
    if let Some(c_idx) = comment.find('c') {
        if c_idx + 4 <= comment.len() {
            if let Ok(course) = comment[c_idx + 1..c_idx + 4].parse() {
                msg.course = Some(course);
            }
        }
    }
}

/// Extract altitude from comment
fn extract_altitude(comment: &str, msg: &mut APRSMessage) {
    // /A=XXXXXX format (6 digits = feet)
    if let Some(eq_idx) = comment.find("/A=") {
        if eq_idx + 9 <= comment.len() {
            let alt_str = &comment[eq_idx + 3..eq_idx + 9.min(comment.len())];
            if let Ok(alt) = alt_str.parse::<i32>() {
                msg.altitude = Some(alt);
            }
        }
    }
    
    // cXXXXX format (altitude in feet / 10)
    if let Some(c_idx) = comment.find('c') {
        if c_idx + 6 <= comment.len() && !comment[c_idx..].starts_with("c...") {
            let alt_str = &comment[c_idx + 1..c_idx + 6];
            if let Ok(alt_tenths) = alt_str.parse::<i32>() {
                msg.altitude = Some(alt_tenths * 10);
            }
        }
    }
}

// ============================================================================
// Object/Item Parsing
// ============================================================================

/// Decode object report (')' DTI)
fn decode_object(msg: &mut APRSMessage, payload: &[u8]) {
    let data = &payload[1..];  // Skip ')'
    
    if data.len() < 9 {
        msg.comment = String::from_utf8_lossy(data).trim_end().to_string();
        return;
    }
    
    // Object name is first 9 characters
    let name_bytes: Vec<u8> = data[..9].iter().map(|&b| b & 0x7F).collect();
    msg.obj_name = Some(String::from_utf8_lossy(&name_bytes).trim().to_string());
    
    // Check if killed (*)
    if (data[9] & 0x7F) == b'*' {
        msg.is_killed = true;
        msg.msg_type = APRSType::Object;
        // Rest should be position data
        if data.len() > 10 {
            decode_position(msg, &data[10..], false);
        }
    } else {
        // Could be Item or Object - check if byte 9 is ')' (alive object)
        if (data[9] & 0x7F) == b')' {
            msg.msg_type = APRSType::Object;
        } else {
            msg.msg_type = APRSType::Item;
        }
        if data.len() > 10 {
            decode_position(msg, &data[10..], false);
        }
    }
    
    if msg.comment.is_empty() && data.len() > 10 {
        msg.comment = String::from_utf8_lossy(&data[10..]).trim_end().to_string();
    }
}

// ============================================================================
// Message Parsing
// ============================================================================

/// Decode APRS message (messaging)
fn decode_message(msg: &mut APRSMessage, payload: &[u8]) {
    let data = &payload[1..];  // Skip ':'

    if let Some(colon_idx) = data.iter().position(|&b| b == b':') {
        let addr_bytes: Vec<u8> = data[..colon_idx].iter().map(|&b| b & 0x7F).collect();
        let addr = String::from_utf8_lossy(&addr_bytes).trim().to_string();

        msg.to_callsign_msg = Some(addr.clone());
        msg.to_callsign = addr;

        if colon_idx + 1 < data.len() {
            let body = &data[colon_idx + 1..];

            // Check for message type
            let body_str: String = String::from_utf8_lossy(body).into_owned();

            if body_str.starts_with("ack") {
                // Acknowledgment
                msg.msg_type = APRSType::MessageAck;
                msg.msg_body = Some(body_str.clone());
                // Extract message ID if present
                if let Some(msg_id) = extract_message_id(&body_str) {
                    msg.msg_id = Some(msg_id);
                }
            } else if body_str.starts_with("rej") {
                // Rejection
                msg.msg_type = APRSType::MessageReject;
                msg.msg_body = Some(body_str.clone());
                if let Some(msg_id) = extract_message_id(&body_str) {
                    msg.msg_id = Some(msg_id);
                }
            } else {
                // Regular message - check for message ID in braces {id
                msg.msg_type = APRSType::Message;
                if let Some(braces_idx) = body_str.find('{') {
                    msg.msg_id = Some(body_str[braces_idx + 1..].trim().to_string());
                    msg.msg_body = Some(body_str[..braces_idx].trim().to_string());
                } else {
                    msg.msg_body = Some(body_str.trim().to_string());
                }
            }
        }
    }
}

/// Extract message ID from acknowledgment body
fn extract_message_id(body: &str) -> Option<String> {
    // ackXXXX format
    if body.len() >= 5 {
        let id = body[3..].trim();
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    None
}

// ============================================================================
// Query Parsing
// ============================================================================

/// Decode APRS query (?' DTI)
fn decode_query(msg: &mut APRSMessage, payload: &[u8]) {
    msg.msg_type = APRSType::Query;
    let data = &payload[1..];  // Skip '?'
    msg.comment = String::from_utf8_lossy(data).trim_end().to_string();
}

// ============================================================================
// NMEA Parsing
// ============================================================================

/// Decode NMEA sentence ($)
fn decode_nmea(msg: &mut APRSMessage, payload: &[u8]) {
    msg.msg_type = APRSType::Nmea;
    let data = &payload[1..];  // Skip '$'
    let sentence = String::from_utf8_lossy(data).trim_end().to_string();
    
    // Parse GPRMC for position, speed, course
    if sentence.starts_with("GPRMC,") || sentence.starts_with("GNRMC,") {
        if let Some((lat, lon, speed, course)) = parse_gprmc(&sentence) {
            // Only update if we don't have a better position
            if msg.position_lat == 0.0 && msg.position_lon == 0.0 {
                msg.position_lat = lat;
                msg.position_lon = lon;
            }
            if speed > 0 {
                msg.speed = Some(speed);
            }
            if course < 360 {
                msg.course = Some(course);
            }
        }
    }
    
    msg.comment = sentence;
}

/// Parse $GPRMC NMEA sentence
/// $GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A
fn parse_gprmc(sentence: &str) -> Option<(f64, f64, u16, u16)> {
    let fields: Vec<&str> = sentence.split(',').collect();
    
    if fields.len() < 12 { return None; }
    if fields.get(2) != Some(&"A") { return None; }  // Status must be 'A' (active)
    
    // Field 3: Latitude (DDMM.MMM)
    let lat_str = fields.get(3)?;
    let lat_dir = fields.get(4).and_then(|s| s.chars().next())?;
    
    // Field 5: Longitude (DDDMM.MMM)
    let lon_str = fields.get(5)?;
    let lon_dir = fields.get(6).and_then(|s| s.chars().next())?;
    
    // Field 7: Speed (knots)
    let speed: u16 = fields.get(7).and_then(|s| s.parse::<f64>().ok()).map(|f| f as u16).unwrap_or(0);
    
    // Field 8: Course (degrees)
    let course: u16 = fields.get(8).and_then(|s| s.parse::<f64>().ok()).map(|f| f as u16).unwrap_or(360);
    
    let lat = parse_nmea_coord(lat_str, lat_dir)?;
    let lon = parse_nmea_coord(lon_str, lon_dir)?;
    
    Some((lat, lon, speed, course))
}

/// Parse NMEA coordinate string to decimal degrees
fn parse_nmea_coord(s: &str, dir: char) -> Option<f64> {
    let s = s.trim();
    let dot_idx = s.find('.')?;
    
    let deg_part = &s[..dot_idx];
    let min_part = &s[dot_idx + 1..];
    
    let deg: f64 = if deg_part.len() == 4 {
        // DDMM.MM format
        deg_part[..2].parse().ok()?
    } else if deg_part.len() == 5 {
        // DDDMM.MM format
        deg_part[..3].parse().ok()?
    } else {
        return None;
    };
    
    let min: f64 = format!("{}.{}", &deg_part[deg_part.len() - 2..], min_part)
        .parse()
        .ok()?;
    
    let mut decimal = deg + min / 60.0;
    
    if dir == 'S' || dir == 's' || dir == 'W' || dir == 'w' {
        decimal = -decimal;
    }
    
    Some(decimal)
}

// ============================================================================
// Status Parsing
// ============================================================================

/// Decode status report (>' DTI)
fn decode_status(msg: &mut APRSMessage, payload: &[u8]) {
    msg.msg_type = APRSType::Status;
    let data = &payload[1..];  // Skip '>'
    msg.comment = String::from_utf8_lossy(data).trim_end().to_string();
}

// ============================================================================
// Weather Parsing
// ============================================================================

/// Decode weather report (# * _ DTIs)
fn decode_weather(msg: &mut APRSMessage, payload: &[u8]) {
    msg.msg_type = APRSType::Weather;
    let data = &payload[1..];
    msg.comment = String::from_utf8_lossy(data).trim_end().to_string();
    
    parse_weather_data(msg, data);
}

/// Parse weather-specific fields from comment
fn parse_weather_data(msg: &mut APRSMessage, data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    
    // Temperature: t123 (in °F, tenths)
    if let Some(start) = text.find('t') {
        if start + 4 <= text.len() {
            let temp_str = &text[start + 1..start + 4];
            if let Ok(temp_f) = temp_str.parse::<f64>() {
                let temp_c = (temp_f - 32.0) * 5.0 / 9.0;
                msg.temperature = Some(temp_c);
            }
        }
    }
    
    // Wind speed: s123
    if let Some(start) = text.find('s') {
        if start + 4 <= text.len() {
            let wind_str = &text[start + 1..start + 4];
            if let Ok(wind) = wind_str.parse::<i32>() {
                msg.wind_force = Some(wind);
            }
        }
    }
    
    // Wind direction: d123
    if let Some(start) = text.find('d') {
        if start + 4 <= text.len() {
            let dir_str = &text[start + 1..start + 4];
            if let Ok(dir) = dir_str.parse::<i32>() {
                msg.wind_dir = Some(format!("{}°", dir));
            }
        }
    }
    
    // Pressure: b12345 (hPa * 10)
    if let Some(start) = text.find('b') {
        if start + 6 <= text.len() {
            let pres_str = &text[start + 1..start + 6];
            if let Ok(pres_hpa) = pres_str.parse::<f64>() {
                msg.pressure = Some(pres_hpa * 10.0);
            }
        }
    }
    
    // Humidity: h00-99 (percentage)
    if let Some(start) = text.find('h') {
        if start + 3 <= text.len() {
            let hum_str = &text[start + 1..start + 3];
            if hum_str != ".." {
                if let Ok(hum) = hum_str.parse::<f64>() {
                    msg.humidity = Some(hum);
                }
            }
        }
    }
}

// ============================================================================
// Utility Functions
// ============================================================================

/// Get the base callsign without SSID
pub fn base_callsign(callsign: &str) -> String {
    callsign.split('-').next().unwrap_or(callsign).to_string()
}

/// Get SSID from callsign
pub fn ssid(callsign: &str) -> i32 {
    callsign.split('-')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// APRS symbol configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AprsSymbol {
    pub table_id: char,
    pub code: char,
}

impl AprsSymbol {
    pub fn default_person() -> Self {
        Self { table_id: '/', code: '>' }
    }
    
    pub fn new(table_id: char, code: char) -> Self {
        Self { table_id, code }
    }
}

/// Build APRS position report string
/// 
/// Format: !DDMM.mmN/DDDMM.mmW[comment]
/// Uses '!' without timestamp (most common ISS format)
pub fn build_position_report(
    lat: f64,
    lon: f64,
    symbol: AprsSymbol,
    comment: &str,
) -> String {
    let lat = lat.clamp(-90.0, 90.0);
    let lon = lon.clamp(-180.0, 180.0);
    
    let (lat_deg, lat_min) = if lat >= 0.0 {
        (lat.floor() as u32, (lat - lat.floor()) * 60.0)
    } else {
        let abs_lat = -lat;
        (abs_lat.floor() as u32, (abs_lat - abs_lat.floor()) * 60.0)
    };
    
    let (lon_deg, lon_min) = if lon >= 0.0 {
        (lon.floor() as u32, (lon - lon.floor()) * 60.0)
    } else {
        let abs_lon = -lon;
        (abs_lon.floor() as u32, (abs_lon - abs_lon.floor()) * 60.0)
    };
    
    let ns = if lat >= 0.0 { 'N' } else { 'S' };
    let ew = if lon >= 0.0 { 'E' } else { 'W' };
    
    let report = format!(
        "!{:02}{:05.2}{}/{:03}{:05.2}{}{}",
        lat_deg, lat_min, ns,
        lon_deg, lon_min, ew,
        symbol.code
    );
    
    if comment.is_empty() {
        report
    } else {
        format!("{} {}", report, comment)
    }
}

/// Build the text payload for an APRS message
/// 
/// Format: :TO:text{id
/// 
/// The message ID is required for acknowledgment tracking.
/// 
/// Note: The actual AX.25 frame encoding is done in radio.rs using kiss::build_ax25_ui_frame
pub fn build_message_payload(
    to_callsign: &str,
    body: &str,
    msg_id: &str,
) -> String {
    // Pad to_callsign to 9 characters (APRS standard)
    let to_padded = format!("{:<9}", to_callsign);
    format!(":{}:{} {{{}", to_padded, body, msg_id)
}

/// Check if a received message is an ACK for a sent message
/// 
/// Format: :CALL:ackMSGID
/// Returns the message ID if this is an ACK for the given sent message.
pub fn parse_message_ack(body: &str) -> Option<String> {
    let body = body.trim();
    if body.starts_with("ack") || body.starts_with("rej") {
        // Extract message ID after "ack" or "rej"
        let id = &body[3..];
        let id = id.trim();
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    None
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a callsign into AX.25 7-byte format
    /// AX.25: each char shifted left by 1, SSID in bits 1-4 of byte 6, final bit in bit 0
    fn encode_call(call: &str, final_bit: bool) -> [u8; 7] {
        let mut result = [0x40u8; 7];  // Default to space (0x20 << 1 = 0x40)
        
        let (call_part, ssid) = if let Some(idx) = call.find('-') {
            (&call[..idx], call[idx + 1..].parse::<u8>().unwrap_or(0))
        } else {
            (call, 0)
        };
        
        // Encode each character: ASCII << 1
        for (i, c) in call_part.chars().take(6).enumerate() {
            result[i] = (c as u8).to_ascii_uppercase() << 1;
        }
        
        // Byte 6: SSID in bits 1-4 (shifted left by 1), final bit in bit 0
        // Default: spaces (0x40) with no final bit means 0x60
        let ssid_bits = ((ssid & 0x0F) << 1) & 0x1E;
        let final_flag = if final_bit { 0x01 } else { 0x00 };
        result[6] = 0x60 | ssid_bits | final_flag;  // 0x60 = spaces with no final
        
        // SSID in bits 1-4 of byte 6, final bit in bit 0
        result[6] = ((ssid & 0x0F) << 1) | (if final_bit { 0x01 } else { 0x60 });
        
        result
    }
    
    #[test]
    fn test_parse_callsign() {
        // Test parsing various callsigns using encode_call
        let (call, _) = parse_callsign(&encode_call("NOCALL", false)).unwrap();
        assert_eq!(call, "NOCALL");
        
        let (call, _) = parse_callsign(&encode_call("KV4P-11", false)).unwrap();
        assert_eq!(call, "KV4P-11");
        
        let (call, _) = parse_callsign(&encode_call("WIDE1-1", false)).unwrap();
        assert_eq!(call, "WIDE1-1");
    }
    
    #[test]
    fn test_parse_addresses() {
        // Simple frame with dest + src (src is last, so src has final bit)
        let mut frame = Vec::new();
        frame.extend_from_slice(&encode_call("APRS", false));  // dest, not final
        frame.extend_from_slice(&encode_call("KD4LCD-9", true));  // src, final
        frame.extend_from_slice(&[0x03, 0xF0]);
        frame.extend_from_slice(b"!4825.14N/00217.67W>");
        
        let parsed = parse_ax25_addresses(&frame).unwrap();
        assert_eq!(parsed.destination, "APRS");
        assert_eq!(parsed.source, "KD4LCD-9");
        assert!(parsed.digipeaters.is_empty());
    }
    
    #[test]
    fn test_parse_ax25_frame() {
        // Simple frame: APRS + KD4LCD-9 + !position
        let mut frame = Vec::new();
        frame.extend_from_slice(&encode_call("APRS", false));  // dest, not final
        frame.extend_from_slice(&encode_call("KD4LCD-9", true));  // src, final
        frame.extend_from_slice(&[0x03, 0xF0]);
        frame.extend_from_slice(b"!4825.14N/00217.67E>");
        
        let msg = parse_ax25_frame(&frame).unwrap();
        assert_eq!(msg.from_callsign, "KD4LCD-9");
        assert_eq!(msg.to_callsign, "APRS");
        assert!((msg.position_lat - 48.419).abs() < 0.01);
        assert!((msg.position_lon - 2.294).abs() < 0.01);
        assert_eq!(msg.symbol_table_id, Some('/'));
        assert_eq!(msg.symbol_code, Some('>'));
    }
    
    #[test]
    fn test_parse_aprs_timestamp() {
        // Just test that it parses HHMMSSz format
        let data = b"123456z";
        if let Some((secs, _, consumed)) = parse_aprs_timestamp(data) {
            assert_eq!(secs, 12 * 3600 + 34 * 60 + 56);
            assert_eq!(consumed, 7);
        }
        // Test /DDHHMMh format
        let data = b"/21123456h";
        if let Some((secs, day, consumed)) = parse_aprs_timestamp(data) {
            assert_eq!(secs, 12 * 3600 + 34 * 60 + 56);
            assert_eq!(day, Some(21));
            assert_eq!(consumed, 7);
        }
    }
    
    #[test]
    fn test_digipeater_entry() {
        let entry = DigipeaterEntry::new("qAS".to_string());
        assert!(entry.is_q_construct());
        
        let entry = DigipeaterEntry::new("WIDE1-1".to_string());
        assert!(!entry.is_q_construct());
    }
    
    #[test]
    fn test_build_position_report() {
        let report = build_position_report(
            48.8584,
            2.2945,
            AprsSymbol::default_person(),
            "ISS",
        );
        
        assert!(report.starts_with("!"));
        assert!(report.contains("N/"));
        assert!(report.ends_with("> ISS"));
    }
    
    #[test]
    fn test_parse_gprmc() {
        let sentence = "$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A";
        let (lat, lon, speed, course) = parse_gprmc(sentence).unwrap();
        
        assert!((lat - 48.1173).abs() < 0.001);
        assert!((lon - 11.5167).abs() < 0.001);
        assert_eq!(speed, 22);
        assert_eq!(course, 84);
    }
    
    #[test]
    fn test_extract_course_speed() {
        let mut msg = APRSMessage::new();
        
        // Test PHG format
        extract_course_speed("PHG7249/394/", &mut msg);
        assert_eq!(msg.course, Some(394));
        
        // Test s/c format
        extract_course_speed("s123c045", &mut msg);
        assert_eq!(msg.speed, Some(123));
        assert_eq!(msg.course, Some(45));
        
        // Test altitude
        extract_altitude("/A=001500", &mut msg);
        assert_eq!(msg.altitude, Some(1500));
    }
}