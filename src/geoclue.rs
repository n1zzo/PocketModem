//! GeoClue2 integration for basic location data
//!
//! GeoClue2 is the standard Linux location service that provides location
//! from various sources (GPS, WiFi, Cell towers). Used as fallback when
//! mmcli/ModemManager is not available.

use std::process::Command;

/// Location data from GeoClue2
#[derive(Debug, Clone)]
pub struct GeoClueLocation {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: Option<f64>,
    pub accuracy: f64,
}

/// Parse object path from gdbus output
fn parse_object_path(output: &str) -> Option<String> {
    let trimmed = output.trim();
    if !trimmed.starts_with("(") {
        return None;
    }
    
    // Find the closing parenthesis or comma
    let rest = &trimmed[1..];
    if let Some(end_pos) = rest.find(|c| c == ',' || c == ')') {
        let path_part = &rest[..end_pos];
        let path = path_part.trim()
            .strip_prefix("objectpath ")
            .unwrap_or(path_part.trim())
            .trim_matches('\'');
        if !path.is_empty() {
            return Some(path.to_string());
        }
    }
    None
}

/// Initialize GeoClue2 client
/// Returns true if GeoClue2 is available
pub fn init_geoclue() -> bool {
    let output = Command::new("gdbus")
        .args(["call", "--system", "--dest", "org.freedesktop.GeoClue2",
               "--object-path", "/org/freedesktop/GeoClue2/Manager",
               "--method", "org.freedesktop.GeoClue2.Manager.GetClient"])
        .output();

    match output {
        Ok(o) => {
            if o.status.success() {
                eprintln!("[geoclue] GeoClue2 client created successfully");
                true
            } else {
                let stderr = String::from_utf8_lossy(&o.stderr);
                eprintln!("[geoclue] GeoClue2 error: {}", stderr);
                false
            }
        }
        Err(e) => {
            eprintln!("[geoclue] gdbus not found: {}", e);
            false
        }
    }
}

/// Get current location from GeoClue2
/// Returns None if location is not available
pub fn get_location() -> Option<GeoClueLocation> {
    // Get a client
    let client_output = match Command::new("gdbus")
        .args(["call", "--system", "--dest", "org.freedesktop.GeoClue2",
               "--object-path", "/org/freedesktop/GeoClue2/Manager",
               "--method", "org.freedesktop.GeoClue2.Manager.GetClient"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return None,
    };

    if !client_output.status.success() {
        return None;
    }

    // Parse client path
    let stdout = String::from_utf8_lossy(&client_output.stdout);
    let client_path = parse_object_path(&stdout)?;

    // Start the client
    let _ = Command::new("gdbus")
        .args(["call", "--system", "--dest", "org.freedesktop.GeoClue2",
               "--object-path", &client_path,
               "--method", "org.freedesktop.GeoClue2.Client.Start"])
        .output();

    // Get location path from client
    let loc_output = match Command::new("gdbus")
        .args(["call", "--system", "--dest", "org.freedesktop.GeoClue2",
               "--object-path", &client_path,
               "--method", "org.freedesktop.GeoClue2.Client.GetLocation"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return None,
    };

    if !loc_output.status.success() {
        return None;
    }

    // Parse location path
    let loc_stdout = String::from_utf8_lossy(&loc_output.stdout);
    let loc_path = parse_object_path(&loc_stdout)?;

    // Get location properties
    let props_output = match Command::new("gdbus")
        .args(["call", "--system", "--dest", "org.freedesktop.GeoClue2",
               "--object-path", &loc_path,
               "--method", "org.freedesktop.DBus.Properties.GetAll",
               "--arg-type", "s", "--arg-value", "org.freedesktop.GeoClue2.Location"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return None,
    };

    if !props_output.status.success() {
        return None;
    }

    // Parse properties from output
    let props = String::from_utf8_lossy(&props_output.stdout);
    parse_location_from_props(&props)
}

/// Parse location from gdbus GetAll output
fn parse_location_from_props(output: &str) -> Option<GeoClueLocation> {
    let mut latitude: f64 = 0.0;
    let mut longitude: f64 = 0.0;
    let mut altitude: Option<f64> = None;
    let mut accuracy: f64 = 999.0;

    for line in output.lines() {
        let line = line.trim();
        if line.starts_with("'Latitude'") || line.starts_with("\"Latitude\"") {
            if let Some(val) = extract_variant_value(line) {
                if let Ok(v) = val.parse::<f64>() {
                    latitude = v;
                }
            }
        } else if line.starts_with("'Longitude'") || line.starts_with("\"Longitude\"") {
            if let Some(val) = extract_variant_value(line) {
                if let Ok(v) = val.parse::<f64>() {
                    longitude = v;
                }
            }
        } else if line.starts_with("'Altitude'") || line.starts_with("\"Altitude\"") {
            if let Some(val) = extract_variant_value(line) {
                if let Ok(v) = val.parse::<f64>() {
                    altitude = Some(v);
                }
            }
        } else if line.starts_with("'Accuracy'") || line.starts_with("\"Accuracy\"") {
            if let Some(val) = extract_variant_value(line) {
                if let Ok(v) = val.parse::<f64>() {
                    accuracy = v;
                }
            }
        }
    }

    if latitude != 0.0 || longitude != 0.0 {
        Some(GeoClueLocation {
            latitude,
            longitude,
            altitude,
            accuracy,
        })
    } else {
        None
    }
}

/// Extract value from gdbus variant output
fn extract_variant_value(line: &str) -> Option<&str> {
    // Format: 'Latitude' = <double 37.123456>, or "Latitude" = <double 37.123456>
    if let Some(pos) = line.find("double ") {
        let val = &line[pos + 7..];
        return Some(val.trim_end_matches('>').trim_end_matches(','));
    }
    if let Some(pos) = line.find("int64 ") {
        let val = &line[pos + 6..];
        return Some(val.trim_end_matches('>').trim_end_matches(','));
    }
    None
}