//! PocketModem libadwaita UI with native KV4P protocol implementation
//!
//! Uses libadwaita patterns for a modern GNOME-style interface:
//! - ViewStack for page navigation
//! - PreferencesGroup + ActionRow for settings
//! - ToastOverlay for notifications
//! - Clamp for content width

mod audio;
mod gps;
mod kiss;
mod radio;
mod settings;

use audio::{AudioConfig, AudioManager};
use gps::GpsManager;
use settings::{SettingsManager, Channel, Duplex, ToneMode, PowerLevel};

use radio::{KV4PRadio, SerialConfig};

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

/// GSettings application ID (matches schema path)
const APP_ID: &str = "org.pocketmodem.pocket-modem";

fn main() {
    // Initialize settings (GSettings-backed)
    let settings = SettingsManager::new();
    eprintln!("[pocket-modem] Settings loaded: freq={} kHz, squelch={}", 
              settings.frequency(),
              settings.squelch());
    
    // Check for test mode arguments
    let args: Vec<String> = std::env::args().collect();
    
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
    // Create the main window
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(360)
        .default_height(800)
        .title("PocketModem")
        .build();
    window.set_size_request(360, -1);
    
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
        .maximum_size(500)
        .build();
    
    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content_box.set_halign(gtk::Align::Center);
    content_box.set_hexpand(true);
    
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
    
    // Sheet content for modem
    let modem_sheet = gtk::Box::new(gtk::Orientation::Vertical, 12);
    modem_sheet.set_margin_top(16);
    modem_sheet.set_margin_start(16);
    modem_sheet.set_margin_end(16);
    modem_sheet.set_margin_bottom(16);
    let modem_title = gtk::Label::new(Some("<b>MODEM</b>"));
    modem_title.set_markup("<b>MODEM</b>");
    modem_title.set_halign(gtk::Align::Center);
    let modem_fw_label = gtk::Label::new(Some("Firmware: --"));
    modem_fw_label.set_halign(gtk::Align::Start);
    let modem_hw_label = gtk::Label::new(Some("Hardware: --"));
    modem_hw_label.set_halign(gtk::Align::Start);
    let modem_rf_label = gtk::Label::new(Some("RF Module: --"));
    modem_rf_label.set_halign(gtk::Align::Start);
    let modem_close_btn = gtk::Button::with_label("Close");
    modem_close_btn.set_halign(gtk::Align::Center);
    modem_sheet.append(&modem_title);
    modem_sheet.append(&modem_fw_label);
    modem_sheet.append(&modem_hw_label);
    modem_sheet.append(&modem_rf_label);
    modem_sheet.append(&modem_close_btn);
    
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
    gps_status_box.set_cursor_from_name(Some("pointer"));
    
    // Sheet content for GPS
    let gps_sheet = gtk::Box::new(gtk::Orientation::Vertical, 12);
    gps_sheet.set_margin_top(16);
    gps_sheet.set_margin_start(16);
    gps_sheet.set_margin_end(16);
    gps_sheet.set_margin_bottom(16);
    let gps_title = gtk::Label::new(Some("<b>GPS</b>"));
    gps_title.set_markup("<b>GPS</b>");
    gps_title.set_halign(gtk::Align::Center);
    let gps_locator_label = gtk::Label::new(Some("Maidenhead: --"));
    gps_locator_label.set_halign(gtk::Align::Start);
    let gps_coords_label = gtk::Label::new(Some("Coordinates: --"));
    gps_coords_label.set_halign(gtk::Align::Start);
    let gps_alt_label = gtk::Label::new(Some("Altitude: --"));
    gps_alt_label.set_halign(gtk::Align::Start);
    let gps_fix_label = gtk::Label::new(Some("Fix Type: --"));
    gps_fix_label.set_halign(gtk::Align::Start);
    let gps_speed_label = gtk::Label::new(Some("Speed: --"));
    gps_speed_label.set_halign(gtk::Align::Start);
    let gps_sats_label = gtk::Label::new(Some("Satellites: --"));
    gps_sats_label.set_halign(gtk::Align::Start);
    let gps_close_btn = gtk::Button::with_label("Close");
    gps_close_btn.set_halign(gtk::Align::Center);
    gps_sheet.append(&gps_title);
    gps_sheet.append(&gps_fix_label);
    gps_sheet.append(&gps_sats_label);
    gps_sheet.append(&gps_locator_label);
    gps_sheet.append(&gps_coords_label);
    gps_sheet.append(&gps_alt_label);
    gps_sheet.append(&gps_speed_label);
    gps_sheet.append(&gps_close_btn);
    
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
    
    // Sheet content for audio
    let audio_sheet = gtk::Box::new(gtk::Orientation::Vertical, 12);
    audio_sheet.set_margin_top(16);
    audio_sheet.set_margin_start(16);
    audio_sheet.set_margin_end(16);
    audio_sheet.set_margin_bottom(16);
    let audio_title = gtk::Label::new(Some("<b>AUDIO</b>"));
    audio_title.set_markup("<b>AUDIO</b>");
    audio_title.set_halign(gtk::Align::Center);
    let audio_codec_label = gtk::Label::new(Some("Codec: ADPCM (IMA WAV)"));
    audio_codec_label.set_halign(gtk::Align::Start);
    let audio_rate_label = gtk::Label::new(Some("Sample Rate: 16 kHz"));
    audio_rate_label.set_halign(gtk::Align::Start);
    let audio_buf_label = gtk::Label::new(Some("Buffer Size: 249 samples"));
    audio_buf_label.set_halign(gtk::Align::Start);
    let audio_latency_label = gtk::Label::new(Some("Latency: ~15 ms"));
    audio_latency_label.set_halign(gtk::Align::Start);
    let audio_close_btn = gtk::Button::with_label("Close");
    audio_close_btn.set_halign(gtk::Align::Center);
    audio_sheet.append(&audio_title);
    audio_sheet.append(&audio_codec_label);
    audio_sheet.append(&audio_rate_label);
    audio_sheet.append(&audio_buf_label);
    audio_sheet.append(&audio_latency_label);
    audio_sheet.append(&audio_close_btn);
    
    // Add status boxes to status_row
    status_row.append(&modem_status_box);
    status_row.append(&gps_status_box);
    status_row.append(&audio_status_box);
    
    // Create sheets overlay (slides up from bottom)
    // Only one sheet shown at a time
    let sheets_stack = gtk::Stack::new();
    sheets_stack.set_valign(gtk::Align::End);
    sheets_stack.add_named(&modem_sheet, Some("modem"));
    sheets_stack.add_named(&gps_sheet, Some("gps"));
    sheets_stack.add_named(&audio_sheet, Some("audio"));
    sheets_stack.set_visible_child_name("modem");
    sheets_stack.set_visible(false);
    
    // Connect close buttons
    sheets_stack.set_visible(false);
    let m = sheets_stack.clone();
    modem_close_btn.connect_clicked(move |_| { m.set_visible(false); });
    let g = sheets_stack.clone();
    gps_close_btn.connect_clicked(move |_| { g.set_visible(false); });
    let a = sheets_stack.clone();
    audio_close_btn.connect_clicked(move |_| { a.set_visible(false); });
    
    // Add click controllers to show sheets
    modem_status_box.add_controller({
        let s = sheets_stack.clone();
        let click = gtk::GestureClick::new();
        click.connect_pressed(move |_, _, _, _| {
            s.set_visible_child_name("modem");
            s.set_visible(true);
        });
        click
    });
    gps_status_box.add_controller({
        let s = sheets_stack.clone();
        let click = gtk::GestureClick::new();
        click.connect_pressed(move |_, _, _, _| {
            s.set_visible_child_name("gps");
            s.set_visible(true);
        });
        click
    });
    audio_status_box.add_controller({
        let s = sheets_stack.clone();
        let click = gtk::GestureClick::new();
        click.connect_pressed(move |_, _, _, _| {
            s.set_visible_child_name("audio");
            s.set_visible(true);
        });
        click
    });
    
    content_box.append(&status_row);
    content_box.append(&sheets_stack);
    
    // --- Frequency display ---
    let freq_entry = gtk::Entry::new();
    freq_entry.set_text(&format!("{}.{:03}", saved_freq / 1000, saved_freq % 1000));
    gtk::prelude::EntryExt::set_alignment(&freq_entry, 0.5);
    freq_entry.add_css_class("freq-display");
    freq_entry.set_size_request(260, 100);
    freq_entry.set_margin_start(16);
    freq_entry.set_margin_end(16);
    freq_entry.set_margin_top(8);
    freq_entry.set_margin_bottom(4);
    freq_entry.set_editable(true);
    freq_entry.set_can_focus(true);
    
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
    
    // Clone freq_entry for use in callbacks (before closures move it)
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
    
    rssi_sbar.set_hexpand(true);
    rssi_sbar.set_valign(gtk::Align::Center);
    
    let signal_value = gtk::Label::new(None);
    signal_value.add_css_class("signal-value");
    signal_value.set_markup(&format!("<span color='#FFB000'>{}</span>", "-- dBm"));
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
    mode_box.set_margin_bottom(16);
    
    let btn_fm = gtk::ToggleButton::with_label("FM");
    btn_fm.add_css_class("mode-btn");
    btn_fm.add_css_class("mode-btn-active");
    
    let btn_rade = gtk::ToggleButton::with_label("RADE");
    btn_rade.add_css_class("mode-btn");
    btn_rade.set_sensitive(false);
    
    let btn_m17 = gtk::ToggleButton::with_label("M17");
    btn_m17.add_css_class("mode-btn");
    btn_m17.set_sensitive(false);
    
    btn_fm.set_active(true);
    mode_box.append(&btn_fm);
    mode_box.append(&btn_rade);
    mode_box.append(&btn_m17);
    content_box.append(&mode_box);
    
    // --- Channel list ---
    // Create a custom container for channels with title + add button header
    let channel_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    channel_container.set_margin_start(16);
    channel_container.set_margin_end(16);
    channel_container.set_margin_top(8);
    
    // Header row with "Channels" title and + button
    let channel_header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    channel_header.set_valign(gtk::Align::Center);
    channel_header.set_margin_bottom(8);
    
    let channel_title = gtk::Label::new(Some("<b>Channels</b>"));
    channel_title.set_markup("<b>Channels</b>");
    channel_title.set_halign(gtk::Align::Start);
    channel_title.add_css_class("channel-section-title");
    
    // Spacer to push button to right
    let channel_header_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    channel_header_spacer.set_hexpand(true);
    
    // Add button aligned to right
    let add_channel_btn = gtk::Button::new();
    add_channel_btn.set_icon_name("list-add-symbolic");
    add_channel_btn.add_css_class("flat");
    add_channel_btn.set_tooltip_text(Some("Add current frequency as new channel"));
    
    channel_header.append(&channel_title);
    channel_header.append(&channel_header_spacer);
    channel_header.append(&add_channel_btn);
    
    // Channel list container (where channel rows will be added)
    let channel_list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    channel_list.add_css_class("channel-list");
    
    // "No channels" placeholder
    let no_channels_row = adw::ActionRow::builder()
        .title("No channels")
        .subtitle("Tap + to add current frequency")
        .build();
    no_channels_row.set_sensitive(false);
    
    // Show/hide based on channel count
    let channel_count = unsafe { (*(settings as *const SettingsManager as *mut SettingsManager)).channels().len() };
    no_channels_row.set_visible(channel_count == 0);
    channel_list.append(&no_channels_row);
    
    // Function to refresh channel list - takes Arc-wrapped Box, cloned ActionRow
    fn refresh_channel_list(
        channel_list: &Arc<gtk::Box>,
        no_channels_row: &adw::ActionRow,
        settings: *mut SettingsManager,
        radio: &Arc<Mutex<KV4PRadio>>,
        freq_entry: gtk::Entry,
    ) {
        unsafe {
            let channels = (*settings).channels();
            
            // Clear existing rows (keep "No channels" placeholder)
            while let Some(child) = channel_list.first_child() {
                channel_list.remove(&child);
            }
            
            // Show/hide "No channels" placeholder
            no_channels_row.set_visible(channels.is_empty());
            channel_list.append(no_channels_row);
            
            // Add channel rows
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
                );
                channel_list.append(&row);
            }
            channel_list.show();
        }
    }
    
    // Function to create a channel row with edit button
    fn create_channel_row(
        channel: &Channel,
        channel_index: usize,
        settings: *mut SettingsManager,
        channel_list: Arc<gtk::Box>,
        no_channels_row: adw::ActionRow,
        radio: Arc<Mutex<KV4PRadio>>,
        freq_entry: gtk::Entry,
        settings_for_channel: *mut SettingsManager,
    ) -> gtk::Box {
        let freq_mhz = channel.rx_freq_khz as f64 / 1000.0;
        
        // Container for the row with edit button
        let row_container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row_container.set_valign(gtk::Align::Center);
        row_container.add_css_class("channel-row");
        row_container.set_hexpand(true);
        
        // Action row for channel - takes remaining space
        let row = adw::ActionRow::builder()
            .title(&channel.name)
            .subtitle(&format!("{:.3} MHz", freq_mhz))
            .build();
        row.set_hexpand(true);
        
        // Spacer to push edit button to the right
        let row_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row_spacer.set_hexpand(true);
        
        // Edit button
        let edit_btn = gtk::Button::new();
        edit_btn.set_icon_name("document-edit-symbolic");
        edit_btn.add_css_class("flat");
        edit_btn.set_tooltip_text(Some("Edit channel"));
        
        let channel_freq = channel.rx_freq_khz;
        let radio_for_click = Arc::clone(&radio);
        let freq_entry_for_row = freq_entry.clone();
        
        // Handle click on the row
        row.add_controller({
            let click = gtk::GestureClick::new();
            click.set_button(1);
            let freq_entry = freq_entry_for_row.clone();
            let radio = radio_for_click.clone();
            let settings = settings_for_channel;
            let ch_freq = channel.rx_freq_khz;
            let ch_name = channel.name.clone();
            let ch_tone_mode = match channel.tone_mode {
                ToneMode::None => 0,
                ToneMode::Tone => 1,
                ToneMode::Tsql => 2,
            };
            let ch_ctone = channel.ctone_hz;  // TX CTCSS
            let ch_rtone = channel.rtone_hz;  // RX CTCSS
            click.connect_pressed(move |_, _, _, _| {
                let tone_mode_str = match ch_tone_mode {
                    0 => "None",
                    1 => "Tone",
                    2 => "TSQL",
                    _ => "Unknown",
                };
                eprintln!("[channel] ==========");
                eprintln!("[channel] SELECT: {}", ch_name);
                eprintln!("[channel] freq: {} MHz", ch_freq as f64 / 1000.0);
                eprintln!("[channel] tone_mode: {} ({})", ch_tone_mode, tone_mode_str);
                eprintln!("[channel] TX tone (ctone): {} Hz", ch_ctone);
                eprintln!("[channel] RX tone (rtone): {} Hz", ch_rtone);
                
                // Update freq entry UI on main thread
                freq_entry.set_text(&format!("{}.{:03}", ch_freq / 1000, ch_freq % 1000));
                
                // Save to settings
                unsafe { (*settings).set_frequency(ch_freq); }
                
                // Tune radio with CTCSS from channel
                let r = radio.clone();
                std::thread::spawn(move || {
                    if let Ok(r) = r.lock() {
                        let _ = r.set_frequency_with_ctcss(ch_freq, ch_tone_mode, ch_ctone, ch_rtone);
                    }
                });
            });
            click
        });
        
        // Edit button callback - show edit dialog
        let edit_channel = channel.clone();
        let settings_edit = settings;
        let radio_for_edit = Arc::clone(&radio);
        
        edit_btn.connect_clicked(move |btn| {
            // Clone for each callback
            let ch_list_save = channel_list.clone();
            let no_ch_save = no_channels_row.clone();
            let radio_save = radio_for_edit.clone();
            let freq_save = freq_entry.clone();
            
            let ch_list_delete = channel_list.clone();
            let no_ch_delete = no_channels_row.clone();
            let radio_delete = radio_for_edit.clone();
            let freq_delete = freq_entry.clone();
            
            // Show a simple edit dialog with FnOnce callbacks
            show_channel_edit_dialog(
                btn,
                &edit_channel,
                move |updated| {
                    // Update channel in settings
                    unsafe {
                        (*settings_edit).update_channel(channel_index, updated.clone());
                        eprintln!("[pocket-modem] Updated channel: {}", updated.name);
                    }
                    
                    // Refresh the channel list
                    refresh_channel_list(
                        &ch_list_save,
                        &no_ch_save,
                        settings_edit,
                        &radio_save,
                        freq_save,
                    );
                },
                move || {
                    // Delete callback
                    unsafe {
                        (*settings_edit).delete_channel(channel_index);
                        eprintln!("[pocket-modem] Deleted channel at index {}", channel_index);
                    }
                    
                    // Refresh the channel list
                    refresh_channel_list(
                        &ch_list_delete,
                        &no_ch_delete,
                        settings_edit,
                        &radio_delete,
                        freq_delete,
                    );
                },
            );
        });
        
        row_container.append(&row);
        row_container.append(&row_spacer);
        row_container.append(&edit_btn);
        row_container
    }
    
    // Wrap in Arc for ownership
    let channel_list_arc = Arc::new(channel_list);
    let no_channels_row_arc = Arc::new(no_channels_row);
    
    // Clone everything needed for the add callback
    let settings_add = settings as *const SettingsManager as *mut SettingsManager;
    let radio_for_add = Arc::clone(&radio);
    let radio_for_list = Arc::clone(&radio);
    let freq_entry_for_load = freq_entry.clone();
    
    // Load existing channels into the UI
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
            );
            channel_list_arc.append(&row);
        }
        // Hide "No channels" if we have channels
        if !(*settings_add).channels().is_empty() {
            no_channels_row_arc.set_visible(false);
        }
    }
    
    // Add button callback
    let channel_list_add = channel_list_arc.clone();
    let no_channels_row_add = (*no_channels_row_arc).clone();
    
    add_channel_btn.connect_clicked(move |_| {
        unsafe {
            let freq = {
                if let Ok(r) = radio_for_add.lock() {
                    r.state().frequency
                } else {
                    145500  // Default
                }
            };
            
            // Find next available location number
            let existing = (*settings_add).channels();
            let next_location = if existing.is_empty() {
                1
            } else {
                existing.iter().map(|c| c.location).max().unwrap_or(0) + 1
            };
            
            // Create new channel with current frequency
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
            
            // Add to settings
            (*settings_add).add_channel(new_channel.clone());
            let new_index = (*settings_add).channels().len() - 1;
            eprintln!("[pocket-modem] Added channel: location={}, freq={} kHz", next_location, freq);
            
            // Hide "No channels" message
            no_channels_row_add.set_visible(false);
            
            // Create and append a UI row for the new channel
            let row = create_channel_row(
                &new_channel,
                new_index,
                settings_add,
                channel_list_add.clone(),
                no_channels_row_add.clone(),
                radio_for_add.clone(),
                freq_entry.clone(),
                settings_add,
            );
            channel_list_add.append(&row);
            channel_list_add.show();
        }
    });
    
    // Put channel list in a scrolled window
    let channel_scroll = gtk::ScrolledWindow::new();
    channel_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    channel_scroll.set_hexpand(true);
    channel_scroll.set_vexpand(true);  // Takes all available space before PTT
    channel_scroll.set_min_content_height(80);
    // Max height ensures PTT button is always visible
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
    
    // PTT using GestureClick
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
            
            // Get current RX frequency and calculate TX frequency based on duplex
            let (tx_freq, rx_freq) = unsafe {
                let rx_freq = (*settings).frequency();
                let channels = (*settings).channels();
                
                // Find if current frequency matches a channel
                let tx_freq = if let Some(ch) = channels.iter().find(|c| c.rx_freq_khz == rx_freq) {
                    match ch.duplex {
                        Duplex::Simplex => rx_freq,
                        Duplex::Plus => rx_freq + ch.offset_khz,
                        Duplex::Minus => rx_freq.saturating_sub(ch.offset_khz),
                        Duplex::Split => ch.tx_freq_khz.unwrap_or(rx_freq),
                    }
                } else {
                    // Default: simplex (no offset)
                    rx_freq
                };
                (tx_freq, rx_freq)
            };
            
            // Update VFO to show TX frequency (red)
            freq_entry.set_text(&format!("{}.{:03}", tx_freq / 1000, tx_freq % 1000));
            freq_entry.add_css_class("tx-frequency");
            
            // Tune radio to TX frequency and PTT on
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
            
            // Get RX frequency for restoration
            let rx_freq = unsafe { (*settings).frequency() };
            
            // PTT off
            if let Ok(mut rad) = r.lock() {
                let _ = rad.ptt_off();
            }
            
            // Restore VFO to RX frequency (normal color)
            freq_entry.remove_css_class("tx-frequency");
            freq_entry.set_text(&format!("{}.{:03}", rx_freq / 1000, rx_freq % 1000));
            
            // Tune radio back to RX frequency
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
    
    // Small margin above PTT
    let ptt_spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    ptt_spacer.set_vexpand(false);
    ptt_spacer.set_size_request(-1, 8);
    
    content_box.append(&ptt_spacer);
    content_box.append(&ptt_btn);
    
    clamp.set_child(Some(&content_box));
    
    // =========================================================================
    // ViewStack for navigation
    // =========================================================================
    let stack = adw::ViewStack::new();
    
    // =========================================================================
    // SETTINGS PAGE (no header bar - back via toggle button)
    // =========================================================================
    let settings_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    
    let settings_clamp = adw::Clamp::builder()
        .maximum_size(500)
        .build();
    
    let settings_content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    
    // Scroll view for settings
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
    // Note: lower values = more permissive (opens easier), higher = stricter
    // Default is 4 (mid-range)
    squelch_scale.set_draw_value(false);
    squelch_scale.set_has_origin(true);
    squelch_scale.set_hexpand(true);
    
    let squelch_value_label = gtk::Label::new(Some(&saved_squelch.to_string()));
    squelch_value_label.set_width_request(20);
    squelch_value_label.add_css_class("squelch-value");
    
    // Squelch callback
    let radio_squelch = Arc::clone(radio);
    let settings_clone = settings as *const SettingsManager as *mut SettingsManager;
    let last_sent: Arc<std::sync::atomic::AtomicU8> = Arc::new(std::sync::atomic::AtomicU8::new(saved_squelch));
    let squelch_label_clone = squelch_value_label.clone();
    
    squelch_scale.connect_value_changed(move |scale| {
        let level = scale.value().round() as u8;
        squelch_label_clone.set_text(&format!("{}", level));
        
        if level != last_sent.load(std::sync::atomic::Ordering::SeqCst) {
            // Settings on main thread
            unsafe {
                (*settings_clone).set_squelch(level);
            }
            
            // Radio on spawned thread
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
    
    // === Audio Filters section ===
    let filters_group = adw::PreferencesGroup::builder()
        .title("Audio Filters")
        .build();
    
    // Pre-emphasis
    let pre_emph_row = adw::ActionRow::new();
    pre_emph_row.set_title("Pre-emphasis (TX)");
    let pre_emph_switch = gtk::Switch::new();
    pre_emph_switch.set_valign(gtk::Align::Center);
    pre_emph_switch.set_active(false);
    pre_emph_row.add_suffix(&pre_emph_switch);
    pre_emph_row.set_activatable_widget(Some(&pre_emph_switch));
    
    let radio_pre_emph = Arc::clone(radio);
    pre_emph_switch.connect_state_set(move |_sw, state| {
        unsafe { (*settings_clone).set_pre_emphasis(state); }
        let radio_clone = radio_pre_emph.clone();
        std::thread::spawn(move || {
            if let Ok(r) = radio_clone.lock() {
                let _ = r.set_filter_pre_emphasis(state);
            }
        });
        glib::Propagation::Proceed
    });
    filters_group.add(&pre_emph_row);
    
    // De-emphasis
    let de_emph_row = adw::ActionRow::new();
    de_emph_row.set_title("De-emphasis (RX)");
    let de_emph_switch = gtk::Switch::new();
    de_emph_switch.set_valign(gtk::Align::Center);
    de_emph_switch.set_active(false);
    de_emph_row.add_suffix(&de_emph_switch);
    de_emph_row.set_activatable_widget(Some(&de_emph_switch));
    
    let radio_de_emph = Arc::clone(radio);
    de_emph_switch.connect_state_set(move |_sw, state| {
        unsafe { (*settings_clone).set_de_emphasis(state); }
        let radio_clone = radio_de_emph.clone();
        std::thread::spawn(move || {
            if let Ok(r) = radio_clone.lock() {
                let _ = r.set_filter_de_emphasis(state);
            }
        });
        glib::Propagation::Proceed
    });
    filters_group.add(&de_emph_row);
    
    // High-pass filter
    let hp_row = adw::ActionRow::new();
    hp_row.set_title("High-pass Filter");
    let hp_switch = gtk::Switch::new();
    hp_switch.set_valign(gtk::Align::Center);
    hp_switch.set_active(true);
    hp_row.add_suffix(&hp_switch);
    hp_row.set_activatable_widget(Some(&hp_switch));
    
    let radio_hp = Arc::clone(radio);
    hp_switch.connect_state_set(move |_sw, state| {
        unsafe { (*settings_clone).set_high_pass_filter(state); }
        let radio_clone = radio_hp.clone();
        std::thread::spawn(move || {
            if let Ok(r) = radio_clone.lock() {
                let _ = r.set_filter_high_pass(state);
            }
        });
        glib::Propagation::Proceed
    });
    filters_group.add(&hp_row);
    
    // Low-pass filter
    let lp_row = adw::ActionRow::new();
    lp_row.set_title("Low-pass Filter");
    let lp_switch = gtk::Switch::new();
    lp_switch.set_valign(gtk::Align::Center);
    lp_switch.set_active(true);
    lp_row.add_suffix(&lp_switch);
    lp_row.set_activatable_widget(Some(&lp_switch));
    
    let radio_lp = Arc::clone(radio);
    lp_switch.connect_state_set(move |_sw, state| {
        unsafe { (*settings_clone).set_low_pass_filter(state); }
        let radio_clone = radio_lp.clone();
        std::thread::spawn(move || {
            if let Ok(r) = radio_clone.lock() {
                let _ = r.set_filter_low_pass(state);
            }
        });
        glib::Propagation::Proceed
    });
    filters_group.add(&lp_row);
    
    settings_box.append(&filters_group);
    
    // === TX Power section ===
    let tx_power_group = adw::PreferencesGroup::builder()
        .title("TX Power")
        .build();
    
    let tx_power_row = adw::ActionRow::new();
    tx_power_row.set_title("TX Power Level");
    
    let tx_power_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    tx_power_box.add_css_class("linked");
    tx_power_box.set_margin_top(6);
    tx_power_box.set_margin_bottom(6);
    
    let btn_low = gtk::ToggleButton::with_label("Low");
    let btn_high = gtk::ToggleButton::with_label("High");
    btn_high.set_active(true);
    
    let radio_tx = Arc::clone(radio);
    let radio_tx2 = Arc::clone(&radio_tx);
    let btn_low_weak = btn_low.clone();
    let btn_high_weak = btn_high.clone();
    
    btn_low.connect_clicked(move |_| {
        btn_low_weak.set_active(true);
        btn_high_weak.set_active(false);
        let radio_clone = radio_tx.clone();
        std::thread::spawn(move || {
            if let Ok(r) = radio_clone.lock() {
                let _ = r.set_tx_power(false);
            }
        });
    });
    
    let btn_low_weak2 = btn_low.clone();
    let btn_high_weak2 = btn_high.clone();
    
    btn_high.connect_clicked(move |_| {
        btn_low_weak2.set_active(false);
        btn_high_weak2.set_active(true);
        let radio_clone = radio_tx2.clone();
        std::thread::spawn(move || {
            if let Ok(r) = radio_clone.lock() {
                let _ = r.set_tx_power(true);
            }
        });
    });
    
    tx_power_box.append(&btn_low);
    tx_power_box.append(&btn_high);
    tx_power_row.add_suffix(&tx_power_box);
    tx_power_row.set_activatable_widget(Some(&tx_power_box));
    tx_power_group.add(&tx_power_row);
    settings_box.append(&tx_power_group);
    
    // === Mic Gain section ===
    let mic_group = adw::PreferencesGroup::builder()
        .title("Mic Gain")
        .build();
    
    let mic_row = adw::ActionRow::new();
    mic_row.set_title("Mic Gain Boost");
    let mic_dropdown_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    mic_dropdown_box.set_margin_top(6);
    mic_dropdown_box.set_margin_bottom(6);
    let mic_dropdown = gtk::DropDown::from_strings(&[
        "None", "Low", "Med", "High",
    ]);
    mic_dropdown_box.append(&mic_dropdown);
    mic_row.add_suffix(&mic_dropdown_box);
    mic_row.set_activatable_widget(Some(&mic_dropdown_box));
    
    let radio_mic = Arc::clone(radio);
    mic_dropdown.connect_selected_notify(move |dd| {
        let idx = dd.selected();
        let level = match idx {
            0 => "none",
            1 => "low", 
            2 => "med",
            _ => "high",
        };
        unsafe { (*settings_clone).set_mic_gain(level); }
        let radio_clone = radio_mic.clone();
        std::thread::spawn(move || {
            if let Ok(r) = radio_clone.lock() {
                let _ = r.set_mic_gain(level);
            }
        });
    });
    mic_group.add(&mic_row);
    settings_box.append(&mic_group);
    
    // === Channels section ===
    let channels_group = adw::PreferencesGroup::builder()
        .title("Channels")
        .build();
    
    // Import CSV row
    let import_row = adw::ActionRow::new();
    import_row.set_title("Import from CSV");
    import_row.set_subtitle("Import channels from CHIRP CSV file");
    
    let import_icon = gtk::Image::from_icon_name("document-open-symbolic");
    let import_btn = gtk::Button::new();
    import_btn.set_child(Some(&import_icon));
    import_btn.add_css_class("flat");
    import_row.set_activatable_widget(Some(&import_btn));
    import_row.add_suffix(&import_btn);
    
    // Export CSV row
    let export_row = adw::ActionRow::new();
    export_row.set_title("Export to CSV");
    export_row.set_subtitle("Export channels to CHIRP CSV file");
    
    let export_icon = gtk::Image::from_icon_name("document-save-as-symbolic");
    let export_btn = gtk::Button::new();
    export_btn.set_child(Some(&export_icon));
    export_btn.add_css_class("flat");
    export_row.set_activatable_widget(Some(&export_btn));
    export_row.add_suffix(&export_btn);
    
    let settings_for_import = settings_clone;
    import_btn.connect_clicked(move |_| {
        let dialog = gtk::FileChooserDialog::new(
            Some("Import Channels from CSV"),
            None::<&gtk::Window>,
            gtk::FileChooserAction::Open,
            &[
                ("Cancel", gtk::ResponseType::Cancel),
                ("Import", gtk::ResponseType::Accept),
            ],
        );
        
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("CSV Files"));
        filter.add_pattern("*.csv");
        filter.add_mime_type("text/csv");
        dialog.add_filter(&filter);
        
        let settings = settings_for_import;
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                if let Some(file) = dialog.file() {
                    if let Some(path) = file.path() {
                        eprintln!("[pocket-modem] Importing channels from: {:?}", path);
                        let result = unsafe { (*settings).import_csv(&path) };
                        match result {
                            Ok(channels) => eprintln!("[pocket-modem] Imported {} channels", channels.len()),
                            Err(e) => eprintln!("[pocket-modem] Import failed: {}", e),
                        }
                    }
                }
            }
            dialog.close();
        });
        
        dialog.show();
    });
    
    let settings_for_export = settings_clone;
    export_btn.connect_clicked(move |_| {
        let dialog = gtk::FileChooserDialog::new(
            Some("Export Channels to CSV"),
            None::<&gtk::Window>,
            gtk::FileChooserAction::Save,
            &[
                ("Cancel", gtk::ResponseType::Cancel),
                ("Export", gtk::ResponseType::Accept),
            ],
        );
        
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("CSV Files"));
        filter.add_pattern("*.csv");
        filter.add_mime_type("text/csv");
        dialog.add_filter(&filter);
        
        dialog.set_current_name("pocket-modem-channels.csv");
        
        let settings = settings_for_export;
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                if let Some(file) = dialog.file() {
                    if let Some(path) = file.path() {
                        eprintln!("[pocket-modem] Exporting channels to: {:?}", path);
                        let result = unsafe { (*settings).export_csv(&path) };
                        match result {
                            Ok(_) => eprintln!("[pocket-modem] Exported channels successfully"),
                            Err(e) => eprintln!("[pocket-modem] Export failed: {}", e),
                        }
                    }
                }
            }
            dialog.close();
        });
        
        dialog.show();
    });
    
    channels_group.add(&import_row);
    channels_group.add(&export_row);
    settings_box.append(&channels_group);
    
    // === Reset to Defaults ===
    let reset_btn = gtk::Button::with_label("Reset to Defaults");
    reset_btn.add_css_class("destructive-action");
    reset_btn.set_margin_top(12);
    
    let settings_for_reset = settings_clone;
    let squelch_scale_for_reset = squelch_scale.clone();
    reset_btn.connect_clicked(move |_| {
        unsafe {
            (*settings_for_reset).reset_to_defaults();
        }
        squelch_scale_for_reset.set_value(4.0);
        eprintln!("[pocket-modem] Settings reset to defaults");
    });
    settings_box.append(&reset_btn);
    
    settings_scroll.set_child(Some(&settings_box));
    settings_content.append(&settings_scroll);
    settings_clamp.set_child(Some(&settings_content));
    settings_page.append(&settings_clamp);
    
    // =========================================================================
    // Navigation - Connect settings button to ViewStack
    // =========================================================================
    stack.add_titled(&clamp, Some("main"), "Main");
    stack.add_titled(&settings_page, Some("settings"), "Settings");
    
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
    
    // Update loop for live status
    let radio_update = Arc::clone(radio);
    let modem_label_clone = modem_label.clone();
    let rssi_sbar_clone = rssi_sbar.clone();
    let signal_value_clone = signal_value.clone();
    let audio_clone = Arc::clone(audio);
    let audio_label_clone = audio_label.clone();
    let gps_clone = Arc::clone(gps);
    let gps_led_clone = gps_led.clone();
    let ptt_label_update = ptt_label.clone();
    let settings_channels = settings as *const SettingsManager as *mut SettingsManager;
    
    // Bottom sheet labels
    let modem_fw_label_clone = modem_fw_label.clone();
    let modem_hw_label_clone = modem_hw_label.clone();
    let modem_rf_label_clone = modem_rf_label.clone();
    let gps_locator_label_clone = gps_locator_label.clone();
    let gps_coords_label_clone = gps_coords_label.clone();
    let gps_alt_label_clone = gps_alt_label.clone();
    let gps_fix_label_clone = gps_fix_label.clone();
    let gps_speed_label_clone = gps_speed_label.clone();
    let gps_sats_label_clone = gps_sats_label.clone();

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
            
            let dbm = (state.raw_rssi as f64) * 1.2 - 160.8;
            
            if state.connected && state.raw_rssi > 0 {
                let dbm_text = format!("{} dBm", dbm as i32);
                signal_value_clone.set_markup(&format!("<span color='#FFB000'>{}</span>", dbm_text));
                let frac = ((dbm + 120.0) / 90.0).clamp(0.0, 1.0);
                rssi_sbar_clone.set_fraction(frac);
            } else {
                signal_value_clone.set_markup(&format!("<span color='#FFB000'>{}</span>", "-- dBm"));
                rssi_sbar_clone.set_fraction(0.0);
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

        // Update GPS status (LED only, no coordinates display)
        if let Ok(g) = gps_clone.lock() {
            let gps_data = g.get_data();
            
            // Three states: GPS disabled (gray), enabled but no fix (yellow), has fix (green)
            if gps_data.has_fix {
                gps_led_clone.set_text("●");
                gps_led_clone.remove_css_class("gps-led-off");
                gps_led_clone.remove_css_class("gps-led-searching");
                gps_led_clone.add_css_class("gps-led-on");
            } else if gps_data.gps_enabled {
                // GPS enabled but no fix yet - use same circle, different color
                gps_led_clone.set_text("●");
                gps_led_clone.remove_css_class("gps-led-off");
                gps_led_clone.remove_css_class("gps-led-on");
                gps_led_clone.add_css_class("gps-led-searching");
            } else {
                // GPS not enabled or not detected
                gps_led_clone.set_text("●");
                gps_led_clone.remove_css_class("gps-led-on");
                gps_led_clone.remove_css_class("gps-led-searching");
                gps_led_clone.add_css_class("gps-led-off");
            }
        }
        
        // Update MODEM bottom sheet
        if let Ok(r) = radio_update.lock() {
            let state = r.state();
            if state.connected {
                if let Some(version) = r.version() {
                    modem_fw_label_clone.set_text(&format!("Firmware: v{}", version.firmware_version));
                    modem_hw_label_clone.set_text(&format!("Hardware: KV4P HT"));
                    modem_rf_label_clone.set_text(&format!("RF Module: {:?}", version.rf_module_type));
                }
            } else {
                modem_fw_label_clone.set_text("Firmware: --");
                modem_hw_label_clone.set_text("Hardware: --");
                modem_rf_label_clone.set_text("RF Module: --");
            }
        }
        
        // Update GPS bottom sheet
        if let Ok(g) = gps_clone.lock() {
            let gps_data = g.get_data();
            
            if gps_data.gps_enabled {
                if gps_data.has_fix {
                    gps_fix_label_clone.set_text(&format!("Fix Type: {}", if gps_data.speed.is_some() { "3D Fix" } else { "2D Fix" }));
                    gps_sats_label_clone.set_text(&format!("Satellites: {}", gps_data.satellites));
                    if let (Some(lat), Some(lon)) = (gps_data.latitude, gps_data.longitude) {
                        gps_locator_label_clone.set_text(&format!("Maidenhead: {}", maidenhead_locator(lat, lon)));
                        gps_coords_label_clone.set_text(&format!("Coordinates: {:.6}, {:.6}", lat, lon));
                    }
                    if let Some(alt) = gps_data.altitude {
                        gps_alt_label_clone.set_text(&format!("Altitude: {:.1} m", alt));
                    }
                    if let Some(speed) = gps_data.speed {
                        gps_speed_label_clone.set_text(&format!("Speed: {:.1} km/h", speed));
                    }
                } else {
                    // GPS enabled but no fix yet
                    gps_fix_label_clone.set_text("Fix Type: No Fix");
                    gps_sats_label_clone.set_text(&format!("Satellites: {}", gps_data.satellites));
                    gps_locator_label_clone.set_text("Maidenhead: --");
                    gps_coords_label_clone.set_text("Coordinates: --");
                    gps_alt_label_clone.set_text("Altitude: --");
                    gps_speed_label_clone.set_text("Speed: --");
                }
            } else {
                // GPS not enabled or not detected
                gps_fix_label_clone.set_text("Fix Type: No GPS");
                gps_sats_label_clone.set_text("Satellites: --");
                gps_locator_label_clone.set_text("Maidenhead: --");
                gps_coords_label_clone.set_text("Coordinates: --");
                gps_alt_label_clone.set_text("Altitude: --");
                gps_speed_label_clone.set_text("Speed: --");
            }
        }
        
        unsafe {
            let _count = (*settings_channels).channels().len();
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
        // Create dialog window using gtk::Dialog (not libadwaita)
        let dialog = gtk::Dialog::with_buttons(
            Some(&format!("Edit Channel: {}", channel.name)),
            parent.root().and_then(|r| r.downcast::<gtk::Window>().ok()).as_ref(),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[("Cancel", gtk::ResponseType::Cancel), ("Save", gtk::ResponseType::Accept)],
        );
        
        // Content box
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(12);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.set_margin_bottom(12);
        
        // Name field
        let name_row = adw::ActionRow::new();
        name_row.set_title("Name");
        let name_entry = gtk::Entry::new();
        name_entry.set_text(&channel.name);
        name_entry.set_hexpand(true);
        name_row.add_suffix(&name_entry);
        name_row.set_activatable_widget(Some(&name_entry));
        content.append(&name_row);
        
        // Frequency field
        let freq_row = adw::ActionRow::new();
        freq_row.set_title("Frequency (MHz)");
        let freq_entry = gtk::Entry::new();
        freq_entry.set_text(&format!("{:.3}", channel.rx_freq_khz as f64 / 1000.0));
        freq_entry.set_hexpand(true);
        freq_row.add_suffix(&freq_entry);
        freq_row.set_activatable_widget(Some(&freq_entry));
        content.append(&freq_row);
        
        // Duplex selection
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
        
        // Offset field (for +/- duplex)
        let offset_row = adw::ActionRow::new();
        offset_row.set_title("Offset (kHz)");
        let offset_entry = gtk::Entry::new();
        offset_entry.set_text(&channel.offset_khz.to_string());
        offset_entry.set_hexpand(true);
        offset_row.add_suffix(&offset_entry);
        offset_row.set_activatable_widget(Some(&offset_entry));
        content.append(&offset_row);
        
        // Tone mode selection
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
        
        // RX Tone frequency
        let rtone_row = adw::ActionRow::new();
        rtone_row.set_title("RX Tone (Hz)");
        let rtone_entry = gtk::Entry::new();
        rtone_entry.set_text(&format!("{:.1}", channel.rtone_hz));
        rtone_entry.set_hexpand(true);
        rtone_row.add_suffix(&rtone_entry);
        rtone_row.set_activatable_widget(Some(&rtone_entry));
        content.append(&rtone_row);
        
        // TX Tone frequency
        let ctone_row = adw::ActionRow::new();
        ctone_row.set_title("TX Tone (Hz)");
        let ctone_entry = gtk::Entry::new();
        ctone_entry.set_text(&format!("{:.1}", channel.ctone_hz));
        ctone_entry.set_hexpand(true);
        ctone_row.add_suffix(&ctone_entry);
        ctone_row.set_activatable_widget(Some(&ctone_entry));
        content.append(&ctone_row);
        
        // Delete button
        let delete_btn = gtk::Button::with_label("Delete Channel");
        delete_btn.add_css_class("destructive-action");
        delete_btn.set_margin_top(12);
        content.append(&delete_btn);
        
        // Add content to dialog
        let content_area = dialog.content_area();
        content_area.append(&content);
        
        // Use RefCell to allow mutating Option in Fn closure
        use std::cell::RefCell;
        let on_delete_opt = RefCell::new(Some(on_delete));
        
        // Delete button - use FnOnce via Option::take
        let dialog_for_delete = dialog.clone();
        delete_btn.connect_clicked(move |_| {
            if let Some(callback) = on_delete_opt.borrow_mut().take() {
                callback();
            }
            dialog_for_delete.close();
        });
        
        // Handle response - wrap on_save in RefCell<Option>
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
                // Parse values and call on_save
                let name = name_entry_clone.text().to_string();
                let freq_text = freq_entry_clone.text().to_string();
                let offset_text = offset_entry_clone.text().to_string();
                let rtone_text = rtone_entry_clone.text().to_string();
                let ctone_text = ctone_entry_clone.text().to_string();
                
                // Parse frequency
                let freq_mhz: f64 = freq_text.parse().unwrap_or(channel_clone.rx_freq_khz as f64 / 1000.0);
                let rx_freq_khz = (freq_mhz * 1000.0) as u32;
                
                // Parse offset
                let offset_khz: u32 = offset_text.parse().unwrap_or(channel_clone.offset_khz);
                
                // Parse duplex
                let duplex = match duplex_dropdown.selected() {
                    0 => Duplex::Simplex,
                    1 => Duplex::Plus,
                    2 => Duplex::Minus,
                    _ => Duplex::Split,
                };
                
                // Parse tone mode
                let tone_mode = match tone_dropdown.selected() {
                    0 => ToneMode::None,
                    1 => ToneMode::Tone,
                    _ => ToneMode::Tsql,
                };
                
                // Parse tone frequencies
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
        .freq-display:focus {
            border-color: #FFB000;
        }
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
        .mode-btn:disabled {
            opacity: 0.4;
        }
        .mode-btn-active {
            background: #1a2a1a;
            border: 2px solid #33D17A;
            color: #33D17A;
            box-shadow: 0 0 6px rgba(51, 209, 122, 0.3);
        }
        .status-icon-green { font-size: 14px; color: #33D17A; }
        .status-icon-red { font-size: 14px; color: #ff4444; }
        .dim-label { font-size: 12px; color: #888; }
        .gps-led-on { color: #33D17A; font-size: 14px; }
        .gps-led-off { color: #666; font-size: 14px; }
        .gps-led-searching { color: #FFB000; font-size: 14px; }
        .gps-location { font-size: 10px; font-family: monospace; color: #888; }
        .gps-searching { color: #666; font-style: italic; }
        .gps-fixed { color: #33D17A; }
        .status-icon-gray-empty { font-size: 14px; color: #666; }
        .status-icon-gray-filled { font-size: 14px; color: #888; }
        .modem-label { color: #666; font-size: 11px; }
        .gps-label { color: #666; font-size: 11px; }
        .audio-label { color: #666; font-size: 11px; }
        .signal-text { font-size: 11px; color: #888; font-weight: bold; }
        .signal-value { font-size: 11px; font-weight: bold; }
        .rssi-bar { background: #2a2a2a; border: 1px solid #444; border-radius: 4px; }
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
    "#);
    
    gtk::style_context_add_provider_for_display(
        &gtk::prelude::RootExt::display(&window),
        &css_provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION
    );
    
    // Main container with header bar
    let main_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main_container.append(&header_bar);
    main_container.append(&toast_overlay);
    
    window.set_content(Some(&main_container));
    window.show();
}