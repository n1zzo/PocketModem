//! APRS Parser Tests
//!
//! Test vectors adapted from libaprs (https://github.com/iontodirel/libaprs)
//! Test data is in the public domain, obtained from APRS-IS.

use pocket_modem_aprs::*;

/// Convert ASCII callsign to AX.25 encoded format (7 bytes per address)
fn encode_callsign(call: &str, is_last: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(7);
    let call_upper = call.to_uppercase();
    
    // Parse base call and SSID
    let (base, ssid) = if let Some(dash) = call_upper.find('-') {
        (&call_upper[..dash], call_upper[dash + 1..].parse::<u8>().unwrap_or(0))
    } else {
        (call_upper.as_str(), 0u8)
    };
    
    // Encode 6 characters (shift left by 1, bit 0 is control bit)
    for c in base.chars().take(6) {
        bytes.push((c as u8) << 1);
    }
    // Pad with spaces if needed
    while bytes.len() < 6 {
        bytes.push(b' ' << 1);
    }
    
    // SSID byte: SSID in bits 1-4, last-address flag in bit 0
    let ssid_byte = (ssid & 0x0F) << 1 | if is_last { 0x01 } else { 0x00 };
    bytes.push(ssid_byte);
    
    bytes
}

/// Create a raw AX.25 UI frame from components
fn make_ax25_frame(from: &str, to: &str, via: &[&str], info: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    
    // Destination address (first address, bit 0 set = last address)
    frame.extend_from_slice(&encode_callsign(to, via.is_empty()));
    
    // Source address
    frame.extend_from_slice(&encode_callsign(from, via.is_empty()));
    
    // Digipeater addresses (bit 0 cleared for intermediate, set for last)
    for (i, digi) in via.iter().enumerate() {
        let is_last = i == via.len() - 1;
        frame.extend_from_slice(&encode_callsign(digi, is_last));
    }
    
    // Control byte (UI frame = 0x03) and PID byte (No Layer 3 = 0xF0)
    frame.push(0x03);  // UI frame
    frame.push(0xF0);  // PID: no layer 3 protocol
    
    // Information field
    frame.extend_from_slice(info);
    
    frame
}

// =============================================================================
// AX.25 Frame Parsing Tests
// =============================================================================

#[test]
fn test_parse_ax25_frame_basic() {
    // Test decoding a raw AX.25 frame
    // Frame: JUPITR > APN382, WIDE3-3 with position report
    let frame = make_ax25_frame(
        "JUPITR",
        "APN382",
        &["WIDE3-3"],
        b"!4741.70NB12258.05W# MT. JUPITER   K7IDX"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse frame");
    
    assert_eq!(msg.from_callsign, "JUPITR");
    assert_eq!(msg.to_callsign, "APN382");
    assert_eq!(msg.msg_type, APRSType::Position);
    assert!((msg.position_lat - 47.695).abs() < 0.001, "lat = {}", msg.position_lat);
    assert!((msg.position_lon - (-122.9675)).abs() < 0.001, "lon = {}", msg.position_lon);
}

#[test]
fn test_parse_ax25_frame_no_via() {
    // Frame with no digipeater path
    let frame = make_ax25_frame(
        "NOCALL",
        "APRS",
        &[],
        b"Hello world"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse frame");
    
    assert_eq!(msg.from_callsign, "NOCALL");
    assert_eq!(msg.to_callsign, "APRS");
}

#[test]
fn test_parse_ax25_frame_with_ssid() {
    // Test with SSID in callsigns
    let frame = make_ax25_frame(
        "KV4P-11",
        "APN382",
        &["WIDE1-1", "WIDE2-2"],
        b"!4548.33N\\00952.27E>Test"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse frame");
    
    assert_eq!(msg.from_callsign, "KV4P-11");
    assert_eq!(msg.to_callsign, "APN382");
}

// =============================================================================
// Position Report Parsing Tests (from libaprs)
// =============================================================================

#[test]
fn test_position_without_timestamp() {
    // From libaprs: !4903.50N/07201.75W-Test 001234
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"!4903.50N/07201.75W-Test 001234"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Position);
    // 49° 03.50' N = 49 + 3.5/60 = 49.0583
    assert!((msg.position_lat - 49.0583).abs() < 0.001, "lat = {}", msg.position_lat);
    // 072° 01.75' W = -(72 + 1.75/60) = -72.0292
    assert!((msg.position_lon - (-72.0292)).abs() < 0.001, "lon = {}", msg.position_lon);
    assert_eq!(msg.symbol_table_id, Some('/'));
    assert_eq!(msg.symbol_code, Some('-'));
    assert!(msg.comment.contains("Test 001234"), "comment = {}", msg.comment);
}

#[test]
fn test_position_with_timestamp_zulu() {
    // From libaprs: /092345z4903.50N/07201.75W>Test1234
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"/092345z4903.50N/07201.75W>Test1234"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Position);
    assert!((msg.position_lat - 49.0583).abs() < 0.001, "lat = {}", msg.position_lat);
    assert_eq!(msg.symbol_code, Some('>'));
}

#[test]
fn test_position_with_timestamp_at() {
    // From libaprs: @092345/4903.50N/07201.75W>Test1234
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"@092345/4903.50N/07201.75W>Test1234"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Position);
    assert!((msg.position_lat - 49.0583).abs() < 0.001, "lat = {}", msg.position_lat);
}

