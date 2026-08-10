//! PocketModem libadwaita UI with native KV4P protocol implementation
//!
//! Uses only libadwaita bindings for a modern GNOME-style interface

mod audio;
mod gps;
mod kiss;
mod radio;

use audio::{AudioConfig, AudioManager};
use gps::GpsManager;

use radio::{KV4PRadio, SerialConfig};

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use libadwaita::prelude::*;
use libadwaita as adw;

fn main() {
    // Parse command line args BEFORE GTK processes them
    // Note: We use POCKET_MODEM_DEVICE env var instead of command line args
    // to avoid GTK interpreting the serial device as a file to open
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

    // Create radio
    let radio = Arc::new(Mutex::new(KV4PRadio::new(SerialConfig {
        port: serial_device,
        baudrate: 115200,
        timeout_ms: 500,
    })));
    
    // Try to connect
    let connected = {
        let mut radio = radio.lock().unwrap();
        match radio.open() {
            Ok(version) => {
                if let Some(v) = version {
                    eprintln!("[pocket-modem] Connected: fw=v{}, rf={:?}", 
                             v.firmware_version, v.rf_module_type);
                    // Use tune_freq() to preserve firmware's default squelch (don't override)
                    let _ = radio.tune_freq(145500, 145500);
                    // Open audio after tuning - like Android's openFirmwareAudio()
                    let _ = radio.open_audio();
                    eprintln!("[pocket-modem] Tuned to 145.500 MHz");
                    true
                } else {
                    false
                }
            }
            Err(e) => {
                eprintln!("[pocket-modem] Connection failed: {}", e);
                false
            }
        }
    };
    
    let radio_clone = Arc::clone(&radio);

    // Create GPS manager
    let gps_manager = Arc::new(Mutex::new(GpsManager::new()));
    {
        // Enable GPS location in ModemManager
        GpsManager::enable_gps_location();
        let gps = gps_manager.lock().unwrap();
        gps.start();
    }

    // Create audio manager with KV4P settings
    let audio_config = AudioConfig {
        sample_rate: 16000,
        tx_gain: 2.0,
        rx_gain: 1.0,
        gate_threshold: 0.005,
        pre_emphasis_alpha: 0.0,
        hard_limit: 0.95,
    };
    let audio_manager = Arc::new(Mutex::new(AudioManager::new(audio_config)));
    

    
    // Connect audio TX to radio (ADPCM frames)
    {
        let mut audio = audio_manager.lock().unwrap();
        let radio = Arc::clone(&radio);
        audio.on_tx_audio(move |adpcm_data| {
            if let Ok(mut r) = radio.lock() {
                let _ = r.send_audio(adpcm_data);
            }
        });
    }
    

    
    // Connect radio RX audio to speaker playback
    {
        let mut radio = radio.lock().unwrap();
        let audio = Arc::clone(&audio_manager);
        radio.on_rx_audio(move |adpcm_data| {
            // Accumulate audio - playback will start when squelch opens
            if let Ok(mut a) = audio.lock() {
                a.accumulate_and_start(adpcm_data);
            }
        });
    }
    
    // Connect radio state to update audio squelch state
    {
        let audio = Arc::clone(&audio_manager);
        let mut radio = radio.lock().unwrap();
        radio.on_state(move |state| {
            // Update audio squelch state and RSSI - used for UI display
            if let Ok(mut a) = audio.lock() {
                a.update_squelch(state.is_squelched());
                a.update_rssi(state.rssi);
            }
        });
    }
    
    // Connect physical PTT button on radio hardware
    {
        let radio2 = Arc::clone(&radio);
        let audio = Arc::clone(&audio_manager);
        {
            let mut r = radio.lock().unwrap();
            r.on_phys_ptt(move |pressed| {
                eprintln!("[main] Phys PTT: {}", if pressed { "pressed" } else { "released" });
                // Call ptt_on/ptt_off for firmware control FIRST
                if let Ok(mut r) = radio2.try_lock() {
                    if pressed {
                        let _ = r.ptt_on();
                        eprintln!("[main] ptt_on called");
                    } else {
                        let _ = r.ptt_off();
                        eprintln!("[main] ptt_off called");
                    }
                }
                // Handle audio capture
                if let Ok(mut a) = audio.lock() {
                    if pressed {
                        // Stop RX playback when TX starts
                        a.stop_playback();
                        let _ = a.start_capture();
                    } else {
                        let _ = a.stop_capture();
                        // Re-enable RX playback after TX stops
                        let _ = a.start_playback();
                    }
                }
            });
        }
    }
    
    eprintln!("[main] Radio lock released, starting app...");
    
    let app = adw::Application::builder()
        .application_id("org.pocketmodem.gtk")
        .flags(gtk4::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    
    // Register empty open handler to prevent GTK from exiting when command line args
    // are passed - GTK default is to exit on open, we want to ignore those args
    app.connect_open(|_app, _files, _hint| {
        // Empty handler - ignore file open requests
    });
    
    // Clone for shutdown handler before move into activate
    let audio_for_shutdown = Arc::clone(&audio_manager);
    let gps_manager_activate = Arc::clone(&gps_manager);
    
    app.connect_activate(move |app| {
        create_ui(app, &radio_clone, &audio_manager, &gps_manager_activate, connected);
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
        if let Ok(mut r) = radio_for_shutdown.lock() {
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
    connected: bool
) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(320)
        .default_height(720)
        .title("PocketModem")
        .build();
    window.set_size_request(320, -1);  // Constrain width to 320
    
    // Main content box using HeaderBar for title area
    let header_bar = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("PocketModem", ""))
        .build();
    
    // Settings button - toggles view
    let settings_btn = gtk4::ToggleButton::new();
    settings_btn.set_icon_name("emblem-system-symbolic");
    settings_btn.add_css_class("flat");
    settings_btn.set_tooltip_text(Some("Settings"));
    
    // Settings view content
    let settings_view = gtk4::Box::new(gtk4::Orientation::Vertical, 16);
    settings_view.set_margin_top(24);
    settings_view.set_margin_start(16);
    settings_view.set_margin_end(16);
    settings_view.set_margin_bottom(24);
    settings_view.set_visible(false);
    
    // Squelch section
    let squelch_section = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    let squelch_title = gtk4::Label::new(Some("<b>Squelch</b>"));
    squelch_title.set_markup("<b>Squelch</b>");
    squelch_title.set_halign(gtk4::Align::Start);
    squelch_section.append(&squelch_title);
    
    let squelch_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    squelch_row.set_valign(gtk4::Align::Center);
    
    let squelch_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 9.0, 1.0);
    squelch_scale.set_value(0.0);  // Default 0 matches Android
    squelch_scale.set_hexpand(true);
    squelch_scale.set_draw_value(false);
    squelch_scale.set_has_origin(true);
    
    let squelch_value_label = gtk4::Label::new(Some("4"));
    squelch_value_label.set_width_request(24);
    squelch_value_label.set_halign(gtk4::Align::Start);
    squelch_value_label.add_css_class("squelch-value");
    
    // Squelch callback with debouncing
    let radio_squelch = Arc::clone(radio);
    let label_for_closure = squelch_value_label.clone();
    let last_sent: Arc<std::sync::atomic::AtomicU8> = Arc::new(std::sync::atomic::AtomicU8::new(0));
    
    squelch_scale.connect_value_changed(move |scale| {
        let level = scale.value().round() as u8;
        label_for_closure.set_text(&format!("{}", level));
        
        // Immediately send to radio (no lock holding)
        // Use spawn to avoid blocking GTK main loop
        if level != last_sent.load(std::sync::atomic::Ordering::SeqCst) {
            let radio_clone = radio_squelch.clone();
            let sent = Arc::clone(&last_sent);
            std::thread::spawn(move || {
                // Try to acquire lock with timeout
                let deadline = std::time::Instant::now() + Duration::from_millis(500);
                while std::time::Instant::now() < deadline {
                    if let Ok(mut r) = radio_clone.try_lock() {
                        if let Err(e) = r.set_squelch(level) {
                            eprintln!("[main] set_squelch error: {}", e);
                        }
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                sent.store(level, std::sync::atomic::Ordering::SeqCst);
            });
        }
    });
    // Note: Debounce handles sending after 300ms, no need for separate button release
    
    squelch_row.append(&squelch_scale);
    squelch_row.append(&squelch_value_label);
    squelch_section.append(&squelch_row);
    settings_view.append(&squelch_section);
    
    // Audio Filters section
    let filters_section = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    let filters_title = gtk4::Label::new(Some("<b>Audio Filters</b>"));
    filters_title.set_markup("<b>Audio Filters</b>");
    filters_title.set_halign(gtk4::Align::Start);
    filters_section.append(&filters_title);
    
    // Pre-emphasis
    let pre_emph_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    pre_emph_row.set_hexpand(true);
    let pre_emph_label = gtk4::Label::new(Some("Pre-emphasis (TX)"));
    pre_emph_label.set_hexpand(true);
    pre_emph_label.set_halign(gtk4::Align::Start);
    let pre_emph_switch = gtk4::Switch::new();
    pre_emph_switch.set_valign(gtk4::Align::Center);
    let radio_pre_emph = Arc::clone(radio);
    pre_emph_switch.connect_state_set(move |_sw, state| {
        let radio_clone = radio_pre_emph.clone();
        std::thread::spawn(move || {
            if let Ok(mut r) = radio_clone.lock() {
                let _ = r.set_filter_pre_emphasis(state);
            }
        });
        glib::Propagation::Proceed
    });
    pre_emph_row.append(&pre_emph_label);
    pre_emph_row.append(&pre_emph_switch);
    filters_section.append(&pre_emph_row);
    
    // De-emphasis
    let de_emph_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    de_emph_row.set_hexpand(true);
    let de_emph_label = gtk4::Label::new(Some("De-emphasis (RX)"));
    de_emph_label.set_hexpand(true);
    de_emph_label.set_halign(gtk4::Align::Start);
    let de_emph_switch = gtk4::Switch::new();
    de_emph_switch.set_valign(gtk4::Align::Center);
    let radio_de_emph = Arc::clone(radio);
    de_emph_switch.connect_state_set(move |_sw, state| {
        let radio_clone = radio_de_emph.clone();
        std::thread::spawn(move || {
            if let Ok(mut r) = radio_clone.lock() {
                let _ = r.set_filter_de_emphasis(state);
            }
        });
        glib::Propagation::Proceed
    });
    de_emph_row.append(&de_emph_label);
    de_emph_row.append(&de_emph_switch);
    filters_section.append(&de_emph_row);
    
    // High-pass filter
    let hp_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    hp_row.set_hexpand(true);
    let hp_label = gtk4::Label::new(Some("High-pass filter"));
    hp_label.set_hexpand(true);
    hp_label.set_halign(gtk4::Align::Start);
    let hp_switch = gtk4::Switch::new();
    hp_switch.set_valign(gtk4::Align::Center);
    let radio_hp = Arc::clone(radio);
    hp_switch.connect_state_set(move |_sw, state| {
        let radio_clone = radio_hp.clone();
        std::thread::spawn(move || {
            if let Ok(mut r) = radio_clone.lock() {
                let _ = r.set_filter_high_pass(state);
            }
        });
        glib::Propagation::Proceed
    });
    hp_row.append(&hp_label);
    hp_row.append(&hp_switch);
    filters_section.append(&hp_row);
    
    // Low-pass filter
    let lp_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    lp_row.set_hexpand(true);
    let lp_label = gtk4::Label::new(Some("Low-pass filter"));
    lp_label.set_hexpand(true);
    lp_label.set_halign(gtk4::Align::Start);
    let lp_switch = gtk4::Switch::new();
    lp_switch.set_valign(gtk4::Align::Center);
    let radio_lp = Arc::clone(radio);
    lp_switch.connect_state_set(move |_sw, state| {
        let radio_clone = radio_lp.clone();
        std::thread::spawn(move || {
            if let Ok(mut r) = radio_clone.lock() {
                let _ = r.set_filter_low_pass(state);
            }
        });
        glib::Propagation::Proceed
    });
    lp_row.append(&lp_label);
    lp_row.append(&lp_switch);
    filters_section.append(&lp_row);
    
    settings_view.append(&filters_section);
    
    // TX Power section
    let tx_power_section = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    let tx_power_title = gtk4::Label::new(Some("<b>TX Power</b>"));
    tx_power_title.set_markup("<b>TX Power</b>");
    tx_power_title.set_halign(gtk4::Align::Start);
    tx_power_section.append(&tx_power_title);
    
    let tx_power_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    tx_power_row.set_hexpand(true);
    let tx_power_label = gtk4::Label::new(Some("High power"));
    tx_power_label.set_hexpand(true);
    tx_power_label.set_halign(gtk4::Align::Start);
    let tx_power_switch = gtk4::Switch::new();
    tx_power_switch.set_active(true);
    tx_power_switch.set_valign(gtk4::Align::Center);
    let radio_tx_power = Arc::clone(radio);
    tx_power_switch.connect_state_set(move |_sw, state| {
        let radio_clone = radio_tx_power.clone();
        std::thread::spawn(move || {
            if let Ok(mut r) = radio_clone.lock() {
                let _ = r.set_tx_power(state);
            }
        });
        glib::Propagation::Proceed
    });
    tx_power_row.append(&tx_power_label);
    tx_power_row.append(&tx_power_switch);
    tx_power_section.append(&tx_power_row);
    settings_view.append(&tx_power_section);
    
    // Mic Gain section
    let mic_section = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    let mic_title = gtk4::Label::new(Some("<b>Mic Gain Boost</b>"));
    mic_title.set_markup("<b>Mic Gain Boost</b>");
    mic_title.set_halign(gtk4::Align::Start);
    mic_section.append(&mic_title);
    
    let mic_dropdown = gtk4::DropDown::from_strings(&[
        "None", "Low", "Med", "High",
    ]);
    let radio_mic = Arc::clone(radio);
    mic_dropdown.connect_selected_notify(glib::clone!(@weak mic_dropdown => move |dd| {
        let idx = dd.selected();
        let level = match idx {
            0 => "None",
            1 => "Low", 
            2 => "Med",
            _ => "High",
        };
        let radio_clone = radio_mic.clone();
        std::thread::spawn(move || {
            if let Ok(mut r) = radio_clone.lock() {
                let _ = r.set_mic_gain(level);
            }
        });
    }));
    mic_section.append(&mic_dropdown);
    settings_view.append(&mic_section);
    
    // Add back button
    let back_btn = gtk4::Button::with_label("Back");
    back_btn.set_margin_top(24);
    back_btn.connect_clicked(glib::clone!(@weak settings_btn => move |_| {
        settings_btn.set_active(false);
    }));
    settings_view.append(&back_btn);
    
    header_bar.pack_end(&settings_btn);
    
    // Status row using a horizontal Box with labels
    let status_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 32);
    status_row.set_halign(gtk4::Align::Center);
    status_row.set_margin_top(16);
    status_row.set_margin_bottom(16);
    
    // Modem status indicator
    let modem_label = gtk4::Label::new(Some(if connected { "●" } else { "○" }));
    modem_label.add_css_class(if connected { "status-icon-green" } else { "status-icon-red" });
    let modem_status_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    let modem_icon = gtk4::Image::from_icon_name("network-wireless-symbolic");
    modem_icon.set_pixel_size(28);
    let modem_status_label = gtk4::Label::new(Some("MODEM"));
    modem_status_label.add_css_class("status-text");
    modem_status_label.add_css_class("modem-label");
    modem_status_box.append(&modem_icon);
    modem_status_box.append(&modem_label);
    modem_status_box.append(&modem_status_label);
    
    // GPS icon and status LED
    let gps_icon = gtk4::Image::from_icon_name("location-services-active-symbolic");
    gps_icon.set_pixel_size(28);
    let gps_led = gtk4::Label::new(Some("○"));
    gps_led.add_css_class("gps-led-off");
    let gps_status_label = gtk4::Label::new(Some("GPS"));
    gps_status_label.add_css_class("status-text");
    gps_status_label.add_css_class("gps-label");
    let gps_status_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    gps_status_box.add_css_class("gps-status-box");
    gps_status_box.append(&gps_icon);
    gps_status_box.append(&gps_led);
    gps_status_box.append(&gps_status_label);
    
    status_row.append(&modem_status_box);
    status_row.append(&gps_status_box);

    // GPS location display (below status row)
    let gps_location_label = gtk4::Label::new(Some("Searching..."));
    gps_location_label.add_css_class("gps-location");
    gps_location_label.set_margin_top(4);

    // Audio status indicator
    let audio_label = gtk4::Label::new(Some("○"));
    audio_label.add_css_class("status-icon-gray");
    let audio_status_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    let audio_icon = gtk4::Image::from_icon_name("audio-volume-medium-symbolic");
    audio_icon.set_pixel_size(28);
    let audio_status_label = gtk4::Label::new(Some("AUDIO"));
    audio_status_label.add_css_class("status-text");
    audio_status_label.add_css_class("audio-label");
    audio_status_box.append(&audio_icon);
    audio_status_box.append(&audio_label);
    audio_status_box.append(&audio_status_label);
    status_row.append(&audio_status_box);
    
    // RSSI / S-meter - below VFO
    let rssi_sbar = gtk4::ProgressBar::new();
    rssi_sbar.set_fraction(if connected { 0.5 } else { 0.0 });
    rssi_sbar.add_css_class("rssi-bar");
    
    // S-meter: bar and dBm value side by side
    let smeter_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    smeter_box.set_halign(gtk4::Align::Center);
    smeter_box.set_margin_start(24);
    smeter_box.set_margin_end(24);
    smeter_box.set_valign(gtk4::Align::Center);
    smeter_box.set_size_request(-1, 20);
    
    // SIGNAL label on left
    let signal_label = gtk4::Label::new(Some("SIGNAL"));
    signal_label.add_css_class("signal-text");
    signal_label.set_valign(gtk4::Align::Center);
    
    // Bar in center, takes remaining space
    rssi_sbar.set_hexpand(true);
    rssi_sbar.set_valign(gtk4::Align::Center);
    
    // dBm value on right
    let signal_value = gtk4::Label::new(None);
    signal_value.add_css_class("signal-value");
    signal_value.set_markup(&format!("<span color='#FFB000'>{}</span>", if connected { "-97 dBm" } else { "-- dBm" }));
    signal_value.set_valign(gtk4::Align::Center);
    signal_value.set_width_request(70);
    
    smeter_box.append(&signal_label);
    smeter_box.append(&rssi_sbar);
    smeter_box.append(&signal_value);
    
    // VFO frequency display - using Entry for interaction
    let freq_entry = gtk4::Entry::new();
    freq_entry.set_text("145.500");
    gtk4::prelude::EditableExt::set_alignment(&freq_entry, 0.5);
    freq_entry.add_css_class("freq-display");
    freq_entry.set_size_request(260, 100);
    freq_entry.set_margin_start(16);
    freq_entry.set_margin_end(16);
    freq_entry.set_margin_top(8);
    freq_entry.set_margin_bottom(20);
    freq_entry.set_editable(true);
    freq_entry.set_can_focus(true);
    // Clear selection by default (select region -1,-1 deselects)
    freq_entry.select_region(-1, -1);
    
    let radio_freq = Arc::clone(&radio);
    freq_entry.connect_activate(move |entry| {
        let text = entry.text().to_string();
        
        // Parse frequency - user enters "145.500" or "144.8" (MHz with optional decimal)
        if let Ok(freq_mhz) = text.parse::<f64>() {
            let khz = (freq_mhz * 1000.0) as u32;
            let radio = Arc::clone(&radio_freq);
            
            // Spawn thread for serial operation, don't update UI
            std::thread::spawn(move || {
                if let Ok(mut r) = radio.lock() {
                    if r.set_frequency(khz).is_ok() {
                        eprintln!("[pocket-modem] Frequency set to {} kHz", khz);
                    }
                }
            });
            
            // Update display immediately with expected format
            entry.set_text(&format!("{}.{:03}", khz / 1000, khz % 1000));
        } else {
            eprintln!("[pocket-modem] Invalid frequency: {}", text);
        }
    });
    
    // Mode buttons using gtk4::ToggleButtons in a Box
    let mode_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    mode_box.set_homogeneous(true);
    mode_box.set_margin_start(16);
    mode_box.set_margin_end(16);
    mode_box.set_margin_top(20);
    mode_box.set_margin_bottom(16);
    
    let btn_fm = gtk4::ToggleButton::with_label("FM");
    btn_fm.add_css_class("mode-btn");
    btn_fm.add_css_class("mode-btn-active");  // FM is default active
    
    let btn_rade = gtk4::ToggleButton::with_label("RADE");
    btn_rade.add_css_class("mode-btn");
    btn_rade.set_sensitive(false);
    
    let btn_m17 = gtk4::ToggleButton::with_label("M17");
    btn_m17.add_css_class("mode-btn");
    btn_m17.set_sensitive(false);
    
    btn_fm.set_active(true);
    mode_box.append(&btn_fm);
    mode_box.append(&btn_rade);
    mode_box.append(&btn_m17);
    
    // Channel list using PreferencesGroup with ActionRows
    let channel_group = adw::PreferencesGroup::builder()
        .title("Channels")
        .build();
    
    // Add empty state message as a disabled row
    let no_channels_row = adw::ActionRow::builder()
        .title("No channels configured")
        .build();
    no_channels_row.set_sensitive(false);
    channel_group.add(&no_channels_row);
    
    // PTT Button using SplitButton for modern look (without dropdown)
    let ptt_btn = gtk4::Button::new();
    ptt_btn.add_css_class("ptt-button");
    ptt_btn.set_valign(gtk4::Align::Center);
    
    // PTT box with circle and label underneath
    let ptt_box = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    ptt_box.set_halign(gtk4::Align::Center);
    ptt_box.set_valign(gtk4::Align::Center);
    
    let ptt_icon = gtk4::Image::from_icon_name("media-record-symbolic");
    ptt_icon.set_pixel_size(40);
    ptt_icon.add_css_class("ptt-icon");
    ptt_icon.set_halign(gtk4::Align::Center);
    ptt_icon.set_valign(gtk4::Align::Center);
    
    let ptt_label = gtk4::Label::new(Some("PTT"));
    ptt_label.add_css_class("ptt-label");
    ptt_label.set_halign(gtk4::Align::Center);
    
    ptt_box.append(&ptt_icon);
    ptt_box.append(&ptt_label);
    
    ptt_btn.set_child(Some(&ptt_box));
    ptt_btn.set_tooltip_text(Some("Hold to transmit"));
    ptt_btn.set_hexpand(true);
    ptt_btn.set_margin_start(20);
    ptt_btn.set_margin_end(20);
    ptt_btn.set_margin_bottom(20);
    ptt_btn.set_valign(gtk4::Align::End);
    
    let radio_ptt_press = Arc::clone(radio);
    let radio_ptt_release = Arc::clone(radio);
    let audio_ptt_press = Arc::clone(audio);
    let audio_ptt_release = Arc::clone(audio);
    let gesture = gtk4::GestureClick::new();
    // Allow any button (0) for touch events
    gesture.set_button(0);
    gesture.set_propagation_phase(gtk4::PropagationPhase::Capture);
    gesture.connect_pressed(move |_, _, _, _| {
        // Start PTT
        if let Ok(mut r) = radio_ptt_press.lock() {
            if let Err(e) = r.ptt_on() {
                eprintln!("[main] ptt_on error: {}", e);
            } else {
                eprintln!("[main] ptt_on success");
            }
        }
        // Start audio capture for TX
        if let Ok(mut a) = audio_ptt_press.lock() {
            // Stop RX playback when TX starts
            a.stop_playback();
            if let Err(e) = a.start_capture() {
                eprintln!("[main] start_capture error: {}", e);
            } else {
                eprintln!("[main] start_capture success");
            }
        }
    });
    gesture.connect_released(move |_, _, _, _| {
        // Stop PTT first - signal firmware to stop TX
        if let Ok(mut r) = radio_ptt_release.lock() {
            if let Err(e) = r.ptt_off() {
                eprintln!("[main] ptt_off error: {}", e);
            } else {
                eprintln!("[main] ptt_off success");
            }
        }
        // Stop audio capture and re-enable RX playback
        if let Ok(mut a) = audio_ptt_release.lock() {
            a.stop_capture();
            let _ = a.start_playback();
        }
    });
    ptt_btn.add_controller(gesture);
    
    // Main content area with Clamp for responsive width
    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content_box.set_halign(gtk4::Align::Center);
    content_box.set_hexpand(false);
    content_box.set_size_request(320, -1);
    
    // Connect toggle button to switch views (after content_box exists)
    let settings_view_clone = settings_view.clone();
    let content_box_clone = content_box.clone();
    settings_btn.connect_toggled(move |btn| {
        if btn.is_active() {
            settings_view_clone.set_visible(true);
            content_box_clone.set_visible(false);
        } else {
            settings_view_clone.set_visible(false);
            content_box_clone.set_visible(true);
        }
    });
    
    // Status row at top
    content_box.append(&status_row);
    
    // Frequency display and signal
    content_box.append(&freq_entry);
    content_box.append(&smeter_box);
    
    // Mode buttons
    content_box.append(&mode_box);
    
    // Channel list
    content_box.append(channel_group.as_ref() as &gtk4::Widget);
    
    // Spacer to push PTT to bottom
    let spacer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    content_box.append(&spacer);
    
    // PTT at bottom
    content_box.append(&ptt_btn);
    
    // Main layout box
    let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    main_box.append(&header_bar);
    main_box.append(&content_box);
    main_box.append(&settings_view);
    
    // Update loop for live status
    let radio_update = Arc::clone(radio);
    let modem_label_clone = modem_label.clone();
    let rssi_sbar_clone = rssi_sbar.clone();
    let signal_value_clone = signal_value.clone();
    let audio_clone = Arc::clone(audio);
    let audio_label_clone = audio_label.clone();
    let gps_clone = Arc::clone(gps);

    let gps_led_clone = gps_led.clone();

    let gps_location_clone = gps_location_label.clone();

    glib::timeout_add_local(Duration::from_millis(1000), move || {
        if let Ok(r) = radio_update.lock() {
            let state = r.state();
            
            if state.connected {
                modem_label_clone.set_text("●");
                modem_label_clone.remove_css_class("status-icon-red");
                modem_label_clone.add_css_class("status-icon-green");
            } else {
                modem_label_clone.set_text("○");
                modem_label_clone.remove_css_class("status-icon-green");
                modem_label_clone.add_css_class("status-icon-red");
            }
            
            // S-meter: Standard VHF/UHF formula (S9 = -93 dBm)
            // dBm = raw * 1.2 - 160.8
            // S-val = 9 + (dBm - (-93)) / 6
            // Below S9: round to nearest whole S-unit
            // Calculate dBm from raw RSSI (matches DeviceState::rssi_dbm)
            let dbm = -120.0 + (state.raw_rssi as f64 * 2.0);
            
            // Update dBm display
            let dbm_text = if state.connected { format!("{} dBm", dbm as i32) } else { "-- dBm".to_string() };
            signal_value_clone.set_markup(&format!("<span color='#FFB000'>{}</span>", dbm_text));
            
            // Update amber bar (dBm range: -120 to -60 = 0% to 100%)
            let frac = if state.connected {
                ((dbm + 120.0) / 60.0).clamp(0.0, 1.0)
            } else {
                0.0
            };
            rssi_sbar_clone.set_fraction(frac);
            
            // Update audio status indicator
            if let Ok(a) = audio_clone.lock() {
                let capturing = a.is_capturing();
                let playing = a.is_playing();
                if capturing || playing {
                    audio_label_clone.set_text("●");
                    audio_label_clone.remove_css_class("status-icon-gray");
                    audio_label_clone.add_css_class("status-icon-green");
                } else {
                    audio_label_clone.set_text("○");
                    audio_label_clone.remove_css_class("status-icon-green");
                    audio_label_clone.add_css_class("status-icon-gray");
                }
            }

            // Update GPS status indicator
            if let Ok(g) = gps_clone.lock() {
                let gps_data = g.get_data();
                if gps_data.has_fix {
                    gps_led_clone.set_text("●");
                    gps_led_clone.remove_css_class("gps-led-off");
                    gps_led_clone.add_css_class("gps-led-on");

                    // Update location display
                    if let (Some(lat), Some(lon)) = (gps_data.latitude, gps_data.longitude) {
                        let location = format!("{:.6}, {:.6}", lat, lon);
                        gps_location_clone.set_text(&location);
                        gps_location_clone.remove_css_class("gps-searching");
                        gps_location_clone.add_css_class("gps-fixed");
                    }
                } else {
                    gps_led_clone.set_text("○");
                    gps_led_clone.remove_css_class("gps-led-on");
                    gps_led_clone.add_css_class("gps-led-off");
                    gps_location_clone.set_text("Searching...");
                    gps_location_clone.remove_css_class("gps-fixed");
                    gps_location_clone.add_css_class("gps-searching");
                }
            }
        }
        glib::ControlFlow::Continue
    });
    
    // CSS - preserved styling from original
    let css_provider = gtk4::CssProvider::new();
    css_provider.load_from_data(r#"
        .freq-display {
            font-size: 56px;
            font-family: "monospace";
            font-weight: bold;
            color: #FFB000;
            background: #1E1E1E;
            border: 4px solid #444;
            border-radius: 16px;
            text-shadow: 0 0 8px rgba(255, 176, 0, 0.5);
            box-shadow: inset 0 0 20px rgba(0, 0, 0, 0.8);
            caret-color: #FFB000;
        }
        .freq-display:focus {
            border-color: #FFB000;
        }
        .mode-btn {
            font-size: 14px;
            font-weight: bold;
            padding: 8px 16px;
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
            box-shadow: 0 0 8px rgba(51, 209, 122, 0.3);
        }
        .status-icon-green {
            font-size: 16px;
            color: #33D17A;
        }
        .status-icon-red {
            font-size: 16px;
            color: #ff4444;
        }
        .status-text {
            font-size: 13px;
            color: #FFB000;
            font-weight: bold;
        }
        .s-meter-header {
            font-size: 12px;
            color: #888;
        }
        .s-level {
            font-size: 18px;
            min-width: 40px;
        }
        .modem-label {
            color: #666;
            font-size: 13px;
        }
        .gps-label {
            color: #666;
            font-size: 13px;
        }
        .audio-label {
            color: #666;
            font-size: 13px;
        }
        .audio-status-box {
            color: @theme_fg_color;
        }
        .gps-status-box {
            color: @theme_fg_color;
        }
        .location-services-active-symbolic {
            color: inherit;
        }
        .gps-led-on {
            color: #33D17A;
            font-size: 16px;
        }
        .gps-led-off {
            color: #666;
            font-size: 16px;
        }

        .gps-location {
            font-size: 11px;
            font-family: monospace;
            color: #888;
        }
        .gps-searching {
            color: #666;
            font-style: italic;
        }
        .gps-fixed {
            color: #33D17A;
        }
        .status-icon-gray {
            font-size: 16px;
            color: #666;
        }

        .squelch-label {
            font-size: 12px;
            color: #888;
            font-weight: bold;
        }
        .signal-text {
            font-size: 12px;
            color: #666;
            font-weight: bold;
        }
        .signal-value {
            font-size: 12px;
            font-weight: bold;
        }
        .rssi-bar {
            background: #2a2a2a;
            border: 1px solid #444;
            border-radius: 6px;
        }
        .rssi-bar > trough {
            background: #2a2a2a;
            border-radius: 6px;
        }
        .rssi-bar > trough > progress {
            background: #FFB000;
            border-radius: 5px;
        }
        .ptt-button {
            min-width: 100px;
            min-height: 110px;
            border-radius: 16px;
            background: #333;
            border: 2px solid #555;
        }
        .ptt-button:hover {
            background: #3a3a3a;
            border-color: #666;
        }
        .ptt-button:active {
            background: #444;
            border-color: #FFB000;
        }
        .ptt-icon {
            color: #888;
        }
        .ptt-button:hover .ptt-icon {
            color: #aaa;
        }
        .ptt-button:active .ptt-icon {
            color: #FFB000;
        }
        .ptt-label {
            font-size: 12px;
            font-weight: bold;
            color: #888;
        }
        .ptt-button:hover .ptt-label {
            color: #aaa;
        }
        .ptt-button:active .ptt-label {
            color: #FFB000;
        }
    "#);
    
    gtk4::style_context_add_provider_for_display(
        &gtk4::prelude::RootExt::display(&window),
        &css_provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION
    );
    
    // Set main_box as content
    window.set_content(Some(&main_box));
    window.show();
}

