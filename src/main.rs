//! PocketModem GTK4 UI with native KV4P protocol implementation
//!
//! No FFI dependency - uses native Rust KISS protocol for kv4p-ht

mod kiss;
mod radio;

use radio::{KV4PRadio, SerialConfig};

use std::sync::{Arc, Mutex};
use std::time::Duration;

use libadwaita::prelude::*;
use gtk4::prelude::*;

fn main() {
    // Parse command line args BEFORE GTK processes them
    let serial_device = std::env::args()
        .skip(1)  // Skip app name
        .find(|arg| !arg.starts_with('-'))  // Find first non-option arg
        .unwrap_or_else(|| {
            // Auto-detect if no device specified
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
                    let _ = radio.tune(145500, 145500, 4, 1);
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
    
    let app = libadwaita::Application::builder()
        .application_id("org.pocketmodem.gtk")
        .flags(gtk4::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    
    // Override the open handler to prevent GTK from trying to open serial device as file
    app.connect_open(|app, files, _hint| {
        // Just activate the app without opening any files
        app.activate();
    });

    app.connect_activate(move |app| {
        create_ui(app, &radio_clone, connected);
    });
    
    app.run();
}

fn create_ui(app: &libadwaita::Application, radio: &Arc<Mutex<KV4PRadio>>, connected: bool) {
    let window = libadwaita::ApplicationWindow::builder()
        .application(app)
        .default_width(360)
        .default_height(720)
        .title("PocketModem")
        .build();
    
    let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    main_box.set_halign(gtk4::Align::Fill);
    main_box.set_hexpand(true);
    
    // Status row
    let status_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 32);
    status_row.set_halign(gtk4::Align::Center);
    status_row.set_margin_top(16);
    status_row.set_margin_bottom(16);
    
    // Modem status
    let modem_icon = gtk4::Image::from_icon_name("network-wireless-symbolic");
    modem_icon.set_pixel_size(32);
    let modem_label = gtk4::Label::new(Some(if connected { "●" } else { "○" }));
    modem_label.add_css_class(if connected { "status-icon-green" } else { "status-icon-red" });
    let modem_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    modem_box.append(&modem_icon);
    modem_box.append(&modem_label);
    let modem_label2 = modem_label.clone();
    
    // RSSI / S-meter
    let rssi_label = gtk4::Label::new(None);
    rssi_label.add_css_class("s-meter");
    let rssi_icon = gtk4::Image::from_icon_name("network-cellular-signal-excellent-symbolic");
    rssi_icon.set_pixel_size(32);
    let rssi_sbar = gtk4::LevelBar::new();
    rssi_sbar.set_size_request(60, 8);
    rssi_sbar.set_min_value(0.0);
    rssi_sbar.set_max_value(9.0);
    rssi_sbar.set_value(if connected { 5.0 } else { 0.0 });
    rssi_label.set_markup(&format!("<span color='#44dd66'>{}</span>", if connected { "S5" } else { "S0" }));
    let rssi_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    rssi_box.append(&rssi_icon);
    rssi_box.append(&rssi_sbar);
    rssi_box.append(&rssi_label);
    
    status_row.append(&modem_box);
    status_row.append(&rssi_box);
    main_box.append(&status_row);
    
    // Mode badge
    let mode_label = gtk4::Label::new(Some("FM"));
    mode_label.add_css_class("mode-badge");
    mode_label.set_halign(gtk4::Align::Center);
    mode_label.set_margin_bottom(12);
    main_box.append(&mode_label);
    
    // VFO frequency display
    let freq_entry = gtk4::Entry::new();
    freq_entry.set_text("145.500");
    gtk4::prelude::EditableExt::set_alignment(&freq_entry, 0.5);
    freq_entry.add_css_class("freq-display");
    freq_entry.set_size_request(300, 100);
    freq_entry.set_margin_start(16);
    freq_entry.set_margin_end(16);
    freq_entry.set_margin_top(8);
    freq_entry.set_margin_bottom(20);
    freq_entry.set_editable(true);
    freq_entry.set_can_focus(true);
    
    let radio_freq = Arc::clone(radio);
    freq_entry.connect_activate(move |entry| {
        let text = entry.text();
        if let Ok(freq) = text.replace(".", "").parse::<u32>() {
            let khz = if freq > 999999 { freq } else { freq * 1000 };
            if let Ok(mut r) = radio_freq.lock() {
                if r.set_frequency(khz).is_ok() {
                    entry.set_text(&format!("{}.{:03}", khz / 1000, khz % 1000));
                    eprintln!("[pocket-modem] Frequency set to {} kHz", khz);
                }
            }
        }
    });
    main_box.append(&freq_entry);
    
    // Mode buttons
    let mode_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    mode_box.set_homogeneous(true);
    mode_box.set_margin_start(16);
    mode_box.set_margin_end(16);
    mode_box.set_margin_bottom(16);
    
    let btn_fm = gtk4::ToggleButton::with_label("FM");
    let btn_rade = gtk4::ToggleButton::with_label("RADE");
    let btn_m17 = gtk4::ToggleButton::with_label("M17");
    btn_rade.set_sensitive(false);
    btn_m17.set_sensitive(false);
    btn_fm.set_active(true);
    mode_box.append(&btn_fm);
    mode_box.append(&btn_rade);
    mode_box.append(&btn_m17);
    main_box.append(&mode_box);
    
    // Channel list header
    let channel_header = gtk4::Label::new(Some("CHANNELS"));
    channel_header.add_css_class("section-header");
    channel_header.set_halign(gtk4::Align::Start);
    channel_header.set_margin_start(16);
    channel_header.set_margin_bottom(8);
    main_box.append(&channel_header);
    
    // Channel list
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_margin_start(12);
    scroll.set_margin_end(12);
    scroll.set_hexpand(true);
    scroll.set_vexpand(true);
    
    let channel_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    channel_box.set_halign(gtk4::Align::Fill);
    
    let no_channels = gtk4::Label::new(Some("No channels configured"));
    no_channels.add_css_class("dim-label");
    no_channels.set_halign(gtk4::Align::Center);
    channel_box.append(&no_channels);
    
    scroll.set_child(Some(&channel_box));
    main_box.append(&scroll);
    
    // PTT Button
    let ptt_btn = gtk4::Button::new();
    ptt_btn.add_css_class("ptt-button");
    
    // PTT box with circle and label underneath
    let ptt_box = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    ptt_box.set_halign(gtk4::Align::Center);
    
    let ptt_icon = gtk4::Image::from_icon_name("media-record-symbolic");
    ptt_icon.set_pixel_size(40);
    ptt_icon.add_css_class("ptt-icon");
    
    let ptt_label = gtk4::Label::new(Some("PTT"));
    ptt_label.add_css_class("ptt-label");
    
    ptt_box.append(&ptt_icon);
    ptt_box.append(&ptt_label);
    
    ptt_btn.set_child(Some(&ptt_box));
    ptt_btn.set_tooltip_text(Some("Hold to transmit"));
    ptt_btn.set_size_request(100, 110);
    ptt_btn.set_margin_top(12);
    ptt_btn.set_margin_bottom(20);
    ptt_btn.set_halign(gtk4::Align::Center);
    
    let radio_ptt_press = Arc::clone(radio);
    let radio_ptt_release = Arc::clone(radio);
    let gesture = gtk4::GestureClick::new();
    gesture.set_button(gtk4::gdk::BUTTON_PRIMARY);
    gesture.connect_pressed(move |_, _, _, _| {
        if let Ok(mut r) = radio_ptt_press.lock() {
            let _ = r.ptt_on();
        }
    });
    gesture.connect_released(move |_, _, _, _| {
        if let Ok(mut r) = radio_ptt_release.lock() {
            let _ = r.ptt_off();
        }
    });
    ptt_btn.add_controller(gesture);
    
    main_box.append(&ptt_btn);
    
    // Update loop
    let radio_update = Arc::clone(radio);
    let modem_label3 = modem_label2.clone();
    let rssi_sbar_clone = rssi_sbar.clone();
    let rssi_label_clone = rssi_label.clone();
    
    glib::timeout_add_local(Duration::from_millis(500), move || {
        if let Ok(r) = radio_update.lock() {
            let state = r.state();
            
            if state.connected {
                modem_label3.set_text("●");
                modem_label3.remove_css_class("status-icon-red");
                modem_label3.add_css_class("status-icon-green");
            } else {
                modem_label3.set_text("○");
                modem_label3.remove_css_class("status-icon-green");
                modem_label3.add_css_class("status-icon-red");
            }
            
            let s_val = (state.smeter_bars as f64).max(1.0).min(9.0);
            rssi_sbar_clone.set_value(s_val);
            rssi_label_clone.set_markup(&format!("<span color='#44dd66'>S{}</span>", state.smeter_bars));
        }
        glib::ControlFlow::Continue
    });
    
    // CSS
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
        .mode-badge {
            font-size: 18px;
            font-weight: bold;
            color: #33D17A;
            background: #1a2a1a;
            padding: 6px 20px;
            border-radius: 20px;
            border: 1px solid #33D17A;
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
        .s-meter {
            font-size: 14px;
            font-weight: bold;
            color: #FFB000;
        }
        levelbar > trough {
            background: #2a2a2a;
            border-radius: 4px;
        }
        levelbar > trough > block {
            background: #FFB000;
            border-radius: 4px;
        }
        .section-header {
            font-size: 12px;
            font-weight: bold;
            color: #666;
            letter-spacing: 2px;
        }
        .dim-label {
            color: #666;
            font-style: italic;
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
    
    window.set_content(Some(&main_box));
    window.show();
    
    // Focus will be managed by GTK's default focus chain
}