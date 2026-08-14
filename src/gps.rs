//! GPS module using ModemManager to get NMEA location data
//!
//! GPS requires:
//! 1. Cellular modem with GPS capability (separate from KV4P radio)
//! 2. GPS to be enabled via ModemManager
//! 3. Satellite visibility for fix

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// GPS data parsed from NMEA sentences
#[derive(Debug, Clone, Default)]
pub struct GpsData {
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub altitude: Option<f64>,
    pub speed: Option<f64>,
    pub course: Option<f64>,
    pub satellites: u8,
    /// True if we have valid NMEA data from the modem
    pub gps_enabled: bool,
    /// True if we have a position fix
    pub has_fix: bool,
}

/// GPS manager that queries ModemManager
pub struct GpsManager {
    pub data: Arc<Mutex<GpsData>>,
    running: Arc<AtomicBool>,
    enabled: Arc<AtomicBool>,
    /// Track which modem has GPS (auto-detected)
    modem_index: Arc<Mutex<Option<u32>>>,
}

impl GpsManager {
    pub fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(GpsData::default())),
            running: Arc::new(AtomicBool::new(false)),
            enabled: Arc::new(AtomicBool::new(true)),
            modem_index: Arc::new(Mutex::new(None)),
        }
    }

    /// Start GPS polling thread
    pub fn start(&self) {
        if self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }

        self.running.store(true, std::sync::atomic::Ordering::SeqCst);
        let data = Arc::clone(&self.data);
        let running = Arc::clone(&self.running);
        let enabled = Arc::clone(&self.enabled);

        let modem_index = Arc::clone(&self.modem_index);
        
        thread::spawn(move || {
            eprintln!("[gps] Starting GPS polling thread");

            // Auto-detect GPS modem on startup
            if let Some(idx) = Self::find_gps_modem() {
                *modem_index.lock().unwrap() = Some(idx);
                // Enable GPS on the detected modem
                Self::enable_gps_location_internal(idx);
            } else {
                eprintln!("[gps] WARNING: No GPS modem found. GPS will not work.");
                eprintln!("[gps] Make sure cellular modem with GPS is connected.");
            }

            loop {
                if !running.load(Ordering::SeqCst) {
                    break;
                }

                if enabled.load(Ordering::SeqCst) {
                    let idx = *modem_index.lock().unwrap();
                    Self::poll_location(&data, idx);
                }

                // Poll at 1 second intervals (GPS updates every ~1 second)
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

    /// Find the modem index that has GPS capability
    /// Queries all modems and returns the first one with GPS
    fn find_gps_modem() -> Option<u32> {
        use std::process::Command;
        
        // Get list of modems
        match Command::new("mmcli").args(["-L"]).output() {
            Ok(o) if o.status.success() => {
                let output = String::from_utf8_lossy(&o.stdout);
    
                
                // Parse modem indices from output
                // Format: /org/freedesktop/ModemManager1/Modem/N
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
                eprintln!("[gps] Failed to run mmcli -L: {}", e);
                None
            }
        }
    }
    
    /// Enable GPS location in ModemManager for the specified modem
    fn enable_gps_location_internal(modem_idx: u32) -> bool {
        use std::process::Command;
        
        // First check if already enabled
        match Command::new("mmcli")
            .args(["-m", &modem_idx.to_string(), "--location-status"])
            .output()
        {
            Ok(o) => {
                let status = String::from_utf8_lossy(&o.stdout);
                
                // Check if GPS is already enabled - "enabled: gps-nmea" line indicates it's active
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

    /// Poll location from ModemManager via mmcli
    fn poll_location(data: &Arc<Mutex<GpsData>>, modem_index: Option<u32>) {
        use std::process::Command;

        let mut new_data = GpsData::default();
        
        // If no modem found, mark GPS as not enabled
        let idx = match modem_index {
            Some(i) => i,
            None => {
                // Update shared data with no GPS
                if let Ok(mut d) = data.lock() {
                    *d = new_data;
                }
                return;
            }
        };

        // Query ModemManager for GPS location
        let output = Command::new("mmcli")
            .args(["-m", &idx.to_string(), "--location-get"])
            .output();

        if output.is_ok() && output.as_ref().unwrap().status.success() {
            let stdout = String::from_utf8_lossy(&output.as_ref().unwrap().stdout);
            
            // GPS hardware is present and responding - enabled is true
            new_data.gps_enabled = true;
            

            

            
            // Parse NMEA sentences from output
            for line in stdout.lines() {
                // Extract NMEA sentence (handle "|       $GPGGA,..." format)
                if let Some(nmea_start) = line.find("$") {
                    let nmea = &line[nmea_start..];
                    
                    // Handle GPS, GLONASS, and Galileo GGA sentences
                    if nmea.starts_with("$GPGGA") || nmea.starts_with("$GNGGA") || nmea.starts_with("$GAGGA") {
                        if let Some(parsed) = Self::parse_gga(nmea) {
                            new_data.latitude = parsed.latitude;
                            new_data.longitude = parsed.longitude;
                            new_data.altitude = parsed.altitude;
                            new_data.satellites = parsed.satellites;
                            new_data.has_fix = parsed.has_fix;
                        }
                    } else if nmea.starts_with("$GPGSV") || nmea.starts_with("$GLGSV") || nmea.starts_with("$GAGSV") {
                        // GSV: Satellite in view information
                        // Format: $GPGSV,total,num,count,sv1,elv1,az1,snr1,sv2,elv2,az2,snr2,...*checksum
                        // Only count field (field 3) gives total satellites in view
                        if let Some(sats) = Self::parse_gsv(nmea) {
                            // Only update if we don't have satellites from GGA yet
                            if new_data.satellites == 0 {
                                new_data.satellites = sats;
                            }
                        }
                    } else if nmea.starts_with("$GPRMC") || nmea.starts_with("$GNRMC") || nmea.starts_with("$GARMC") {
                        if let Some(rmc) = Self::parse_rmc(nmea) {
                            new_data.speed = rmc.speed;
                            new_data.course = rmc.course;
                            
                            // RMC has its own validity check - if status is 'A' (active), position is valid
                            // The GGA fix quality is more authoritative, but RMC active is a good indicator
                        }
                    }
                }
            }
        
            

        } else if let Some(e) = output.as_ref().err() {
            eprintln!("[gps] mmcli error: {}", e);
        }

        // Update shared data
        if let Ok(mut d) = data.lock() {
            *d = new_data;
        }
    }

    /// Parse GGA sentence: $GPGGA,time,lat,lat_dir,lon,lon_dir,fix,sats,hdop,alt,alt_unit,sep,sep_unit,diff*checksum
    fn parse_gga(line: &str) -> Option<GpsData> {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 10 {
            return None;
        }

        let fix_quality: u8 = parts[6].parse().unwrap_or(0);
        let has_fix = fix_quality > 0;

        // Parse latitude: ddmm.mmmmm
        let lat_raw: f64 = parts[2].parse().unwrap_or(0.0);
        let lat_dir = parts[3];
        let lat = Self::convert_nmea_coord(lat_raw) * if lat_dir == "S" { -1.0 } else { 1.0 };

        // Parse longitude: dddmm.mmmmm
        let lon_raw: f64 = parts[4].parse().unwrap_or(0.0);
        let lon_dir = parts[5];
        let lon = Self::convert_nmea_coord(lon_raw) * if lon_dir == "W" { -1.0 } else { 1.0 };

        // Altitude
        let altitude: f64 = parts[9].parse().unwrap_or(0.0);

        // Number of satellites
        let sats: u8 = parts[7].parse().unwrap_or(0);

        let mut data = GpsData::default();
        data.latitude = if lat != 0.0 { Some(lat) } else { None };
        data.longitude = if lon != 0.0 { Some(lon) } else { None };
        data.altitude = if altitude != 0.0 { Some(altitude) } else { None };
        data.satellites = sats;
        data.has_fix = has_fix;
        // Note: gps_enabled is set by caller, not here

        Some(data)
    }

    /// Parse RMC sentence: $GPRMC,time,status,lat,lat_dir,lon,lon_dir,speed,course,date,mag_var,mag_dir*checksum
    fn parse_rmc(line: &str) -> Option<GpsData> {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 11 {
            return None;
        }

        // Speed in knots -> km/h
        let speed_knots: f64 = parts[7].parse().unwrap_or(0.0);
        let speed_kmh = speed_knots * 1.852;

        // Course in degrees
        let course: f64 = parts[8].parse().unwrap_or(0.0);

        let mut data = GpsData::default();
        data.speed = if speed_kmh > 0.0 { Some(speed_kmh) } else { None };
        data.course = if course > 0.0 { Some(course) } else { None };
        // Note: gps_enabled and has_fix are set by caller based on GGA data

        Some(data)
    }

    /// Parse GSV sentence: $GPGSV,total,num,count,sv1,elv1,az1,snr1,...*checksum
    /// Returns the number of satellites in view (from the count field)
    fn parse_gsv(line: &str) -> Option<u8> {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 4 {
            return None;
        }
        // Field 3 is the total number of satellites in view
        parts[3].parse().ok()
    }

    /// Convert NMEA coordinate format (ddmm.mmmm) to decimal degrees
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

    /// Check if GPS is enabled and receiving data
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
            let lat_str = format!("{:.6}", lat);
            let lon_str = format!("{:.6}", lon);
            format!("{}, {}", lat_str, lon_str)
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