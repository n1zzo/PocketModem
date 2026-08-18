//! APRS Parser - Decodes AX.25 frames into APRS messages
//!
//! Based on the AVRS APRS parser and KV4P HT Android implementation.

use std::time::{SystemTime, UNIX_EPOCH};

/// APRS message types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum APRSType {
    Unspecified,
    Position,
    Weather,
    Message,
    Object,
    Status,
    Unknown,
}

impl Default for APRSType {
    fn default() -> Self { Self::Unknown }
}

/// APRS message with decoded data
#[derive(Debug, Clone, Default)]
pub struct APRSMessage {
    pub msg_type: APRSType,
    pub from_callsign: String,
    pub to_callsign: String,
    pub timestamp: u64,
    pub position_lat: f64,
    pub position_lon: f64,
    pub comment: String,
    pub relay_callsign: String,
    
    // APRS Symbol (symbol table ID and symbol code from position report)
    // Symbol table ID: '/' (primary) or '\\' (alternate)
    // Symbol code: character from APRS symbol table
    pub symbol_table_id: Option<char>,
    pub symbol_code: Option<char>,
    
    // Message-specific
    pub msg_body: Option<String>,
    pub to_callsign_msg: Option<String>,
    
    // Object-specific
    pub obj_name: Option<String>,
    
    // Weather-specific
    pub temperature: Option<f64>,
    pub humidity: Option<f64>,
    pub pressure: Option<f64>,
    pub wind_force: Option<i32>,
    pub wind_dir: Option<String>,
}

impl APRSMessage {
    pub fn new() -> Self {
        Self {
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            ..Default::default()
        }
    }
}

/// Parse an AX.25 UI frame into an APRS message
pub fn parse_ax25_frame(frame: &[u8]) -> Option<APRSMessage> {
    if frame.len() < 16 { return None; }
    
    // Find ctrl (0x03) and pid (0xF0) by scanning backwards
    // They should be the last two bytes before the payload
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
    
    // Parse destination (first 7 bytes)
    let (dst_call, _) = parse_callsign(&frame[0..7])?;
    
    // Source is typically at offset 7 (right after dest)
    let (src_call, _) = parse_callsign(&frame[7..14])?;
    
    // Payload starts after PID
    let payload_start = pid_idx + 1;
    let payload = &frame[payload_start..];
    
    // Create base message
    let mut msg = APRSMessage::new();
    msg.from_callsign = src_call;
    msg.to_callsign = dst_call;
    
    // Decode APRS payload based on DTI
    if payload.is_empty() { return None; }
    
    let dti = payload[0] as char;
    match dti {
        '=' | '!' | '}' => decode_position(&mut msg, payload),    // Position (various formats)
        '/' | '@' => decode_position(&mut msg, payload),          // Position with timestamp
        ')' => decode_object(&mut msg, payload),                  // Object
        ':' => decode_message(&mut msg, payload),                 // Message
        '$' => decode_nmea(&mut msg, payload),                    // NMEA
        '>' => decode_status(&mut msg, payload),                  // Status
        '#' | '*' | '_' => decode_weather(&mut msg, payload),     // Weather
        _ => {
            msg.msg_type = APRSType::Unknown;
            msg.comment = String::from_utf8_lossy(payload).trim_end().to_string();
        }
    }
    
    Some(msg)
}

/// Parse a 7-byte AX.25 callsign field
fn parse_callsign(data: &[u8]) -> Option<(String, usize)> {
    if data.len() < 7 { return None; }
    
    // AX.25 callsigns are 6 characters stored in 7 bytes
    // Each character occupies bits 1-7 (shifted right by 1)
    // Byte 7 contains SSID in lower 4 bits and final-bit flag in bit 0
    let mut chars = Vec::new();
    for i in 0..6 {
        // Get character by shifting right 1 (bit 0 is control/flag)
        let c = (data[i] >> 1) as u8;
        if c == 0x20 || c == 0x00 {
            break;  // Padding
        }
        if c >= 0x20 && c < 0x7F {
            chars.push(c as char);
        } else {
            chars.push('?');  // Invalid character
        }
    }
    
    if chars.is_empty() {
        return None;
    }
    
    let call: String = chars.into_iter().collect();
    let ssid = (data[6] & 0x0F) as i32;
    
    let result = if ssid > 0 {
        format!("{}-{}", call, ssid)
    } else {
        call
    };
    
    Some((result, 7))
}

