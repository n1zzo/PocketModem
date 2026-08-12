//! PocketModem libadwaita UI with native KV4P protocol implementation
//!
//! Uses only libadwaita bindings for a modern GNOME-style interface

mod audio;
mod gps;
mod kiss;
mod radio;
mod settings;

use audio::{AudioConfig, AudioManager};
use gps::GpsManager;
use settings::SettingsManager;

use radio::{KV4PRadio, SerialConfig};

use std::sync::{Arc, Mutex};
use std::time::Duration;

use libadwaita::prelude::*;
use libadwaita as adw;

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
        let _ = GpsManager::enable_gps_location();
        let gps = gps_manager.lock().unwrap();
        gps.start();
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
        .flags(gtk4::gio::ApplicationFlags::NON_UNIQUE)
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
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(320)
        .default_height(720)
        .title("PocketModem")
        .build();
    window.set_size_request(320, -1);
    
    window.show();
    
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
    
    // Main content box using HeaderBar
    let header_bar = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("PocketModem", ""))
        .build();
    
    // Settings button
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
    
    // === SETTINGS PANEL (operates on main thread) ===
    let settings_clone = settings as *const SettingsManager as *mut SettingsManager;
    
    // Squelch section
    let squelch_section = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    let squelch_title = gtk4::Label::new(Some("<b>Squelch</b>"));
    squelch_title.set_markup("<b>Squelch</b>");
    squelch_title.set_halign(gtk4::Align::Start);
    squelch_section.append(&squelch_title);
    
    let squelch_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    squelch_row.set_valign(gtk4::Align::Center);
    
    let squelch_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 8.0, 1.0);
    squelch_scale.set_value(saved_squelch as f64);
    squelch_scale.set_inverted(true);
    squelch_scale.set_hexpand(true);
    squelch_scale.set_draw_value(false);
    squelch_scale.set_has_origin(true);
    
    let squelch_value_label = gtk4::Label::new(Some(&saved_squelch.to_string()));
    squelch_value_label.set_width_request(24);
    squelch_value_label.set_halign(gtk4::Align::Start);
    squelch_value_label.add_css_class("squelch-value");
    
    // Squelch callback - settings on main thread, radio on spawned thread
    let radio_squelch = Arc::clone(radio);
    let label_for_closure = squelch_value_label.clone();
    let last_sent: Arc<std::sync::atomic::AtomicU8> = Arc::new(std::sync::atomic::AtomicU8::new(saved_squelch));
    
    squelch_scale.connect_value_changed(move |scale| {
        let level = scale.value().round() as u8;
        label_for_closure.set_text(&format!("{}", level));
        
        if level != last_sent.load(std::sync::atomic::Ordering::SeqCst) {
            // Settings on main thread (via unsafe pointer - OK because we're on main thread)
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
    pre_emph_switch.set_active(false);
    
    let radio_pre_emph = Arc::clone(radio);
    pre_emph_switch.connect_state_set(move |_sw, state| {
        // Settings on main thread
        unsafe { (*settings_clone).set_pre_emphasis(state); }
        // Radio on spawned thread
        let radio_clone = radio_pre_emph.clone();
        std::thread::spawn(move || {
            if let Ok(r) = radio_clone.lock() {
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
    de_emph_switch.set_active(false);
    
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
    hp_switch.set_active(true);
    
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
    lp_switch.set_active(true);
    
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
    tx_power_switch.set_active(false);
    tx_power_switch.set_valign(gtk4::Align::Center);
    
    let radio_tx_power = Arc::clone(radio);
    tx_power_switch.connect_state_set(move |_sw, state| {
        unsafe { (*settings_clone).set_tx_power_high(state); }
        let radio_clone = radio_tx_power.clone();
        std::thread::spawn(move || {
            if let Ok(r) = radio_clone.lock() {
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
    mic_section.append(&mic_dropdown);
    settings_view.append(&mic_section);
    
    // Channels section
    let channels_section = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    let channels_title = gtk4::Label::new(Some("<b>Channels</b>"));
    channels_title.set_markup("<b>Channels</b>");
    channels_title.set_halign(gtk4::Align::Start);
    channels_section.append(&channels_title);
    
    // Import CSV row using libadwaita ActionRow
    let import_row = adw::ActionRow::new();
    import_row.set_title("Import from CSV");
    import_row.set_subtitle("Import channels from CHIRP CSV file");
    
    let import_icon = gtk4::Image::from_icon_name("document-open-symbolic");
    let import_btn = gtk4::Button::new();
    import_btn.set_child(Some(&import_icon));
    import_btn.add_css_class("flat");
    import_row.set_activatable_widget(Some(&import_btn));
    import_row.add_suffix(&import_btn);
    
    // Export CSV row using libadwaita ActionRow
    let export_row = adw::ActionRow::new();
    export_row.set_title("Export to CSV");
    export_row.set_subtitle("Export channels to CHIRP CSV file");
    
    let export_icon = gtk4::Image::from_icon_name("document-save-as-symbolic");
    let export_btn = gtk4::Button::new();
    export_btn.set_child(Some(&export_icon));
    export_btn.add_css_class("flat");
    export_row.set_activatable_widget(Some(&export_btn));
    export_row.add_suffix(&export_btn);
    
    // Import CSV - opens file chooser dialog
    let settings_for_import = settings_clone;
    import_btn.connect_clicked(move |_| {
        // We can't use @weak window here since we're capturing it in the inner closure
        // So we capture it in the outer closure instead
        let dialog = gtk4::FileChooserDialog::new(
            Some("Import Channels from CSV"),
            None::<&gtk4::Window>,
            gtk4::FileChooserAction::Open,
            &[
                ("Cancel", gtk4::ResponseType::Cancel),
                ("Import", gtk4::ResponseType::Accept),
            ],
        );
        
        // Add CSV file filter
        let filter = gtk4::FileFilter::new();
        filter.set_name(Some("CSV Files"));
        filter.add_pattern("*.csv");
        filter.add_mime_type("text/csv");
        dialog.add_filter(&filter);
        
        // Connect response handler
        let settings = settings_for_import;
        dialog.connect_response(move |dialog, response| {
            if response == gtk4::ResponseType::Accept {
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
    
    // Export CSV - opens save file chooser dialog
    let settings_for_export = settings_clone;
    export_btn.connect_clicked(move |_| {
        let dialog = gtk4::FileChooserDialog::new(
            Some("Export Channels to CSV"),
            None::<&gtk4::Window>,
            gtk4::FileChooserAction::Save,
            &[
                ("Cancel", gtk4::ResponseType::Cancel),
                ("Export", gtk4::ResponseType::Accept),
            ],
        );
        
        // Add CSV file filter
        let filter = gtk4::FileFilter::new();
        filter.set_name(Some("CSV Files"));
        filter.add_pattern("*.csv");
        filter.add_mime_type("text/csv");
        dialog.add_filter(&filter);
        
        // Set default filename
        dialog.set_current_name("pocket-modem-channels.csv");
        
        // Connect response handler
        let settings = settings_for_export;
        dialog.connect_response(move |dialog, response| {
            if response == gtk4::ResponseType::Accept {
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
    
    channels_section.append(&import_row);
    channels_section.append(&export_row);
    
    settings_view.append(&channels_section);
    
    // Reset to Defaults button
    let reset_btn = gtk4::Button::with_label("Reset to Defaults");
    reset_btn.add_css_class("destructive-action");
    reset_btn.set_margin_top(24);
    
    reset_btn.connect_clicked(move |_| {
        unsafe {
            (*settings_clone).reset_to_defaults();
        }
        squelch_scale.set_value(4.0);
        squelch_value_label.set_text("4");
        eprintln!("[pocket-modem] Settings reset to defaults");
    });
    settings_view.append(&reset_btn);
    
    // Back button
    let back_btn = gtk4::Button::with_label("Back");
    back_btn.set_margin_top(16);
    back_btn.connect_clicked(glib::clone!(@weak settings_btn => move |_| {
        settings_btn.set_active(false);
    }));
    settings_view.append(&back_btn);
    
    header_bar.pack_end(&settings_btn);
    
    // Status row
    let status_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 32);
    status_row.set_halign(gtk4::Align::Center);
    status_row.set_margin_top(16);
    status_row.set_margin_bottom(16);
    
    let modem_label = gtk4::Label::new(Some("○"));
    modem_label.add_css_class("status-icon-red");
    let modem_status_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    let modem_icon = gtk4::Image::from_icon_name("network-wireless-symbolic");
    modem_icon.set_pixel_size(28);
    let modem_status_label = gtk4::Label::new(Some("MODEM"));
    modem_status_label.add_css_class("status-text");
    modem_status_label.add_css_class("modem-label");
    modem_status_box.append(&modem_icon);
    modem_status_box.append(&modem_label);
    modem_status_box.append(&modem_status_label);
    
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

    let gps_location_label = gtk4::Label::new(Some("Searching..."));
    gps_location_label.add_css_class("gps-location");
    gps_location_label.set_margin_top(4);

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
    
    // RSSI / S-meter
    let rssi_sbar = gtk4::ProgressBar::new();
    rssi_sbar.set_fraction(0.0);
    rssi_sbar.add_css_class("rssi-bar");
    
    let smeter_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    smeter_box.set_halign(gtk4::Align::Center);
    smeter_box.set_margin_start(24);
    smeter_box.set_margin_end(24);
    smeter_box.set_valign(gtk4::Align::Center);
    smeter_box.set_size_request(-1, 20);
    
    let signal_label = gtk4::Label::new(Some("SIGNAL"));
    signal_label.add_css_class("signal-text");
    signal_label.set_valign(gtk4::Align::Center);
    
    rssi_sbar.set_hexpand(true);
    rssi_sbar.set_valign(gtk4::Align::Center);
    
    let signal_value = gtk4::Label::new(None);
    signal_value.add_css_class("signal-value");
    signal_value.set_markup(&format!("<span color='#FFB000'>{}</span>", "-- dBm"));
    signal_value.set_valign(gtk4::Align::Center);
    signal_value.set_width_request(70);
    
    smeter_box.append(&signal_label);
    smeter_box.append(&rssi_sbar);
    smeter_box.append(&signal_value);
    
    // VFO frequency display
    let freq_entry = gtk4::Entry::new();
    freq_entry.set_text(&format!("{}.{:03}", saved_freq / 1000, saved_freq % 1000));
    gtk4::prelude::EntryExt::set_alignment(&freq_entry, 0.5);
    freq_entry.add_css_class("freq-display");
    freq_entry.set_size_request(260, 100);
    freq_entry.set_margin_start(16);
    freq_entry.set_margin_end(16);
    freq_entry.set_margin_top(8);
    freq_entry.set_margin_bottom(20);
    freq_entry.set_editable(true);
    freq_entry.set_can_focus(true);
    freq_entry.select_region(-1, -1);
    
    let radio_freq = Arc::clone(&radio);
    let settings_for_freq = settings_clone;
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
    
    // Mode buttons
    let mode_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    mode_box.set_homogeneous(true);
    mode_box.set_margin_start(16);
    mode_box.set_margin_end(16);
    mode_box.set_margin_top(20);
    mode_box.set_margin_bottom(16);
    
    let btn_fm = gtk4::ToggleButton::with_label("FM");
    btn_fm.add_css_class("mode-btn");
    btn_fm.add_css_class("mode-btn-active");
    
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
    
    // Channel list
    let channel_group = adw::PreferencesGroup::builder()
        .title("Channels")
        .build();
    
    let no_channels_row = adw::ActionRow::builder()
        .title("No channels configured")
        .build();
    no_channels_row.set_sensitive(false);
    channel_group.add(&no_channels_row);
    
    // PTT Button
    let ptt_btn = gtk4::Button::new();
    ptt_btn.add_css_class("ptt-button");
    ptt_btn.set_valign(gtk4::Align::Center);
    
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
    
    // PTT using GestureClick
    let radio_pressed = Arc::clone(radio);
    let audio_pressed = Arc::clone(audio);
    let radio_released = Arc::clone(radio);
    let audio_released = Arc::clone(audio);
    
    let gesture = gtk4::GestureClick::new();
    gesture.set_propagation_phase(gtk4::PropagationPhase::Capture);
    gesture.set_button(0);
    
    gesture.connect_pressed(glib::clone!(
        @strong radio_pressed, 
        @strong audio_pressed,
        @weak ptt_label,
        => move |_gesture, _n_press, _x, _y| {
        ptt_label.set_text("TX");
        if let Ok(r) = radio_pressed.lock() {
            let _ = r.ptt_on();
        }
        if let Ok(mut a) = audio_pressed.lock() {
            let _ = a.start_capture();
        }
    }));
    
    gesture.connect_released(glib::clone!(
        @strong radio_released, 
        @strong audio_released,
        @weak ptt_label,
        => move |_gesture, _n_press, _x, _y| {
        if let Ok(mut a) = audio_released.lock() {
            let _ = a.stop_capture();
        }
        if let Ok(r) = radio_released.lock() {
            let _ = r.ptt_off();
        }
        ptt_label.set_text("PTT");
    }));
    
    ptt_btn.add_controller(gesture);
    
    // Main content area
    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content_box.set_halign(gtk4::Align::Center);
    content_box.set_hexpand(false);
    content_box.set_size_request(320, -1);
    
    // Connect toggle button to switch views
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
    
    content_box.append(&status_row);
    content_box.append(&freq_entry);
    content_box.append(&smeter_box);
    content_box.append(&mode_box);
    content_box.append(channel_group.as_ref() as &gtk4::Widget);
    
    let spacer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    content_box.append(&spacer);
    content_box.append(&ptt_btn);
    
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
    let ptt_label_update = ptt_label.clone();
    let settings_channels = settings as *const SettingsManager as *mut SettingsManager;

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

        // Update GPS status
        if let Ok(g) = gps_clone.lock() {
            let gps_data = g.get_data();
            if gps_data.has_fix {
                gps_led_clone.set_text("●");
                gps_led_clone.remove_css_class("gps-led-off");
                gps_led_clone.add_css_class("gps-led-on");

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
        
        // Channel count is shown in settings via ActionRow subtitles
        // This timer can be used for other periodic updates if needed
        unsafe {
            let _count = (*settings_channels).channels().len();
            // Channel count is displayed in the ActionRow subtitles dynamically
        }
        
        ptt_label_update.set_text("PTT");
        
        glib::ControlFlow::Continue
    });
    
    // CSS styling
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
        .status-icon-green { font-size: 16px; color: #33D17A; }
        .status-icon-red { font-size: 16px; color: #ff4444; }
        .status-text { font-size: 13px; color: #FFB000; font-weight: bold; }
        .modem-label { color: #666; font-size: 13px; }
        .gps-label { color: #666; font-size: 13px; }
        .audio-label { color: #666; font-size: 13px; }
        .gps-led-on { color: #33D17A; font-size: 16px; }
        .gps-led-off { color: #666; font-size: 16px; }
        .gps-location { font-size: 11px; font-family: monospace; color: #888; }
        .gps-searching { color: #666; font-style: italic; }
        .gps-fixed { color: #33D17A; }
        .status-icon-gray-empty { font-size: 16px; color: #666; }
        .status-icon-gray-filled { font-size: 16px; color: #888; }
        .signal-text { font-size: 12px; color: #666; font-weight: bold; }
        .signal-value { font-size: 12px; font-weight: bold; }
        .rssi-bar { background: #2a2a2a; border: 1px solid #444; border-radius: 6px; }
        .rssi-bar > trough { background: #2a2a2a; border-radius: 6px; }
        .rssi-bar > trough > progress { background: #FFB000; border-radius: 5px; }
        .ptt-button { min-width: 100px; min-height: 110px; border-radius: 16px; background: #333; border: 2px solid #555; }
        .ptt-button:hover { background: #3a3a3a; border-color: #666; }
        .ptt-button:active { background: #444; border-color: #FFB000; }
        .ptt-icon { color: #888; }
        .ptt-button:hover .ptt-icon { color: #aaa; }
        .ptt-button:active .ptt-icon { color: #FFB000; }
        .ptt-label { font-size: 12px; font-weight: bold; color: #888; }
        .ptt-button:hover .ptt-label { color: #aaa; }
        .ptt-button:active .ptt-label { color: #FFB000; }
        .dim-label { font-size: 12px; color: #666; }
    "#);
    
    gtk4::style_context_add_provider_for_display(
        &gtk4::prelude::RootExt::display(&window),
        &css_provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION
    );
    
    window.set_content(Some(&main_box));
    window.show();
}