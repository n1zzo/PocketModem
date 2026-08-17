//! PocketModem libadwaita UI with native KV4P protocol implementation
//!
//! Uses libadwaita patterns for a modern GNOME-style interface:
//! - AdwCarousel for swipe navigation (FM, APRS Messages, Map)
//! - PreferencesGroup + ActionRow for settings
//! - ToastOverlay for notifications
//! - Clamp for content width

mod aprs;
mod audio;
mod gps;
mod kiss;
mod map;
mod radio;
mod settings;

use aprs::APRSMessage;
use audio::{AudioConfig, AudioManager};
use gps::GpsManager;
use map::MapManager;
use settings::{SettingsManager, Channel, Duplex, ToneMode, PowerLevel, TileSource};

use radio::{KV4PRadio, SerialConfig};

#[cfg(feature = "notifications")]
use libnotify::{init, is_initted, Notification, Urgency};

use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::thread;

use adw::prelude::*;
use adw;

/// Calculate Maidenhead locator from lat/lon
/// Uses the standard Maidenhead Locator System algorithm:
/// - Field: 18x18 grid (A-R), 18° lon x 9° lat
/// - Square: 10x10 grid (0-9), 2° lon x 1° lat  
/// - Subsquare: 24x24 grid (a-x), 5' lon x 2.5' lat
/// 
/// Reference: hamutils/maidenhead Python library
fn maidenhead_locator(lat: f64, lon: f64) -> String {
    // Handle edge cases
    if lat < -90.0 || lat > 90.0 || lon < -180.0 || lon > 180.0 {
        return "-----".to_string();
    }
    
    // Radix progression for 6-character precision:
    // Field (18), Square (10), Subsquare (24), Square (10), Subsquare (24), Square (10)
    let radix: [i32; 6] = [18, 10, 24, 10, 24, 10];
    
    // Calculate multiplier from all radix values
    let multiplier: i64 = radix.iter().map(|&r| r as i64).product();
    let half_multiplier = multiplier / 2;
    
    // Convert to integer representation
    // int_lat = int((lat + 90) * multiplier) // (radix[0] * radix[1])
    let int_lat = ((lat + 90.0) * multiplier as f64).round() as i64 
                  / (radix[0] * radix[1]) as i64;
    let int_lon = (((lon + 180.0).rem_euclid(360.0)) * half_multiplier as f64).round() as i64
                  / (radix[0] * radix[1]) as i64;
    
    // Helper to convert integer to Maidenhead digits
    fn convert(mut val: i64, radix: &[i32; 6]) -> [i32; 6] {
        let mut result = [0i32; 6];
        let mut idx = 5;
        let mut remaining_radix: Vec<i32> = radix.to_vec();
        
        while !remaining_radix.is_empty() {
            let r = remaining_radix.pop().unwrap();
            let (p, q) = (val / r as i64, val % r as i64);
            result[idx] = q as i32;
            val = p;
            if idx > 0 { idx -= 1; }
        }
        result
    }
    
    let lat_digits = convert(int_lat, &radix);
    let lon_digits = convert(int_lon, &radix);
    
    // Build Maidenhead string
    let c1 = (b'A' + (lon_digits[0] % 18) as u8) as char;  // Field lon: A-R
    let c2 = (b'A' + (lat_digits[0] % 18) as u8) as char;  // Field lat: A-R
    let c3 = (b'0' + (lon_digits[1] % 10) as u8) as char;  // Square lon: 0-9
    let c4 = (b'0' + (lat_digits[1] % 10) as u8) as char;  // Square lat: 0-9
    let c5 = (b'a' + (lon_digits[2] % 24) as u8) as char;  // Subsquare lon: a-x
    let c6 = (b'a' + (lat_digits[2] % 24) as u8) as char;  // Subsquare lat: a-x
    
    format!("{}{}{}{}{}{}", c1, c2, c3, c4, c5, c6)
}

/// Calculate distance (km) and bearing (degrees) from my_pos to target
fn calculate_distance_bearing(my_lat: f64, my_lon: f64, target_lat: f64, target_lon: f64) -> Option<(f64, f64)> {
    if my_lat == 0.0 && my_lon == 0.0 {
        return None;
    }
    
    let lat1 = my_lat.to_radians();
    let lat2 = target_lat.to_radians();
    let delta_lon = (target_lon - my_lon).to_radians();
    
    let (sin_lat1, cos_lat1, sin_lat2, cos_lat2) = (lat1.sin(), lat1.cos(), lat2.sin(), lat2.cos());
    
    // Haversine formula: a = sin²(Δlat/2) + cos(lat1) * cos(lat2) * sin²(Δlon/2)
    let delta_lat = target_lat - my_lat;
    let sin_delta_lat = (delta_lat.to_radians() / 2.0).sin();
    let sin_delta_lon = (delta_lon / 2.0).sin();
    let a = sin_delta_lat.powi(2) + cos_lat1 * cos_lat2 * sin_delta_lon.powi(2);
    
    // c = 2 * asin(min(1.0, sqrt(a)))
    let c = 2.0 * (a.sqrt().min(1.0)).asin();
    let distance_km = 6371.0 * c;
    
    // Bearing calculation
    let y = delta_lon.sin() * cos_lat2;
    let x = cos_lat1 * sin_lat2 - sin_lat1 * cos_lat2 * delta_lon.cos();
    let bearing = y.atan2(x).to_degrees().rem_euclid(360.0);
    
    Some((distance_km, bearing))
}