/// Parse callsign with digipeater flags (used/has-been-used)
fn parse_callsign_with_flags(data: &[u8]) -> Option<(String, usize)> {
    if let Some((mut call, offset)) = parse_callsign(data) {
        // Check if this digipeater was used (has * after it in original)
        // The final bit in byte 6 indicates "last address"
        let is_last = (data[6] & 0x01) != 0;
        
        // Check if this is a WIDEn-N path
        if call.contains("WIDE") || call.contains("RELAY") || call.contains("TRACE") {
            // This is a path alias
        }
        
        Some((call, offset))
    } else {
        None
    }
}

/// Find the payload start in an AX.25 frame
fn find_payload_start(frame: &[u8]) -> Option<usize> {
    let mut offset = 0;  // Start at beginning
    
    // Count addresses until we find the last one (final bit set in byte 6)
    // Check BEFORE incrementing to catch the final address
    loop {
        if offset + 7 > frame.len() {
            return None;
        }
        
        let is_final = (frame[offset + 6] & 0x01) != 0;
        offset += 7;
        
        if is_final {
            break;
        }
    }
    
    // Now offset points to byte AFTER the last address
    // That's the ctrl byte. Skip ctrl(1) + pid(1) to get payload
    if offset + 2 > frame.len() {
        return None;
    }
    
    Some(offset + 2)  // Skip ctrl and pid
}

/// Find i-gate callsign from Q-constructs in digipeater path
fn find_igate(digipeaters: &[String]) -> String {
    let q_constructs = ["qar", "qas", "qac", "qao", "qai"];
    
    for (i, digi) in digipeaters.iter().enumerate() {
        let lower = digi.to_lowercase();
        if q_constructs.contains(&lower.as_str()) {
            // Next digipeater after Q-construct is the i-gate
            if i + 1 < digipeaters.len() {
                return digipeaters[i + 1].clone();
            }
        }
    }
    
    String::new()
}

/// Decode APRS position report
fn decode_position(msg: &mut APRSMessage, payload: &[u8]) {
    msg.msg_type = APRSType::Position;
    
    // Position format: T/DDDmm.mmN/DDDmm.mmWHHMMSShDD.D°c...
    // or: T/DDDmm.mmN/DDDmm.mmW/...
    // or: !DDMM.mmN/DDDMM.mmW...
    
    let data = &payload[1..];  // Skip DTI
    
    // Try to find latitude and longitude
    if let Some((lat, lon, consumed)) = parse_aprs_position(data) {
        msg.position_lat = lat;
        msg.position_lon = lon;
        
        // Rest is comment/symbol
        if consumed < data.len() {
            let remainder = &data[consumed..];
            // Look for symbol table ID (first char after position)
            // and symbol code (second char)
            if remainder.len() >= 2 {
                let sym1 = remainder[0] as char;
                let sym2 = remainder[1] as char;
                
                // Valid APRS symbol: symbol table ID is / or \, symbol code is graphic
                if (sym1 == '/' || sym1 == '\\') && sym2.is_ascii_graphic() {
                    msg.symbol_table_id = Some(sym1);
                    msg.symbol_code = Some(sym2);
                    msg.comment = String::from_utf8_lossy(&remainder[2..]).trim_end().to_string();
                } else {
                    msg.comment = String::from_utf8_lossy(remainder).trim_end().to_string();
                }
            } else if !remainder.is_empty() {
                msg.comment = String::from_utf8_lossy(remainder).trim_end().to_string();
            }
        }
    } else {
        // Couldn't parse position, treat rest as comment
        msg.comment = String::from_utf8_lossy(data).trim_end().to_string();
    }
}

