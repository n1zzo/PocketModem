//! APRS Parser Tests
//!
//! Test vectors adapted from libaprs (https://github.com/iontodirel/libaprs)
//! Test data is in the public domain, obtained from APRS-IS.

use pocket_modem::*;

/// Convert ASCII callsign to AX.25 encoded format (7 bytes per address)
fn encode_callsign(call: &str, is_last: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(7);
    let call_upper = call.to_uppercase();
    
    let (base, ssid) = if let Some(dash) = call_upper.find('-') {
        (&call_upper[..dash], call_upper[dash + 1..].parse::<u8>().unwrap_or(0))
    } else {
        (call_upper.as_str(), 0u8)
    };
    
    for c in base.chars().take(6) {
        bytes.push((c as u8) << 1);
    }
    while bytes.len() < 6 {
        bytes.push(b' ' << 1);
    }
    
    let ssid_byte = (ssid & 0x0F) << 1 | if is_last { 0x01 } else { 0x00 };
    bytes.push(ssid_byte);
    
    bytes
}

/// Create a raw AX.25 UI frame
fn make_ax25_frame(from: &str, to: &str, via: &[&str], info: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    
    // In AX.25: Destination (final-bit set), Source (final-bit clear), Digipeaters
    frame.extend_from_slice(&encode_callsign(to, true));
    frame.extend_from_slice(&encode_callsign(from, false));  // Source NEVER has final-bit
    
    for (i, digi) in via.iter().enumerate() {
        let is_last = i == via.len() - 1;
        frame.extend_from_slice(&encode_callsign(digi, is_last));
    }
    
    frame.push(0x03);  // UI frame
    frame.push(0xF0);  // PID: no layer 3
    frame.extend_from_slice(info);
    
    frame
}

// =============================================================================
// AX.25 Frame Parsing Tests
// =============================================================================

#[test]
fn test_parse_ax25_frame_basic() {
    let frame = make_ax25_frame(
        "JUPITR",
        "APN382",
        &["WIDE3-3"],
        b"!4741.70N/12258.05W#"
    );
    
    // Debug output
    eprintln!("Frame: {:02X?}", frame);
    eprintln!("Frame len: {}", frame.len());
    
    // Test parse_addresses directly
    let addr_result = parse_addresses(&frame);
    eprintln!("parse_addresses: {:?}", addr_result);
    
    // Test parse_ax25_addresses
    let ax25_result = parse_ax25_addresses(&frame);
    match ax25_result {
        Some(parsed) => {
            eprintln!("parse_ax25_addresses: Some");
            eprintln!("  dst={}, src={}", parsed.destination, parsed.source);
            eprintln!("  payload len={}", parsed.payload.len());
            eprintln!("  payload: {:?}", String::from_utf8_lossy(&parsed.payload));
        },
        None => {
            eprintln!("parse_ax25_addresses: None!");
            // Debug parse_addresses with slice
            eprintln!("  Trying parse_addresses with &frame[..21]...");
            let slice_result = parse_addresses(&frame[..21]);
            eprintln!("  parse_addresses[..21]: {:?}", slice_result);
        }
    }
    
    let msg = parse_ax25_frame(&frame).expect("Should parse frame");
    
    assert_eq!(msg.from_callsign, "JUPITR");
    assert_eq!(msg.to_callsign, "APN382");
    assert_eq!(msg.msg_type, APRSType::Position);
}

#[test]
fn test_parse_ax25_frame_no_via() {
    let frame = make_ax25_frame(
        "NOCALL",
        "APRS",
        &[],
        b"!4500.00N/01200.00W>"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse frame");
    
    assert_eq!(msg.from_callsign, "NOCALL");
    assert_eq!(msg.to_callsign, "APRS");
    assert_eq!(msg.msg_type, APRSType::Position);
}

#[test]
fn test_parse_ax25_frame_with_ssid() {
    let frame = make_ax25_frame(
        "KV4P-11",
        "APRS",
        &["WIDE1-1"],
        b"!4500.00N/01200.00W-"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse frame");
    
    assert_eq!(msg.from_callsign, "KV4P-11");
    assert_eq!(msg.to_callsign, "APRS");
}

// =============================================================================
// Position Parsing Tests
// =============================================================================

#[test]
fn test_parse_aprs_position_various() {
    // Format A: DDMM.MMN/DDDMM.MMW (17 chars total: 7+1+8+1)
    // DDMM.MM = 7, N/S = 1, / = 1, DDDMM.MM = 8, E/W = 1
    // Note: E/W is NOT part of the 8-char longitude, it's separate
    let data = b"4917.20N/12306.89W".to_vec();
    eprintln!("Testing parse_aprs_position with: {:?} ({} bytes)", String::from_utf8_lossy(&data), data.len());
    
    // Check the actual format
    eprintln!("Bytes 0-8: {:?}", &data[0..9]);
    eprintln!("Bytes 9-17: {:?}", &data[9..17.min(data.len())]);
    
    let result = parse_aprs_position(&data);
    eprintln!("Result: {:?}", result.is_some());
    
    if let Some((lat, lon, sym_table, sym_code, remainder)) = result {
        eprintln!("Parsed: lat={}, lon={}, sym={:?}{:?}, rem={:?}", lat, lon, sym_table, sym_code, String::from_utf8_lossy(remainder));
        assert!((lat - 49.2867).abs() < 0.001, "lat = {}", lat);
        assert!((lon - (-123.1148)).abs() < 0.001, "lon = {}", lon);
    } else {
        panic!("Failed to parse position");
    }
}

#[test]
fn test_position_without_timestamp() {
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"!4903.50N/07201.75W-Test"
    );
    
    eprintln!("Frame: {:02X?}", frame);
    eprintln!("Payload: {:?}", String::from_utf8_lossy(&frame[16..]));
    
    // Debug parse_ax25_addresses
    match parse_ax25_addresses(&frame) {
        Some(parsed) => {
            eprintln!("parse_ax25_addresses succeeded");
            eprintln!("  dst={}, src={}", parsed.destination, parsed.source);
            eprintln!("  payload: {:?}", String::from_utf8_lossy(&parsed.payload));
        },
        None => {
            eprintln!("parse_ax25_addresses failed!");
        }
    }
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    eprintln!("msg_type: {:?}, lat: {}, lon: {}", msg.msg_type, msg.position_lat, msg.position_lon);
    
    assert_eq!(msg.msg_type, APRSType::Position);
    assert!((msg.position_lat - 49.0583).abs() < 0.001, "lat = {}", msg.position_lat);
}

#[test]
fn test_position_with_timestamp_zulu() {
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"/092345z4903.50N/07201.75W>Test"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::PositionWithTimestamp);
    assert!((msg.position_lat - 49.0583).abs() < 0.001, "lat = {}", msg.position_lat);
}

