//! GPS module using ModemManager (via mmcli) or GeoClue2
//!
//! Primary: ModemManager via mmcli (full NMEA data, GPS hardware required)
//! Fallback: GeoClue2 D-Bus (WiFi/Cell tower geolocation)
//!
//! GPS requires:
//! 1. Cellular modem with GPS capability OR GeoClue2 service
//! 2. For ModemManager: GPS to be enabled via ModemManager

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// GPS data parsed from NMEA sentences or GeoClue2
#[derive(Debug, Clone, Default)]
pub struct GpsData {
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub altitude: Option<f64>,
    pub speed: Option<f64>,
    pub course: Option<f64>,
    pub satellites: u8,
    /// True if we have valid GPS data
    pub gps_enabled: bool,
    /// True if we have a position fix
    pub has_fix: bool,
}

/// GPS manager that queries ModemManager or GeoClue2
pub struct GpsManager {
    pub data: Arc<Mutex<GpsData>>,
    running: Arc<AtomicBool>,
    enabled: Arc<AtomicBool>,
    /// Track which modem has GPS (auto-detected) - only used for mmcli
    modem_index: Arc<Mutex<Option<u32>>>,
    /// Which backend we're using
    use_geoclue: Arc<Mutex<bool>>,
}

impl GpsManager {
    pub fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(GpsData::default())),
            running: Arc::new(AtomicBool::new(false)),
            enabled: Arc::new(AtomicBool::new(true)),
            modem_index: Arc::new(Mutex::new(None)),
            use_geoclue: Arc::new(Mutex::new(false)),
        }
    }

    /// Start GPS polling thread
    pub fn start(&self) {
        if self.running.load(Ordering::SeqCst) {
            return;
        }

        self.running.store(true, Ordering::SeqCst);
        let data = Arc::clone(&self.data);
        let running = Arc::clone(&self.running);
        let enabled = Arc::clone(&self.enabled);
        let modem_index = Arc::clone(&self.modem_index);
        let use_geoclue = Arc::clone(&self.use_geoclue);

        thread::spawn(move || {
            eprintln!("[gps] Starting GPS polling thread");

            // Try mmcli first (ModemManager - full NMEA GPS data)
            eprintln!("[gps] Trying mmcli (ModemManager) first...");
            if let Some(idx) = Self::find_gps_modem() {
                eprintln!("[gps] Using ModemManager (mmcli) for GPS");
                *modem_index.lock().unwrap() = Some(idx);
                Self::enable_gps_location_internal(idx);
                *use_geoclue.lock().unwrap() = false;
            } else {
                // Fallback to ashpd/XDG portal (WiFi/Cell tower geolocation)
                eprintln!("[gps] mmcli not available, trying ashpd (XDG Location portal)...");
                if crate::geoclue::init_geoclue() {
                    eprintln!("[gps] Using ashpd/XDG Location portal for GPS");
                    *use_geoclue.lock().unwrap() = true;
                } else {
                    eprintln!("[gps] WARNING: No GPS source found. GPS will not work.");
                    eprintln!("[gps] Install ModemManager or enable XDG Location portal.");
                }
            }


            loop {
                if !running.load(Ordering::SeqCst) {
                    break;
                }

                if enabled.load(Ordering::SeqCst) {
                    let geoclue = *use_geoclue.lock().unwrap();
                    if geoclue {
                        Self::poll_location_geoclue(&data);
                    } else {
                        let idx = *modem_index.lock().unwrap();
                        Self::poll_location_mmcli(&data, idx);
                    }
                }

                thread::sleep(Duration::from_secs(1));
            }

            eprintln!("[gps] GPS polling thread stopped");
        });
    }

    /// Stop GPS polling
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Enable/disable GPS polling
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    /// Find the modem index that has GPS capability using mmcli
    fn find_gps_modem() -> Option<u32> {
        use std::process::Command;

        // Get list of modems
        match Command::new("mmcli").args(["-L"]).output() {
            Ok(o) if o.status.success() => {
                let output = String::from_utf8_lossy(&o.stdout);

                for line in output.lines() {
                    if let Some(idx) = line.rsplit('/').next()?.split_whitespace().next() {
                        if let Ok(modem_num) = idx.parse::<u32>() {
                            // Check if this modem has GPS capability
                            let status = Command::new("mmcli")
                                .args(["-m", &modem_num.to_string(), "--location-status"])
                                .output();

                            if let Ok(s) = status {
                                let status_str = String::from_utf8_lossy(&s.stdout);
                                if status_str.contains("gps-raw") || status_str.contains("gps-nmea") {
                                    eprintln!("[gps] Found GPS on modem {}", modem_num);
                                    return Some(modem_num);
                                }
                            }
                        }
                    }
                }
                eprintln!("[gps] No modem with GPS capability found");
                None
            }
            Ok(o) => {
                eprintln!("[gps] mmcli -L failed: {}", String::from_utf8_lossy(&o.stderr));
                None
            }
            Err(e) => {
                eprintln!("[gps] mmcli not found: {}", e);
                None
            }
        }
    }

    /// Enable GPS location in ModemManager
    fn enable_gps_location_internal(modem_idx: u32) -> bool {
        use std::process::Command;

        // Check if already enabled
        match Command::new("mmcli")
            .args(["-m", &modem_idx.to_string(), "--location-status"])
            .output()
        {
            Ok(o) => {
                let status = String::from_utf8_lossy(&o.stdout);
                if status.contains("enabled: gps-nmea") {
                    return true;
                }
            }
            Err(e) => {
                eprintln!("[gps] Failed to check location status: {}", e);
            }
        }

        // Enable GPS NMEA
        match Command::new("mmcli")
            .args(["-m", &modem_idx.to_string(), "--location-enable-gps-nmea"])
            .output()
        {
            Ok(o) => {
                if o.status.success() {
                    eprintln!("[gps] GPS location enabled on modem {}", modem_idx);
                    true
                } else {
                    eprintln!("[gps] Failed to enable GPS: {}", String::from_utf8_lossy(&o.stderr));
                    false
                }
            }
            Err(e) => {
                eprintln!("[gps] Failed to run mmcli: {}", e);
                false
            }
        }
    }

    /// Poll location using mmcli (ModemManager)
    fn poll_location_mmcli(data: &Arc<Mutex<GpsData>>, modem_index: Option<u32>) {
        use std::process::Command;

        let mut new_data = GpsData::default();

        let idx = match modem_index {
            Some(i) => i,
            None => {
                if let Ok(mut d) = data.lock() {
                    *d = new_data;
                }
                return;
            }
        };

        let output = Command::new("mmcli")
            .args(["-m", &idx.to_string(), "--location-get"])
            .output();

        if output.is_ok() && output.as_ref().unwrap().status.success() {
            let stdout = String::from_utf8_lossy(&output.as_ref().unwrap().stdout);
            new_data.gps_enabled = true;

            for line in stdout.lines() {
                if let Some(nmea_start) = line.find("$") {
                    let nmea = &line[nmea_start..];

                    if nmea.starts_with("$GPGGA") || nmea.starts_with("$GNGGA") || nmea.starts_with("$GAGGA") {
                        if let Some(parsed) = Self::parse_gga(nmea) {
                            new_data.latitude = parsed.latitude;
                            new_data.longitude = parsed.longitude;
                            new_data.altitude = parsed.altitude;
                            new_data.satellites = parsed.satellites;
                            new_data.has_fix = parsed.has_fix;
                        }
                    } else if nmea.starts_with("$GPGSV") || nmea.starts_with("$GLGSV") || nmea.starts_with("$GAGSV") {
                        if let Some(sats) = Self::parse_gsv(nmea) {
                            if new_data.satellites == 0 {
                                new_data.satellites = sats;
                            }
                        }
                    } else if nmea.starts_with("$GPRMC") || nmea.starts_with("$GNRMC") || nmea.starts_with("$GARMC") {
                        if let Some(rmc) = Self::parse_rmc(nmea) {
                            new_data.speed = rmc.speed;
                            new_data.course = rmc.course;
                        }
                    }
                }
            }
        } else if let Some(e) = output.as_ref().err() {
            eprintln!("[gps] mmcli error: {}", e);
        }

        if let Ok(mut d) = data.lock() {
            *d = new_data;
        }
    }

    /// Poll location using GeoClue2 (fallback)
    fn poll_location_geoclue(data: &Arc<Mutex<GpsData>>) {
        let mut new_data = GpsData::default();

        if let Some(loc) = crate::geoclue::get_location() {
            new_data.gps_enabled = true;
            new_data.latitude = Some(loc.latitude);
            new_data.longitude = Some(loc.longitude);
            // Only set altitude if it's a valid value
            // GeoClue2 returns f64::MIN for unknown altitude
            if let Some(alt) = loc.altitude {
                if alt > f64::MIN && alt > -1000.0 && alt < 50000.0 {
                    new_data.altitude = Some(alt);
                }
            }
            // GeoClue2 always has a "fix" (WiFi/IP location)
            new_data.has_fix = true;
        }

        if let Ok(mut d) = data.lock() {
            *d = new_data;
        }
    }

    /// Parse GGA sentence
    fn parse_gga(line: &str) -> Option<GpsData> {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 10 {
            return None;
        }

        let fix_quality: u8 = parts[6].parse().unwrap_or(0);
        let has_fix = fix_quality > 0;

        let lat_raw: f64 = parts[2].parse().unwrap_or(0.0);
        let lat_dir = parts[3];
        let lat = Self::convert_nmea_coord(lat_raw) * if lat_dir == "S" { -1.0 } else { 1.0 };

        let lon_raw: f64 = parts[4].parse().unwrap_or(0.0);
        let lon_dir = parts[5];
        let lon = Self::convert_nmea_coord(lon_raw) * if lon_dir == "W" { -1.0 } else { 1.0 };

        let altitude: f64 = parts[9].parse().unwrap_or(0.0);
        let sats: u8 = parts[7].parse().unwrap_or(0);

        let mut data = GpsData::default();
        data.latitude = if lat != 0.0 { Some(lat) } else { None };
        data.longitude = if lon != 0.0 { Some(lon) } else { None };
        data.altitude = if altitude != 0.0 { Some(altitude) } else { None };
        data.satellites = sats;
        data.has_fix = has_fix;

        Some(data)
    }

    /// Parse RMC sentence
    fn parse_rmc(line: &str) -> Option<GpsData> {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 11 {
            return None;
        }

        let speed_knots: f64 = parts[7].parse().unwrap_or(0.0);
        let speed_kmh = speed_knots * 1.852;
        let course: f64 = parts[8].parse().unwrap_or(0.0);

        let mut data = GpsData::default();
        data.speed = if speed_kmh > 0.0 { Some(speed_kmh) } else { None };
        data.course = if course > 0.0 { Some(course) } else { None };

        Some(data)
    }

    /// Parse GSV sentence
    fn parse_gsv(line: &str) -> Option<u8> {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 4 {
            return None;
        }
        parts[3].parse().ok()
    }

    /// Convert NMEA coordinate format to decimal degrees
    fn convert_nmea_coord(nmea: f64) -> f64 {
        if nmea == 0.0 {
            return 0.0;
        }
        let degrees = (nmea / 100.0).floor();
        let minutes = nmea - (degrees * 100.0);
        degrees + (minutes / 60.0)
    }

    /// Get current GPS data
    pub fn get_data(&self) -> GpsData {
        self.data.lock().unwrap().clone()
    }

    /// Check if GPS is enabled
    pub fn is_enabled(&self) -> bool {
        self.data.lock().unwrap().gps_enabled
    }

    /// Get formatted location string
    pub fn get_location_string(&self) -> String {
        let data = self.get_data();

        if !data.gps_enabled {
            return "No GPS".to_string();
        }

        if let (Some(lat), Some(lon)) = (data.latitude, data.longitude) {
            format!("{:.6}, {:.6}", lat, lon)
        } else {
            "Searching...".to_string()
        }
    }

    /// Check if GPS has a fix
    pub fn has_fix(&self) -> bool {
        self.data.lock().unwrap().has_fix
    }
}

impl Default for GpsManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gps_data_default() {
        let data = GpsData::default();
        assert!(!data.gps_enabled);
        assert!(!data.has_fix);
        assert!(data.latitude.is_none());
        assert!(data.longitude.is_none());
    }
}