/// Parse APRS position format: DDMM.MM N/S DDDMM.MM E/W or DDMM.MM N/S / DDDMM.MM E/W
fn parse_aprs_position(data: &[u8]) -> Option<(f64, f64, usize)> {
    // APRS position can start with timestamp or directly with position
    let mut offset = 0;
    
    // Check for timestamp prefix
    // Formats: @HHMMSSz (8), /HHMMSS (7), HHMMSSz (7), HHMMSS/ (7)
    if data.len() >= 7 {
        let first = data[0] as char;
        let sixth = data[5] as char;
        let seventh = data[6] as char;
        
        if first == '@' && seventh == 'z' {
            offset = 8;  // @HHMMSSz
        } else if first == '@' || first == '/' {
            offset = 7;  // @HHMMSS/ or /HHMMSS
        } else if sixth.is_ascii_digit() && (seventh == 'z' || seventh == '/') {
            offset = 7;  // HHMMSSz or HHMMSS/ (timestamp without leading @ or /)
        }
    }
    
    // Format A: DDMM.MM + N/S + / + DDDMM.MM + E/W (18 chars with / separator)
    // Example: 4534.19N/00927.35E
    if offset + 18 <= data.len() {
        let lat_str = String::from_utf8_lossy(&data[offset..offset + 7]);
        let ns = data[offset + 7] as char;
        let sep = data[offset + 8] as char;
        if (ns == 'N' || ns == 'S' || ns == 'n' || ns == 's') && sep == '/' {
            let lon_str = String::from_utf8_lossy(&data[offset + 9..offset + 17]);
            let ew = data[offset + 17] as char;
            if let Some((lat_deg, lat_min)) = parse_deg_min(&lat_str) {
                if let Some((lon_deg, lon_min)) = parse_deg_min(&lon_str) {
                    if ew == 'E' || ew == 'W' || ew == 'e' || ew == 'w' {
                        let mut lat = lat_deg + lat_min / 60.0;
                        let mut lon = lon_deg + lon_min / 60.0;
                        if ns == 'S' || ns == 's' { lat = -lat; }
                        if ew == 'W' || ew == 'w' { lon = -lon; }
                        return Some((lat, lon, offset + 18));
                    }
                }
            }
        }
    }
    
    // Format B: DDMM.MM + N/S + DDDMM.MM + E/W (17 chars, no / separator)
    // Example: 4534.19N00927.35E
    if offset + 17 <= data.len() {
        let lat_str = String::from_utf8_lossy(&data[offset..offset + 7]);
        let ns = data[offset + 7] as char;
        if ns == 'N' || ns == 'S' || ns == 'n' || ns == 's' {
            let lon_str = String::from_utf8_lossy(&data[offset + 8..offset + 16]);
            let ew = data[offset + 16] as char;
            if let Some((lat_deg, lat_min)) = parse_deg_min(&lat_str) {
                if let Some((lon_deg, lon_min)) = parse_deg_min(&lon_str) {
                    if ew == 'E' || ew == 'W' || ew == 'e' || ew == 'w' {
                        let mut lat = lat_deg + lat_min / 60.0;
                        let mut lon = lon_deg + lon_min / 60.0;
                        if ns == 'S' || ns == 's' { lat = -lat; }
                        if ew == 'W' || ew == 'w' { lon = -lon; }
                        return Some((lat, lon, offset + 17));
                    }
                }
            }
        }
    }
    
    // Format C: DDMM.MM + N/S + / + DDDMM.MM + E/W (15 chars, no decimal on minutes)
    // Example: 4534.N/00927.E
    if offset + 15 <= data.len() {
        let lat_str = String::from_utf8_lossy(&data[offset..offset + 6]);
        if let Some((lat_deg, lat_min)) = parse_deg_min(&lat_str) {
            let ns = data[offset + 6] as char;
            if ns == 'N' || ns == 'S' || ns == 'n' || ns == 's' {
                let sep = data[offset + 7] as char;
                if sep == '/' {
                    let lon_start = offset + 8;
                    let lon_str = String::from_utf8_lossy(&data[lon_start..lon_start + 7]);
                    let ew = data[lon_start + 7] as char;
                    if let Some((lon_deg, lon_min)) = parse_deg_min(&lon_str) {
                        let mut lat = lat_deg + lat_min / 60.0;
                        let mut lon = lon_deg + lon_min / 60.0;
                        if ns == 'S' || ns == 's' { lat = -lat; }
                        if ew == 'W' || ew == 'w' { lon = -lon; }
                        return Some((lat, lon, lon_start + 8));
                    }
                }
            }
        }
    }
    
    None
}

