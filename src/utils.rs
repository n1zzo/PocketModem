//! Utility functions for PocketModem
//!
//! Pure functions with no dependencies on other app modules.

/// Calculate Maidenhead locator from lat/lon (6-character grid square)
/// 
/// Returns "-----" for invalid coordinates.
pub fn calculate_maidenhead(lat: f64, lon: f64) -> String {
    if lat < -90.0 || lat > 90.0 || lon < -180.0 || lon > 180.0 {
        return "-----".to_string();
    }
    
    let radix: [i32; 6] = [18, 10, 24, 10, 24, 10];
    
    let int_lat = ((lat + 90.0) * 10800.0).round() as i64;
    let int_lon = (((lon + 180.0).rem_euclid(360.0)) * 5400.0).round() as i64;
    
    fn convert(val: i64, radix: &[i32; 6]) -> [i32; 6] {
        let mut result = [0i32; 6];
        let mut idx = 5;
        let mut remaining_radix: Vec<i32> = radix.to_vec();
        
        while !remaining_radix.is_empty() {
            let r = remaining_radix.pop().unwrap();
            let (p, q) = (val / r as i64, val % r as i64);
            result[idx] = q as i32;
            if idx > 0 { idx -= 1; }
        }
        result
    }
    
    let lat_digits = convert(int_lat, &radix);
    let lon_digits = convert(int_lon, &radix);
    
    let c1 = (b'A' + (lon_digits[0] % 18) as u8) as char;
    let c2 = (b'A' + (lat_digits[0] % 18) as u8) as char;
    let c3 = (b'0' + (lon_digits[1] % 10) as u8) as char;
    let c4 = (b'0' + (lat_digits[1] % 10) as u8) as char;
    let c5 = (b'a' + (lon_digits[2] % 24) as u8) as char;
    let c6 = (b'a' + (lat_digits[2] % 24) as u8) as char;
    
    format!("{}{}{}{}{}{}", c1, c2, c3, c4, c5, c6)
}

/// Calculate distance (km) and bearing (degrees) between two points
/// 
/// Uses the haversine formula. Returns `None` if the origin is (0, 0).
pub fn calculate_distance_bearing(my_lat: f64, my_lon: f64, target_lat: f64, target_lon: f64) -> Option<(f64, f64)> {
    if my_lat == 0.0 && my_lon == 0.0 {
        return None;
    }
    
    let lat1 = my_lat.to_radians();
    let lat2 = target_lat.to_radians();
    let delta_lon = (target_lon - my_lon).to_radians();
    
    let (sin_lat1, cos_lat1, sin_lat2, cos_lat2) = (lat1.sin(), lat1.cos(), lat2.sin(), lat2.cos());
    
    let a = ((target_lat - my_lat) / 2.0).to_radians().sin().powi(2) 
          + cos_lat1 * cos_lat2 * (delta_lon / 2.0).sin().powi(2);
    let c = 2.0 * (a.sqrt().min(1.0)).asin();
    let distance_km = 6371.0 * c;
    
    let y = delta_lon.sin() * cos_lat2;
    let x = cos_lat1 * sin_lat2 - sin_lat1 * cos_lat2 * delta_lon.cos();
    let bearing = y.atan2(x).to_degrees().rem_euclid(360.0);
    
    Some((distance_km, bearing))
}

/// Calculate distance and format for display
/// 
/// Returns formatted string: "<1km" as meters, "<10km" with 1 decimal, otherwise integer km.
/// Returns "??" if distance calculation fails.
pub fn calculate_distance_display(my_lat: f64, my_lon: f64, target_lat: f64, target_lon: f64) -> String {
    if let Some((dist_km, _)) = calculate_distance_bearing(my_lat, my_lon, target_lat, target_lon) {
        if dist_km < 1.0 {
            format!("{:.0}m", dist_km * 1000.0)
        } else if dist_km < 10.0 {
            format!("{:.1}km", dist_km)
        } else {
            format!("{:.0}km", dist_km)
        }
    } else {
        "??".to_string()
    }
}

/// Convert bearing to compass direction with arrow and degrees
/// 
/// Examples: 0° → "↑0°", 45° → "↗45°", 180° → "↓180°"
pub fn bearing_to_compass(bearing: f64) -> String {
    let arrow = match (bearing.round() as i32) % 360 {
        0..=22 | 338..=360 => "↑",
        23..=67 => "↗",
        68..=112 => "→",
        113..=157 => "↘",
        158..=202 => "↓",
        203..=247 => "↙",
        248..=292 => "←",
        293..=337 => "↖",
        _ => "?",
    };
    
    format!("{}{:.0}°", arrow, bearing)
}

/// Escape special characters for Pango markup
/// 
/// Escapes: & < > " '
pub fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
     .replace("'", "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_maidenhead() {
        // Swiss coordinates (HB9) - should be in JN47
        let locator = calculate_maidenhead(46.8, 8.2);
        assert_eq!(&locator[0..4], "JN47");
        
        // Invalid coords
        assert_eq!(calculate_maidenhead(91.0, 0.0), "-----");
    }

    #[test]
    fn test_distance_bearing() {
        // Same point
        let result = calculate_distance_bearing(46.8, 8.2, 46.8, 8.2);
        assert!(result.is_none());
        
        // Known distance (approximately 10km north)
        let result = calculate_distance_bearing(46.8, 8.2, 46.9, 8.2);
        if let Some((dist, bearing)) = result {
            assert!(dist > 10.0 && dist < 12.0);
            assert!((bearing - 0.0).abs() < 1.0 || (bearing - 360.0).abs() < 1.0);
        }
    }

    #[test]
    fn test_bearing_to_compass() {
        assert!(bearing_to_compass(0.0).starts_with('↑'));
        assert!(bearing_to_compass(90.0).starts_with('→'));
        assert!(bearing_to_compass(180.0).starts_with('↓'));
        assert!(bearing_to_compass(270.0).starts_with('←'));
    }

    #[test]
    fn test_escape_markup() {
        assert_eq!(escape_markup("A & B < C > D"), "A &amp; B &lt; C &gt; D");
        assert_eq!(escape_markup("\"test\""), "&quot;test&quot;");
    }
}