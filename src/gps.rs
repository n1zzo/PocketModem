//! GPS module using ModemManager to get NMEA location data

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
    pub valid: bool,
    pub has_fix: bool,
}

/// GPS manager that queries ModemManager
pub struct GpsManager {
    pub data: Arc<Mutex<GpsData>>,
    running: Arc<AtomicBool>,
    enabled: Arc<AtomicBool>,
}

impl GpsManager {
    pub fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(GpsData::default())),
            running: Arc::new(AtomicBool::new(false)),
            enabled: Arc::new(AtomicBool::new(true)),
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

        thread::spawn(move || {
            eprintln!("[gps] Starting GPS polling thread");

            loop {
                if !running.load(Ordering::SeqCst) {
                    break;
                }

                if enabled.load(Ordering::SeqCst) {
                    Self::poll_location(&data);
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

    /// Enable GPS location in ModemManager
    pub fn enable_gps_location() -> bool {
        use std::process::Command;
        match Command::new("mmcli")
            .args(["-m", "0", "--location-enable-gps-nmea"])
            .output()
        {
            Ok(o) => {
                if o.status.success() {
                    eprintln!("[gps] GPS location enabled");
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
    fn poll_location(data: &Arc<Mutex<GpsData>>) {
        use std::process::Command;

        // Query ModemManager for GPS location
        let output = Command::new("mmcli")
            .args(["-m", "0", "--location-get"])
            .output();

        let mut new_data = GpsData::default();


        if output.is_ok() && output.as_ref().unwrap().status.success() {
            let stdout = String::from_utf8_lossy(&output.as_ref().unwrap().stdout);

            // Parse NMEA sentences from output
            for line in stdout.lines() {
                // Extract NMEA sentence (handle "|       $GPGGA,..." format)
                if let Some(nmea_start) = line.find("$") {
                    let nmea = &line[nmea_start..];
                    if nmea.starts_with("$GPGGA") || nmea.starts_with("$GNGGA") {
                        if let Some(parsed) = Self::parse_gga(nmea) {
                            new_data.latitude = parsed.latitude;
                            new_data.longitude = parsed.longitude;
                            new_data.altitude = parsed.altitude;
                            new_data.satellites = parsed.satellites;
                            new_data.has_fix = parsed.has_fix;
                            new_data.valid = true;
                        }
                    } else if nmea.starts_with("$GPRMC") || nmea.starts_with("$GNRMC") {
                        if let Some(parsed) = Self::parse_rmc(nmea) {
                            new_data.speed = parsed.speed;
                            new_data.course = parsed.course;
                            if parsed.valid && !new_data.has_fix {
                                new_data.has_fix = true;
                            }
                        }
                    }
                }
            }
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
        data.valid = true;

        Some(data)
    }

    /// Parse RMC sentence: $GPRMC,time,status,lat,lat_dir,lon,lon_dir,speed,course,date,mag_var,mag_dir*checksum
    fn parse_rmc(line: &str) -> Option<GpsData> {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 11 {
            return None;
        }

        let status = parts[2];
        let valid = status == "A";

        // Speed in knots -> km/h
        let speed_knots: f64 = parts[7].parse().unwrap_or(0.0);
        let speed_kmh = speed_knots * 1.852;

        // Course in degrees
        let course: f64 = parts[8].parse().unwrap_or(0.0);

        let mut data = GpsData::default();
        data.speed = if speed_kmh > 0.0 { Some(speed_kmh) } else { None };
        data.course = if course > 0.0 { Some(course) } else { None };
        data.valid = valid;

        Some(data)
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

    /// Get formatted location string
    pub fn get_location_string(&self) -> String {
        let data = self.get_data();

        if !data.valid {
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