/// Calculate distance in km (simplified, no GPS coordinates needed)
fn calculate_distance_display(my_lat: f64, my_lon: f64, target_lat: f64, target_lon: f64) -> String {
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

/// Show a desktop notification for APRS messages
#[cfg(feature = "notifications")]
fn show_aprs_notification(body: &str, _from: &str) -> Result<(), String> {
    if !is_initted() {
        init("pocket-modem").map_err(|e| format!("Failed to init: {:?}", e))?;
    }
    
    let n = Notification::new("APRS", Some(body), None);
    n.show().map_err(|e| format!("Show failed: {:?}", e))
}

/// Format bearing as compass direction with arrow
fn bearing_to_compass(bearing: f64) -> String {
    // Arrow character based on bearing
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

/// GSettings application ID (matches schema path)
const APP_ID: &str = "org.pocketmodem.pocket-modem";

fn main() {
    // Initialize settings (GSettings-backed)
    let settings = SettingsManager::new();
    eprintln!("[pocket-modem] Settings loaded: freq={} kHz, squelch={}", 
              settings.frequency(),
              settings.squelch());
    
    let serial_device = std::env::var("POCKET_MODEM_DEVICE").ok().unwrap_or_else(|| {
        // Auto-detect if no env var set
        if let Ok(entries) = std::fs::read_dir("/dev/serial/by-id") {
            for entry in entries.flatten() {
                if let Some(name) = entry.path().to_str() {
                    if name.contains("10c4") || name.contains("CP2102") || name.contains("Silicon_Labs") {
                        eprintln!("[pocket-modem] Found device: {}", name);
                        return name.to_string();
                    }
                }
            }
        }
        "/dev/ttyUSB0".to_string()
    });

    eprintln!("[pocket-modem] Using: {}", serial_device);

    // Create radio - I/O thread handles serial internally
    let radio = Arc::new(Mutex::new(KV4PRadio::new(SerialConfig {
        port: serial_device.clone(),
        baudrate: 115200,
        timeout_ms: 500,
    })));
    let radio_clone = Arc::clone(&radio);

    // Create GPS manager
    let gps_manager = Arc::new(Mutex::new(GpsManager::new()));
    {
        let gps = gps_manager.lock().unwrap();
        gps.start();
        // GPS auto-detection and enabling happens in the thread
    }

    // Create audio manager with KV4P settings
    let audio_config = AudioConfig {
        sample_rate: 16000,
        tx_gain: 1.0,
        rx_gain: 1.0,
        gate_threshold: 0.001,
        pre_emphasis_alpha: 0.0,
        hard_limit: 0.95,
    };
    let audio_manager = Arc::new(Mutex::new(AudioManager::new(audio_config)));
    
    // Connect audio TX to radio TX
    {
        let mut audio = audio_manager.lock().unwrap();
        let radio = Arc::clone(&radio);
        audio.on_tx_audio(move |adpcm_data| {
            if let Ok(r) = radio.lock() {
                let _ = r.send_audio(adpcm_data);
            }
        });
    }

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    
    // Register empty open handler to prevent GTK from exiting when command line args
    app.connect_open(|_app, _files, _hint| {});
    
    let audio_for_shutdown = Arc::clone(&audio_manager);
    let gps_manager_activate = Arc::clone(&gps_manager);
    let audio_activate = Arc::clone(&audio_manager);
    
    app.connect_activate(move |app| {
        create_ui(app, &radio_clone, &audio_activate, &gps_manager_activate, &settings);
    });

    // Close radio, audio and GPS on shutdown
    let radio_for_shutdown = Arc::clone(&radio);
    let gps_for_shutdown = Arc::clone(&gps_manager);
    app.connect_shutdown(move |_| {
        eprintln!("[pocket-modem] App shutting down...");
        // Stop audio capture/playback
        if let Ok(mut a) = audio_for_shutdown.lock() {
            a.stop_capture();
            a.stop_playback();
        }
        // Close radio connection
        if let Ok(r) = radio_for_shutdown.lock() {
            r.close();
        }
        // Stop GPS polling
        if let Ok(g) = gps_for_shutdown.lock() {
            g.stop();
        }
    });
    
    app.run();
}

fn create_ui(
    app: &adw::Application,
    radio: &Arc<Mutex<KV4PRadio>>,
    audio: &Arc<Mutex<AudioManager>>,
    gps: &Arc<Mutex<GpsManager>>,
    settings: &SettingsManager,
) {
    // Create the main window (360px width for mobile displays)
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(360)
        .default_height(800)
        .title("PocketModem")
        .build();
    
    // Use 360x700 size (Phosh minimum is ~354px wide)
    window.set_size_request(360, 700);
    window.set_resizable(false);
    window.set_default_size(360, 700);
    
    // Apply saved frequency from settings on startup
    let saved_freq = settings.frequency();
    let saved_squelch = settings.squelch();
    eprintln!("[pocket-modem] Restoring: freq={} kHz, squelch={}", saved_freq, saved_squelch);
    
    // Async connection
    let radio_async = Arc::clone(radio);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        
        let connected = {
            if let Ok(r) = radio_async.lock() {
                match r.open() {
                    Ok(version) => {
                        if let Some(v) = version {
                            eprintln!("[pocket-modem] Connected: fw=v{}, rf={:?}", 
                                     v.firmware_version, v.rf_module_type);
                        }
                        true
                    }
                    Err(e) => {
                        eprintln!("[pocket-modem] Connection failed: {}", e);
                        false
                    }
                }
            } else { false }
        };
        
        if connected {
            if let Ok(r) = radio_async.lock() {
                let _ = r.tune_freq(saved_freq, saved_freq);
                let _ = r.set_squelch(saved_squelch);
                let _ = r.open_audio();
            }
        }
    });
    
    // Register radio callbacks
    {
        let r = radio.lock().unwrap();
        let audio_cb = Arc::clone(audio);
        r.on_rx_audio(move |adpcm_data| {
            if let Ok(mut a) = audio_cb.lock() {
                a.accumulate_and_start(adpcm_data);
            }
        });
        
        let radio_ptt = Arc::clone(&radio);
        let audio_ptt = Arc::clone(audio);
        r.on_phys_ptt(move |pressed| {
            if let Ok(r) = radio_ptt.lock() {
                if pressed { let _ = r.ptt_on(); } 
                else { let _ = r.ptt_off(); }
            }
            if let Ok(mut a) = audio_ptt.lock() {
                if pressed {
                    a.stop_playback();
                    let _ = a.start_capture();
                } else {
                    let _ = a.stop_capture();
                    let _ = a.start_playback();
                }
            }
        });
    }
    
    // =========================================================================
    // ViewStack for navigation (created later)
    // =========================================================================
    // Header bar with settings button
    // =========================================================================
    let header_bar = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("PocketModem", ""))
        .build();
    
    // Settings button in header
    let settings_btn = gtk::ToggleButton::new();
    settings_btn.set_icon_name("emblem-system-symbolic");
    settings_btn.add_css_class("flat");
    settings_btn.set_tooltip_text(Some("Settings"));
    header_bar.pack_end(&settings_btn);
    
    // =========================================================================
    // MAIN PAGE
    // =========================================================================
    
    // Clamp content width for proper libadwaita layout
    let clamp = adw::Clamp::builder()
        .maximum_size(360)
        .build();
    clamp.set_size_request(360, 700);
    
    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content_box.set_halign(gtk::Align::Center);
    content_box.set_hexpand(true);
    content_box.set_vexpand(true);
    
    // --- Status indicators ---
    let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 32);
    status_row.set_halign(gtk::Align::Center);
    status_row.set_margin_top(16);
    status_row.set_margin_bottom(16);
    
    // Modem status (clickable)
    let modem_status_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let modem_icon = gtk::Image::from_icon_name("network-wireless-symbolic");
    modem_icon.set_pixel_size(28);
    let modem_label = gtk::Label::new(Some("○"));
    modem_label.add_css_class("status-icon-red");
    let modem_status_label = gtk::Label::new(Some("MODEM"));
    modem_status_label.add_css_class("status-text");
    modem_status_label.add_css_class("modem-label");
    modem_status_box.append(&modem_icon);
    modem_status_box.append(&modem_label);
    modem_status_box.append(&modem_status_label);
    
    // GPS status (clickable)
    let gps_status_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let gps_icon = gtk::Image::from_icon_name("location-services-active-symbolic");
    gps_icon.set_pixel_size(28);
    let gps_led = gtk::Label::new(Some("○"));
    gps_led.add_css_class("gps-led-off");
    let gps_status_label = gtk::Label::new(Some("GPS"));
    gps_status_label.add_css_class("status-text");
    gps_status_label.add_css_class("gps-label");
    gps_status_box.append(&gps_icon);
    gps_status_box.append(&gps_led);
    gps_status_box.append(&gps_status_label);
    
    // Audio status (clickable)
    let audio_status_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let audio_icon = gtk::Image::from_icon_name("audio-volume-medium-symbolic");
    audio_icon.set_pixel_size(28);
    let audio_label = gtk::Label::new(Some("○"));
    audio_label.add_css_class("status-icon-gray");
    let audio_status_label = gtk::Label::new(Some("AUDIO"));
    audio_status_label.add_css_class("status-text");
    audio_status_label.add_css_class("audio-label");
    audio_status_box.append(&audio_icon);
    audio_status_box.append(&audio_label);
    audio_status_box.append(&audio_status_label);
    
    // Add status boxes to status_row
    status_row.append(&modem_status_box);
    status_row.append(&gps_status_box);
    status_row.append(&audio_status_box);
    
    content_box.append(&status_row);
    
    // --- Frequency display ---
    let freq_entry = gtk::Entry::new();
    freq_entry.set_text(&format!("{}.{:03}", saved_freq / 1000, saved_freq % 1000));
    gtk::prelude::EntryExt::set_alignment(&freq_entry, 0.5);
    freq_entry.add_css_class("freq-display");
    freq_entry.set_size_request(340, 100);
    freq_entry.set_margin_start(16);
    freq_entry.set_margin_end(16);
    freq_entry.set_margin_top(8);
    freq_entry.set_margin_bottom(4);
    freq_entry.set_editable(true);
    freq_entry.set_can_focus(true);
    freq_entry.set_input_purpose(gtk::InputPurpose::Number);
    
    let radio_freq = Arc::clone(&radio);
    let settings_for_freq = settings as *const SettingsManager as *mut SettingsManager;
    freq_entry.connect_activate(move |entry| {
        let text = entry.text().to_string();
        
        if let Ok(freq_mhz) = text.parse::<f64>() {
            let khz = (freq_mhz * 1000.0) as u32;
            let radio = Arc::clone(&radio_freq);
            
            // Save frequency to GSettings immediately
            unsafe { (*settings_for_freq).set_frequency(khz); }
            eprintln!("[pocket-modem] Saving freq to GSettings: {} kHz", khz);
            
            std::thread::spawn(move || {
                if let Ok(r) = radio.lock() {
                    if r.set_frequency(khz).is_ok() {
                        eprintln!("[pocket-modem] Frequency tuned to {} kHz", khz);
                    }
                }
            });
            
            entry.set_text(&format!("{}.{:03}", khz / 1000, khz % 1000));
        } else {
            eprintln!("[pocket-modem] Invalid frequency: {}", text);
        }
    });
    content_box.append(&freq_entry);
    
    let freq_entry_for_channel_click = freq_entry.clone();
    let freq_entry_for_add = freq_entry.clone();
    let freq_entry_for_ptt_press = freq_entry.clone();
    let freq_entry_for_ptt_release = freq_entry.clone();
    
    // --- RSSI / S-meter ---
    let rssi_sbar = gtk::ProgressBar::new();
    rssi_sbar.set_fraction(0.0);
    rssi_sbar.add_css_class("rssi-bar");
    
    let smeter_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    smeter_box.set_halign(gtk::Align::Center);
    smeter_box.set_margin_start(24);
    smeter_box.set_margin_end(24);
    smeter_box.set_margin_top(12);
    smeter_box.set_margin_bottom(20);
    smeter_box.set_valign(gtk::Align::Center);
    smeter_box.set_size_request(-1, 20);
    
    let signal_label = gtk::Label::new(Some("SIGNAL"));
    signal_label.add_css_class("signal-text");
    signal_label.set_valign(gtk::Align::Center);
    signal_label.set_width_request(60);
    
    rssi_sbar.set_hexpand(true);
    rssi_sbar.set_valign(gtk::Align::Center);
    
    let signal_value = gtk::Label::new(None);
    signal_value.add_css_class("signal-value");
    signal_value.set_markup(&format!("<span color='#FFB000'>{}</span>", "--%"));
    signal_value.set_valign(gtk::Align::Center);
    signal_value.set_width_request(70);
    
    smeter_box.append(&signal_label);
    smeter_box.append(&rssi_sbar);
    smeter_box.append(&signal_value);
    content_box.append(&smeter_box);
    
    // --- Mode buttons ---
    let mode_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    mode_box.set_homogeneous(true);
    mode_box.set_margin_start(16);
    mode_box.set_margin_end(16);
    mode_box.set_margin_bottom(8);
    mode_box.set_visible(false);  // No mode buttons - APRS always active
    
    let btn_fm = gtk::ToggleButton::with_label("FM");
    btn_fm.add_css_class("mode-btn");
    btn_fm.add_css_class("mode-btn-active");
    
    btn_fm.set_active(true);
    mode_box.append(&btn_fm);
    content_box.append(&mode_box);
    
    // --- Channel list ---
    let channel_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    channel_container.set_margin_start(16);
    channel_container.set_margin_end(16);
    channel_container.set_margin_top(4);
    
    let current_channel_index: Arc<std::sync::atomic::AtomicI32> = Arc::new(std::sync::atomic::AtomicI32::new(-1));
    let current_channel_index_clone = Arc::clone(&current_channel_index);
    
    let channel_header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    channel_header.set_valign(gtk::Align::Center);
    channel_header.set_margin_bottom(8);
    
    let channel_title = gtk::Label::new(Some("<b>Channels</b>"));
    channel_title.set_markup("<b>Channels</b>");
    channel_title.set_halign(gtk::Align::Start);
    channel_title.add_css_class("channel-section-title");
    
    let channel_header_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    channel_header_spacer.set_hexpand(true);
    
    let add_channel_btn = gtk::Button::new();
    add_channel_btn.set_icon_name("list-add-symbolic");
    add_channel_btn.add_css_class("flat");
    add_channel_btn.set_tooltip_text(Some("Add current frequency as new channel"));
    
    channel_header.append(&channel_title);
    channel_header.append(&channel_header_spacer);
    channel_header.append(&add_channel_btn);
    
    let channel_list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    channel_list.add_css_class("channel-list");
    
    let no_channels_row = adw::ActionRow::builder()
        .title("No channels")
        .subtitle("Tap + to add current frequency")
        .build();
    no_channels_row.set_sensitive(false);
    
    let channel_count = unsafe { (*(settings as *const SettingsManager as *mut SettingsManager)).channels().len() };
    no_channels_row.set_visible(channel_count == 0);
    channel_list.append(&no_channels_row);
    
    fn refresh_channel_list(
        channel_list: &Arc<gtk::Box>,
        no_channels_row: &adw::ActionRow,
        settings: *mut SettingsManager,
        radio: &Arc<Mutex<KV4PRadio>>,
        freq_entry: gtk::Entry,
        current_channel_index: Arc<std::sync::atomic::AtomicI32>,
    ) {
        unsafe {
            let channels = (*settings).channels();
            
            while let Some(child) = channel_list.first_child() {
                channel_list.remove(&child);
            }
            
            no_channels_row.set_visible(channels.is_empty());
            channel_list.append(no_channels_row);
            
            for (idx, ch) in channels.iter().enumerate() {
                let row = create_channel_row(
                    ch,
                    idx,
                    settings,
                    channel_list.clone(),
                    no_channels_row.clone(),
                    radio.clone(),
                    freq_entry.clone(),
                    settings,
                    current_channel_index.clone(),
                );
                channel_list.append(&row);
            }
            channel_list.show();
        }
    }
    
    fn create_channel_row(
        channel: &Channel,
        channel_index: usize,
        settings: *mut SettingsManager,
        channel_list: Arc<gtk::Box>,
        no_channels_row: adw::ActionRow,
        radio: Arc<Mutex<KV4PRadio>>,
        freq_entry: gtk::Entry,
        settings_for_channel: *mut SettingsManager,
        current_channel_index: Arc<std::sync::atomic::AtomicI32>,
    ) -> gtk::Box {
        let freq_mhz = channel.rx_freq_khz as f64 / 1000.0;
        
        let mut subtitle_parts = Vec::new();
        
        match channel.duplex {
            Duplex::Plus => subtitle_parts.push(format!("(+{:.3})", channel.offset_khz as f64 / 1000.0)),
            Duplex::Minus => subtitle_parts.push(format!("(-{:.3})", channel.offset_khz as f64 / 1000.0)),
            Duplex::Split => subtitle_parts.push(format!("(split {}.{:.3})", 
                channel.tx_freq_khz.unwrap_or(channel.rx_freq_khz) / 1000,
                channel.tx_freq_khz.unwrap_or(channel.rx_freq_khz) % 1000)),
            Duplex::Simplex => {}
        }
        
        match channel.tone_mode {
            ToneMode::None => {}
            ToneMode::Tone => subtitle_parts.push(format!("[{}]", channel.rtone_hz)),
            ToneMode::Tsql => subtitle_parts.push(format!("[{}, {}]", channel.rtone_hz, channel.ctone_hz)),
        }
        
        let subtitle = if subtitle_parts.is_empty() {
            format!("{:.3} MHz", freq_mhz)
        } else {
            format!("{:.3} MHz {}", freq_mhz, subtitle_parts.join(" "))
        };
        
        let row_container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row_container.set_valign(gtk::Align::Center);
        row_container.add_css_class("channel-row");
        row_container.set_hexpand(true);
        
        if current_channel_index.load(std::sync::atomic::Ordering::SeqCst) == channel_index as i32 {
            row_container.add_css_class("channel-row-selected");
        }
        
        let row = adw::ActionRow::builder()
            .title(&channel.name)
            .subtitle(&subtitle)
            .build();
        row.set_hexpand(true);
        
        let row_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row_spacer.set_hexpand(true);
        
        let edit_btn = gtk::Button::new();
        edit_btn.set_icon_name("document-edit-symbolic");
        edit_btn.add_css_class("flat");
        edit_btn.set_tooltip_text(Some("Edit channel"));
        
        let radio_for_click = Arc::clone(&radio);
        let freq_entry_for_row = freq_entry.clone();
        
        row.add_controller({
            let click = gtk::GestureClick::new();
            click.set_button(1);
            let freq_entry = freq_entry_for_row.clone();
            let radio = radio_for_click.clone();
            let settings = settings_for_channel;
            let ch_freq = channel.rx_freq_khz;
            let ch_tone_mode = match channel.tone_mode {
                ToneMode::None => 0,
                ToneMode::Tone => 1,
                ToneMode::Tsql => 2,
            };
            let ch_ctone = channel.ctone_hz;
            let ch_rtone = channel.rtone_hz;
            let ch_index = channel_index as i32;
            let cc_idx = current_channel_index.clone();
            
            let ch_list = channel_list.clone();
            let no_ch = no_channels_row.clone();
            
            click.connect_pressed(move |_, _, _, _| {
                let prev_idx = cc_idx.swap(ch_index, std::sync::atomic::Ordering::SeqCst);
                
                if prev_idx != ch_index {
                    refresh_channel_list(
                        &ch_list,
                        &no_ch,
                        settings,
                        &radio,
                        freq_entry.clone(),
                        cc_idx.clone(),
                    );
                }
                
                freq_entry.set_text(&format!("{}.{:03}", ch_freq / 1000, ch_freq % 1000));
                
                unsafe { (*settings).set_frequency(ch_freq); }
                
                let r = radio.clone();
                std::thread::spawn(move || {
                    if let Ok(r) = r.lock() {
                        let _ = r.set_frequency_with_ctcss(ch_freq, ch_tone_mode, ch_ctone, ch_rtone);
                    }
                });
            });
            click
        });
        
        let edit_channel = channel.clone();
        let settings_edit = settings;
        let radio_for_edit = Arc::clone(&radio);
        let cc_idx_edit = Arc::clone(&current_channel_index);
        
        edit_btn.connect_clicked(move |btn| {
            let cc_idx_save = cc_idx_edit.clone();
            let cc_idx_delete = cc_idx_edit.clone();
            
            let ch_list_save = channel_list.clone();
            let no_ch_save = no_channels_row.clone();
            let radio_save = radio_for_edit.clone();
            let freq_save = freq_entry.clone();
            
            let ch_list_delete = channel_list.clone();
            let no_ch_delete = no_channels_row.clone();
            let radio_delete = radio_for_edit.clone();
            let freq_delete = freq_entry.clone();
            
            show_channel_edit_dialog(
                btn,
                &edit_channel,
                move |updated| {
                    unsafe {
                        (*settings_edit).update_channel(channel_index, updated.clone());
                        eprintln!("[pocket-modem] Updated channel: {}", updated.name);
                    }
                    
                    let ch = &updated;
                    let ch_freq = ch.rx_freq_khz;
                    let ch_tone_mode = match ch.tone_mode {
                        ToneMode::None => 0,
                        ToneMode::Tone => 1,
                        ToneMode::Tsql => 2,
                    };
                    let ch_ctone = ch.ctone_hz;
                    let ch_rtone = ch.rtone_hz;
                    
                    cc_idx_save.store(channel_index as i32, std::sync::atomic::Ordering::SeqCst);
                    
                    freq_save.set_text(&format!("{}.{:03}", ch_freq / 1000, ch_freq % 1000));
                    
                    let r = radio_save.clone();
                    std::thread::spawn(move || {
                        if let Ok(mut r) = r.lock() {
                            let _ = r.set_frequency_with_ctcss(ch_freq, ch_tone_mode, ch_ctone, ch_rtone);
                        }
                    });
                    
                    refresh_channel_list(
                        &ch_list_save,
                        &no_ch_save,
                        settings_edit,
                        &radio_save,
                        freq_save,
                        cc_idx_save.clone(),
                    );
                },
                move || {
                    unsafe {
                        (*settings_edit).delete_channel(channel_index);
                        eprintln!("[pocket-modem] Deleted channel at index {}", channel_index);
                    }
                    
                    if cc_idx_delete.load(std::sync::atomic::Ordering::SeqCst) == channel_index as i32 {
                        cc_idx_delete.store(-1, std::sync::atomic::Ordering::SeqCst);
                    }
                    
                    refresh_channel_list(
                        &ch_list_delete,
                        &no_ch_delete,
                        settings_edit,
                        &radio_delete,
                        freq_delete,
                        cc_idx_delete.clone(),
                    );
                },
            );
        });
        
        row_container.append(&row);
        row_container.append(&row_spacer);
        row_container.append(&edit_btn);
        row_container
    }
    
    let channel_list_arc = Arc::new(channel_list);
    let no_channels_row_arc = Arc::new(no_channels_row);
    
    let settings_add = settings as *const SettingsManager as *mut SettingsManager;
    let radio_for_add = Arc::clone(radio);
    let radio_for_list = Arc::clone(radio);
    let freq_entry_for_load = freq_entry.clone();
    
    unsafe {
        let existing_channels = (*settings_add).channels();
        for (idx, ch) in existing_channels.iter().enumerate() {
            let row = create_channel_row(
                ch,
                idx,
                settings_add,
                channel_list_arc.clone(),
                (*no_channels_row_arc).clone(),
                radio_for_list.clone(),
                freq_entry_for_load.clone(),
                settings_add,
                current_channel_index_clone.clone(),
            );
            channel_list_arc.append(&row);
        }
        if !(*settings_add).channels().is_empty() {
            no_channels_row_arc.set_visible(false);
        }
    }
    
    let channel_list_add = channel_list_arc.clone();
    let no_channels_row_add = (*no_channels_row_arc).clone();
    let current_channel_index_add = Arc::clone(&current_channel_index);
    
    add_channel_btn.connect_clicked(move |_| {
        unsafe {
            let freq = {
                if let Ok(r) = radio_for_add.lock() {
                    r.state().frequency
                } else {
                    145500
                }
            };
            
            let existing = (*settings_add).channels();
            let next_location = if existing.is_empty() {
                1
            } else {
                existing.iter().map(|c| c.location).max().unwrap_or(0) + 1
            };
            
            let new_channel = Channel {
                location: next_location,
                name: format!("CH{}", next_location),
                rx_freq_khz: freq,
                duplex: Duplex::Simplex,
                offset_khz: 0,
                tx_freq_khz: None,
                tone_mode: ToneMode::None,
                rtone_hz: 88.5,
                ctone_hz: 88.5,
                mode: "FM".to_string(),
                power: PowerLevel::High,
                comment: String::new(),
            };
            
            (*settings_add).add_channel(new_channel.clone());
            let new_index = (*settings_add).channels().len() - 1;
            eprintln!("[pocket-modem] Added channel: location={}, freq={} kHz", next_location, freq);
            
            no_channels_row_add.set_visible(false);
            
            let row = create_channel_row(
                &new_channel,
                new_index,
                settings_add,
                channel_list_add.clone(),
                no_channels_row_add.clone(),
                radio_for_add.clone(),
                freq_entry.clone(),
                settings_add,
                current_channel_index_add.clone(),
            );
            channel_list_add.append(&row);
            channel_list_add.show();
        }
    });
    
    let channel_scroll = gtk::ScrolledWindow::new();
    channel_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    channel_scroll.set_hexpand(true);
    channel_scroll.set_vexpand(true);
    channel_scroll.set_min_content_height(80);
    channel_scroll.set_max_content_height(1000);
    channel_scroll.set_child(Some(&*channel_list_arc));
    
    channel_container.append(&channel_header);
    channel_container.append(&channel_scroll);
    content_box.append(&channel_container);
    
    // --- PTT Button ---
    let ptt_btn = gtk::Button::new();
    ptt_btn.add_css_class("ptt-button");
    ptt_btn.set_valign(gtk::Align::Center);
    
    let ptt_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    ptt_box.set_halign(gtk::Align::Center);
    ptt_box.set_valign(gtk::Align::Center);
    
    let ptt_icon = gtk::Image::from_icon_name("media-record-symbolic");
    ptt_icon.set_pixel_size(40);
    ptt_icon.add_css_class("ptt-icon");
    ptt_icon.set_halign(gtk::Align::Center);
    ptt_icon.set_valign(gtk::Align::Center);
    
    let ptt_label = gtk::Label::new(Some("PTT"));
    ptt_label.add_css_class("ptt-label");
    ptt_label.set_halign(gtk::Align::Center);
    
    ptt_box.append(&ptt_icon);
    ptt_box.append(&ptt_label);
    
    ptt_btn.set_child(Some(&ptt_box));
    ptt_btn.set_tooltip_text(Some("Hold to transmit"));
    ptt_btn.set_hexpand(false);
    ptt_btn.set_margin_start(20);
    ptt_btn.set_margin_end(20);
    ptt_btn.set_margin_bottom(8);
    ptt_btn.set_valign(gtk::Align::End);
    
    let radio_pressed = Arc::clone(radio);
    let audio_pressed = Arc::clone(audio);
    let radio_released = Arc::clone(radio);
    let audio_released = Arc::clone(audio);
    let settings_for_ptt = settings as *const SettingsManager as *mut SettingsManager;
    
    let gesture = gtk::GestureClick::new();
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    gesture.set_button(0);
    
    gesture.connect_pressed({
        let r = radio_pressed.clone();
        let a = audio_pressed.clone();
        let label = ptt_label.clone();
        let freq_entry = freq_entry_for_ptt_press.clone();
        let settings = settings_for_ptt;
        move |_gesture, _n_press, _x, _y| {
            label.set_text("TX");
            
            let (tx_freq, rx_freq) = unsafe {
                let rx_freq = (*settings).frequency();
                let channels = (*settings).channels();
                
                let tx_freq = if let Some(ch) = channels.iter().find(|c| c.rx_freq_khz == rx_freq) {
                    match ch.duplex {
                        Duplex::Simplex => rx_freq,
                        Duplex::Plus => rx_freq + ch.offset_khz,
                        Duplex::Minus => rx_freq.saturating_sub(ch.offset_khz),
                        Duplex::Split => ch.tx_freq_khz.unwrap_or(rx_freq),
                    }
                } else {
                    rx_freq
                };
                (tx_freq, rx_freq)
            };
            
            freq_entry.set_text(&format!("{}.{:03}", tx_freq / 1000, tx_freq % 1000));
            freq_entry.add_css_class("tx-frequency");
            
            if let Ok(mut rad) = r.lock() {
                let _ = rad.set_frequency(tx_freq);
                let _ = rad.ptt_on();
            }
            if let Ok(mut aud) = a.lock() {
                let _ = aud.start_capture();
            }
        }
    });
    
    gesture.connect_released({
        let r = radio_released.clone();
        let a = audio_released.clone();
        let label = ptt_label.clone();
        let freq_entry = freq_entry_for_ptt_release.clone();
        let settings = settings_for_ptt;
        move |_gesture, _n_press, _x, _y| {
            if let Ok(mut aud) = a.lock() {
                let _ = aud.stop_capture();
            }
            
            let rx_freq = unsafe { (*settings).frequency() };
            
            if let Ok(mut rad) = r.lock() {
                let _ = rad.ptt_off();
            }
            
            freq_entry.remove_css_class("tx-frequency");
            freq_entry.set_text(&format!("{}.{:03}", rx_freq / 1000, rx_freq % 1000));
            
            let rad = r.clone();
            std::thread::spawn(move || {
                if let Ok(rad) = rad.lock() {
                    let _ = rad.set_frequency(rx_freq);
                }
            });
            
            label.set_text("PTT");
        }
    });
    
    ptt_btn.add_controller(gesture);
    
    let ptt_spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    ptt_spacer.set_vexpand(false);
    ptt_spacer.set_size_request(-1, 24);
    
    content_box.append(&ptt_spacer);
    content_box.append(&ptt_btn);
    
    clamp.set_child(Some(&content_box));
    
    // =========================================================================
    // APRS PAGE
    // =========================================================================
    
    let aprs_messages: Arc<Mutex<Vec<APRSMessage>>> = Arc::new(Mutex::new(Vec::new()));
    let aprs_messages_clone = Arc::clone(&aprs_messages);
    
    let aprs_last_displayed: Arc<std::sync::atomic::AtomicUsize> = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    
    let aprs_list_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    aprs_list_box.set_margin_start(16);
    aprs_list_box.set_margin_end(16);
    aprs_list_box.set_margin_top(8);
    aprs_list_box.add_css_class("aprs-list");
    
    let aprs_empty_label = gtk::Label::new(Some("No APRS messages\nReceived packets will appear here"));
    aprs_empty_label.set_halign(gtk::Align::Center);
    aprs_empty_label.add_css_class("aprs-empty-text");
    aprs_empty_label.set_visible(true);
    aprs_list_box.append(&aprs_empty_label);
    
    fn add_aprs_message_to_list(
        msg: &APRSMessage,
        list_box: &gtk::Box,
        empty_label: &gtk::Label,
        my_lat: f64,
        my_lon: f64,
    ) {
        empty_label.set_visible(false);
        
        let msg_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        msg_row.set_margin_top(8);
        msg_row.set_margin_bottom(8);
        msg_row.add_css_class("aprs-message-row");
        
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.set_halign(gtk::Align::Start);
        
        let from_label = gtk::Label::new(None);
        from_label.set_markup(&format!("<span color='#FFB000'>{}</span>", msg.from_callsign));
        from_label.add_css_class("aprs-callsign");
        
        let to_label = gtk::Label::new(None);
        to_label.set_markup(&format!("→ <span color='#FFB000'>{}</span>", msg.to_callsign));
        to_label.add_css_class("aprs-to-callsign");
        
        let time_label = gtk::Label::new(None);
        let time_str = if msg.timestamp > 0 {
            let t = msg.timestamp;
            let hours = (t / 3600) % 24;
            let mins = (t / 60) % 60;
            format!("{:02}:{:02}", hours, mins)
        } else {
            "--:--".to_string()
        };
        time_label.set_text(&time_str);
        time_label.add_css_class("aprs-timestamp");
        
        header.append(&from_label);
        header.append(&to_label);
        header.append(&time_label);
        
        let content = gtk::Label::new(None);
        content.set_halign(gtk::Align::Start);
        content.set_valign(gtk::Align::Start);
        content.set_wrap(true);
        content.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        content.set_size_request(280, -1);  // Limit width to clamp size
        
        match msg.msg_type {
            aprs::APRSType::Position => {
                if msg.position_lat != 0.0 || msg.position_lon != 0.0 {
                    // Calculate distance and bearing from our position
                    let dist_bearing = calculate_distance_bearing(my_lat, my_lon, msg.position_lat, msg.position_lon);
                    
                    let (dist_text, bearing_text) = if let Some((dist_km, bearing)) = dist_bearing {
                        let dist_str = if dist_km < 1.0 {
                            format!("{:.0}m", dist_km * 1000.0)
                        } else if dist_km < 10.0 {
                            format!("{:.1}km", dist_km)
                        } else {
                            format!("{:.0}km", dist_km)
                        };
                        let bear_str = bearing_to_compass(bearing);
                        (dist_str, bear_str)
                    } else {
                        ("??".to_string(), "--°".to_string())
                    };
                    
                    let pos_text = format!("{} {}", dist_text, bearing_text);
                    // Escape & for GTK markup
                    let escaped_comment = msg.comment.replace('&', "&amp;");
                    let comment = if escaped_comment.is_empty() {
                        String::new()
                    } else {
                        format!(" - {}", escaped_comment)
                    };
                    content.set_markup(&format!(
                        "<span color='#33D17A'>📍 {}</span>{}",
                        pos_text, comment
                    ));
                } else {
                    content.set_text(&msg.comment);
                    content.add_css_class("aprs-comment");
                }
            }
            aprs::APRSType::Message => {
                let body = msg.msg_body.as_deref().unwrap_or("");
                content.set_markup(&format!(
                    "<span color='#888888'>Message to {}:</span>\n{}",
                    msg.to_callsign_msg.as_deref().unwrap_or(&msg.to_callsign),
                    body
                ));
                content.add_css_class("aprs-message-body");
            }
            aprs::APRSType::Weather => {
                let mut weather_text = String::new();
                if let Some(temp) = msg.temperature {
                    weather_text.push_str(&format!("🌡 {:.1}°C ", temp));
                }
                if let Some(hum) = msg.humidity {
                    weather_text.push_str(&format!("💧 {:.0}% ", hum));
                }
                if let Some(wind) = msg.wind_force {
                    if let Some(dir) = &msg.wind_dir {
                        weather_text.push_str(&format!("💨 {} {} ", wind, dir));
                    } else {
                        weather_text.push_str(&format!("💨 {} km/h ", wind));
                    }
                }
                if weather_text.is_empty() {
                    weather_text = msg.comment.clone();
                }
                content.set_text(&weather_text);
                content.add_css_class("aprs-weather");
            }
            aprs::APRSType::Object => {
                content.set_markup(&format!(
                    "<span color='#888888'>Object:</span> <span color='#FFB000'>{}</span>",
                    msg.obj_name.as_deref().unwrap_or("??")
                ));
            }
            _ => {
                content.set_text(&msg.comment);
                content.add_css_class("aprs-comment");
            }
        }
        
        msg_row.append(&header);
        msg_row.append(&content);
        
        list_box.prepend(&msg_row);
        list_box.show();
    }
    
    let aprs_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    
    let aprs_header = gtk::Label::new(Some("<b>APRS Messages</b>"));
    aprs_header.set_markup("<b>APRS Messages</b>");
    aprs_header.set_halign(gtk::Align::Start);
    aprs_header.set_margin_start(16);
    aprs_header.set_margin_top(16);
    aprs_header.set_margin_bottom(8);
    
    let aprs_scroll = gtk::ScrolledWindow::new();
    aprs_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    aprs_scroll.set_hexpand(true);
    aprs_scroll.set_vexpand(true);
    aprs_scroll.set_min_content_height(200);
    aprs_scroll.set_child(Some(&aprs_list_box));
    
    aprs_page.append(&aprs_header);
    aprs_page.append(&aprs_scroll);
    
    // APRS clamp
    let aprs_clamp = adw::Clamp::builder()
        .maximum_size(360)
        .build();
    aprs_clamp.set_size_request(360, 700);
    aprs_clamp.set_child(Some(&aprs_page));
    
    // =========================================================================
    // MAP PAGE (libshumate)
    // =========================================================================
    eprintln!("[main] Creating MapManager...");
    
    let mut manager = MapManager::new();
    eprintln!("[main] MapManager created");
    
    // Get the view (SimpleMap is refcounted via GObject)
    let map_view = manager.view().clone();
    
    // Wrap manager in Arc for future update capability
    let map_manager = Arc::new(Mutex::new(manager));
    
    // Create map page with GPS header and map view
    let map_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    
    // GPS info header with locator
    let gps_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    gps_header.set_halign(gtk::Align::Center);
    gps_header.set_margin_start(4);
    gps_header.set_margin_end(4);
    gps_header.set_margin_top(4);
    gps_header.set_margin_bottom(4);
    
    let locator_label = gtk::Label::new(Some("--"));
    locator_label.set_markup(&format!("<span color='#FFB000'>MAIDENHEAD: --</span>"));
    locator_label.add_css_class("locator-display");
    
    let coords_label = gtk::Label::new(Some("Lat: -- Lon: --"));
    coords_label.add_css_class("coords-display");
    
    gps_header.append(&locator_label);
    gps_header.append(&coords_label);
    
    // Wrap map in ScrolledWindow for clipping
    let map_scroll = gtk::ScrolledWindow::new();
    map_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
    map_scroll.set_hexpand(false);
    map_scroll.set_vexpand(true);
    map_scroll.set_size_request(360, 600);
    map_scroll.set_child(Some(&map_view));
    
    map_page.append(&gps_header);
    map_page.append(&map_scroll);
    
    // Map clamp
    let map_clamp = adw::Clamp::builder()
        .maximum_size(360)
        .build();
    map_clamp.set_size_request(360, 700);
    map_clamp.set_child(Some(&map_page));
    
    // =========================================================================
    // CAROUSEL (Swipe Navigation)
    // =========================================================================
    let carousel = adw::Carousel::new();
    carousel.set_interactive(true);
    carousel.set_vexpand(true);
    
    // Pages: FM (0), APRS (1), Map (2)
    carousel.append(&clamp);       // Page 0: FM
    carousel.append(&aprs_clamp);  // Page 1: APRS
    carousel.append(&map_clamp);   // Page 2: Map
    
    // Page indicators
    let indicator = adw::CarouselIndicatorDots::new();
    indicator.set_carousel(Some(&carousel));
    indicator.set_halign(gtk::Align::Center);
    indicator.set_margin_bottom(8);
    
    // Carousel wrapper
    let carousel_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    carousel_box.append(&carousel);
    carousel_box.append(&indicator);
    carousel_box.set_vexpand(true);
    
    // Mode buttons hidden - APRS is always active
    btn_fm.set_visible(false);
    mode_box.set_visible(false);
    
    // =========================================================================
    // SETTINGS PAGE
    // =========================================================================
    let settings_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    
    let settings_clamp = adw::Clamp::builder()
        .maximum_size(360)
        .build();
    settings_clamp.set_size_request(360, 700);
    
    let settings_content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    
    let settings_scroll = gtk::ScrolledWindow::new();
    settings_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    settings_scroll.set_hexpand(true);
    settings_scroll.set_vexpand(true);
    
    let settings_box = gtk::Box::new(gtk::Orientation::Vertical, 24);
    settings_box.set_margin_top(12);
    settings_box.set_margin_start(12);
    settings_box.set_margin_end(12);
    settings_box.set_margin_bottom(12);
    
    // === Squelch section ===
    let squelch_group = adw::PreferencesGroup::builder()
        .title("Squelch")
        .build();
    
    let squelch_row = adw::ActionRow::new();
    squelch_row.set_title("Squelch Level");
    
    let squelch_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 8.0, 1.0);
    squelch_scale.set_value(saved_squelch as f64);
    squelch_scale.set_draw_value(false);
    squelch_scale.set_has_origin(true);
    squelch_scale.set_hexpand(true);
    
    let squelch_value_label = gtk::Label::new(Some(&saved_squelch.to_string()));
    squelch_value_label.set_width_request(20);
    squelch_value_label.add_css_class("squelch-value");
    
    let radio_squelch = Arc::clone(radio);
    let settings_clone = settings as *const SettingsManager as *mut SettingsManager;
    let last_sent: Arc<std::sync::atomic::AtomicU8> = Arc::new(std::sync::atomic::AtomicU8::new(saved_squelch));
    let squelch_label_clone = squelch_value_label.clone();
    
    squelch_scale.connect_value_changed(move |scale| {
        let level = scale.value().round() as u8;
        squelch_label_clone.set_text(&format!("{}", level));
        
        if level != last_sent.load(std::sync::atomic::Ordering::SeqCst) {
            unsafe {
                (*settings_clone).set_squelch(level);
            }
            
            let radio_clone = radio_squelch.clone();
            let sent = Arc::clone(&last_sent);
            std::thread::spawn(move || {
                if let Ok(r) = radio_clone.lock() {
                    let _ = r.set_squelch(level);
                }
                sent.store(level, std::sync::atomic::Ordering::SeqCst);
            });
        }
    });
    
    squelch_row.add_suffix(&squelch_scale);
    squelch_row.add_suffix(&squelch_value_label);
    squelch_row.set_activatable_widget(Some(&squelch_scale));
    squelch_group.add(&squelch_row);
    settings_box.append(&squelch_group);
    
    settings_scroll.set_child(Some(&settings_box));
    settings_content.append(&settings_scroll);
    settings_clamp.set_child(Some(&settings_content));
    settings_page.append(&settings_clamp);
    
    // =========================================================================
    // ViewStack for navigation
    // =========================================================================
    let stack = adw::ViewStack::new();
    
    stack.add_titled(&carousel_box, Some("main"), "Main");
    stack.add_titled(&settings_page, Some("settings"), "Settings");
    
    // =========================================================================
    // Navigation - Connect settings button to ViewStack
    // =========================================================================
    let stack_for_toggle = stack.clone();
    settings_btn.connect_toggled(move |btn| {
        if btn.is_active() {
            stack_for_toggle.set_visible_child_name("settings");
        } else {
            stack_for_toggle.set_visible_child_name("main");
        }
    });
    
    // =========================================================================
    // Toast Overlay for notifications
    // =========================================================================
    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&stack));
    
    // =========================================================================
    // Register APRS callback
    // =========================================================================
    {
        let r = radio.lock().unwrap();
        let aprs_msgs = Arc::clone(&aprs_messages);
        
        r.on_aprs(move |msg| {
            let mut msgs = aprs_msgs.lock().unwrap();
            msgs.push(msg.clone());
            if msgs.len() > 100 {
                msgs.remove(0);
            }
            
            eprintln!("[pocket-modem] APRS: {} -> {} ({:?})", 
                      msg.from_callsign, msg.to_callsign, msg.msg_type);
            
            // Build notification text
            let notification_text = match msg.msg_type {
                aprs::APRSType::Position => {
                    if msg.position_lat != 0.0 || msg.position_lon != 0.0 {
                        format!("{}: {}km", msg.from_callsign, 
                                calculate_distance_display(45.496, 9.376, msg.position_lat, msg.position_lon))
                    } else {
                        msg.from_callsign.clone()
                    }
                }
                aprs::APRSType::Message => {
                    format!("{}: {}", msg.from_callsign, 
                            msg.msg_body.as_deref().unwrap_or(""))
                }
                aprs::APRSType::Weather => format!("{}: Weather", msg.from_callsign),
                _ => msg.from_callsign.clone(),
            };
            
            // Truncate long messages
            let notification_text = if notification_text.len() > 50 { 
                format!("{}...", &notification_text[..47]) 
            } else { 
                notification_text 
            };
            
            // Show desktop notification
            #[cfg(feature = "notifications")]
            if let Err(e) = show_aprs_notification(&notification_text, &msg.from_callsign) {
                eprintln!("[pocket-modem] Notification error: {}", e);
            }
        });
    }
    
    // Initialize notifications
    #[cfg(feature = "notifications")]
    {
        if let Err(e) = init("pocket-modem") {
            eprintln!("[pocket-modem] Failed to init notifications: {:?}", e);
        } else {
            eprintln!("[pocket-modem] Notifications initialized");
        }
    }
    
    // Update loop for live status
    let radio_update = Arc::clone(radio);
    let modem_label_clone = modem_label.clone();
    let rssi_sbar_clone = rssi_sbar.clone();
    let signal_value_clone = signal_value.clone();
    let signal_label = signal_label.clone();
    let audio_clone = Arc::clone(audio);
    let audio_label_clone = audio_label.clone();
    let gps_clone = Arc::clone(gps);
    let gps_led_clone = gps_led.clone();
    let ptt_label_update = ptt_label.clone();
    
    let aprs_messages_clone2 = Arc::clone(&aprs_messages);
    let aprs_empty_label_clone = aprs_empty_label.clone();
    let aprs_list_box_clone = aprs_list_box.clone();
    let aprs_last_displayed_clone = Arc::clone(&aprs_last_displayed);
    
    let locator_label_clone = locator_label.clone();
    let coords_label_clone = coords_label.clone();
    // Note: map_manager updates disabled for now to avoid borrow issues
    // let map_manager_clone = Arc::clone(&map_manager);

    glib::timeout_add_local(Duration::from_millis(100), move || {
        if let Ok(r) = radio_update.lock() {
            let state = r.state();
            
            if state.connected && state.raw_rssi > 0 {
                modem_label_clone.set_text("●");
                modem_label_clone.remove_css_class("status-icon-red");
                modem_label_clone.add_css_class("status-icon-green");
            } else if state.connected {
                modem_label_clone.set_text("●");
                modem_label_clone.remove_css_class("status-icon-red");
                modem_label_clone.add_css_class("status-icon-green");
            } else {
                modem_label_clone.set_text("○");
                modem_label_clone.remove_css_class("status-icon-green");
                modem_label_clone.add_css_class("status-icon-red");
            }
            
            if state.connected && state.raw_rssi > 0 {
                if state.tx_active {
                    let pct = (state.raw_rssi as f64) / 255.0 * 100.0;
                    let pct_text = format!("{}%", pct as i32);
                    signal_value_clone.set_markup(&format!("<span color='#FF4444'>{}</span>", pct_text));
                    signal_label.set_text("AUDIO");
                    rssi_sbar_clone.remove_css_class("bar-rx");
                    rssi_sbar_clone.add_css_class("bar-tx");
                } else {
                    let dbm = (state.raw_rssi as f64) * 1.2 - 160.8;
                    let dbm_text = format!("{} dBm", dbm as i32);
                    signal_value_clone.set_markup(&format!("<span color='#FFB000'>{}</span>", dbm_text));
                    signal_label.set_text("SIGNAL");
                    rssi_sbar_clone.remove_css_class("bar-tx");
                    rssi_sbar_clone.add_css_class("bar-rx");
                }
                let frac = (state.raw_rssi as f64) / 255.0;
                rssi_sbar_clone.set_fraction(frac);
                rssi_sbar_clone.remove_css_class("empty");
            } else {
                signal_value_clone.set_markup(&format!("<span color='#888888'>{}</span>", "--"));
                rssi_sbar_clone.set_fraction(0.0);
                rssi_sbar_clone.add_css_class("empty");
                signal_label.set_text("SIGNAL");
                rssi_sbar_clone.remove_css_class("bar-tx");
                rssi_sbar_clone.remove_css_class("bar-rx");
            }
        }
        
        // Update audio status
        {
            let a = audio_clone.lock().unwrap();
            let audio_started = a.audio_started();
            let has_error = a.has_decode_error();
            drop(a);
            
            let squelch_open = if let Ok(r) = radio_update.lock() {
                r.state().squelch_open
            } else {
                false
            };
            
            if has_error {
                audio_label_clone.set_text("○");
                audio_label_clone.remove_css_class("status-icon-green");
                audio_label_clone.remove_css_class("status-icon-gray-filled");
                audio_label_clone.add_css_class("status-icon-red");
            } else if !audio_started {
                audio_label_clone.set_text("○");
                audio_label_clone.remove_css_class("status-icon-green");
                audio_label_clone.remove_css_class("status-icon-red");
                audio_label_clone.remove_css_class("status-icon-gray-filled");
                audio_label_clone.add_css_class("status-icon-gray-empty");
            } else if squelch_open {
                audio_label_clone.set_text("●");
                audio_label_clone.remove_css_class("status-icon-gray-empty");
                audio_label_clone.remove_css_class("status-icon-gray-filled");
                audio_label_clone.remove_css_class("status-icon-red");
                audio_label_clone.add_css_class("status-icon-green");
            } else {
                audio_label_clone.set_text("●");
                audio_label_clone.remove_css_class("status-icon-green");
                audio_label_clone.remove_css_class("status-icon-red");
                audio_label_clone.remove_css_class("status-icon-gray-empty");
                audio_label_clone.add_css_class("status-icon-gray-filled");
            }
        }

        // Update GPS status
        if let Ok(g) = gps_clone.lock() {
            let gps_data = g.get_data();
            
            if gps_data.has_fix {
                gps_led_clone.set_text("●");
                gps_led_clone.remove_css_class("gps-led-off");
                gps_led_clone.remove_css_class("gps-led-searching");
                gps_led_clone.add_css_class("gps-led-on");
            } else if gps_data.gps_enabled {
                gps_led_clone.set_text("●");
                gps_led_clone.remove_css_class("gps-led-off");
                gps_led_clone.remove_css_class("gps-led-on");
                gps_led_clone.add_css_class("gps-led-searching");
            } else {
                gps_led_clone.set_text("●");
                gps_led_clone.remove_css_class("gps-led-on");
                gps_led_clone.remove_css_class("gps-led-searching");
                gps_led_clone.add_css_class("gps-led-off");
            }
        }
        
        // Update APRS UI with distance/bearing from GPS position
        let (my_lat, my_lon) = {
            if let Ok(g) = gps_clone.lock() {
                let gps_data = g.get_data();
                if let (Some(lat), Some(lon)) = (gps_data.latitude, gps_data.longitude) {
                    (lat, lon)
                } else {
                    (0.0, 0.0)
                }
            } else {
                (0.0, 0.0)
            }
        };
        

        
        {
            let msgs = aprs_messages_clone2.lock().unwrap();
            let total_count = msgs.len();
            let last_displayed = aprs_last_displayed_clone.load(std::sync::atomic::Ordering::SeqCst);
            
            if total_count > 0 {
                aprs_empty_label_clone.set_visible(false);
                
                let mut new_last = last_displayed;
                for i in last_displayed..total_count {
                    if let Some(msg) = msgs.get(i) {
                        add_aprs_message_to_list(
                            msg,
                            &aprs_list_box_clone,
                            &aprs_empty_label_clone,
                            my_lat,
                            my_lon,
                        );
                        // Update map with new APRS station
                        // if let Ok(mut map) = map_manager_clone.lock() {
                        //     map.update_station(msg);
                        // }
                        new_last = i + 1;
                    }
                }
                aprs_last_displayed_clone.store(new_last, std::sync::atomic::Ordering::SeqCst);
            }
        }
        
        // Update Map page with GPS data
        if let Ok(g) = gps_clone.lock() {
            let gps_data = g.get_data();
            
            if gps_data.has_fix {
                if let (Some(lat), Some(lon)) = (gps_data.latitude, gps_data.longitude) {
                    let locator = maidenhead_locator(lat, lon);
                    locator_label_clone.set_markup(&format!(
                        "<span color='#FFB000'>MAIDENHEAD: {}</span>",
                        locator
                    ));
                    coords_label_clone.set_text(&format!(
                        "Lat: {:.6}° Lon: {:.6}°",
                        lat, lon
                    ));
                    // Update map with user position
                    // if let Ok(mut map) = map_manager_clone.lock() {
                    //     map.set_user_position(lat, lon);
                    //     map.center_on_user(lat, lon);
                    // }
                } else {
                    locator_label_clone.set_markup(
                        "<span color='#FFB000'>MAIDENHEAD: --- (searching)</span>"
                    );
                    coords_label_clone.set_text("Lat: -- Lon: -- (no fix)");
                }
            } else if gps_data.gps_enabled {
                locator_label_clone.set_markup(
                    "<span color='#FFB000'>MAIDENHEAD: --- (searching)</span>"
                );
                coords_label_clone.set_text("Lat: -- Lon: -- (no fix)");
            } else {
                locator_label_clone.set_markup(
                    "<span color='#888888'>MAIDENHEAD: -- (GPS off)</span>"
                );
                coords_label_clone.set_text("Lat: -- Lon: --");
            }
        }
        
        ptt_label_update.set_text("PTT");
        
        glib::ControlFlow::Continue
    });
    
    // =========================================================================
    // Channel Edit Dialog
    // =========================================================================
    fn show_channel_edit_dialog<F, D>(
        parent: &gtk::Button,
        channel: &Channel,
        on_save: F,
        on_delete: D,
    )
    where
        F: FnOnce(Channel) + 'static,
        D: FnOnce() + 'static,
    {
        let dialog = gtk::Dialog::with_buttons(
            Some(&format!("Edit Channel: {}", channel.name)),
            parent.root().and_then(|r| r.downcast::<gtk::Window>().ok()).as_ref(),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[("Cancel", gtk::ResponseType::Cancel), ("Save", gtk::ResponseType::Accept)],
        );
        
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(12);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.set_margin_bottom(12);
        
        let name_row = adw::ActionRow::new();
        name_row.set_title("Name");
        let name_entry = gtk::Entry::new();
        name_entry.set_text(&channel.name);
        name_entry.set_hexpand(true);
        name_entry.set_input_purpose(gtk::InputPurpose::Alpha);
        name_row.add_suffix(&name_entry);
        name_row.set_activatable_widget(Some(&name_entry));
        content.append(&name_row);
        
        let freq_row = adw::ActionRow::new();
        freq_row.set_title("Frequency (MHz)");
        let freq_entry = gtk::Entry::new();
        freq_entry.set_text(&format!("{:.3}", channel.rx_freq_khz as f64 / 1000.0));
        freq_entry.set_hexpand(true);
        freq_entry.set_input_purpose(gtk::InputPurpose::Number);
        freq_row.add_suffix(&freq_entry);
        freq_row.set_activatable_widget(Some(&freq_entry));
        content.append(&freq_row);
        
        let duplex_row = adw::ActionRow::new();
        duplex_row.set_title("Duplex");
        let duplex_dropdown = gtk::DropDown::from_strings(&["Simplex", "+", "-", "Split"]);
        match channel.duplex {
            Duplex::Simplex => duplex_dropdown.set_selected(0),
            Duplex::Plus => duplex_dropdown.set_selected(1),
            Duplex::Minus => duplex_dropdown.set_selected(2),
            Duplex::Split => duplex_dropdown.set_selected(3),
        }
        duplex_row.add_suffix(&duplex_dropdown);
        duplex_row.set_activatable_widget(Some(&duplex_dropdown));
        content.append(&duplex_row);
        
        let offset_row = adw::ActionRow::new();
        offset_row.set_title("Offset (MHz)");
        let offset_entry = gtk::Entry::new();
        offset_entry.set_text(&format!("{:.3}", channel.offset_khz as f64 / 1000.0));
        offset_entry.set_hexpand(true);
        offset_entry.set_input_purpose(gtk::InputPurpose::Number);
        offset_row.add_suffix(&offset_entry);
        offset_row.set_activatable_widget(Some(&offset_entry));
        content.append(&offset_row);
        
        let tone_row = adw::ActionRow::new();
        tone_row.set_title("Tone Mode");
        let tone_dropdown = gtk::DropDown::from_strings(&["None", "Tone", "TSQL"]);
        match channel.tone_mode {
            ToneMode::None => tone_dropdown.set_selected(0),
            ToneMode::Tone => tone_dropdown.set_selected(1),
            ToneMode::Tsql => tone_dropdown.set_selected(2),
        }
        tone_row.add_suffix(&tone_dropdown);
        tone_row.set_activatable_widget(Some(&tone_dropdown));
        content.append(&tone_row);
        
        let rtone_row = adw::ActionRow::new();
        rtone_row.set_title("RX Tone (Hz)");
        let rtone_entry = gtk::Entry::new();
        rtone_entry.set_text(&format!("{:.1}", channel.rtone_hz));
        rtone_entry.set_hexpand(true);
        rtone_entry.set_input_purpose(gtk::InputPurpose::Number);
        rtone_row.add_suffix(&rtone_entry);
        rtone_row.set_activatable_widget(Some(&rtone_entry));
        content.append(&rtone_row);
        
        let ctone_row = adw::ActionRow::new();
        ctone_row.set_title("TX Tone (Hz)");
        let ctone_entry = gtk::Entry::new();
        ctone_entry.set_text(&format!("{:.1}", channel.ctone_hz));
        ctone_entry.set_hexpand(true);
        ctone_entry.set_input_purpose(gtk::InputPurpose::Number);
        ctone_row.add_suffix(&ctone_entry);
        ctone_row.set_activatable_widget(Some(&ctone_entry));
        content.append(&ctone_row);
        
        let delete_btn = gtk::Button::with_label("Delete Channel");
        delete_btn.add_css_class("destructive-action");
        delete_btn.set_margin_top(12);
        content.append(&delete_btn);
        
        let content_area = dialog.content_area();
        content_area.append(&content);
        
        use std::cell::RefCell;
        let on_delete_opt = RefCell::new(Some(on_delete));
        
        let dialog_for_delete = dialog.clone();
        delete_btn.connect_clicked(move |_| {
            if let Some(callback) = on_delete_opt.borrow_mut().take() {
                callback();
            }
            dialog_for_delete.close();
        });
        
        let on_save_opt = RefCell::new(Some(on_save));
        let channel_clone = channel.clone();
        let name_entry_clone = name_entry.clone();
        let freq_entry_clone = freq_entry.clone();
        let offset_entry_clone = offset_entry.clone();
        let rtone_entry_clone = rtone_entry.clone();
        let ctone_entry_clone = ctone_entry.clone();
        let dialog_for_close = dialog.clone();
        
        dialog.connect_response(move |_d, response| {
            if response == gtk::ResponseType::Accept {
                let name = name_entry_clone.text().to_string();
                let freq_text = freq_entry_clone.text().to_string();
                let offset_text = offset_entry_clone.text().to_string();
                let rtone_text = rtone_entry_clone.text().to_string();
                let ctone_text = ctone_entry_clone.text().to_string();
                
                let freq_mhz: f64 = freq_text.parse().unwrap_or(channel_clone.rx_freq_khz as f64 / 1000.0);
                let rx_freq_khz = (freq_mhz * 1000.0) as u32;
                
                let offset_khz: u32 = {
                    let trimmed = offset_text.trim();
                    if trimmed.is_empty() {
                        600
                    } else if let Ok(mhz) = trimmed.parse::<f64>() {
                        (mhz * 1000.0).round() as u32
                    } else {
                        channel_clone.offset_khz
                    }
                };
                
                let duplex = match duplex_dropdown.selected() {
                    0 => Duplex::Simplex,
                    1 => Duplex::Plus,
                    2 => Duplex::Minus,
                    _ => Duplex::Split,
                };
                
                let tone_mode = match tone_dropdown.selected() {
                    0 => ToneMode::None,
                    1 => ToneMode::Tone,
                    _ => ToneMode::Tsql,
                };
                
                let rtone_hz: f32 = rtone_text.parse().unwrap_or(channel_clone.rtone_hz);
                let ctone_hz: f32 = ctone_text.parse().unwrap_or(channel_clone.ctone_hz);
                
                let updated = Channel {
                    location: channel_clone.location,
                    name,
                    rx_freq_khz,
                    duplex,
                    offset_khz,
                    tx_freq_khz: if duplex == Duplex::Split {
                        Some(rx_freq_khz + offset_khz)
                    } else {
                        None
                    },
                    tone_mode,
                    rtone_hz,
                    ctone_hz,
                    mode: channel_clone.mode.clone(),
                    power: channel_clone.power,
                    comment: channel_clone.comment.clone(),
                };
                
                if let Some(callback) = on_save_opt.borrow_mut().take() {
                    callback(updated);
                }
            }
            dialog_for_close.close();
        });
        
        dialog.show();
    }
    
    // =========================================================================
    // CSS Styling
    // =========================================================================
    let css_provider = gtk::CssProvider::new();
    css_provider.load_from_data(r#"
        .freq-display {
            font-size: 60px;
            font-family: "monospace";
            font-weight: bold;
            color: #FFB000;
            background: #1E1E1E;
            border: 3px solid #444;
            border-radius: 12px;
            text-shadow: 0 0 8px rgba(255, 176, 0, 0.5);
            box-shadow: inset 0 0 16px rgba(0, 0, 0, 0.8);
            caret-color: #FFB000;
        }
        .freq-display:focus { border-color: #FFB000; }
        .tx-frequency {
            color: #ff4444;
            border-color: #ff4444;
            text-shadow: 0 0 8px rgba(255, 68, 68, 0.5);
        }
        .mode-btn {
            font-size: 13px;
            font-weight: bold;
            padding: 8px 12px;
            border-radius: 8px;
            background: #2a2a2a;
            border: 1px solid #444;
            color: #888;
        }
        .mode-btn-active {
            background: #1a2a1a;
            border: 2px solid #33D17A;
            color: #33D17A;
            box-shadow: 0 0 6px rgba(51, 209, 122, 0.3);
        }
        .status-icon-green { font-size: 14px; color: #33D17A; }
        .status-icon-red { font-size: 14px; color: #ff4444; }
        .gps-led-on { color: #33D17A; font-size: 14px; }
        .gps-led-off { color: #666; font-size: 14px; }
        .gps-led-searching { color: #FFB000; font-size: 14px; }
        .status-icon-gray-empty { font-size: 14px; color: #666; }
        .status-icon-gray-filled { font-size: 14px; color: #888; }
        .modem-label { color: #666; font-size: 11px; }
        .gps-label { color: #666; font-size: 11px; }
        .audio-label { color: #666; font-size: 11px; }
        .signal-text { font-size: 11px; color: #888; font-weight: bold; }
        .signal-value { font-size: 11px; font-weight: bold; }
        .rssi-bar { background: #2a2a2a; border: 1px solid #444; border-radius: 4px; }
        .rssi-bar.empty progress { background: transparent; }
        .rssi-bar.empty { opacity: 0.3; }
        .rssi-bar.bar-tx progress { background: #FF4444; }
        .rssi-bar.bar-rx progress { background: #FFB000; }
        .ptt-button { min-width: 90px; min-height: 100px; border-radius: 12px; background: #333; border: 2px solid #555; }
        .ptt-button:hover { background: #3a3a3a; border-color: #666; }
        .ptt-button:active { background: #444; border-color: #FFB000; }
        .ptt-icon { color: #888; }
        .ptt-button:hover .ptt-icon { color: #aaa; }
        .ptt-button:active .ptt-icon { color: #FFB000; }
        .ptt-label { font-size: 11px; font-weight: bold; color: #888; }
        .ptt-button:hover .ptt-label { color: #aaa; }
        .ptt-button:active .ptt-label { color: #FFB000; }
        .squelch-value { font-size: 14px; font-weight: bold; color: #888; }
        .channel-section-title { font-size: 14px; color: #888; }
        .channel-list { background: #2a2a2a; border-radius: 8px; border: 1px solid #444; }
        .channel-row { background: transparent; }
        .channel-row:hover { background: #333; }
        .channel-row-selected { background: #3a3a3a; }
        .channel-row-selected:hover { background: #3a3a3a; }
        .aprs-list { background: #2a2a2a; border-radius: 8px; border: 1px solid #444; }
        .aprs-message-row { background: transparent; border-bottom: 1px solid #333; padding: 8px; }
        .aprs-message-row:hover { background: #333; }
        .aprs-callsign { font-size: 14px; font-weight: bold; }
        .aprs-to-callsign { font-size: 14px; color: #888; }
        .aprs-timestamp { font-size: 12px; color: #666; }
        .aprs-comment { font-size: 13px; color: #888; font-style: italic; }
        .aprs-message-body { font-size: 14px; color: #aaa; }
        .aprs-weather { font-size: 14px; color: #aaa; }
        .aprs-empty-text { font-size: 14px; color: #666; padding: 32px; }
        .map-placeholder-icon { color: #666; }
        .map-placeholder-text { font-size: 14px; color: #888; }
        .locator-display { font-size: 18px; font-family: monospace; }
        .coords-display { font-size: 14px; color: #888; font-family: monospace; }
    "#);
    
    gtk::style_context_add_provider_for_display(
        &gtk::prelude::RootExt::display(&window),
        &css_provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION
    );
    
    let main_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main_container.append(&header_bar);
    main_container.append(&toast_overlay);
    
    window.set_content(Some(&main_container));
    window.show();
}