#[test]
fn test_position_with_hash_symbol() {
    // From libaprs: =4903.50N/07201.75W#PHG5132
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"=4903.50N/07201.75W#PHG5132"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Position);
    assert_eq!(msg.symbol_code, Some('#'));
}

#[test]
fn test_position_compressed_without_timestamp() {
    // From libaprs: =/5L!!<*e7> sTComment
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"=/5L!!<*e7> sTComment"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Position);
}

#[test]
fn test_position_with_spaces() {
    // From libaprs: !49  .  N/072  .  W-
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"!49  .  N/072  .  W-"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Position);
    assert!((msg.position_lat - 49.0).abs() < 0.001, "lat = {}", msg.position_lat);
    assert!((msg.position_lon - (-72.0)).abs() < 0.001, "lon = {}", msg.position_lon);
}

#[test]
fn test_position_alternate_table() {
    // Position with alternate symbol table (\)
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"!4548.33N\\00952.27E>Test"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Position);
    assert_eq!(msg.symbol_table_id, Some('\\'));
    assert_eq!(msg.symbol_code, Some('>'));
}

// =============================================================================
// Object Report Parsing Tests
// =============================================================================

#[test]
fn test_object_basic() {
    // From libaprs: ;LEADER   *092345z4903.50N/07201.75W>088/036
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b";LEADER   *092345z4903.50N/07201.75W>088/036"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Object);
    assert_eq!(msg.obj_name.as_deref(), Some("LEADER"));
}

#[test]
fn test_object_compressed() {
    // From libaprs: ;LEADER   *092345z/5L!!<*e7>7P[
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b";LEADER   *092345z/5L!!<*e7>7P["
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Object);
}

// =============================================================================
// Message Parsing Tests
// =============================================================================

#[test]
fn test_message_basic() {
    // Test basic APRS message
    let frame = make_ax25_frame(
        "FROM",
        "TO",
        &[],
        b":TO       :Hello world"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Message);
    assert_eq!(msg.to_callsign_msg.as_deref(), Some("TO"));
    assert_eq!(msg.msg_body.as_deref(), Some("Hello world"));
}

#[test]
fn test_message_bulletin() {
    // From libaprs: :BLN3     :Snow expected in Tampa RSN
    let frame = make_ax25_frame(
        "APRS",
        "BLN3",
        &[],
        b":BLN3     :Snow expected in Tampa RSN"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Message);
    assert_eq!(msg.to_callsign_msg.as_deref(), Some("BLN3"));
    assert!(msg.msg_body.as_deref().unwrap().contains("Snow expected"));
}

#[test]
fn test_message_bulletin_named() {
    // From libaprs: :BLN0WXA  :FROST ADVISORY IN EFFECT FROM 2AM TO 8AM WED
    let frame = make_ax25_frame(
        "APRS",
        "BLN0WXA",
        &[],
        b":BLN0WXA  :FROST ADVISORY IN EFFECT FROM 2AM TO 8AM WED"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Message);
    assert_eq!(msg.to_callsign_msg.as_deref(), Some("BLN0WXA"));
}