/// Parse degrees/minutes format: DDMM.MM or DDDMM.MM
fn parse_deg_min(s: &str) -> Option<(f64, f64)> {
    let s = s.trim();
    if s.len() < 4 { return None; }
    
    // Find the decimal point
    if let Some(dot_idx) = s.find('.') {
        let deg_part = &s[..dot_idx];
        let min_part = &s[dot_idx + 1..];
        
        // Check for DDDMM.MM (5 chars before decimal, e.g., "00941.09")
        if deg_part.len() == 5 {
            // First 3 chars = degrees, last 2 chars = whole minutes
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

/// Decode object report
fn decode_object(msg: &mut APRSMessage, payload: &[u8]) {
    msg.msg_type = APRSType::Object;
    
    let data = &payload[1..];  // Skip ')'
    
    // Object name is first 9 characters
    if data.len() >= 9 {
        let name_bytes: Vec<u8> = data[..9].iter().map(|&b| b & 0x7F).collect();
        msg.obj_name = Some(String::from_utf8_lossy(&name_bytes).trim().to_string());
        
        // Rest should be position data
        decode_position(msg, &data[9..]);
    }
    
    if msg.comment.is_empty() && data.len() > 9 {
        msg.comment = String::from_utf8_lossy(&data[9..]).trim_end().to_string();
    }
}

/// Decode APRS message (messaging)
fn decode_message(msg: &mut APRSMessage, payload: &[u8]) {
    msg.msg_type = APRSType::Message;
    
    let data = &payload[1..];  // Skip ':'
    
    // Format: ADDRESSEE:text
    if let Some(colon_idx) = data.iter().position(|&b| b == b':') {
        let addr_bytes: Vec<u8> = data[..colon_idx].iter().map(|&b| b & 0x7F).collect();
        let addr = String::from_utf8_lossy(&addr_bytes).trim().to_string();
        
        msg.to_callsign_msg = Some(addr.clone());
        msg.to_callsign = addr;  // Also update main to_callsign
        
        if colon_idx + 1 < data.len() {
            let body = &data[colon_idx + 1..];
            msg.msg_body = Some(String::from_utf8_lossy(body).trim_end().to_string());
        }
    }
}

/// Decode NMEA sentence
fn decode_nmea(msg: &mut APRSMessage, payload: &[u8]) {
    // $GPGGA or similar
    let data = &payload[1..];  // Skip '$'
    msg.comment = String::from_utf8_lossy(data).trim_end().to_string();
}

/// Decode status report
fn decode_status(msg: &mut APRSMessage, payload: &[u8]) {
    msg.msg_type = APRSType::Status;
    let data = &payload[1..];  // Skip '>'
    msg.comment = String::from_utf8_lossy(data).trim_end().to_string();
}

/// Decode weather report
fn decode_weather(msg: &mut APRSMessage, payload: &[u8]) {
    msg.msg_type = APRSType::Weather;
    let data = &payload[1..];
    msg.comment = String::from_utf8_lossy(data).trim_end().to_string();
    
    // Try to parse weather data from comment
    // Common format: c...s...g...t...r...p...P...h..b......
    parse_weather_data(msg, data);
}

/// Parse weather-specific fields from comment
fn parse_weather_data(msg: &mut APRSMessage, data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    
    // Temperature: t123 (in °F, tenths)
    if let Some(start) = text.find("t") {
        if start + 4 <= text.len() {
            let temp_str = &text[start + 1..start + 4];
            if let Ok(temp_f) = temp_str.parse::<f64>() {
                let temp_c = (temp_f - 32.0) * 5.0 / 9.0;
                msg.temperature = Some(temp_c);
            }
        }
    }
    
    // Wind speed: s123
    if let Some(start) = text.find("s") {
        if start + 4 <= text.len() {
            let wind_str = &text[start + 1..start + 4];
            if let Ok(wind) = wind_str.parse::<i32>() {
                msg.wind_force = Some(wind);
            }
        }
    }
    
    // Wind direction: d123
    if let Some(start) = text.find("d") {
        if start + 4 <= text.len() {
            let dir_str = &text[start + 1..start + 4];
            if let Ok(dir) = dir_str.parse::<i32>() {
                msg.wind_dir = Some(format!("{}°", dir));
            }
        }
    }
    
    // Pressure: b12345 (hPa * 10)
    if let Some(start) = text.find("b") {
        if start + 6 <= text.len() {
            let pres_str = &text[start + 1..start + 6];
            if let Ok(pres_hpa) = pres_str.parse::<f64>() {
                msg.pressure = Some(pres_hpa * 10.0);  // Convert to Pa
            }
        }
    }
    
    // Humidity: h00-99 (percentage)
    if let Some(start) = text.find("h") {
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

/// Get the base callsign without SSID
pub fn base_callsign(callsign: &str) -> String {
    if let Some(idx) = callsign.find('-') {
        callsign[..idx].to_string()
    } else {
        callsign.to_string()
    }
}

/// Get SSID from callsign
pub fn ssid(callsign: &str) -> i32 {
    if let Some(idx) = callsign.find('-') {
        callsign[idx + 1..].parse().unwrap_or(0)
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_callsign() {
        // "NOCALL " padded to 6 chars + SSID
        let data = [0x4E, 0x4F, 0x43, 0x43, 0x41, 0x4C, 0x20];
        let (call, _) = parse_callsign(&data).unwrap();
        assert_eq!(call, "NOCALL");
        
        // With SSID
        let data = [0x4B, 0x56, 0x34, 0x50, 0x2D, 0x31, 0x31];  // KV4P-11
        let (call, _) = parse_callsign(&data).unwrap();
        assert_eq!(call, "KV4P-11");
    }
    
    #[test]
    fn test_parse_aprs_position() {
        // 4917.20N/12306.89W
        let data = b"4917.20N/12306.89W".to_vec();
        let (lat, lon, _) = parse_aprs_position(&data).unwrap();
        
        assert!((lat - 49.2867).abs() < 0.001);
        assert!((lon - (-123.1148)).abs() < 0.001);
    }
}