#[test]
fn test_position_with_timestamp_at() {
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"@092345z4903.50N/07201.75W>Test"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::PositionWithTimestamp);
}

#[test]
fn test_position_compressed_without_timestamp() {
    // This is actually a timestamp format test
    // /092345z = /DDHHMMh timestamp format
    let frame = make_ax25_frame("TEST", "APRS", &[], b"/092345z4500.00N/01200.00W-");
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::PositionWithTimestamp);
}

// =============================================================================
// Callsign Tests
// =============================================================================

#[test]
fn test_callsign_parsing() {
    use pocket_modem::parse_callsign;
    
    let data = [0x9C, 0x9E, 0x86, 0x82, 0x98, 0x98, 0x20];
    let (call, _) = parse_callsign(&data).expect("Should parse");
    assert_eq!(call, "NOCALL");
    
    let data = [0x96, 0xAC, 0x68, 0xA0, 0x40, 0x40, 0x17];
    let (call, _) = parse_callsign(&data).expect("Should parse");
    assert_eq!(call, "KV4P-11");
}

#[test]
fn test_base_callsign() {
    use pocket_modem::base_callsign;
    
    assert_eq!(base_callsign("KV4P-11"), "KV4P");
    assert_eq!(base_callsign("NOCALL"), "NOCALL");
    assert_eq!(base_callsign("WIDE1-1*"), "WIDE1");
}

#[test]
fn test_ssid() {
    use pocket_modem::ssid;
    
    assert_eq!(ssid("KV4P-11"), 11);
    assert_eq!(ssid("NOCALL"), 0);
    assert_eq!(ssid("WIDE1-1"), 1);
}

// =============================================================================
// Message Parsing Tests
// =============================================================================

#[test]
fn test_message_basic() {
    let frame = make_ax25_frame(
        "NOCALL",
        "APRS",
        &[],
        b":TEST     :Hello World"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Message);
    // Messages have body in msg_body, not comment
    assert!(msg.msg_body.as_ref().map_or(false, |b| b.contains("Hello World")),
            "msg_body should contain 'Hello World', got: {:?}", msg.msg_body);
}

// =============================================================================
// Status Tests
// =============================================================================

#[test]
fn teststatus_basic() {
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b">Status text here"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Status);
    assert!(msg.comment.contains("Status text"));
}

// =============================================================================
// Weather Tests
// =============================================================================

#[test]
fn test_weather_basic() {
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"_c123456s00767t077h55b09900"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Weather);
}

// =============================================================================
// Object/Item Tests
// =============================================================================

#[test]
fn test_object_basic() {
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b");OBJECT  *092345z4500.00N/01200.00W-"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Object, "Expected Object type");
    assert!(msg.obj_name.as_ref().map_or(false, |n| n.contains("OBJECT")),
            "obj_name should contain OBJECT, got: {:?}", msg.obj_name);
}

// =============================================================================
// Invalid Frame Tests
// =============================================================================

#[test]
fn test_invalid_frame_too_short() {
    let frame = vec![0x03, 0xF0];
    let result = parse_ax25_frame(&frame);
    assert!(result.is_none());
}