// =============================================================================
// Status Report Parsing Tests
// =============================================================================

#[test]
fn test_status_basic() {
    // Test basic status report
    let frame = make_ax25_frame(
        "STATUS",
        "APRS",
        &[],
        b">Testing APRS parser"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Status);
    assert!(msg.comment.contains("Testing APRS parser"));
}

// =============================================================================
// Weather Report Parsing Tests
// =============================================================================

#[test]
fn test_weather_basic() {
    // Test weather report (prefix with #, *, or _)
    let frame = make_ax25_frame(
        "WXR1",
        "APRS",
        &[],
        b"_c/100s10t70r50p005h55b10123"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Weather);
}

// =============================================================================
// NMEA Parsing Tests
// =============================================================================

#[test]
fn test_nmea_basic() {
    // Test NMEA sentence
    let frame = make_ax25_frame(
        "GPS",
        "APRS",
        &[],
        b"$GPGGA,123456,4917.20,N,12306.89,W,1,08,0.9,545.4,M,46.9,M,,*47"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert!(msg.msg_type == APRSType::Unknown || msg.msg_type == APRSType::Position);
}

// =============================================================================
// Timestamp Parsing Tests (from libaprs)
// =============================================================================

#[test]
fn test_timestamp_various_formats() {
    // Test different timestamp formats
    // @HHMMSSz format
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"@210358z4903.50N/07201.75W>"
    );
    let msg = parse_ax25_frame(&frame).expect("Should parse @HHMMSSz");
    assert_eq!(msg.msg_type, APRSType::Position);
    
    // /HHMMSS/ format  
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"/170100/4903.50N/07201.75W>"
    );
    let msg = parse_ax25_frame(&frame).expect("Should parse /HHMMSS/");
    assert_eq!(msg.msg_type, APRSType::Position);
    
    // HHMMSSz format (no @)
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"234517h4903.50N/07201.75W>"
    );
    let msg = parse_ax25_frame(&frame).expect("Should parse HHMMSSz");
    assert_eq!(msg.msg_type, APRSType::Position);
}

// =============================================================================
// Invalid Frame Tests
// =============================================================================

#[test]
fn test_invalid_frame_too_short() {
    // Frame too short to parse
    let frame = b"\x00\x00\x00";
    let msg = parse_ax25_frame(frame);
    assert!(msg.is_none());
}

#[test]
fn test_invalid_frame_wrong_pid() {
    // Frame with wrong PID (not 0xF0 for APRS)
    let mut frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"Test"
    );
    // Replace PID byte
    frame[frame.len() - 5] = 0x01;  // Not 0xF0
    
    let msg = parse_ax25_frame(&frame);
    assert!(msg.is_none());
}

#[test]
fn test_invalid_frame_wrong_ctrl() {
    // Frame with wrong control byte (not 0x03 for UI)
    let mut frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"Test"
    );
    // Replace control byte  
    frame[frame.len() - 6] = 0x01;  // Not 0x03
    
    let msg = parse_ax25_frame(&frame);
    assert!(msg.is_none());
}

// =============================================================================
// Utility Function Tests
// =============================================================================

#[test]
fn test_callsign_parsing() {
    // Test the parse_callsign function directly
    use pocket_modem_aprs::parse_callsign;
    
    // Basic callsign
    let data = [0x4E, 0x4F, 0x43, 0x43, 0x41, 0x4C, 0x20]; // "NOCALL "
    let (call, _) = parse_callsign(&data).expect("Should parse");
    assert_eq!(call, "NOCALL");
    
    // With SSID
    let data = [0x4B, 0x56, 0x34, 0x50, 0x2D, 0x31, 0x31]; // "KV4P-11"
    let (call, _) = parse_callsign(&data).expect("Should parse");
    assert_eq!(call, "KV4P-11");
    
    // W7ION-10* (with mark)
    let data = [0x57, 0x37, 0x49, 0x4F, 0x4E, 0x2D, 0x15]; // W7ION-10 with mark
    let (call, _) = parse_callsign(&data).expect("Should parse");
    assert_eq!(call, "W7ION-10");
}

