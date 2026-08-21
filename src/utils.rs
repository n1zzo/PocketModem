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
    
    // Normalize longitude to [0, 360) range
    let adj_lon = ((lon + 180.0).rem_euclid(360.0));
    
    // Field (first pair): 18x18 grid, 20° x 10° each
    let lon_field = (adj_lon / 20.0).floor() as i32;
    let lat_field = ((lat + 90.0) / 10.0).floor() as i32;
    
    // Square (second pair): 10x10 grid, 2° longitude x 1° latitude
    let lon_in_field = adj_lon - (lon_field as f64 * 20.0);
    let lat_in_field = (lat + 90.0) - (lat_field as f64 * 10.0);
    let lon_sq = (lon_in_field / 2.0).floor() as i32;
    let lat_sq = (lat_in_field / 1.0).floor() as i32;
    
    // Subsquare (third pair): 24x24 grid, 2°/24 longitude x 1°/24 latitude
    let lon_ss = ((lon_in_field - (lon_sq as f64 * 2.0)) / (2.0 / 24.0)).floor() as i32;
    let lat_ss = ((lat_in_field - (lat_sq as f64 * 1.0)) / (1.0 / 24.0)).floor() as i32;
    
    let c1 = (b'A' + (lon_field % 18) as u8) as char;
    let c2 = (b'A' + (lat_field % 18) as u8) as char;
    let c3 = (b'0' + (lon_sq % 10) as u8) as char;
    let c4 = (b'0' + (lat_sq % 10) as u8) as char;
    let c5 = (b'a' + (lon_ss % 24) as u8) as char;
    let c6 = (b'a' + (lat_ss % 24) as u8) as char;
    
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
        // Zurich, Switzerland area - should be in JN47
        let locator = calculate_maidenhead(47.4, 8.5);
        assert_eq!(&locator[0..4], "JN47");
        
        // Zurich exact area (47.37°N, 8.55°E)
        let locator = calculate_maidenhead(47.37, 8.55);
        assert_eq!(&locator[0..4], "JN47");
        
        // New York City area (FN30 for -74°)
        let locator = calculate_maidenhead(40.71, -74.0);
        assert_eq!(&locator[0..2], "FN");
        
        // Tokyo area (PM95)
        let locator = calculate_maidenhead(35.68, 139.65);
        assert_eq!(&locator[0..4], "PM95");
        
        // Invalid coords
        assert_eq!(calculate_maidenhead(91.0, 0.0), "-----");
        assert_eq!(calculate_maidenhead(0.0, 200.0), "-----");
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