#[test]
fn test_invalid_frame_wrong_pid() {
    // For UI frames, PID must be 0xF0. Any other value should reject the frame.
    let frame = make_ax25_frame("A", "B", &[], b"test");
    
    // Find ctrl/pid by scanning backwards
    let mut ctrl_idx = 0;
    for i in 0..frame.len() - 1 {
        if frame[i] == 0x03 && frame[i + 1] == 0xF0 {
            ctrl_idx = i;
            break;
        }
    }
    
    let pid_idx = ctrl_idx + 1;
    
    // Create frame with wrong PID
    let mut bad_frame = frame.clone();
    bad_frame[pid_idx] = 0x01;  // Wrong PID (not 0xF0)
    
    let result = parse_ax25_frame(&bad_frame);
    assert!(result.is_none(), "Should reject frame with wrong PID");
}

#[test]
fn test_invalid_frame_wrong_ctrl() {
    // For UI frames, ctrl must be 0x03. Any other value should reject the frame.
    let frame = make_ax25_frame("A", "B", &[], b"test");
    
    // Find ctrl/pid by scanning backwards
    let mut ctrl_idx = 0;
    for i in 0..frame.len() - 1 {
        if frame[i] == 0x03 && frame[i + 1] == 0xF0 {
            ctrl_idx = i;
            break;
        }
    }
    
    // Create frame with wrong ctrl
    let mut bad_frame = frame.clone();
    bad_frame[ctrl_idx] = 0x04;  // Wrong ctrl (not 0x03 for UI)
    
    let result = parse_ax25_frame(&bad_frame);
    assert!(result.is_none(), "Should reject frame with wrong ctrl");
}

#[test]
fn test_empty_payload() {
    let frame = make_ax25_frame("A", "B", &[], b"");
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    assert_eq!(msg.msg_type, APRSType::Unknown);
}

// =============================================================================
// NMEA Tests
// =============================================================================

#[test]
fn test_nmea_basic() {
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"$GPRMC,081836,A,4534.19,N,01230.86,E,0.0,0.0,010220,0.0,E*41"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Nmea);
}

// =============================================================================
// Timestamp Tests
// =============================================================================

#[test]
fn test_timestamp_various_formats() {
    // @HHMMSSz format
    let frame = make_ax25_frame("T", "A", &[], b"@120000z4500.00N/01200.00W");
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    assert_eq!(msg.msg_type, APRSType::PositionWithTimestamp);
    
    // /DDHHMMh format
    let frame = make_ax25_frame("T", "A", &[], b"/211200z4500.00N/01200.00W");
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    assert_eq!(msg.msg_type, APRSType::PositionWithTimestamp);
}

// =============================================================================
// Mixed Case Tests
// =============================================================================

#[test]
fn test_mixed_case_callsigns() {
    let frame = make_ax25_frame(
        "Kv4p-11",
        "Aprs",
        &["Wide1-1"],
        b"!4500.00N/01200.00W-"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    // Callsigns should be uppercased
    assert!(msg.from_callsign.contains("KV4P"));
    assert!(msg.to_callsign.contains("APRS"));
}

// =============================================================================
// Unknown DTI Test
// =============================================================================

#[test]
fn test_unknown_dti() {
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"%Some unknown format"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Unknown);
}

// =============================================================================
// Symbol Tests
// =============================================================================

#[test]
fn test_position_with_hash_symbol() {
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"!4500.00N/01200.00W#"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.symbol_code, Some('#'));
}

#[test]
fn test_position_alternate_table() {
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"!4500.00N\\01200.00W\\"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.symbol_table_id, Some('\\'));
}

#[test]
fn test_position_with_spaces() {
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b"!4500.00 N/01200.00 W "
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Position);
}

// =============================================================================
// Object Compressed Test
// =============================================================================

#[test]
fn test_object_compressed() {
    // Note: This format has no `*` marker after the name, so it's an Item, not Object
    // Position format: DDMM.MMN/DDDMM.MMW[symbol]
    // Standard uncompressed Format A: DDMM.MM + N/S + / + DDDMM.MM + E/W + symbol
    // Note: Object name must be exactly 9 characters (space-padded)
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b");OBJ      !4917.20N/12258.05W#Comment"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    // Position should be parsed (verify latitude is reasonable)
    // 4917.20N = 49 degrees + 17.2 minutes = 49.2867
    assert!(msg.position_lat > 40.0 && msg.position_lat < 50.0,
            "Latitude should be around 49, got: {}", msg.position_lat);
    // 12258.05W = -(122 degrees + 58.05 minutes) = -122.9675
    assert!(msg.position_lon < -100.0 && msg.position_lon > -130.0,
            "Longitude should be around -122, got: {}", msg.position_lon);
    assert_eq!(msg.symbol_code, Some('#'), "Symbol code should be '#'");
    assert_eq!(msg.msg_type, APRSType::Item);
}

// =============================================================================
// Bulletin Tests
// =============================================================================

#[test]
fn test_message_bulletin() {
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b":BLN     :Bulletin message here"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Message);
    assert!(msg.msg_body.as_ref().map_or(false, |b| b.contains("Bulletin")));
}

#[test]
fn test_message_bulletin_named() {
    let frame = make_ax25_frame(
        "TEST",
        "APRS",
        &[],
        b":BLN1    :Named bulletin text"
    );
    
    let msg = parse_ax25_frame(&frame).expect("Should parse");
    
    assert_eq!(msg.msg_type, APRSType::Message);
}