#[test]
fn test_base_callsign() {
    use pocket_modem_aprs::base_callsign;
    
    assert_eq!(base_callsign("KV4P-11"), "KV4P");
    assert_eq!(base_callsign("NOCALL"), "NOCALL");
    assert_eq!(base_callsign("WIDE1-1*"), "WIDE1");
}

#[test]
fn test_ssid() {
    use pocket_modem_aprs::ssid;
    
    assert_eq!(ssid("KV4P-11"), 11);
    assert_eq!(ssid("NOCALL"), 0);
    assert_eq!(ssid("WIDE1-1*"), 1);
}

// =============================================================================
// Position Parsing Tests
// =============================================================================

#[test]
fn test_parse_aprs_position_various() {
    use pocket_modem_aprs::parse_aprs_position;
    
    // Format A: DDMM.MMN/DDDMM.MSEW (19 chars with / separator)
    let data = b"4917.20N/12306.89W".to_vec();
    let (lat, lon, sym_table, sym_code, _) = parse_aprs_position(&data).expect("Should parse");
    assert!((lat - 49.2867).abs() < 0.001, "lat = {}", lat);
    assert!((lon - (-123.1148)).abs() < 0.001, "lon = {}", lon);
    assert_eq!(sym_table, '/');
    assert_eq!(sym_code, 'W');
    
    // Format B: DDMM.MMN DDDMM.MSEW (17 chars no separator)
    let data = b"4534.19N00927.35W".to_vec();
    let (lat, lon, _, _, _) = parse_aprs_position(&data).expect("Should parse");
    assert!((lat - 45.5698).abs() < 0.01, "lat = {}", lat);
    assert!((lon - (-9.4558)).abs() < 0.01, "lon = {}", lon);
    
    // With South/West (negative coordinates)
    let data = b"4534.19S00927.35W".to_vec();
    let (lat, lon, _, _, _) = parse_aprs_position(&data).expect("Should parse");
    assert!(lat < 0.0, "lat should be negative for S");
    assert!(lon < 0.0, "lon should be negative for W");
}

// =============================================================================
// Integration Tests with Real-World Data
// =============================================================================

#[test]
fn test_real_world_position_mt_jupiter() {
    // From libaprs test: !4741.70NB12258.05W# MT. JUPITER   K7IDX
    let frame = make_ax25_frame(
        "K7IDX",
        "APN382",
        &["WIDE3-3"],
        b"!4741.70NB12258.05W# MT. JUPITER   K7IDX"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Position);
    assert_eq!(msg.from_callsign, "K7IDX");
    // 47° 41.70' N, 122° 58.05' W
    assert!(msg.position_lat > 47.0 && msg.position_lat < 48.0, "lat = {}", msg.position_lat);
    assert!(msg.position_lon < -122.0 && msg.position_lon > -123.0, "lon = {}", msg.position_lon);
    assert_eq!(msg.symbol_code, Some('#'));
    assert!(msg.comment.contains("MT. JUPITER"));
}

// =============================================================================
// Edge Cases
// =============================================================================

#[test]
fn test_empty_payload() {
    // UI frame with no info field - this shouldn't happen in practice
    // but we should handle it gracefully
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b""
    );
    let msg = parse_ax25_frame(&frame);
    assert!(msg.is_none());
}

#[test]
fn test_unknown_dti() {
    // Unknown DTI character
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"XSome unknown data"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse unknown DTI");
    assert_eq!(msg.msg_type, APRSType::Unknown);
    assert!(msg.comment.contains("Some unknown data"));
}

#[test]
fn test_mixed_case_callsigns() {
    // Test that callsigns are handled case-insensitively
    let frame = make_ax25_frame(
        "Test-1",
        "ApRs",
        &["Wide1-1"],
        b"!4903.50N/07201.75W>"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    // Callsigns should be uppercased
    assert_eq!(msg.from_callsign.to_uppercase(), msg.from_callsign);
    assert_eq!(msg.to_callsign.to_uppercase(), msg.to_callsign);
}