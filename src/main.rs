//! PocketModem libadwaita UI with native KV4P protocol implementation
//!
//! Uses libadwaita patterns for a modern GNOME-style interface:
//! - AdwCarousel for swipe navigation (FM, APRS Messages, Map)
//! - PreferencesGroup + ActionRow for settings
//! - ToastOverlay for notifications

mod aprs;
mod aprs_icons;
mod audio;
mod geoclue;
mod gps;
mod kiss;
mod map;
mod radio;
mod settings;
mod utils;
mod ui;

use aprs::{APRSMessage, APRSType, DirectMessage, DirectMessageStatus};
use audio::{AudioConfig, AudioManager};
use gps::GpsManager;
use map::MapManager;
use settings::{SettingsManager, Channel, Duplex, ToneMode, PowerLevel};
use utils::{calculate_maidenhead, calculate_distance_bearing, escape_markup, bearing_to_compass};

use radio::{KV4PRadio, SerialConfig};

#[cfg(feature = "notifications")]
use libnotify::init;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use adw::prelude::*;
use adw;

/// Map margins for easier carousel swiping (in pixels)
const MAP_MARGIN: i32 = 8;

const APP_ID: &str = "org.pocketmodem.pocket-modem";



// Signal that chat UI needs refresh
static CHAT_REFRESH_SIGNAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static THREAD_REFRESH_SIGNAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

// APRS Message Retransmission Constants (per APRS 1.2 specification)
// https://github.com/wb2osz/aprsspec
// 
// Standard APRS behavior:
// - Messages should be retried at 30-second intervals initially
// - Exponential backoff doubles the interval after each failed attempt: 30s → 60s → 120s → 240s → 480s → 960s
// - Maximum 5 retries (6 total attempts including initial transmission)
// - After 6 failed attempts with no ACK, the message is considered failed
const APRS_BASE_RETRY_INTERVAL_SECS: u64 = 30;  // Initial retry interval (30 seconds)
const APRS_MAX_RETRIES: u8 = 5;                  // Maximum retry count (max 6 total attempts)

fn main() {
    let settings = SettingsManager::new();
    eprintln!("[pocket-modem] Settings loaded: freq={} kHz, squelch={}", 
              settings.frequency(), settings.squelch());
    
    let serial_device = std::env::var("POCKET_MODEM_DEVICE").ok().unwrap_or_else(|| {
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

    let radio = Arc::new(Mutex::new(KV4PRadio::new(SerialConfig {
        port: serial_device,
        baudrate: 115200,
        timeout_ms: 500,
    })));
    let radio_clone = Arc::clone(&radio);

    let gps_manager = Arc::new(Mutex::new(GpsManager::new()));
    {
        let gps = gps_manager.lock().unwrap();
        gps.start();
    }

    let audio_config = AudioConfig {
        sample_rate: 16000,
        tx_gain: 1.0,
        rx_gain: 1.0,
        gate_threshold: 0.001,
        pre_emphasis_alpha: 0.0,
        hard_limit: 0.95,
    };
    let audio_manager = Arc::new(Mutex::new(AudioManager::new(audio_config)));
    
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
    
    app.connect_open(|_app, _files, _hint| {});
    
    let audio_for_shutdown = Arc::clone(&audio_manager);
    let gps_manager_activate = Arc::clone(&gps_manager);
    let audio_activate = Arc::clone(&audio_manager);
    
    let radio_for_ui = Arc::clone(&radio);
    let radio_for_shutdown = Arc::clone(&radio);
    app.connect_activate(move |app| {
        create_ui(app, &radio_for_ui, &audio_activate, &gps_manager_activate, &settings);
    });
    
    let gps_for_shutdown = Arc::clone(&gps_manager);
    app.connect_shutdown(move |_| {
        eprintln!("[pocket-modem] App shutting down...");
        if let Ok(mut a) = audio_for_shutdown.lock() {
            a.stop_capture();
            a.stop_playback();
        }
        if let Ok(r) = radio_for_shutdown.lock() {
            r.close();
        }
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
    // Window sized to fit screen
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(340)
        .default_height(700)
        .title("PocketModem")
        .width_request(340)
        .height_request(700)
        .build();
    
    window.set_resizable(false);
    
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
    
    // Header bar
    let header_bar = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("PocketModem", ""))
        .build();
    header_bar.set_size_request(330, 46);
    
    let settings_btn = gtk::ToggleButton::new();
    settings_btn.set_icon_name("emblem-system-symbolic");
    settings_btn.add_css_class("flat");
    settings_btn.set_tooltip_text(Some("Settings"));
    header_bar.pack_end(&settings_btn);
    
    // =========================================================================
    // MAIN PAGE - Fixed 320px Box container
    // =========================================================================
    let clamp = gtk::Box::new(gtk::Orientation::Vertical, 0);
    clamp.set_size_request(330, 700);
    clamp.set_hexpand(false);
    clamp.set_vexpand(false);
    clamp.set_halign(gtk::Align::Center);  // Center the 320px content
    
    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content_box.set_size_request(330, 700);
    content_box.set_hexpand(false);
    content_box.set_vexpand(true);
    content_box.set_halign(gtk::Align::Center);
    
    // --- Status indicators (clickable buttons) ---
    let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 32);
    status_row.set_halign(gtk::Align::Center);
    status_row.set_hexpand(false);
    status_row.set_margin_top(16);
    status_row.set_margin_bottom(16);
    
    // MODEM button
    let modem_status_btn = gtk::Button::new();
    modem_status_btn.add_css_class("flat");
    modem_status_btn.add_css_class("status-btn");
    modem_status_btn.set_tooltip_text(Some("Modem Settings"));
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
    modem_status_btn.set_child(Some(&modem_status_box));
    
    // GPS button
    let gps_status_btn = gtk::Button::new();
    gps_status_btn.add_css_class("flat");
    gps_status_btn.add_css_class("status-btn");
    gps_status_btn.set_tooltip_text(Some("GPS / APRS Settings"));
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
    gps_status_btn.set_child(Some(&gps_status_box));
    
    // AUDIO button
    let audio_status_btn = gtk::Button::new();
    audio_status_btn.add_css_class("flat");
    audio_status_btn.add_css_class("status-btn");
    audio_status_btn.set_tooltip_text(Some("Audio Settings"));
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
    audio_status_btn.set_child(Some(&audio_status_box));
    
    status_row.append(&modem_status_btn);
    status_row.append(&gps_status_btn);
    status_row.append(&audio_status_btn);
    
    content_box.append(&status_row);
    
    // --- Frequency display ---
    let freq_entry = gtk::Entry::new();
    freq_entry.set_text(&format!("{}.{:03}", saved_freq / 1000, saved_freq % 1000));
    gtk::prelude::EntryExt::set_alignment(&freq_entry, 0.5);
    freq_entry.add_css_class("freq-display");
    freq_entry.set_size_request(330, 100);
    freq_entry.set_margin_start(16);
    freq_entry.set_margin_end(16);
    freq_entry.set_margin_top(8);
    freq_entry.set_margin_bottom(4);
    freq_entry.set_editable(true);
    freq_entry.set_can_focus(true);
    freq_entry.set_input_purpose(gtk::InputPurpose::Number);
    
    let radio_freq = Arc::clone(radio);
    let settings_for_freq = settings as *const SettingsManager as *mut SettingsManager;
    freq_entry.connect_activate(move |entry| {
        let text = entry.text().to_string();
        
        if let Ok(freq_mhz) = text.parse::<f64>() {
            let khz = (freq_mhz * 1000.0) as u32;
            let radio = Arc::clone(&radio_freq);
            
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
        }
    });
    content_box.append(&freq_entry);
    
    // --- RSSI / S-meter ---
    let rssi_sbar = gtk::ProgressBar::new();
    rssi_sbar.set_fraction(0.0);
    rssi_sbar.add_css_class("rssi-bar");
    
    let smeter_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    smeter_box.set_halign(gtk::Align::Center);
    smeter_box.set_hexpand(false);
    smeter_box.set_margin_start(24);
    smeter_box.set_margin_end(24);
    smeter_box.set_margin_top(12);
    smeter_box.set_margin_bottom(20);
    smeter_box.set_valign(gtk::Align::Center);
    
    let signal_label = gtk::Label::new(Some("SIGNAL"));
    signal_label.add_css_class("signal-text");
    signal_label.set_valign(gtk::Align::Center);
    signal_label.set_width_request(60);
    
    rssi_sbar.set_hexpand(false);
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
    
    // --- Channel list ---
    let channel_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    channel_container.set_margin_start(16);
    channel_container.set_margin_end(16);
    
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
    add_channel_btn.set_tooltip_text(Some("Add current frequency"));
    
    channel_header.append(&channel_title);
    channel_header.append(&channel_header_spacer);
    channel_header.append(&add_channel_btn);
    
    let channel_list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    channel_list.add_css_class("channel-list");
    
    let no_channels_row = adw::ActionRow::builder()
        .title("No channels")
        .subtitle("Tap + to add")
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
                    ch, idx, settings, channel_list.clone(), no_channels_row.clone(),
                    radio.clone(), freq_entry.clone(), settings, current_channel_index.clone(),
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
        row_container.set_hexpand(false);
        
        if current_channel_index.load(std::sync::atomic::Ordering::SeqCst) == channel_index as i32 {
            row_container.add_css_class("channel-row-selected");
        }
        
        let row = adw::ActionRow::builder()
            .title(&channel.name)
            .subtitle(&subtitle)
            .build();
        row.set_hexpand(false);
        
        let row_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row_spacer.set_hexpand(true);  // Expand to push edit button to right
        
        let edit_btn = gtk::Button::new();
        edit_btn.set_icon_name("document-edit-symbolic");
        edit_btn.add_css_class("flat");
        
        row.add_controller({
            let click = gtk::GestureClick::new();
            click.set_button(1);
            let freq_entry = freq_entry.clone();
            let radio = radio.clone();
            let settings = settings_for_channel;
            let ch_freq = channel.rx_freq_khz;
            let ch_tone_mode = match channel.tone_mode {
                ToneMode::None => 0, ToneMode::Tone => 1, ToneMode::Tsql => 2,
            };
            let ch_ctone = channel.ctone_hz;
            let ch_rtone = channel.rtone_hz;
            let ch_squelch = channel.squelch;
            let ch_index = channel_index as i32;
            let cc_idx = current_channel_index.clone();
            let ch_list = channel_list.clone();
            let no_ch = no_channels_row.clone();
            
            click.connect_pressed(move |_, _, _, _| {
                let prev_idx = cc_idx.swap(ch_index, std::sync::atomic::Ordering::SeqCst);
                
                if prev_idx != ch_index {
                    refresh_channel_list(&ch_list, &no_ch, settings, &radio, freq_entry.clone(), cc_idx.clone());
                }
                
                freq_entry.set_text(&format!("{}.{:03}", ch_freq / 1000, ch_freq % 1000));
                unsafe { (*settings).set_frequency(ch_freq); }
                
                let r = radio.clone();
                std::thread::spawn(move || {
                    if let Ok(r) = r.lock() {
                        let _ = r.set_frequency_with_ctcss(ch_freq, ch_tone_mode, ch_ctone, ch_rtone);
                        let _ = r.set_squelch(ch_squelch);
                    }
                });
                
                // Also update global squelch setting to match channel
                unsafe { (*settings).set_squelch(ch_squelch); }
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
                btn, &edit_channel,
                move |updated: Channel| {
                    unsafe {
                        (*settings_edit).update_channel(channel_index, updated.clone());
                    }
                    let ch_freq = updated.rx_freq_khz;
                    let ch_tone_mode = match updated.tone_mode {
                        ToneMode::None => 0, ToneMode::Tone => 1, ToneMode::Tsql => 2,
                    };
                    let ch_ctone = updated.ctone_hz;
                    let ch_rtone = updated.rtone_hz;
                    cc_idx_save.store(channel_index as i32, std::sync::atomic::Ordering::SeqCst);
                    freq_save.set_text(&format!("{}.{:03}", ch_freq / 1000, ch_freq % 1000));
                    let r = radio_save.clone();
                    std::thread::spawn(move || {
                        if let Ok(mut r) = r.lock() {
                            let _ = r.set_frequency_with_ctcss(ch_freq, ch_tone_mode, ch_ctone, ch_rtone);
                        }
                    });
                    refresh_channel_list(&ch_list_save, &no_ch_save, settings_edit, &radio_save, freq_save, cc_idx_save.clone());
                },
                move || {
                    unsafe { (*settings_edit).delete_channel(channel_index); }
                    if cc_idx_delete.load(std::sync::atomic::Ordering::SeqCst) == channel_index as i32 {
                        cc_idx_delete.store(-1, std::sync::atomic::Ordering::SeqCst);
                    }
                    refresh_channel_list(&ch_list_delete, &no_ch_delete, settings_edit, &radio_delete, freq_delete, cc_idx_delete.clone());
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
                ch, idx, settings_add, channel_list_arc.clone(), (*no_channels_row_arc).clone(),
                radio_for_list.clone(), freq_entry_for_load.clone(), settings_add, current_channel_index_clone.clone(),
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
    let freq_entry_for_add = freq_entry.clone();
    
    add_channel_btn.connect_clicked(move |_| {
        unsafe {
            let freq = {
                if let Ok(r) = radio_for_add.lock() { r.state().frequency } else { 145500 }
            };
            
            let existing = (*settings_add).channels();
            let next_location = if existing.is_empty() { 1 } else { existing.iter().map(|c| c.location).max().unwrap_or(0) + 1 };
            
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
                squelch: settings::defaults::SQUELCH,
                comment: String::new(),
            };
            
            (*settings_add).add_channel(new_channel.clone());
            let new_index = (*settings_add).channels().len() - 1;
            eprintln!("[pocket-modem] Added channel: {}", next_location);
            
            no_channels_row_add.set_visible(false);
            
            let row = create_channel_row(
                &new_channel, new_index, settings_add, channel_list_add.clone(),
                no_channels_row_add.clone(), radio_for_add.clone(), freq_entry_for_add.clone(),
                settings_add, current_channel_index_add.clone(),
            );
            channel_list_add.append(&row);
            channel_list_add.show();
        }
    });
    
    let channel_scroll = gtk::ScrolledWindow::new();
    channel_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    channel_scroll.set_hexpand(false);
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
    
    let ptt_label = gtk::Label::new(Some("PTT"));
    ptt_label.add_css_class("ptt-label");
    
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
    let freq_entry_for_ptt_press = freq_entry.clone();
    let freq_entry_for_ptt_release = freq_entry.clone();
    
    let gesture = gtk::GestureClick::new();
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    gesture.set_button(0);
    
    gesture.connect_pressed({
        let r = radio_pressed.clone();
        let a = audio_pressed.clone();
        let label = ptt_label.clone();
        let freq_entry = freq_entry_for_ptt_press.clone();
        let settings = settings_for_ptt;
        move |_, _, _, _| {
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
                } else { rx_freq };
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
        move |_, _, _, _| {
            if let Ok(mut aud) = a.lock() { let _ = aud.stop_capture(); }
            
            let rx_freq = unsafe { (*settings).frequency() };
            
            if let Ok(mut rad) = r.lock() { let _ = rad.ptt_off(); }
            
            freq_entry.remove_css_class("tx-frequency");
            freq_entry.set_text(&format!("{}.{:03}", rx_freq / 1000, rx_freq % 1000));
            
            let rad = r.clone();
            std::thread::spawn(move || {
                if let Ok(rad) = rad.lock() { let _ = rad.set_frequency(rx_freq); }
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
    
    clamp.append(&content_box);
    
    // =========================================================================
    // APRS PAGE
    // =========================================================================
    
    let aprs_messages: Arc<Mutex<Vec<APRSMessage>>> = Arc::new(Mutex::new(Vec::new()));
    let aprs_messages_clone = Arc::clone(&aprs_messages);
    let aprs_last_displayed: Arc<std::sync::atomic::AtomicUsize> = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    
    // Reference to settings for APRS message management
    let settings_for_aprs = settings as *const SettingsManager as *mut SettingsManager;
    
    let aprs_list_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    aprs_list_box.set_margin_start(16);
    aprs_list_box.set_margin_end(16);
    aprs_list_box.set_margin_top(8);
    aprs_list_box.add_css_class("aprs-list");
    
    let aprs_empty_label = gtk::Label::new(Some("No APRS messages"));
    aprs_empty_label.set_halign(gtk::Align::Center);
    aprs_empty_label.add_css_class("aprs-empty-text");
    aprs_empty_label.set_visible(true);
    aprs_list_box.append(&aprs_empty_label);
    
    // Function to add a thread row button that opens chat
    let aprs_list_for_thread = aprs_list_box.clone();
    let aprs_empty_for_thread = aprs_empty_label.clone();
    let radio_for_thread = Arc::clone(radio);
    let settings_for_thread = settings as *const SettingsManager as *mut SettingsManager;
    
    // Build threads from stored messages and display as buttons
    fn build_and_display_threads(
        stored_msgs: &[DirectMessage],
        list_box: &gtk::Box,
        empty_label: &gtk::Label,
        radio: &Arc<Mutex<KV4PRadio>>,
        settings: *mut SettingsManager,
    ) {
        eprintln!("[pocket-modem] build_and_display_threads called with {} messages", stored_msgs.len());
        
        // Group messages by thread_id (other party's callsign)
        use std::collections::HashMap;
        let mut threads: HashMap<String, Vec<&DirectMessage>> = HashMap::new();
        
        for msg in stored_msgs {
            // Use thread_id which stores the "other" party's callsign
            let thread_key = if msg.thread_id.is_empty() {
                // Fallback: use from_callsign if thread_id is empty
                msg.from_callsign.clone()
            } else {
                msg.thread_id.clone()
            };
    
            if !thread_key.is_empty() {
                threads.entry(thread_key).or_default().push(msg);
            }
        }
        
        if threads.is_empty() {
            return;
        }
        
        empty_label.set_visible(false);
        
        for (other_call, messages) in threads {
            // Sort messages by timestamp
            let mut sorted = messages.clone();
            sorted.sort_by_key(|m| m.timestamp);
            let last_msg = sorted.last();
            let msg_count = sorted.len();
            
            // Create a button that acts as the thread row
            let thread_btn = gtk::Button::new();
            thread_btn.add_css_class("flat");
            thread_btn.set_hexpand(true);
            thread_btn.set_size_request(-1, 60);
            
            // Inner content box
            let thread_content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            thread_content.set_margin_start(12);
            thread_content.set_margin_end(12);
            thread_content.set_margin_top(8);
            thread_content.set_margin_bottom(8);
            thread_content.set_hexpand(true);
            
            // Callsign label
            let call_label = gtk::Label::new(None);
            call_label.set_markup(&format!("<span color='#FFB000' weight='bold'>{}</span>", escape_markup(&other_call)));
            call_label.set_size_request(100, -1);
            thread_content.append(&call_label);
            
            // Message count
            let count_label = gtk::Label::new(None);
            count_label.set_text(&format!("{} msg{}", msg_count, if msg_count != 1 { "s" } else { "" }));
            count_label.add_css_class("dimmed");
            thread_content.append(&count_label);
            
            // Last message preview
            if let Some(last) = last_msg {
                let preview = if last.body.len() > 30 { 
                    format!("{}...", &last.body[..27])
                } else {
                    last.body.clone()
                };
                let preview_label = gtk::Label::new(Some(&preview));
                preview_label.set_hexpand(true);
                preview_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                thread_content.append(&preview_label);
            }
            
            // Arrow indicator
            let arrow = gtk::Label::new(Some("→"));
            arrow.add_css_class("dimmed");
            thread_content.append(&arrow);
            
            thread_btn.set_child(Some(&thread_content));
            
            // Click handler
            let other_clone = other_call.clone();
            let radio_clone = Arc::clone(radio);
            let settings_clone = settings;
            thread_btn.connect_clicked(move |_| {
                eprintln!("[pocket-modem] Opening chat for thread: {}", other_clone);
                show_aprs_chat_screen(
                    None,
                    &other_clone,
                    settings_clone,
                    &radio_clone,
                    Box::new(|| {}),
                );
            });
            
            list_box.append(&thread_btn);
        }
    }
    
    // Load and display threads
    {
        let stored_msgs = unsafe { (*settings_for_aprs).aprs_messages() };
        if !stored_msgs.is_empty() {
            build_and_display_threads(
                stored_msgs,
                &aprs_list_box,
                &aprs_empty_label,
                radio,
                settings_for_aprs,
            );
        }
    }
    
    fn add_aprs_message_to_list(msg: &APRSMessage, list_box: &gtk::Box, empty_label: &gtk::Label, my_lat: f64, my_lon: f64) {
        empty_label.set_visible(false);
        
        let is_sent = msg.is_sent;
        
        let msg_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        msg_row.set_margin_top(8);
        msg_row.set_margin_bottom(8);
        msg_row.add_css_class("aprs-message-row");
        
        // Align sent messages to the right, received to the left
        if is_sent {
            msg_row.set_halign(gtk::Align::End);
            msg_row.set_margin_start(40);
            msg_row.add_css_class("aprs-message-sent");
        } else {
            msg_row.set_halign(gtk::Align::Start);
            msg_row.set_margin_end(40);
            msg_row.add_css_class("aprs-message-received");
        }
        
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.set_halign(gtk::Align::Start);
        
        let from_label = gtk::Label::new(None);
        from_label.set_markup(&format!("<span color='#FFB000'>{}</span>", escape_markup(&msg.from_callsign)));
        from_label.add_css_class("aprs-callsign");
        
        let to_label = gtk::Label::new(None);
        to_label.set_markup(&format!("→ <span color='#FFB000'>{}</span>", escape_markup(&msg.to_callsign)));
        to_label.add_css_class("aprs-to-callsign");
        
        let time_label = gtk::Label::new(None);
        let time_str = if let Some(ts) = msg.timestamp {
            format!("{:02}:{:02}", (ts / 3600) % 24, (ts / 60) % 60)
        } else { "--:--".to_string() };
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
        content.set_size_request(280, -1);
        
        match msg.msg_type {
            aprs::APRSType::Position => {
                if msg.position_lat != 0.0 || msg.position_lon != 0.0 {
                    let dist_bearing = calculate_distance_bearing(my_lat, my_lon, msg.position_lat, msg.position_lon);
                    let (dist_text, bearing_text) = if let Some((dist_km, bearing)) = dist_bearing {
                        let dist_str = if dist_km < 1.0 { format!("{:.0}m", dist_km * 1000.0) }
                            else if dist_km < 10.0 { format!("{:.1}km", dist_km) }
                            else { format!("{:.0}km", dist_km) };
                        (dist_str, bearing_to_compass(bearing))
                    } else { ("??".to_string(), "--°".to_string()) };
                    
                    let comment = if msg.comment.is_empty() { String::new() } else { format!(" - {}", escape_markup(&msg.comment)) };
                    content.set_markup(&format!("<span color='#33D17A'>📍 {} {}</span>{}", dist_text, bearing_text, comment));
                } else {
                    content.set_text(&msg.comment);
                    content.add_css_class("aprs-comment");
                }
            }
            aprs::APRSType::Message | aprs::APRSType::MessageAck => {
                let body = msg.msg_body.as_deref().unwrap_or("");
                let to = msg.to_callsign_msg.as_deref().unwrap_or(&msg.to_callsign);
                
                if msg.is_sent {
                    // This is a message we sent
                    let ack_indicator = if msg.is_acknowledged {
                        " <span color='#33D17A'>✓</span>"  // Green checkmark for ACK
                    } else {
                        " <span color='#F5A623'>⏳</span>"  // Yellow clock for pending
                    };
                    content.set_markup(&format!(
                        "<span color='#888888'>To {}:</span>{}<span color='#FFFFFF'>{}</span>",
                        escape_markup(to), ack_indicator, escape_markup(body)
                    ));
                    content.add_css_class("aprs-message-body");
                } else {
                    // Received message
                    content.set_markup(&format!(
                        "<span color='#888888'>From {}:</span>\n<span color='#CCE5FF'>{}</span>",
                        escape_markup(&msg.from_callsign), escape_markup(body)
                    ));
                    content.add_css_class("aprs-message-body");
                }
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
    aprs_page.set_size_request(330, 700);
    aprs_page.set_hexpand(false);
    
    // APRS Header with + button for new message
    let aprs_header_container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    aprs_header_container.set_margin_start(16);
    aprs_header_container.set_margin_end(16);
    aprs_header_container.set_margin_top(16);
    aprs_header_container.set_margin_bottom(8);
    
    let aprs_header = gtk::Label::new(Some("<b>APRS Messages</b>"));
    aprs_header.set_markup("<b>APRS Messages</b>");
    aprs_header.set_halign(gtk::Align::Start);
    aprs_header.set_valign(gtk::Align::Center);
    aprs_header.set_hexpand(true);
    
    // + button for new direct message
    let aprs_new_msg_btn = gtk::Button::new();
    aprs_new_msg_btn.set_icon_name("list-add-symbolic");
    aprs_new_msg_btn.add_css_class("flat");
    aprs_new_msg_btn.set_tooltip_text(Some("New direct message"));
    aprs_new_msg_btn.set_valign(gtk::Align::Center);
    
    aprs_header_container.append(&aprs_header);
    aprs_header_container.append(&aprs_new_msg_btn);
    
    // New message button handler
    let app_clone = app.clone();
    let radio_for_chat = Arc::clone(radio);
    let settings_for_chat = settings as *const SettingsManager as *mut SettingsManager;
    aprs_new_msg_btn.connect_clicked(move |_| {
        // Show dialog to get recipient callsign
        let dialog = gtk::Dialog::with_buttons(
            Some("New Direct Message"),
            app_clone.active_window().as_ref(),
            gtk::DialogFlags::MODAL,
            &[("Cancel", gtk::ResponseType::Cancel), ("Next", gtk::ResponseType::Accept)],
        );
        
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_start(20);
        content.set_margin_end(20);
        content.set_margin_top(20);
        content.set_margin_bottom(20);
        
        let label = gtk::Label::new(Some("Enter recipient callsign:"));
        label.set_halign(gtk::Align::Start);
        content.append(&label);
        
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("e.g., KD4LCD-7 (UPPERCASE)"));
        entry.set_input_purpose(gtk::InputPurpose::Alpha);
        entry.set_hexpand(true);
        entry.set_css_classes(&["monospace"]);
        // Note: uppercase hint shown in placeholder; conversion happens on Next/Activate
        
        // Create clones BEFORE using entry by reference
        let entry_next = entry.clone();
        let entry_enter = entry.clone();
        let entry_response = entry.clone();
        
        // Now append entry
        content.append(&entry);
        
        // Add button row
        let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        button_box.set_halign(gtk::Align::End);
        
        let cancel_btn = gtk::Button::with_label("Cancel");
        let next_btn = gtk::Button::with_label("Next");
        next_btn.add_css_class("suggested-action");
        
        button_box.append(&cancel_btn);
        button_box.append(&next_btn);
        content.append(&button_box);
        
        dialog.set_child(Some(&content));
        
        // Next button - emit Accept response
        let dialog_next = dialog.clone();
        next_btn.connect_clicked(move |_| {
            let t = entry_next.text().to_string().trim().to_uppercase();
            if !t.is_empty() {
                dialog_next.response(gtk::ResponseType::Accept);
            }
        });
        
        // Cancel button - emit Cancel response
        let dialog_cancel = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dialog_cancel.response(gtk::ResponseType::Cancel);
        });
        
        // Enter key - same as Next (emit Accept response)
        let dialog_enter = dialog.clone();
        let entry_for_enter = entry_enter.clone();
        entry_enter.connect_activate(move |_| {
            let t = entry_for_enter.text().to_string().trim().to_uppercase();
            if !t.is_empty() {
                dialog_enter.response(gtk::ResponseType::Accept);
            }
        });
        
        // Handle dialog close - only open chat if Next was clicked (response = Accept)
        // Read the text at response time, not when handler is set up
        let entry_for_response = entry_response.clone();
        // Clone before move - Fn closure can't capture by reference
        let radio_for_dialog = radio_for_chat.clone();
        dialog.connect_response(move |d, response| {
            let recipient = entry_for_response.text().to_string().trim().to_uppercase();
            eprintln!("[pocket-modem] Dialog response: {:?}, recipient: '{}'", response, recipient);
            // Always close the dialog on any response
            d.close();
            // Only open chat if Next was clicked (Accept) and we have a recipient
            if response == gtk::ResponseType::Accept && !recipient.is_empty() {
                eprintln!("[pocket-modem] Opening chat window for {}", recipient);
                show_aprs_chat_screen(
                    None,
                    &recipient,
                    settings_for_chat,
                    &radio_for_dialog,
                    Box::new(|| {}),
                );
            }
        });
        
        dialog.show();
    });
    
    // Beacon status FSM
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum BeaconState {
        NeedsConfig,     // No callsign configured
        NeedsTxEnabled,  // Callsign set but TX disabled
        NeedsGpsFix,     // TX enabled but no GPS fix
        Ready,           // Ready to beacon
        Transmitting,    // Currently transmitting
        Success,         // Last beacon succeeded
        Failed,          // Last beacon failed
    }
    
    impl BeaconState {
        fn message(&self) -> &'static str {
            match self {
                BeaconState::NeedsConfig  => "Enter callsign in Settings",
                BeaconState::NeedsTxEnabled => "Enable APRS TX in Settings",
                BeaconState::NeedsGpsFix  => "Waiting for GPS fix...",
                BeaconState::Ready        => "Ready",
                BeaconState::Transmitting => "Transmitting...",
                BeaconState::Success      => "Beacon sent!",
                BeaconState::Failed       => "TX failed - retry",
            }
        }
    }
    
    let beacon_state: Arc<Mutex<BeaconState>> = Arc::new(Mutex::new(BeaconState::NeedsConfig));
    
    // Beacon button (wider than PTT, 2/3 height)
    let beacon_btn = gtk::Button::new();
    beacon_btn.set_tooltip_text(Some("Send APRS beacon"));
    beacon_btn.set_size_request(280, 67);
    beacon_btn.set_hexpand(false);
    beacon_btn.add_css_class("beacon-button");
    
    let beacon_inner = gtk::Box::new(gtk::Orientation::Vertical, 4);
    beacon_inner.set_halign(gtk::Align::Center);
    beacon_inner.set_valign(gtk::Align::Center);
    beacon_inner.set_hexpand(true);
    
    let beacon_icon = gtk::Image::from_icon_name("radio-symbolic");
    beacon_icon.set_pixel_size(24);
    beacon_icon.add_css_class("beacon-icon");
    
    let beacon_label = gtk::Label::new(Some("Beacon"));
    beacon_label.add_css_class("beacon-label");
    
    beacon_inner.append(&beacon_icon);
    beacon_inner.append(&beacon_label);
    beacon_btn.set_child(Some(&beacon_inner));
    
    // Beacon status (above button)
    let beacon_status = gtk::Label::new(Some(BeaconState::NeedsConfig.message()));
    beacon_status.add_css_class("beacon-status");
    beacon_status.set_margin_bottom(8);
    beacon_status.set_halign(gtk::Align::Center);
    
    // Beacon container (status above, button below) - matches PTT layout
    let beacon_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    beacon_container.set_halign(gtk::Align::Center);
    beacon_container.set_valign(gtk::Align::End);
    beacon_container.set_vexpand(false);
    beacon_container.set_margin_start(20);
    beacon_container.set_margin_end(20);
    beacon_container.set_margin_top(8);
    beacon_container.set_margin_bottom(16);
    beacon_container.append(&beacon_status);
    beacon_container.append(&beacon_btn);
    
    let aprs_scroll = gtk::ScrolledWindow::new();
    aprs_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    aprs_scroll.set_hexpand(false);
    aprs_scroll.set_vexpand(true);
    aprs_scroll.set_min_content_height(200);
    aprs_scroll.set_child(Some(&aprs_list_box));
    
    aprs_page.append(&aprs_header_container);
    aprs_page.append(&aprs_scroll);
    aprs_page.append(&beacon_container);
    
    // APRS container - fixed 340px
    let aprs_clamp = gtk::Box::new(gtk::Orientation::Vertical, 0);
    aprs_clamp.set_size_request(330, 700);
    aprs_clamp.set_hexpand(false);
    aprs_clamp.append(&aprs_page);
    
    // =========================================================================
    // MAP PAGE (libshumate with VectorRenderer + GNOME tileserver)
    // =========================================================================
    let map_manager = Arc::new(Mutex::new(MapManager::new()));
    
    // Detect dark mode via Adw::StyleManager
    let dark_mode = adw::StyleManager::default().is_dark();
    eprintln!("[pocket-modem] Initial dark mode: {}", dark_mode);
    
    // Store dark mode state for polling in UI timer
    let dark_mode_state: Arc<std::sync::atomic::AtomicBool> = Arc::new(std::sync::atomic::AtomicBool::new(dark_mode));
    
    // Initialize the map with vector tiles
    {
        let mut mm = map_manager.lock().unwrap();
        mm.initialize(dark_mode);
    }
    
    let map_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    map_page.set_size_request(320, -1);
    map_page.set_hexpand(false);
    map_page.set_vexpand(true);
    map_page.set_halign(gtk::Align::Center);
    // Side margins create swipe zones for carousel
    map_page.set_margin_top(MAP_MARGIN);
    map_page.set_margin_bottom(MAP_MARGIN);
    map_page.set_margin_start(4);
    map_page.set_margin_end(4);
    
    // Get the libshumate Map widget
    let map_view = {
        let mm = map_manager.lock().unwrap();
        mm.view().clone()
    };
    // Fill available space with padding for easier carousel swiping
    map_view.set_hexpand(false);
    map_view.set_vexpand(true);
    map_view.set_valign(gtk::Align::Fill);
    
    // Create re-center button (positioned at bottom-right of map)
    let recenter_btn = gtk::Button::new();
    recenter_btn.set_icon_name("find-location-symbolic");
    recenter_btn.add_css_class("map-recenter-btn");
    recenter_btn.set_tooltip_text(Some("Center on my location"));
    recenter_btn.set_halign(gtk::Align::End);
    recenter_btn.set_valign(gtk::Align::End);
    recenter_btn.set_margin_end(12);
    recenter_btn.set_margin_bottom(12);
    recenter_btn.set_size_request(40, 40);
    
    // Connect re-center button to map manager
    let map_manager_for_recenter = Arc::clone(&map_manager);
    recenter_btn.connect_clicked(move |_| {
        if let Ok(mut mm) = map_manager_for_recenter.lock() {
            mm.center_on_user();
        }
    });
    
    // Use Overlay to stack the button on top of the map
    let map_overlay = gtk::Overlay::new();
    map_overlay.set_child(Some(&map_view));
    map_overlay.add_overlay(&recenter_btn);
    
    map_page.append(&map_overlay);
    
    // Map container with margins for carousel
    let map_clamp = adw::Clamp::new();
    map_clamp.set_size_request(340, 700);
    map_clamp.set_hexpand(false);
    map_clamp.set_vexpand(false);
    map_clamp.set_maximum_size(340);
    map_clamp.set_tightening_threshold(340);
    map_clamp.set_child(Some(&map_page));
    
    // =========================================================================
    // CAROUSEL
    // =========================================================================
    let carousel = adw::Carousel::new();
    carousel.set_interactive(true);
    carousel.set_hexpand(false);
    carousel.set_vexpand(true);
    carousel.set_size_request(360, 654);  // Constrain carousel to prevent expansion
    carousel.set_halign(gtk::Align::Center);
    
    // Wrap carousel in a constrained box
    let carousel_wrapper = gtk::Box::new(gtk::Orientation::Vertical, 0);
    carousel_wrapper.set_size_request(358, 654);
    carousel_wrapper.set_hexpand(false);
    carousel_wrapper.set_vexpand(true);
    carousel_wrapper.set_halign(gtk::Align::Center);
    carousel_wrapper.append(&carousel);
    carousel_wrapper.add_css_class("carousel-wrapper");
    
    carousel.append(&clamp);
    carousel.append(&aprs_clamp);
    carousel.append(&map_clamp);
    
    // Use wrapper instead of carousel directly for carousel_box
    
    let indicator = adw::CarouselIndicatorDots::new();
    indicator.set_carousel(Some(&carousel));
    indicator.set_halign(gtk::Align::Center);
    indicator.set_margin_bottom(8);
    
    let carousel_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    carousel_box.set_size_request(358, 700);
    carousel_box.set_hexpand(false);
    carousel_box.append(&carousel_wrapper);
    carousel_box.append(&indicator);
    
    // =========================================================================
    // SETTINGS PAGE (all settings combined)
    // =========================================================================
    let settings_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    settings_page.set_size_request(330, 700);
    settings_page.set_hexpand(false);
    
    let settings_scroll = gtk::ScrolledWindow::new();
    settings_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    settings_scroll.set_hexpand(false);
    settings_scroll.set_vexpand(true);
    settings_scroll.set_min_content_height(600);
    
    let settings_box = gtk::Box::new(gtk::Orientation::Vertical, 16);
    settings_box.set_vexpand(false);
    settings_box.set_valign(gtk::Align::Start);  // Don't stretch, just take needed space
    settings_box.set_margin_top(12);
    settings_box.set_margin_start(12);
    settings_box.set_margin_end(12);
    settings_box.set_margin_bottom(200);  // Extra bottom margin for scrolling
    
    // === MODEM Section ===
    let modem_group = adw::PreferencesGroup::builder()
        .title("Modem")
        .build();
    
    // Squelch level using SpinRow
    let radio_sq = Arc::clone(radio);
    let settings_sq = settings as *const SettingsManager as *mut SettingsManager;
    let last_sent_sq: Arc<std::sync::atomic::AtomicU8> = Arc::new(std::sync::atomic::AtomicU8::new(saved_squelch));
    
    let adj_sq = gtk::Adjustment::new(saved_squelch as f64, 0.0, 8.0, 1.0, 0.0, 0.0);
    let squelch_row = adw::SpinRow::builder()
        .title("Squelch Level")
        .subtitle("Signal threshold for audio output")
        .adjustment(&adj_sq)
        .build();
    squelch_row.set_digits(0);
    squelch_row.set_numeric(true);
    squelch_row.set_snap_to_ticks(true);
    
    // Connect to adjustment's value-changed signal
    let sent_clone = Arc::clone(&last_sent_sq);
    let r_clone = Arc::clone(&radio_sq);
    let s_clone = settings_sq;
    adj_sq.connect_value_changed(move |adj| {
        let level = adj.value() as u8;
        if level != sent_clone.load(std::sync::atomic::Ordering::SeqCst) {
            unsafe { (*s_clone).set_squelch(level); }
            let r = r_clone.clone();
            let sent = Arc::clone(&sent_clone);
            std::thread::spawn(move || {
                if let Ok(r) = r.lock() { let _ = r.set_squelch(level); }
                sent.store(level, std::sync::atomic::Ordering::SeqCst);
            });
        }
    });
    modem_group.add(&squelch_row);
    
    // TX Power using ToggleGroup
    let radio_tg = Arc::clone(radio);
    let settings_tg = settings as *const SettingsManager as *mut SettingsManager;
    let initial_high = unsafe { (*settings).tx_power_high() };
    
    let toggle_group = adw::ToggleGroup::builder()
        .homogeneous(true)
        .build();
    
    let toggle_low = adw::Toggle::builder()
        .name("low")
        .child(&gtk::Label::new(Some("Low")))
        .build();
    
    let toggle_high = adw::Toggle::builder()
        .name("high")
        .child(&gtk::Label::new(Some("High")))
        .build();
    
    // IMPORTANT: Add toggles BEFORE setting active_name
    toggle_group.add(toggle_low);
    toggle_group.add(toggle_high);
    
    // Now set the active toggle by name (after toggles exist)
    toggle_group.set_active_name(Some(if initial_high { "high" } else { "low" }));
    
    let radio_notify = Arc::clone(&radio_tg);
    let settings_notify = settings_tg;
    toggle_group.connect_active_name_notify(move |group| {
        let is_high = group.active_name().as_deref() == Some("high");
        unsafe { (*settings_notify).set_tx_power_high(is_high); }
        let r = radio_notify.clone();
        std::thread::spawn(move || {
            if let Ok(r) = r.lock() { let _ = r.set_power(is_high); }
        });
    });
    
    let tx_power_row = adw::ActionRow::builder()
        .title("TX Power")
        .build();
    tx_power_row.add_suffix(&toggle_group);
    tx_power_row.set_activatable_widget(Some(&toggle_group));
    modem_group.add(&tx_power_row);
    
    settings_box.append(&modem_group);
    
    // === AUDIO Section ===
    let audio_group = adw::PreferencesGroup::builder()
        .title("Audio")
        .build();
    
    let settings_audio = settings as *const SettingsManager as *mut SettingsManager;
    
    let pre_emph_row = adw::SwitchRow::builder()
        .title("Pre-Emphasis")
        .subtitle("Boost high frequencies before TX")
        .active(unsafe { (*settings).pre_emphasis() })
        .build();
    pre_emph_row.connect_notify_local(Some("active"), move |row, _| {
        unsafe { (*settings_audio).set_pre_emphasis(row.is_active()); }
    });
    audio_group.add(&pre_emph_row);
    
    let settings_deemph = settings as *const SettingsManager as *mut SettingsManager;
    let de_emph_row = adw::SwitchRow::builder()
        .title("De-Emphasis")
        .subtitle("Reduce high frequencies on RX")
        .active(unsafe { (*settings).de_emphasis() })
        .build();
    de_emph_row.connect_notify_local(Some("active"), move |row, _| {
        unsafe { (*settings_deemph).set_de_emphasis(row.is_active()); }
    });
    audio_group.add(&de_emph_row);
    
    let settings_hpf = settings as *const SettingsManager as *mut SettingsManager;
    let hpf_row = adw::SwitchRow::builder()
        .title("High-Pass Filter")
        .subtitle("Remove low frequency rumble (300Hz cutoff)")
        .active(unsafe { (*settings).high_pass_filter() })
        .build();
    hpf_row.connect_notify_local(Some("active"), move |row, _| {
        unsafe { (*settings_hpf).set_high_pass_filter(row.is_active()); }
    });
    audio_group.add(&hpf_row);
    
    let settings_lpf = settings as *const SettingsManager as *mut SettingsManager;
    let lpf_row = adw::SwitchRow::builder()
        .title("Low-Pass Filter")
        .subtitle("Remove high frequency hiss (3.4kHz cutoff)")
        .active(unsafe { (*settings).low_pass_filter() })
        .build();
    lpf_row.connect_notify_local(Some("active"), move |row, _| {
        unsafe { (*settings_lpf).set_low_pass_filter(row.is_active()); }
    });
    audio_group.add(&lpf_row);
    
    let settings_mic = settings as *const SettingsManager as *mut SettingsManager;
    let mic_gains = vec!["None".to_string(), "Low".to_string(), "Medium".to_string(), "High".to_string()];
    let mic_model = gtk::StringList::new(&mic_gains.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    let current_mic = unsafe { (*settings).mic_gain() };
    let mic_idx = mic_gains.iter().position(|g| {
        g.to_lowercase() == current_mic.to_lowercase()
    }).unwrap_or(0) as u32;
    
    let mic_gain_row = adw::ComboRow::builder()
        .title("Mic Gain")
        .subtitle("Adjust microphone input level")
        .selected(mic_idx)
        .model(&mic_model)
        .build();
    mic_gain_row.connect_notify_local(Some("selected"), move |row, _| {
        let idx = row.selected() as usize;
        if idx < mic_gains.len() {
            unsafe { (*settings_mic).set_mic_gain(&mic_gains[idx].to_lowercase()); }
        }
    });
    audio_group.add(&mic_gain_row);
    
    settings_box.append(&audio_group);
    
    // === APRS TX Section ===
    let aprs_group = adw::PreferencesGroup::builder()
        .title("APRS TX")
        .build();
    
    // Callsign input
    let settings_aprs_call = settings as *const SettingsManager as *mut SettingsManager;
    let aprs_call_entry = gtk::Entry::new();
    aprs_call_entry.set_placeholder_text(Some("KD4LCD (UPPERCASE)"));
    aprs_call_entry.set_hexpand(true);
    aprs_call_entry.set_max_width_chars(8);
    aprs_call_entry.set_css_classes(&["monospace"]);
    // Note: uppercase hint shown in placeholder; conversion happens on settings save
    let current_call = unsafe { (*settings).aprs_callsign().to_string() };
    if !current_call.is_empty() {
        aprs_call_entry.set_text(&current_call);
    }
    
    let aprs_call_row = adw::ActionRow::builder()
        .title("Callsign")
        .subtitle("Your amateur radio callsign")
        .build();
    aprs_call_row.add_suffix(&aprs_call_entry);
    aprs_call_row.set_activatable_widget(Some(&aprs_call_entry));
    
    let settings_ssid = settings as *const SettingsManager as *mut SettingsManager;
    let ssid_adj = gtk::Adjustment::new(0.0, 0.0, 15.0, 1.0, 0.0, 0.0);
    let current_ssid = unsafe { (*settings).aprs_ssid() };
    ssid_adj.set_value(current_ssid as f64);
    let aprs_ssid_spin = adw::SpinRow::builder()
        .title("SSID")
        .subtitle("Secondary station identifier (0-15)")
        .adjustment(&ssid_adj)
        .build();
    
    // Symbol picker (ComboRow)
    let aprs_symbols = vec![
        ("Car 🚗", '/', '>'),  // Primary table: car
        ("Person 👤", '\\', '['),  // Alternate table: person
        ("Aircraft ✈️", '\\', '\''),  // Alternate table: aircraft  
        ("Balloon 🎈", '/', 'O'),
        ("House 🏠", '/', '*'),
        ("Bike 🚴", '/', 'b'),
        ("Sailboat ⛵", '/', 'S'),
        ("RV 🚐", '/', 'R'),
    ];
    let symbol_labels: Vec<String> = aprs_symbols.iter().map(|(l, _, _)| l.to_string()).collect();
    let symbol_model = gtk::StringList::new(&symbol_labels.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    
    // Find current symbol index
    let current_symbol_table = unsafe { (*settings).aprs_symbol_table() };
    let current_symbol_code = unsafe { (*settings).aprs_symbol_code() };
    let current_symbol_idx = aprs_symbols.iter().position(|(_, t, c)| *t == current_symbol_table && *c == current_symbol_code).unwrap_or(0) as u32;
    
    let aprs_symbol_row = adw::ComboRow::builder()
        .title("Symbol")
        .subtitle("APRS symbol for your station")
        .model(&symbol_model)
        .build();
    
    // Set selection after build (builder's selected() doesn't always work)
    aprs_symbol_row.set_selected(current_symbol_idx);
    
    // Comment input
    let settings_aprs_comment = settings as *const SettingsManager as *mut SettingsManager;
    let aprs_comment_entry = gtk::Entry::new();
    aprs_comment_entry.set_placeholder_text(Some("PocketModem on KV4P"));
    aprs_comment_entry.set_hexpand(true);
    let current_comment = unsafe { (*settings).aprs_comment().to_string() };
    if !current_comment.is_empty() {
        aprs_comment_entry.set_text(&current_comment);
    }
    
    let aprs_comment_row = adw::ActionRow::builder()
        .title("Comment")
        .subtitle("Optional beacon comment")
        .build();
    aprs_comment_row.add_suffix(&aprs_comment_entry);
    aprs_comment_row.set_activatable_widget(Some(&aprs_comment_entry));
    
    // Beacon destination input
    let settings_aprs_dest = settings as *const SettingsManager as *mut SettingsManager;
    let aprs_dest_entry = gtk::Entry::new();
    aprs_dest_entry.set_placeholder_text(Some("APRS"));
    aprs_dest_entry.set_hexpand(true);
    aprs_dest_entry.set_max_length(9);
    let current_dest = unsafe { (*settings).aprs_beacon_dest().to_string() };
    aprs_dest_entry.set_text(&current_dest);
    
    let aprs_dest_row = adw::ActionRow::builder()
        .title("Beacon Destination")
        .subtitle("Destination callsign (default: APRS)")
        .build();
    aprs_dest_row.add_suffix(&aprs_dest_entry);
    aprs_dest_row.set_activatable_widget(Some(&aprs_dest_entry));
    
    // TX enable switch
    let settings_tx = settings as *const SettingsManager as *mut SettingsManager;
    let current_tx_enabled = unsafe { (*settings).aprs_tx_enabled() };
    let aprs_tx_row = adw::SwitchRow::builder()
        .title("Enable APRS TX")
        .subtitle("Allow beacon transmission")
        .active(current_tx_enabled)
        .build();
    
    aprs_group.add(&aprs_call_row);
    aprs_group.add(&aprs_ssid_spin);
    aprs_group.add(&aprs_symbol_row);
    aprs_group.add(&aprs_comment_row);
    aprs_group.add(&aprs_dest_row);
    aprs_group.add(&aprs_tx_row);
    
    settings_box.append(&aprs_group);
    
    // === APRS Messages Section ===
    let aprs_data_group = adw::PreferencesGroup::builder()
        .title("APRS Messages")
        .build();
    
    // Storage size row
    let aprs_storage_row = adw::ActionRow::builder()
        .title("Storage used")
        .subtitle("--")
        .build();
    
    // Initialize APRS settings button
    let aprs_init_row = adw::ActionRow::builder()
        .title("Initialize APRS settings")
        .subtitle("Reset APRS configuration to defaults")
        .build();
    let aprs_init_btn = gtk::Button::new();
    aprs_init_btn.set_label("Initialize");
    aprs_init_btn.add_css_class("flat");
    aprs_init_row.add_suffix(&aprs_init_btn);
    aprs_init_row.set_activatable_widget(Some(&aprs_init_btn));
    
    // Clear APRS data button
    let aprs_clear_row = adw::ActionRow::builder()
        .title("Clear APRS data")
        .subtitle("Delete all stored messages")
        .build();
    let aprs_clear_btn = gtk::Button::new();
    aprs_clear_btn.set_label("Clear");
    aprs_clear_btn.add_css_class("destructive-action");
    aprs_clear_row.add_suffix(&aprs_clear_btn);
    aprs_clear_row.set_activatable_widget(Some(&aprs_clear_btn));
    
    aprs_data_group.add(&aprs_storage_row);
    aprs_data_group.add(&aprs_init_row);
    aprs_data_group.add(&aprs_clear_row);
    
    settings_box.append(&aprs_data_group);
    
    // Initialize APRS settings handler
    let aprs_init_settings = settings as *const SettingsManager as *mut SettingsManager;
    aprs_init_btn.connect_clicked(move |_| {
        unsafe {
            (*aprs_init_settings).initialize_aprs_settings();
            eprintln!("[pocket-modem] APRS settings initialized");
        }
    });
    
    // Clear APRS data handler - clone the row for updating display
    let aprs_clear_settings = settings as *const SettingsManager as *mut SettingsManager;
    let aprs_storage_for_clear = aprs_storage_row.clone();
    aprs_clear_btn.connect_clicked(move |_| {
        unsafe {
            (*aprs_clear_settings).clear_aprs_messages();
            // Update storage display
            let size = (*aprs_clear_settings).aprs_messages_storage_size_display();
            aprs_storage_for_clear.set_subtitle(&size);
            eprintln!("[pocket-modem] APRS messages cleared");
        }
    });
    
    // === About Section ===
    
    // APRS callsign handler (uppercase conversion done in display handler)
    let settings_aprs_1 = settings as *const SettingsManager as *mut SettingsManager;
    aprs_call_entry.connect_changed(move |entry| {
        let text = entry.text().to_string().trim().to_uppercase();
        unsafe { (*settings_aprs_1).set_aprs_callsign(&text); }
    });
    
    let settings_aprs_2 = settings as *const SettingsManager as *mut SettingsManager;
    ssid_adj.connect_value_changed(move |adj| {
        let ssid = adj.value() as u8;
        unsafe { (*settings_aprs_2).set_aprs_ssid(ssid); }
    });
    
    let settings_aprs_3 = settings as *const SettingsManager as *mut SettingsManager;
    let aprs_symbols_clone = aprs_symbols.clone();
    aprs_symbol_row.connect_notify_local(Some("selected"), move |row, _| {
        let idx = row.selected() as usize;
        if idx < aprs_symbols_clone.len() {
            let (_, table, code) = &aprs_symbols_clone[idx];
            unsafe {
                (*settings_aprs_3).set_aprs_symbol_table(*table);
                (*settings_aprs_3).set_aprs_symbol_code(*code);
            }
        }
    });
    
    let aprs_comment_for_handler = aprs_comment_entry.clone();
    let settings_aprs_4 = settings as *const SettingsManager as *mut SettingsManager;
    aprs_comment_entry.connect_changed(move |entry| {
        let text = entry.text().to_string();
        unsafe { (*settings_aprs_4).set_aprs_comment(&text); }
    });
    
    let settings_aprs_4b = settings as *const SettingsManager as *mut SettingsManager;
    aprs_dest_entry.connect_changed(move |entry| {
        let text = entry.text().to_string().trim().to_uppercase();
        if !text.is_empty() {
            unsafe { (*settings_aprs_4b).set_aprs_beacon_dest(&text); }
        }
    });
    
    let settings_aprs_5 = settings as *const SettingsManager as *mut SettingsManager;
    aprs_tx_row.connect_notify_local(Some("active"), move |row, _| {
        unsafe { (*settings_aprs_5).set_aprs_tx_enabled(row.is_active()); }
    });
    
    let about_group = adw::PreferencesGroup::builder()
        .title("About")
        .build();
    
    let version_row = adw::ActionRow::builder()
        .title("PocketModem")
        .subtitle("Version 1.0.0 - KV4P Radio Controller")
        .build();
    about_group.add(&version_row);
    
    settings_box.append(&about_group);
    
    settings_scroll.set_child(Some(&settings_box));
    settings_page.append(&settings_scroll);
    
    // =========================================================================
    // ViewStack
    // =========================================================================
    let stack = adw::ViewStack::new();
    stack.set_size_request(360, 654);
    stack.set_hexpand(false);
    stack.set_vexpand(false);
    stack.set_halign(gtk::Align::Center);
    stack.set_valign(gtk::Align::Start);
    
    stack.add_titled(&carousel_box, Some("main"), "Main");
    stack.add_titled(&settings_page, Some("settings"), "Settings");
    
    let stack_for_toggle = stack.clone();
    settings_btn.connect_toggled(move |btn| {
        if btn.is_active() { stack_for_toggle.set_visible_child_name("settings"); }
        else { stack_for_toggle.set_visible_child_name("main"); }
    });
    
    // =========================================================================
    // Detail popup windows for MODEM, GPS, AUDIO
    // =========================================================================
    
    // MODEM detail popup
    let modem_detail_window = gtk::Window::builder()
        .title("MODEM")
        .default_width(340)
        .default_height(300)
        .decorated(false)
        .modal(true)
        .build();
    
    let modem_title = gtk::Label::new(Some("MODEM"));
    modem_title.add_css_class("title-label");
    
    let modem_detail_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    modem_detail_box.set_margin_start(12);
    modem_detail_box.set_margin_end(12);
    
    let modem_freq_row = adw::ActionRow::new();
    modem_freq_row.set_title("Frequency");
    let modem_freq_value = gtk::Label::new(Some("---"));
    modem_freq_value.add_css_class("status-text");
    modem_freq_row.add_suffix(&modem_freq_value);
    
    let modem_rssi_row = adw::ActionRow::new();
    modem_rssi_row.set_title("RSSI");
    let modem_rssi_value = gtk::Label::new(Some("---"));
    modem_rssi_value.add_css_class("status-text");
    modem_rssi_row.add_suffix(&modem_rssi_value);
    
    let modem_mode_row = adw::ActionRow::new();
    modem_mode_row.set_title("Mode");
    let modem_mode_value = gtk::Label::new(Some("---"));
    modem_mode_value.add_css_class("status-text");
    modem_mode_row.add_suffix(&modem_mode_value);
    
    let modem_tx_row = adw::ActionRow::new();
    modem_tx_row.set_title("TX/RX");
    let modem_tx_value = gtk::Label::new(Some("---"));
    modem_tx_value.add_css_class("status-text");
    modem_tx_row.add_suffix(&modem_tx_value);
    
    let modem_fw_row = adw::ActionRow::new();
    modem_fw_row.set_title("Firmware");
    let modem_fw_value = gtk::Label::new(Some("---"));
    modem_fw_value.add_css_class("status-text");
    modem_fw_row.add_suffix(&modem_fw_value);
    
    let modem_conn_row = adw::ActionRow::new();
    modem_conn_row.set_title("Connection");
    let modem_conn_value = gtk::Label::new(Some("USB"));
    modem_conn_value.add_css_class("status-text");
    modem_conn_row.add_suffix(&modem_conn_value);
    
    let modem_band_row = adw::ActionRow::new();
    modem_band_row.set_title("Band");
    let modem_band_value = gtk::Label::new(Some("---"));
    modem_band_value.add_css_class("status-text");
    modem_band_row.add_suffix(&modem_band_value);
    
    let modem_detail_group = adw::PreferencesGroup::new();
    modem_detail_group.add(&modem_freq_row);
    modem_detail_group.add(&modem_rssi_row);
    modem_detail_group.add(&modem_mode_row);
    modem_detail_group.add(&modem_tx_row);
    modem_detail_group.add(&modem_fw_row);
    modem_detail_group.add(&modem_conn_row);
    modem_detail_group.add(&modem_band_row);
    
    let modem_close_btn = gtk::Button::with_label("Close");
    let modem_win = modem_detail_window.clone();
    modem_close_btn.connect_clicked(move |_| { modem_win.hide(); });
    
    modem_detail_box.append(&modem_title);
    modem_detail_box.append(&modem_detail_group);
    modem_detail_box.append(&modem_close_btn);
    modem_detail_window.set_child(Some(&modem_detail_box));
    
    // GPS detail popup
    let gps_detail_window = gtk::Window::builder()
        .title("GPS")
        .default_width(340)
        .default_height(280)
        .decorated(false)
        .modal(true)
        .build();
    
    let gps_title = gtk::Label::new(Some("GPS"));
    gps_title.add_css_class("title-label");
    
    let gps_detail_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    gps_detail_box.set_margin_start(12);
    gps_detail_box.set_margin_end(12);
    
    let gps_fix_row = adw::ActionRow::new();
    gps_fix_row.set_title("Fix Type");
    let gps_fix_value_detail = gtk::Label::new(Some("No GPS"));
    gps_fix_value_detail.add_css_class("status-text");
    gps_fix_row.add_suffix(&gps_fix_value_detail);
    
    let gps_sats_detail_row = adw::ActionRow::new();
    gps_sats_detail_row.set_title("Satellites");
    let gps_sats_detail_value = gtk::Label::new(Some("--"));
    gps_sats_detail_value.add_css_class("status-text");
    gps_sats_detail_row.add_suffix(&gps_sats_detail_value);
    
    let gps_coords_detail_row = adw::ActionRow::new();
    gps_coords_detail_row.set_title("Coordinates");
    let gps_coords_detail_value = gtk::Label::new(Some("---"));
    gps_coords_detail_value.add_css_class("status-text");
    gps_coords_detail_row.add_suffix(&gps_coords_detail_value);
    
    let gps_maidenhead_row = adw::ActionRow::new();
    gps_maidenhead_row.set_title("Maidenhead");
    let gps_maidenhead_value = gtk::Label::new(Some("---"));
    gps_maidenhead_value.add_css_class("status-text");
    gps_maidenhead_row.add_suffix(&gps_maidenhead_value);
    
    let gps_alt_detail_row = adw::ActionRow::new();
    gps_alt_detail_row.set_title("Altitude");
    let gps_alt_detail_value = gtk::Label::new(Some("---"));
    gps_alt_detail_value.add_css_class("status-text");
    gps_alt_detail_row.add_suffix(&gps_alt_detail_value);
    
    let gps_speed_detail_row = adw::ActionRow::new();
    gps_speed_detail_row.set_title("Speed");
    let gps_speed_detail_value = gtk::Label::new(Some("---"));
    gps_speed_detail_value.add_css_class("status-text");
    gps_speed_detail_row.add_suffix(&gps_speed_detail_value);
    
    let gps_detail_group = adw::PreferencesGroup::new();
    gps_detail_group.add(&gps_fix_row);
    gps_detail_group.add(&gps_sats_detail_row);
    gps_detail_group.add(&gps_coords_detail_row);
    gps_detail_group.add(&gps_maidenhead_row);
    gps_detail_group.add(&gps_alt_detail_row);
    gps_detail_group.add(&gps_speed_detail_row);
    
    let gps_close_btn = gtk::Button::with_label("Close");
    let gps_win = gps_detail_window.clone();
    gps_close_btn.connect_clicked(move |_| { gps_win.hide(); });
    
    gps_detail_box.append(&gps_title);
    gps_detail_box.append(&gps_detail_group);
    gps_detail_box.append(&gps_close_btn);
    gps_detail_window.set_child(Some(&gps_detail_box));
    
    // AUDIO detail popup
    let audio_detail_window = gtk::Window::builder()
        .title("AUDIO")
        .default_width(340)
        .default_height(180)
        .decorated(false)
        .modal(true)
        .build();
    
    let audio_title = gtk::Label::new(Some("AUDIO"));
    audio_title.add_css_class("title-label");
    
    let audio_detail_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    audio_detail_box.set_margin_start(12);
    audio_detail_box.set_margin_end(12);
    
    let audio_codec_row = adw::ActionRow::new();
    audio_codec_row.set_title("Codec");
    let audio_codec_value = gtk::Label::new(Some("ADPCM"));
    audio_codec_value.add_css_class("status-text");
    audio_codec_row.add_suffix(&audio_codec_value);
    
    let audio_buf_row = adw::ActionRow::new();
    audio_buf_row.set_title("Buffer Size");
    let audio_buf_value = gtk::Label::new(Some("--"));
    audio_buf_value.add_css_class("status-text");
    audio_buf_row.add_suffix(&audio_buf_value);
    
    let audio_latency_row = adw::ActionRow::new();
    audio_latency_row.set_title("Latency");
    let audio_latency_value = gtk::Label::new(Some("---"));
    audio_latency_value.add_css_class("status-text");
    audio_latency_row.add_suffix(&audio_latency_value);
    
    let audio_detail_group = adw::PreferencesGroup::new();
    audio_detail_group.add(&audio_codec_row);
    audio_detail_group.add(&audio_buf_row);
    audio_detail_group.add(&audio_latency_row);
    
    let audio_close_btn = gtk::Button::with_label("Close");
    let audio_win = audio_detail_window.clone();
    audio_close_btn.connect_clicked(move |_| { audio_win.hide(); });
    
    audio_detail_box.append(&audio_title);
    audio_detail_box.append(&audio_detail_group);
    audio_detail_box.append(&audio_close_btn);
    audio_detail_window.set_child(Some(&audio_detail_box));
    
    // =========================================================================
    // Status indicator click handlers - show detail popup windows
    // =========================================================================
    let modem_window = modem_detail_window.clone();
    modem_status_btn.connect_clicked(move |_| {
        modem_window.present();
    });
    
    let gps_window = gps_detail_window.clone();
    gps_status_btn.connect_clicked(move |_| {
        gps_window.present();
    });
    
    let audio_window = audio_detail_window.clone();
    audio_status_btn.connect_clicked(move |_| {
        audio_window.present();
    });
    
    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_size_request(360, 654);
    toast_overlay.set_hexpand(false);
    toast_overlay.set_vexpand(false);
    toast_overlay.set_halign(gtk::Align::Center);
    toast_overlay.set_valign(gtk::Align::Start);
    toast_overlay.set_child(Some(&stack));
    
    // Clone for use in various handlers
    let toast_overlay_clone = toast_overlay.clone();
    
    // =========================================================================
    // APRS callback - process incoming APRS frames
    // =========================================================================
    // Store received ACKs and messages for later processing
    let received_acks: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let received_messages: Arc<Mutex<Vec<(String, String, String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    // Get our callsign before callback (owned string, no lifetime issues)
    let our_callsign = settings.aprs_full_callsign();
    {
        let r = radio.lock().unwrap();
        let aprs_msgs = Arc::clone(&aprs_messages);
        let radio_aprs = Arc::clone(radio);
        let acks_received = Arc::clone(&received_acks);
        let msgs_received = Arc::clone(&received_messages);
        let my_call = our_callsign.clone();
        
        r.on_aprs(move |msg| {
            // Debug: print full received frame
            eprintln!("[pocket-modem] RECEIVED FRAME - type: {:?}, from: {}, to: {}, body: {:?}", 
                      msg.msg_type, msg.from_callsign, msg.to_callsign, msg.msg_body);
            
            // Add to display list (exclude ACKs - they're not user-visible messages)
            if msg.msg_type != APRSType::MessageAck {
                let mut msgs = aprs_msgs.lock().unwrap();
                msgs.push(msg.clone());
                if msgs.len() > 100 { msgs.remove(0); }
            }
            
            // Store received messages for chat threads
            if msg.msg_type == APRSType::Message {
                if let Some(ref body) = msg.msg_body {
                    if !body.is_empty() {
                        // Store: (from, body, thread_id, msg_id)
                        msgs_received.lock().unwrap().push((
                            msg.from_callsign.clone(),
                            body.clone(),
                            msg.from_callsign.clone(),  // thread_id is the sender (other party)
                            msg.msg_id.clone().unwrap_or_default()  // APRS message ID
                        ));
                    }
                }
                
                // Send ACK for messages addressed to us with a message ID
                if let Some(ref msg_id) = msg.msg_id {
                    if msg.to_callsign == my_call {
                        let sender = msg.from_callsign.clone();
                        let ack_id = msg_id.clone();
                        let ack_call = my_call.clone();
                        let from_for_storage = msg.from_callsign.clone();
                        let body_for_storage = msg.msg_body.clone();
                        let settings_ack = Arc::clone(&acks_received);
                        
                        let radio_ack = Arc::clone(&radio_aprs);
                        std::thread::spawn(move || {
                            if let Ok(r) = radio_ack.lock() {
                                if let Err(e) = r.send_aprs_ack(&ack_call, &sender, &ack_id) {
                                    eprintln!("[pocket-modem] Failed to send ACK: {}", e);
                                } else {
                                    eprintln!("[pocket-modem] ACK sent for message {} to {}", ack_id, sender);
                                    // Signal that we sent an ACK - this will trigger message storage with ack_sent=true
                                    settings_ack.lock().unwrap().push(format!("ACK_SENT:{}", ack_id));
                                }
                            }
                        });
                    }
                }
            }
            
            // Log ACKs and queue for processing
            if msg.msg_type == APRSType::MessageAck {
                // Parse ACK ID from raw payload (most reliable) or body
                let raw_ack = msg.raw_payload.as_ref()
                    .and_then(|p| {
                        let id = aprs::parse_ax25_payload_for_ack(p);
                        eprintln!("[pocket-modem] ACK parse_ax25_payload_for_ack: {:?}", id);
                        id
                    });
                let ack_id = raw_ack.or_else(|| {
                    eprintln!("[pocket-modem] ACK msg_id: {:?}", msg.msg_id);
                    msg.msg_id.clone()
                });
                
                if let Some(id) = ack_id {
                    eprintln!("[pocket-modem] Received ACK ID: {}", id);
                    acks_received.lock().unwrap().push(id);
                } else {
                    eprintln!("[pocket-modem] WARNING: Could not extract ACK ID from: {:?} {:?}", 
                              msg.raw_payload.as_ref().map(|p| String::from_utf8_lossy(p)),
                              msg.msg_body);
                }
            }
        });
    }
    
    // =========================================================================
    // APRS Beacon TX Button Handler
    // =========================================================================
    let settings_beacon = settings as *const SettingsManager as *mut SettingsManager;
    let gps_for_beacon = Arc::clone(gps);
    let radio_for_beacon = Arc::clone(radio);
    let beacon_btn_clone = beacon_btn.clone();
    let beacon_status_clone = beacon_status.clone();
    let toast_overlay_for_beacon = toast_overlay.clone();
    let beacon_state_clone = Arc::clone(&beacon_state);
    let aprs_messages_for_beacon = Arc::clone(&aprs_messages);
    let aprs_list_for_beacon = aprs_list_box.clone();
    let aprs_empty_for_beacon = aprs_empty_label.clone();
    let gps_for_aprs = Arc::clone(gps);
    
    beacon_btn.connect_clicked(move |btn| {
        btn.set_sensitive(false);
        beacon_status_clone.set_text("Checking GPS...");
        
        // Read settings fresh at click time
        let (call, symbol_table, symbol_code, comment, tx_enabled, beacon_dest) = unsafe {
            let s = &*settings_beacon;
            (s.aprs_full_callsign(), s.aprs_symbol_table(), s.aprs_symbol_code(), s.aprs_comment().to_string(), s.aprs_tx_enabled(), s.aprs_beacon_dest().to_string())
        };
        
        // Check TX enabled
        if !tx_enabled {
            *beacon_state_clone.lock().unwrap() = BeaconState::NeedsTxEnabled;
            beacon_status_clone.set_text(BeaconState::NeedsTxEnabled.message());
            beacon_status_clone.add_css_class("status-warning");
            beacon_status_clone.remove_css_class("status-success");
            btn.set_sensitive(true);
            
            let toast = adw::Toast::new("Enable APRS TX in Settings");
            toast_overlay_for_beacon.add_toast(toast);
            return;
        }
        
        // Check GPS fix
        let (lat, lon, has_fix) = {
            if let Ok(g) = gps_for_beacon.lock() {
                let data = g.get_data();
                (data.latitude, data.longitude, data.has_fix)
            } else { (None, None, false) }
        };
        
        // Check GPS fix
        if !has_fix || lat.is_none() || lon.is_none() {
            *beacon_state_clone.lock().unwrap() = BeaconState::NeedsGpsFix;
            beacon_status_clone.set_text(BeaconState::NeedsGpsFix.message());
            beacon_status_clone.add_css_class("status-warning");
            beacon_status_clone.remove_css_class("status-success");
            btn.set_sensitive(true);
            
            let toast = adw::Toast::new("GPS fix required for APRS beacon");
            toast_overlay_for_beacon.add_toast(toast);
            return;
        }
        
        // Check callsign
        if call.is_empty() || call == "-0" {
            *beacon_state_clone.lock().unwrap() = BeaconState::NeedsConfig;
            beacon_status_clone.set_text(BeaconState::NeedsConfig.message());
            beacon_status_clone.add_css_class("status-warning");
            beacon_status_clone.remove_css_class("status-success");
            btn.set_sensitive(true);
            
            let toast = adw::Toast::new("Configure your callsign in Settings");
            toast_overlay_for_beacon.add_toast(toast);
            return;
        }
        
        // Use fresh-read settings values
        let symbol = aprs::AprsSymbol::new(symbol_table, symbol_code);
        let call_for_tx = call;
        let comment_for_tx = comment;
        let beacon_dest = beacon_dest;
        
        *beacon_state_clone.lock().unwrap() = BeaconState::Transmitting;
        beacon_status_clone.set_text(BeaconState::Transmitting.message());
        beacon_status_clone.remove_css_class("status-warning");
        beacon_status_clone.remove_css_class("status-success");
        
        // Copy all needed values before spawning thread (cannot pass raw pointers)
        let lat = lat.unwrap();
        let lon = lon.unwrap();
        
        // Clone UI widgets for use in the callback
        let status_for_callback = beacon_status_clone.clone();
        let btn_for_callback = btn.clone();
        let toast_for_success = toast_overlay_clone.clone();
        let radio_for_tx = Arc::clone(&radio_for_beacon);
        let state_for_callback = Arc::clone(&beacon_state_clone);
        
        // Send beacon on main thread (it's fast - just serial write)
        let result = {
            if let Ok(r) = radio_for_tx.lock() {
                r.send_aprs_beacon_with_dest(&beacon_dest, &call_for_tx, lat, lon, None, symbol, &comment_for_tx)
            } else {
                Err("Could not lock radio".to_string())
            }
        };
        
        match result {
            Ok(_msg) => {
                eprintln!("[pocket-modem] Beacon sent");
                *state_for_callback.lock().unwrap() = BeaconState::Success;
                status_for_callback.set_text(BeaconState::Success.message());
                status_for_callback.add_css_class("status-success");
                status_for_callback.remove_css_class("status-warning");
                btn_for_callback.set_sensitive(true);
                
                let toast = adw::Toast::new("Beacon transmitted");
                toast_for_success.add_toast(toast);
                
                // Add beacon to APRS messages list
                let mut beacon_msg = aprs::APRSMessage::new();
                beacon_msg.msg_type = aprs::APRSType::Position;
                beacon_msg.from_callsign = call_for_tx.clone();
                beacon_msg.to_callsign = beacon_dest.clone();
                beacon_msg.position_lat = lat;
                beacon_msg.position_lon = lon;
                beacon_msg.is_sent = true;
                beacon_msg.symbol_table_id = Some(symbol_table);
                beacon_msg.symbol_code = Some(symbol_code);
                beacon_msg.comment = comment_for_tx.clone();
                
                // Note: Don't add to Vec here - on_aprs callback will add it when received
                // Only add to UI list for immediate visual feedback
                let beacon_for_ui = beacon_msg;
                
                // Add to UI list
                let (my_lat, my_lon) = {
                    if let Ok(gps_guard) = gps_for_aprs.lock() {
                        let gps_data = gps_guard.data.lock().unwrap();
                        if let (Some(lat), Some(lon)) = (gps_data.latitude, gps_data.longitude) { (lat, lon) }
                        else { (0.0, 0.0) }
                    } else { (0.0, 0.0) }
                };
                add_aprs_message_to_list(
                    &beacon_for_ui,
                    &aprs_list_for_beacon,
                    &aprs_empty_for_beacon,
                    my_lat, my_lon
                );
            }
            Err(e) => {
                eprintln!("[pocket-modem] Beacon failed: {}", e);
                *state_for_callback.lock().unwrap() = BeaconState::Failed;
                status_for_callback.set_text(BeaconState::Failed.message());
                status_for_callback.add_css_class("status-error");
                status_for_callback.remove_css_class("status-success");
                btn_for_callback.set_sensitive(true);
            }
        }
    });
    
    #[cfg(feature = "notifications")]
    { if let Err(e) = init("pocket-modem") { eprintln!("[pocket-modem] Notifications init failed: {:?}", e); } }
    
    // =========================================================================
    // Update loop
    // =========================================================================
    let radio_update = Arc::clone(radio);
    let modem_label_clone = modem_label.clone();
    let rssi_sbar_clone = rssi_sbar.clone();
    let signal_value_clone = signal_value.clone();
    let signal_label_clone = signal_label.clone();
    let audio_clone = Arc::clone(audio);
    let audio_label_clone = audio_label.clone();
    let gps_clone = Arc::clone(gps);
    let gps_led_clone = gps_led.clone();
    let ptt_label_update = ptt_label.clone();
    
    // Detail window labels
    let modem_freq_value_clone = modem_freq_value.clone();
    let modem_rssi_value_clone = modem_rssi_value.clone();
    let modem_mode_value_clone = modem_mode_value.clone();
    let modem_tx_value_clone = modem_tx_value.clone();
    let modem_fw_value_clone = modem_fw_value.clone();
    let modem_band_value_clone = modem_band_value.clone();
    
    let gps_fix_value_detail_clone = gps_fix_value_detail.clone();
    let gps_sats_detail_value_clone = gps_sats_detail_value.clone();
    let gps_coords_detail_value_clone = gps_coords_detail_value.clone();
    let gps_maidenhead_value_clone = gps_maidenhead_value.clone();
    let gps_alt_detail_value_clone = gps_alt_detail_value.clone();
    let gps_speed_detail_value_clone = gps_speed_detail_value.clone();
    
    let audio_buf_value_clone = audio_buf_value.clone();
    let audio_latency_value_clone = audio_latency_value.clone();
    
    let aprs_messages_clone2 = Arc::clone(&aprs_messages);
    let aprs_empty_label_clone = aprs_empty_label.clone();
    let aprs_list_box_clone = aprs_list_box.clone();
    let aprs_last_displayed_clone = Arc::clone(&aprs_last_displayed);
    
    // Beacon UI elements for update loop
    let beacon_state_update = Arc::clone(&beacon_state);
    let beacon_status_update = beacon_status.clone();
    let beacon_btn_update = beacon_btn.clone();
    let settings_update = settings as *const SettingsManager as *mut SettingsManager;
    let gps_update = Arc::clone(gps);
    
    // APRS storage label for update loop
    let aprs_storage_row_clone = aprs_storage_row.clone();
    
    let map_manager_clone = Arc::clone(&map_manager);
    let window_clone = window.clone();
    let map_page_clone = map_page.clone();
    let map_clamp_clone = map_clamp.clone();
    let dark_mode_state_clone = Arc::clone(&dark_mode_state);

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
                    signal_value_clone.set_markup(&format!("<span color='#FF4444'>{}%</span>", pct as i32));
                    signal_label_clone.set_text("AUDIO");
                    rssi_sbar_clone.remove_css_class("bar-rx");
                    rssi_sbar_clone.add_css_class("bar-tx");
                } else {
                    let dbm = (state.raw_rssi as f64) * 1.2 - 160.8;
                    signal_value_clone.set_markup(&format!("<span color='#FFB000'>{} dBm</span>", dbm as i32));
                    signal_label_clone.set_text("SIGNAL");
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
                signal_label_clone.set_text("SIGNAL");
                rssi_sbar_clone.remove_css_class("bar-tx");
                rssi_sbar_clone.remove_css_class("bar-rx");
            }
            
            // Update MODEM detail window
            modem_freq_value_clone.set_text(&format!("{}.{:03} MHz", 
                state.frequency / 1000,
                state.frequency % 1000
            ));
            if state.raw_rssi > 0 {
                let dbm = (state.raw_rssi as f64) * 1.2 - 160.8;
                modem_rssi_value_clone.set_text(&format!("{} dBm", dbm as i32));
            } else {
                modem_rssi_value_clone.set_text("---");
            }
            modem_mode_value_clone.set_text(match state.mode {
                0 => "FM",
                1 => "NFM",
                _ => "Unknown"
            });
            modem_tx_value_clone.set_text(if state.tx_active { "TX" } else { "RX" });
            modem_fw_value_clone.set_text(&format!("v{}.{}", 
                state.firmware_version / 100, state.firmware_version % 100));
            
            // Determine band from frequency
            let freq_mhz = state.frequency as f64 / 1000.0;
            let band = if freq_mhz >= 136.0 && freq_mhz <= 174.0 {
                "2m (VHF)"
            } else if freq_mhz >= 400.0 && freq_mhz <= 480.0 {
                "70cm (UHF)"
            } else {
                "Unknown"
            };
            modem_band_value_clone.set_text(band);
        }
        
        // Audio status
        {
            let a = audio_clone.lock().unwrap();
            let audio_started = a.audio_started();
            drop(a);
            
            let squelch_open = if let Ok(r) = radio_update.lock() { r.state().squelch_open } else { false };
            
            // audio_label is the circle that should turn green
            if !audio_started {
                audio_label_clone.set_text("○");
                audio_label_clone.remove_css_class("status-icon-green");
                audio_label_clone.remove_css_class("status-icon-red");
                audio_label_clone.add_css_class("status-icon-gray-empty");
            } else if squelch_open {
                audio_label_clone.set_text("●");
                audio_label_clone.remove_css_class("status-icon-gray-empty");
                audio_label_clone.remove_css_class("status-icon-red");
                audio_label_clone.remove_css_class("status-icon-gray-filled");
                audio_label_clone.add_css_class("status-icon-green");
            } else {
                audio_label_clone.set_text("●");
                audio_label_clone.remove_css_class("status-icon-green");
                audio_label_clone.remove_css_class("status-icon-red");
                audio_label_clone.remove_css_class("status-icon-gray-empty");
                audio_label_clone.add_css_class("status-icon-gray-filled");
            }
            
            // Update AUDIO detail window
            // ADPCM: 160 samples = 81 bytes at 16kHz = 10ms per block
            // RX buffer holds multiple blocks for smooth playback
            const ADPCM_BLOCK_SIZE: usize = 81;  // bytes per 10ms
            const ADPCM_LATENCY_MS: f64 = 10.0;  // ms per block
            const RX_BUFFER_BLOCKS: usize = 10;  // typical RX buffer
            
            let total_bytes = ADPCM_BLOCK_SIZE * RX_BUFFER_BLOCKS;
            let total_latency = ADPCM_LATENCY_MS * RX_BUFFER_BLOCKS as f64;
            
            audio_buf_value_clone.set_text(&format!("{} bytes", total_bytes));
            audio_latency_value_clone.set_text(&format!("~{:.0}ms", total_latency));
        }

        // GPS status
        if let Ok(g) = gps_clone.lock() {
            let gps_data = g.get_data();
            
            // Determine fix type
            // Determine fix type
            let (fix_type, fix_class) = if !gps_data.gps_enabled {
                ("No GPS", "status-warning")
            } else if !gps_data.has_fix {
                ("No Fix", "status-warning")
            } else if gps_data.satellites >= 4 {
                ("3D Fix", "status-success")
            } else if gps_data.gps_enabled && gps_data.satellites == 0 {
                // GeoClue2 doesn't report satellites, but has a fix
                ("WiFi Fix", "status-success")
            } else {
                ("2D Fix", "status-warning")
            };
            
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
                gps_led_clone.set_text("○");
                gps_led_clone.remove_css_class("gps-led-on");
                gps_led_clone.remove_css_class("gps-led-searching");
                gps_led_clone.add_css_class("gps-led-off");
            }
            
            gps_fix_value_detail_clone.set_text(fix_type);
            gps_fix_value_detail_clone.remove_css_class("status-success");
            gps_fix_value_detail_clone.remove_css_class("status-warning");
            gps_fix_value_detail_clone.add_css_class(fix_class);
            
            // Update satellite count
            if gps_data.satellites > 0 {
                gps_sats_detail_value_clone.set_text(&format!("{}", gps_data.satellites));
            } else {
                gps_sats_detail_value_clone.set_text("--");
            }
            
            // Update coordinates and Maidenhead
            if let (Some(lat), Some(lon)) = (gps_data.latitude, gps_data.longitude) {
                gps_coords_detail_value_clone.set_text(&format!("{:.6}, {:.6}", lat, lon));
                // Calculate Maidenhead locator
                let locator = calculate_maidenhead(lat, lon);
                gps_maidenhead_value_clone.set_text(&locator);
            } else {
                gps_coords_detail_value_clone.set_text("---");
                gps_maidenhead_value_clone.set_text("---");
            }
            
            // Update altitude and speed
            if let Some(alt) = gps_data.altitude {
                gps_alt_detail_value_clone.set_text(&format!("{:.0}m", alt));
            } else {
                gps_alt_detail_value_clone.set_text("---");
            }
            if let Some(speed) = gps_data.speed {
                gps_speed_detail_value_clone.set_text(&format!("{:.1} km/h", speed * 3.6));
            } else {
                gps_speed_detail_value_clone.set_text("---");
            }
        }
        
        // APRS UI
        let (my_lat, my_lon) = {
            if let Ok(g) = gps_clone.lock() {
                let gps_data = g.get_data();
                if let (Some(lat), Some(lon)) = (gps_data.latitude, gps_data.longitude) { (lat, lon) }
                else { (0.0, 0.0) }
            } else { (0.0, 0.0) }
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
                        add_aprs_message_to_list(msg, &aprs_list_box_clone, &aprs_empty_label_clone, my_lat, my_lon);
                        if let Ok(mut map) = map_manager_clone.lock() {
                            map.update_station(msg);
                        }
                        new_last = i + 1;
                    }
                }
                aprs_last_displayed_clone.store(new_last, std::sync::atomic::Ordering::SeqCst);
            }
        }
        
        // Map with GPS data - throttle updates to reduce lag
        // Only check position every ~1 second even though timer runs at 100ms
        static MAP_UPDATE_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let update_tick = MAP_UPDATE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        
        if update_tick % 10 == 0 {  // Only update every 10 ticks (1 second)
            if let Ok(g) = gps_clone.lock() {
                let gps_data = g.get_data();
                
                if gps_data.has_fix {
                    if let (Some(lat), Some(lon)) = (gps_data.latitude, gps_data.longitude) {
                        // Only update map when position changes significantly (more than ~10m)
                        let current_pos = if let Ok(map) = map_manager_clone.lock() {
                            map.get_user_position()
                        } else { None };
                        
                        let should_update = match current_pos {
                            Some((old_lat, old_lon)) => {
                                let lat_diff = (lat - old_lat).abs();
                                let lon_diff = (lon - old_lon).abs();
                                lat_diff > 0.0001 || lon_diff > 0.0001  // ~10m threshold
                            }
                            None => true,  // First position
                        };
                        
                        if should_update {
                            if let Ok(mut map) = map_manager_clone.lock() {
                                map.set_user_position(lat, lon);
                            }
                        }
                    }
                }
            }
        }
        
        // Map drawing is handled automatically by libshumate
        // Check for dark mode changes and update map style
        {
            let current_dark = adw::StyleManager::default().is_dark();
            let prev_dark = dark_mode_state_clone.load(std::sync::atomic::Ordering::SeqCst);
            if current_dark != prev_dark {
                dark_mode_state_clone.store(current_dark, std::sync::atomic::Ordering::SeqCst);
                eprintln!("[pocket-modem] Dark mode changed to: {}", current_dark);
                if let Ok(mut mm) = map_manager_clone.lock() {
                    mm.update_style(current_dark);
                }
            }
        }
        
        ptt_label_update.set_text("PTT");
        
        // Update APRS storage size display (every 10 ticks = ~1 second)
        static APRS_STORAGE_TICK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let storage_tick = APRS_STORAGE_TICK.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if storage_tick % 10 == 0 {
            let size = unsafe { (*settings_update).aprs_messages_storage_size_display() };
            aprs_storage_row_clone.set_subtitle(&size);
        }
        
        // APRS message retry logic (every 10 seconds = 100 ticks)
        static APRS_RETRY_TICK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let retry_tick = APRS_RETRY_TICK.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if retry_tick % 100 == 0 {
            // Process received ACKs and ACK_SENT signals (all stored in same vec)
            let items: Vec<String> = {
                let mut acks = received_acks.lock().unwrap();
                std::mem::take(&mut *acks)
            };
            
            for item in items {
                if let Some(ack_id) = item.strip_prefix("ACK_SENT:") {
                    // ACK_SENT signal - mark received messages as having our ACK sent
                    eprintln!("[pocket-modem] Processing ACK_SENT for message ID: {}", ack_id);
                    unsafe {
                        let messages = (*settings_update).aprs_messages();
                        let mut found = false;
                        for m in messages.iter().rev() {
                            if m.msg_id.as_ref() == Some(&ack_id.to_string()) {
                                if (*settings_update).mark_message_ack_sent(&m.id) {
                                    eprintln!("[pocket-modem] Marked ACK sent for message {}", m.id);
                                    CHAT_REFRESH_SIGNAL.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                    found = true;
                                }
                                break;
                            }
                        }
                        if !found {
                            for m in messages.iter().rev() {
                                if m.aprs_id.is_empty() && !m.from_callsign.is_empty() {
                                    if (*settings_update).mark_message_ack_sent(&m.id) {
                                        eprintln!("[pocket-modem] Marked ACK sent for recent message {}", m.id);
                                        CHAT_REFRESH_SIGNAL.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                } else {
                    // Received ACK ID - match against sent messages
                    let ack_id = &item;
                    eprintln!("[pocket-modem] Processing received ACK: {}", ack_id);
                    unsafe {
                        let messages = (*settings_update).aprs_messages();
                        for m in messages {
                            if m.aprs_id == *ack_id {
                                if (*settings_update).update_aprs_message_status(&m.id, aprs::DirectMessageStatus::Acknowledged) {
                                    eprintln!("[pocket-modem] Message {} acknowledged (ACK {})", m.id, ack_id);
                                    CHAT_REFRESH_SIGNAL.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                }
                                break;
                            }
                        }
                    }
                }
            }
            
            // Process received messages and add to chat threads
            let msgs_to_process: Vec<(String, String, String, String)> = {
                let mut msgs = received_messages.lock().unwrap();
                std::mem::take(&mut *msgs)
            };
            for (from, body, thread_id, msg_id) in msgs_to_process {
                let my_call = unsafe { (*settings_update).aprs_full_callsign() };
                let uuid = unsafe { (*settings_update).generate_message_uuid() };
                let dm = aprs::DirectMessage::new_received(
                    &my_call,    // to (us)
                    &body,
                    &uuid,       // internal ID
                    &from,       // from (sender)
                    "",          // no APRS ID
                    &msg_id,     // APRS message ID for ACK tracking
                );
                eprintln!("[pocket-modem] Storing received message from {}: {} (id: {})", from, body, msg_id);
                unsafe { (*settings_update).add_aprs_message(dm); }
                // Signal chat UI and thread list to refresh
                CHAT_REFRESH_SIGNAL.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                THREAD_REFRESH_SIGNAL.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            
            // Check if thread list needs refresh
            static LAST_THREAD_SIGNAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let current_signal = THREAD_REFRESH_SIGNAL.load(std::sync::atomic::Ordering::SeqCst);
            let last_signal = LAST_THREAD_SIGNAL.load(std::sync::atomic::Ordering::SeqCst);
            if current_signal != last_signal {
                LAST_THREAD_SIGNAL.store(current_signal, std::sync::atomic::Ordering::SeqCst);
                let stored_msgs = unsafe { (*settings_update).aprs_messages() };
                if !stored_msgs.is_empty() {
                    // Clear and rebuild thread list
                    while let Some(child) = aprs_list_box_clone.first_child() {
                        aprs_list_box_clone.remove(&child);
                    }
                    aprs_list_box_clone.append(&aprs_empty_label_clone);
                    aprs_empty_label_clone.set_visible(true);
                    build_and_display_threads(
                        stored_msgs,
                        &aprs_list_box_clone,
                        &aprs_empty_label_clone,
                        &radio_update,
                        settings_update,
                    );
                }
            }
            
            // Check for pending messages that need retry (5 second interval)
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            
            // Get messages that need retry with APRS-standard exponential backoff
            // APRS 1.2 spec: retry at 30s, then 60s, then 120s (doubling each time)
            // Interval = base_interval * 2^retries (30, 60, 120, ...)
            // First reload messages from GSettings to ensure we have latest state
            unsafe { (*settings_update).reload_aprs_messages(); }
            
            let retry_candidates: Vec<(String, String, String, String, String, u64)> = unsafe {
                (*settings_update).aprs_messages()
                    .iter()
                    .filter(|m| {
                        if m.status != aprs::DirectMessageStatus::Sent || m.retries >= APRS_MAX_RETRIES {
                            return false;
                        }
                        // Calculate retry interval with exponential backoff
                        // First retry: 30s, second: 60s, third: 120s, etc.
                        let retry_interval = APRS_BASE_RETRY_INTERVAL_SECS * (2_u64.pow(m.retries as u32));
                        // If last_retry_timestamp is 0, message was just stored - use message timestamp
                        // Otherwise, check if retry_interval has passed since last retry
                        let reference_time = if m.last_retry_timestamp == 0 { m.timestamp } else { m.last_retry_timestamp };
                        now >= reference_time + retry_interval
                    })
                    .map(|m| (m.id.clone(), m.aprs_id.clone(), m.to_callsign.clone(), m.body.clone(), m.from_callsign.clone(), m.last_retry_timestamp))
                    .collect()
            };
            
            for (msg_id, aprs_id, to, body, _from, _last_ts) in retry_candidates {
                let callsign = unsafe { (*settings_update).aprs_full_callsign() };
                if !callsign.is_empty() {
                    let current_retries = unsafe { (*settings_update).get_aprs_message(&msg_id).map(|m| m.retries).unwrap_or(0) };
                    eprintln!("[pocket-modem] Retrying [{}] to {} (attempt {})", 
                              aprs_id, to, current_retries + 1);
                    
                    // Increment retry count and update timestamp
                    unsafe { (*settings_update).increment_aprs_message_retries(&msg_id); };
                    unsafe { (*settings_update).update_message_last_retry(&msg_id, now); };
                    // Signal chat UI to refresh message status icon (show updated retry count)
                    CHAT_REFRESH_SIGNAL.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    
                    // Retry sending
                    let radio_retry = radio_update.clone();
                    let aprs_id_clone = aprs_id.clone();
                    std::thread::spawn(move || {
                        if let Ok(r) = radio_retry.lock() {
                            if let Err(e) = r.send_aprs_message(&callsign, &to, &body, &aprs_id_clone) {
                                eprintln!("[pocket-modem] Retry failed: {}", e);
                            }
                        }
                    });
                }
            }
            
            // Mark messages that exceeded max retries as failed
            unsafe {
                let messages = (*settings_update).aprs_messages();
                let mut failed_message_ids: Vec<String> = Vec::new();
                for m in messages {
                    if m.status == aprs::DirectMessageStatus::Sent && m.retries >= APRS_MAX_RETRIES {
                        (*settings_update).update_aprs_message_status(
                            &m.id,
                            aprs::DirectMessageStatus::Failed
                        );
                        failed_message_ids.push(m.id.clone());
                        eprintln!("[pocket-modem] Message {} marked as failed after {} retries", m.id, APRS_MAX_RETRIES);
                    }
                }
                // Signal chat UI to refresh failed message icons
                if !failed_message_ids.is_empty() {
                    CHAT_REFRESH_SIGNAL.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
        }
        
        // Update beacon state machine based on current settings and GPS
        let (callsign, tx_enabled) = unsafe {
            let call = (*settings_update).aprs_full_callsign();
            let enabled = (*settings_update).aprs_tx_enabled();
            (call, enabled)
        };
        let (has_gps_fix, _, _) = {
            if let Ok(g) = gps_update.lock() {
                let data = g.get_data();
                (data.has_fix, data.latitude, data.longitude)
            } else { (false, None, None) }
        };
        
        // Compute new state
        let new_state = if callsign.is_empty() || callsign == "-0" {
            BeaconState::NeedsConfig
        } else if !tx_enabled {
            BeaconState::NeedsTxEnabled
        } else if !has_gps_fix {
            BeaconState::NeedsGpsFix
        } else {
            BeaconState::Ready
        };
        
        // Update state if changed
        {
            let mut state = beacon_state_update.lock().unwrap();
            if *state != new_state {
                *state = new_state;
                beacon_status_update.set_text(new_state.message());
                beacon_status_update.remove_css_class("status-warning");
                beacon_status_update.remove_css_class("status-success");
                beacon_status_update.remove_css_class("status-error");
                
                match new_state {
                    BeaconState::NeedsConfig | BeaconState::NeedsTxEnabled | BeaconState::NeedsGpsFix => {
                        beacon_status_update.add_css_class("status-warning");
                        beacon_btn_update.set_sensitive(false);
                    }
                    BeaconState::Ready | BeaconState::Transmitting => {
                        beacon_btn_update.set_sensitive(true);
                    }
                    BeaconState::Success => {
                        beacon_status_update.add_css_class("status-success");
                        beacon_btn_update.set_sensitive(true);
                    }
                    BeaconState::Failed => {
                        beacon_status_update.add_css_class("status-error");
                        beacon_btn_update.set_sensitive(true);
                    }
                }
            }
        }
        
        glib::ControlFlow::Continue
    });
    
    // =========================================================================
    // APRS Direct Message Chat Screen
    // =========================================================================
    fn show_aprs_chat_screen(
        _parent: Option<&gtk::Widget>,
        recipient: &str,
        settings: *mut SettingsManager,
        radio: &Arc<Mutex<KV4PRadio>>,
        _on_message_sent: Box<dyn Fn() + Send + 'static>,
    ) {
        eprintln!("[pocket-modem] Opening chat for: {}", recipient);
        
        // Safety check
        if settings.is_null() {
            eprintln!("[pocket-modem] FATAL: settings is null!");
            return;
        }
        
        // Get our callsign to distinguish sent vs received messages
        let our_callsign = unsafe { (*settings).aprs_full_callsign() };
        
        // Use a Dialog as a proper child window
        let chat_window = gtk::Dialog::builder()
            .title(&format!("Chat: {}", recipient))
            .modal(true)
            .build();
        chat_window.set_default_size(340, 500);
        eprintln!("[pocket-modem] Chat window created");
        
        // Get the content area and add our content
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.set_margin_start(4);
        content.set_margin_end(4);
        content.set_margin_top(8);
        content.set_margin_bottom(8);
        
        // Title bar
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.set_margin_bottom(4);
        
        let title = gtk::Label::new(Some(&format!("Chat: {}", recipient)));
        title.set_hexpand(true);
        header.append(&title);
        
        // Close button - emit close response
        let chat_win_close = chat_window.clone();
        let close_btn = gtk::Button::with_label("✕");
        close_btn.add_css_class("flat");
        close_btn.connect_clicked(move |_| { 
            chat_win_close.response(gtk::ResponseType::Close);
        });
        header.append(&close_btn);
        header.set_halign(gtk::Align::Fill);
        content.append(&header);
        
        // Messages scroll area
        let messages_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        messages_box.add_css_class("chat-messages-box");
        messages_box.set_valign(gtk::Align::Start);  // Don't stretch vertically
        
        let messages_scroll = gtk::ScrolledWindow::new();
        messages_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        messages_scroll.set_hexpand(true);
        messages_scroll.set_vexpand(true);
        messages_scroll.set_child(Some(&messages_box));
        messages_scroll.set_has_frame(true);
        messages_scroll.add_css_class("chat-scroll-frame");
        content.append(&messages_scroll);
        
        // Get adjustment and connect to changes
        let adj = messages_scroll.vadjustment();
        adj.set_step_increment(20.0);
        adj.set_page_increment(100.0);
        
        // Load existing messages for this thread
        let existing_messages: Vec<DirectMessage> = if settings.is_null() {
            Vec::new()
        } else {
            unsafe { (*settings).aprs_messages_for_thread(recipient).into_iter().cloned().collect() }
        };
        
        // Display existing messages
        for msg in &existing_messages {
            add_direct_message_bubble(&messages_box, msg, &our_callsign);
        }
        
        // Input area
        let input_container = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        input_container.set_margin_top(4);
        
        let message_entry = gtk::Entry::new();
        message_entry.set_placeholder_text(Some("Type a message..."));
        message_entry.set_hexpand(true);
        message_entry.set_input_purpose(gtk::InputPurpose::FreeForm);
        message_entry.set_size_request(200, -1);
        // Grab focus when window appears
        message_entry.grab_focus();
        input_container.append(&message_entry);
        
        let send_btn = gtk::Button::with_label("Send");
        send_btn.add_css_class("suggested-action");
        input_container.append(&send_btn);
        content.append(&input_container);
        
        // For Dialog, use content_area instead of set_child
        let chat_window_for_content = chat_window.clone();
        let chat_content = chat_window_for_content.content_area();
        chat_content.set_margin_start(4);
        chat_content.set_margin_end(4);
        chat_content.set_margin_top(4);
        chat_content.set_margin_bottom(4);
        while let Some(child) = chat_content.first_child() {
            chat_content.remove(&child);
        }
        chat_content.append(&content);
        eprintln!("[pocket-modem] Chat: setting content, presenting window");
        
        // Connect dialog response before showing
        let chat_win_resp = chat_window.clone();
        chat_window.connect_response(move |_, response| {
            eprintln!("[pocket-modem] Chat: dialog response {:?}", response);
            if response == gtk::ResponseType::Close {
                chat_win_resp.close();
            }
        });
        
        // Scroll helper function - uses delay to wait for layout
        let scroll_to_bottom = |scroll: &gtk::ScrolledWindow| {
            let scroll_w = scroll.clone();
            glib::timeout_add_local_once(Duration::from_millis(100), move || {
                let adj = scroll_w.vadjustment();
                let max_val = (adj.upper() - adj.page_size()).max(0.0);
                adj.set_value(max_val);
            });
        };
        
        chat_window.show();
        scroll_to_bottom(&messages_scroll);
        
        // Periodic refresh of message status
        let settings_refresh = settings;
        let messages_box_refresh = messages_box.clone();
        let recipient_refresh = recipient.to_string();
        let scroll_refresh = messages_scroll.clone();
        let our_callsign_refresh = our_callsign.clone();
        let mut last_refresh_signal = CHAT_REFRESH_SIGNAL.load(std::sync::atomic::Ordering::SeqCst);
        
        glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
            let current_signal = CHAT_REFRESH_SIGNAL.load(std::sync::atomic::Ordering::SeqCst);
            if current_signal != last_refresh_signal {
                last_refresh_signal = current_signal;
                
                // Explicitly reload messages from GSettings to ensure fresh data
                unsafe { (*settings_refresh).reload_aprs_messages(); }
                
                // Refresh messages for this thread
                let thread_messages = unsafe { (*settings_refresh).aprs_messages_for_thread(&recipient_refresh) };
                
                // Clear and rebuild messages box
                while let Some(child) = messages_box_refresh.first_child() {
                    messages_box_refresh.remove(&child);
                }
                for msg in thread_messages {
                    add_direct_message_bubble(&messages_box_refresh, msg, &our_callsign_refresh);
                }
                // Scroll to bottom after messages are added
                scroll_to_bottom(&scroll_refresh);
            }
            glib::ControlFlow::Continue
        });
        
        // Send message handler - clone resources for each closure
        let entry_btn = message_entry.clone();
        let scroll_btn = messages_scroll.clone();
        let messages_btn = messages_box.clone();
        let settings_btn = settings;
        let recipient_btn = recipient.to_string();
        let radio_btn = Arc::clone(radio);
        
        send_btn.connect_clicked(move |_| {
            let text = entry_btn.text().to_string();
            if text.trim().is_empty() { return; }
            
            if settings_btn.is_null() {
                eprintln!("[pocket-modem] Chat: settings is null");
                return;
            }
            
            let callsign = unsafe { (*settings_btn).aprs_full_callsign() };
            if callsign.is_empty() {
                eprintln!("[pocket-modem] Cannot send: no callsign configured");
                return;
            }
            
            // Generate UUID for internal tracking, APRS ID for RF
            let uuid = unsafe { (*settings_btn).generate_message_uuid() };
            let aprs_id = unsafe { (*settings_btn).generate_aprs_message_id() };
            // thread_id is the other party (recipient for sent messages)
            let mut dm = DirectMessage::new_with_thread(&recipient_btn, &recipient_btn, &text, &uuid, &callsign, &aprs_id);
            
            // Send over RF with message ID for ACK tracking
            if let Ok(r) = radio_btn.lock() {
                if let Err(e) = r.send_aprs_message(&callsign, &recipient_btn, &text, &aprs_id) {
                    eprintln!("[pocket-modem] Failed to send APRS message: {}", e);
                } else {
                    eprintln!("[pocket-modem] APRS message sent to {} [{}]: {}", recipient_btn, aprs_id, text);
                }
            }
            
            dm.mark_sent();
            // Set initial retry timestamp (first APRS retry at 30s per spec)
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            dm.last_retry_timestamp = now;
            unsafe { (*settings_btn).add_aprs_message(dm.clone()); }
            eprintln!("[pocket-modem] Message stored: id={}, aprs_id={}, to={}, status={:?}", 
                      dm.id, dm.aprs_id, dm.to_callsign, dm.status);
            add_direct_message_bubble(&messages_btn, &dm, &our_callsign);
            entry_btn.set_text("");
            scroll_to_bottom(&scroll_btn);
        });
        
        // Enter key handler
        let entry_enter = message_entry.clone();
        let scroll_enter = messages_scroll.clone();
        let scroll_helper_enter = messages_scroll.clone();
        let messages_enter = messages_box.clone();
        let settings_enter = settings;
        let recipient_enter = recipient.to_string();
        let radio_enter = Arc::clone(radio);
        
        message_entry.connect_activate(move |_| {
            let text = entry_enter.text().to_string();
            if text.trim().is_empty() { return; }
            
            if settings_enter.is_null() {
                eprintln!("[pocket-modem] Chat: settings is null");
                return;
            }
            
            let callsign = unsafe { (*settings_enter).aprs_full_callsign() };
            if callsign.is_empty() {
                eprintln!("[pocket-modem] Cannot send: no callsign configured");
                return;
            }
            
            // Generate UUID for internal tracking, APRS ID for RF
            let uuid = unsafe { (*settings_enter).generate_message_uuid() };
            let aprs_id = unsafe { (*settings_enter).generate_aprs_message_id() };
            // thread_id is the other party (recipient for sent messages)
            let mut dm = DirectMessage::new_with_thread(&recipient_enter, &recipient_enter, &text, &uuid, &callsign, &aprs_id);
            
            // Send over RF with message ID for ACK tracking
            if let Ok(r) = radio_enter.lock() {
                if let Err(e) = r.send_aprs_message(&callsign, &recipient_enter, &text, &aprs_id) {
                    eprintln!("[pocket-modem] Failed to send APRS message: {}", e);
                } else {
                    eprintln!("[pocket-modem] APRS message sent to {} [{}]: {}", recipient_enter, aprs_id, text);
                }
            }
            
            dm.mark_sent();
            // Set initial retry timestamp (first APRS retry at 30s per spec)
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            dm.last_retry_timestamp = now;
            unsafe { (*settings_enter).add_aprs_message(dm.clone()); }
            eprintln!("[pocket-modem] Message stored: id={}, aprs_id={}, to={}, status={:?}", 
                      dm.id, dm.aprs_id, dm.to_callsign, dm.status);
            add_direct_message_bubble(&messages_enter, &dm, &callsign);
            entry_enter.set_text("");
            scroll_to_bottom(&scroll_enter);
        });
    }
    
    /// Add a message bubble to the chat UI
    fn add_direct_message_bubble(messages_box: &gtk::Box, msg: &DirectMessage, our_callsign: &str) {
        use crate::aprs::{get_message_status_icon, get_message_status_color};
        
        // Determine if this is a sent message by comparing from_callsign to our callsign
        let is_sent = msg.from_callsign == our_callsign;
        
        // Create a container for the entire message bubble
        let bubble_container = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        bubble_container.set_hexpand(true);
        bubble_container.set_margin_start(4);
        bubble_container.set_margin_end(4);
        bubble_container.set_margin_top(2);
        bubble_container.set_margin_bottom(2);
        
        // Inner bubble box that holds the content
        let bubble = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bubble.set_size_request(200, -1);
        bubble.add_css_class("chat-bubble");
        
        // Message label
        let msg_label = gtk::Label::new(None);
        if is_sent {
            msg_label.set_markup(&format!(
                "<span color='#ffffff'>{}</span>",
                escape_markup(&msg.body)
            ));
            msg_label.set_halign(gtk::Align::End);
            bubble.add_css_class("chat-bubble-sent");
        } else {
            msg_label.set_markup(&format!(
                "<span color='#FFFFFF'>{}</span>",
                escape_markup(&msg.body)
            ));
            msg_label.set_halign(gtk::Align::Start);
            bubble.add_css_class("chat-bubble-received");
        }
        msg_label.set_wrap(true);
        msg_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        msg_label.set_margin_start(6);
        msg_label.set_margin_end(6);
        msg_label.set_margin_top(1);
        msg_label.set_margin_bottom(0);
        
        // Bottom row: time stamp + status
        let bottom_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        bottom_row.set_halign(gtk::Align::End);
        bottom_row.set_margin_start(6);
        bottom_row.set_margin_end(6);
        bottom_row.set_margin_bottom(1);
        
        // Time stamp
        let time_label = gtk::Label::new(None);
        // Format: show date if not today, else just time
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let msg_time = msg.timestamp;
        
        // Check if same day
        let time_str = if msg_time > 0 {
            let msg_day = msg_time / 86400;
            let now_day = now / 86400;
            if msg_day == now_day {
                // Same day: show HH:MM
                format!("{:02}:{:02}", (msg_time / 3600) % 24, (msg_time / 60) % 60)
            } else if msg_day == now_day - 1 {
                // Yesterday: show "yest HH:MM"
                format!("yest {:02}:{:02}", (msg_time / 3600) % 24, (msg_time / 60) % 60)
            } else {
                // Older: show month/day HH:MM
                let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
                let month_idx = ((msg_time % 31557600) / 2592000) as usize;
                let day = ((msg_time % 2592000) / 86400) as usize + 1;
                let month_str = months.get(month_idx).unwrap_or(&"??");
                format!("{} {} {:02}:{:02}", month_str, day, (msg_time / 3600) % 24, (msg_time / 60) % 60)
            }
        } else {
            "--:--".to_string()
        };
        // Use Pango markup for explicit styling
        time_label.set_markup(&format!(
            "<span size='7000' color='#999999'>{}</span>",
            escape_markup(&time_str)
        ));
        time_label.set_halign(gtk::Align::End);
        
        // Status label for sent messages only (to the right of timestamp for sent)
        let status_label = gtk::Label::new(None);
        if is_sent {
            let icon = get_message_status_icon(msg.status);
            let color = get_message_status_color(msg.status);
            let retry_text = if msg.retries > 0 {
                format!(" {} <sub>{}</sub>", icon, msg.retries)
            } else {
                format!(" {}", icon)
            };
            status_label.set_markup(&format!(
                "<span color='{}'>{}</span>",
                color, retry_text
            ));
            bottom_row.append(&time_label);
            bottom_row.append(&status_label);
        } else {
            bottom_row.append(&status_label);  // Hidden for received
            bottom_row.append(&time_label);
        }
        
        bubble.append(&msg_label);
        bubble.append(&bottom_row);
        
        // Put bubble in container with alignment
        if is_sent {
            let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            spacer.set_hexpand(true);
            bubble_container.append(&spacer);
            bubble_container.append(&bubble);
            bubble_container.set_halign(gtk::Align::End);
        } else {
            bubble_container.append(&bubble);
            let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            spacer.set_hexpand(true);
            bubble_container.append(&spacer);
            bubble_container.set_halign(gtk::Align::Start);
        }
        
        messages_box.append(&bubble_container);
        messages_box.show();
    }
    
    // =========================================================================
    // Channel Edit Dialog
    // =========================================================================
    fn show_channel_edit_dialog<F, D>(parent: &gtk::Button, channel: &Channel, on_save: F, on_delete: D)
    where F: FnOnce(Channel) + 'static, D: FnOnce() + 'static {
        use crate::settings::{Duplex, ToneMode, PowerLevel};
        
        let dialog = gtk::Dialog::with_buttons(
            Some(&format!("Edit: {}", channel.name)),
            parent.root().and_then(|r| r.downcast::<gtk::Window>().ok()).as_ref(),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[("Cancel", gtk::ResponseType::Cancel), ("Save", gtk::ResponseType::Accept)],
        );
        
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(12);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.set_margin_bottom(12);
        
        // Name
        let name_row = adw::ActionRow::new();
        name_row.set_title("Name");
        let name_entry = gtk::Entry::new();
        name_entry.set_text(&channel.name);
        name_entry.set_hexpand(true);
        name_row.add_suffix(&name_entry);
        name_row.set_activatable_widget(Some(&name_entry));
        content.append(&name_row);
        
        // Frequency
        let freq_row = adw::ActionRow::new();
        freq_row.set_title("RX Frequency (MHz)");
        let freq_entry = gtk::Entry::new();
        freq_entry.set_text(&format!("{:.3}", channel.rx_freq_khz as f64 / 1000.0));
        freq_entry.set_hexpand(true);
        freq_row.add_suffix(&freq_entry);
        freq_row.set_activatable_widget(Some(&freq_entry));
        content.append(&freq_row);
        
        // Duplex mode
        let duplex_row = adw::ActionRow::new();
        duplex_row.set_title("Duplex");
        let duplex_combo = gtk::DropDown::from_strings(&["Simplex", "+", "-", "Split"]);
        duplex_combo.set_hexpand(true);
        let duplex_idx = match channel.duplex {
            Duplex::Simplex => 0,
            Duplex::Plus => 1,
            Duplex::Minus => 2,
            Duplex::Split => 3,
        };
        duplex_combo.set_selected(duplex_idx as u32);
        duplex_row.add_suffix(&duplex_combo);
        content.append(&duplex_row);
        
        // Offset (shown for + and - duplex)
        let offset_row = adw::ActionRow::new();
        offset_row.set_title("Offset (kHz)");
        let offset_entry = gtk::Entry::new();
        offset_entry.set_text(&format!("{}", channel.offset_khz));
        offset_entry.set_hexpand(true);
        offset_row.add_suffix(&offset_entry);
        content.append(&offset_row);
        
        // TX Frequency (shown for Split duplex)
        let tx_freq_row = adw::ActionRow::new();
        tx_freq_row.set_title("TX Frequency (MHz)");
        let tx_freq_entry = gtk::Entry::new();
        let tx_freq_val = channel.tx_freq_khz.unwrap_or(channel.rx_freq_khz);
        tx_freq_entry.set_text(&format!("{:.3}", tx_freq_val as f64 / 1000.0));
        tx_freq_entry.set_hexpand(true);
        tx_freq_row.add_suffix(&tx_freq_entry);
        content.append(&tx_freq_row);
        
        // Tone Mode
        let tone_row = adw::ActionRow::new();
        tone_row.set_title("Tone Mode");
        let tone_combo = gtk::DropDown::from_strings(&["None", "Tone", "TSQL"]);
        tone_combo.set_hexpand(true);
        let tone_idx = match channel.tone_mode {
            ToneMode::None => 0,
            ToneMode::Tone => 1,
            ToneMode::Tsql => 2,
        };
        tone_combo.set_selected(tone_idx as u32);
        tone_row.add_suffix(&tone_combo);
        content.append(&tone_row);
        
        // RX Tone (for Tone and TSQL)
        let rtone_row = adw::ActionRow::new();
        rtone_row.set_title("RX Tone (Hz)");
        let rtone_entry = gtk::Entry::new();
        rtone_entry.set_text(&format!("{:.1}", channel.rtone_hz));
        rtone_entry.set_hexpand(true);
        rtone_row.add_suffix(&rtone_entry);
        content.append(&rtone_row);
        
        // TX Tone (for TSQL only)
        let ctone_row = adw::ActionRow::new();
        ctone_row.set_title("TX Tone (Hz)");
        let ctone_entry = gtk::Entry::new();
        ctone_entry.set_text(&format!("{:.1}", channel.ctone_hz));
        ctone_entry.set_hexpand(true);
        ctone_row.add_suffix(&ctone_entry);
        content.append(&ctone_row);
        
        // Power Level
        let power_row = adw::ActionRow::new();
        power_row.set_title("Power");
        let power_combo = gtk::DropDown::from_strings(&["High", "Low"]);
        power_combo.set_hexpand(true);
        let power_idx = match channel.power {
            PowerLevel::High => 0,
            PowerLevel::Low => 1,
        };
        power_combo.set_selected(power_idx as u32);
        power_row.add_suffix(&power_combo);
        content.append(&power_row);
        
        // Squelch Level
        let squelch_row = adw::ActionRow::new();
        squelch_row.set_title("Squelch");
        let squelch_combo = gtk::DropDown::from_strings(&["Open", "1", "2", "3", "4", "5", "6", "7", "8"]);
        squelch_combo.set_hexpand(true);
        squelch_combo.set_selected(channel.squelch as u32);
        squelch_row.add_suffix(&squelch_combo);
        content.append(&squelch_row);
        
        // Delete button
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
            if let Some(callback) = on_delete_opt.borrow_mut().take() { callback(); }
            dialog_for_delete.close();
        });
        
        let on_save_opt = RefCell::new(Some(on_save));
        let channel_clone = channel.clone();
        let dialog_for_close = dialog.clone();
        
        dialog.connect_response(move |_d, response| {
            if response == gtk::ResponseType::Accept {
                let name = name_entry.text().to_string();
                let freq_text = freq_entry.text().to_string();
                let offset_text = offset_entry.text().to_string();
                let tx_freq_text = tx_freq_entry.text().to_string();
                let rtone_text = rtone_entry.text().to_string();
                let ctone_text = ctone_entry.text().to_string();
                
                let freq_mhz: f64 = freq_text.parse().unwrap_or(channel_clone.rx_freq_khz as f64 / 1000.0);
                let rx_freq_khz = (freq_mhz * 1000.0) as u32;
                
                let offset_khz: u32 = offset_text.parse().unwrap_or(channel_clone.offset_khz);
                let tx_freq_mhz: f64 = tx_freq_text.parse().unwrap_or(tx_freq_val as f64 / 1000.0);
                let tx_freq_khz = (tx_freq_mhz * 1000.0) as u32;
                
                let rtone_hz: f32 = rtone_text.parse().unwrap_or(channel_clone.rtone_hz);
                let ctone_hz: f32 = ctone_text.parse().unwrap_or(channel_clone.ctone_hz);
                
                let duplex = match duplex_combo.selected() {
                    0 => Duplex::Simplex,
                    1 => Duplex::Plus,
                    2 => Duplex::Minus,
                    3 => Duplex::Split,
                    _ => Duplex::Simplex,
                };
                
                let tone_mode = match tone_combo.selected() {
                    0 => ToneMode::None,
                    1 => ToneMode::Tone,
                    2 => ToneMode::Tsql,
                    _ => ToneMode::None,
                };
                
                let power = match power_combo.selected() {
                    0 => PowerLevel::High,
                    1 => PowerLevel::Low,
                    _ => PowerLevel::High,
                };
                
                let tx_freq_khz_opt = if duplex == Duplex::Split {
                    Some(tx_freq_khz)
                } else {
                    None
                };
                
                let squelch = squelch_combo.selected() as u8;
                
                let updated = Channel {
                    location: channel_clone.location,
                    name,
                    rx_freq_khz,
                    duplex,
                    offset_khz,
                    tx_freq_khz: tx_freq_khz_opt,
                    tone_mode,
                    rtone_hz,
                    ctone_hz,
                    power,
                    mode: channel_clone.mode.clone(),
                    squelch,
                    comment: channel_clone.comment.clone(),
                };
                
                if let Some(callback) = on_save_opt.borrow_mut().take() { callback(updated); }
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
        .freq-display { font-size: 60px; font-family: monospace; font-weight: bold; color: #FFB000; background: #1E1E1E; border: 3px solid #444; border-radius: 12px; text-shadow: 0 0 8px rgba(255, 176, 0, 0.5); box-shadow: inset 0 0 16px rgba(0, 0, 0, 0.8); caret-color: #FFB000; }
        .freq-display:focus { border-color: #FFB000; }
        .tx-frequency { color: #ff4444; border-color: #ff4444; }
        .status-icon-green { font-size: 14px; color: #33D17A; }
        .status-icon-red { font-size: 14px; color: #ff4444; }
        .gps-led-on { color: #33D17A; font-size: 14px; }
        .gps-led-off { color: #666; font-size: 14px; }
        .gps-led-searching { color: #FFB000; font-size: 14px; }
        .status-icon-gray-empty { font-size: 14px; color: #666; }
        .status-icon-gray-filled { font-size: 14px; color: #888; }
        .modem-label, .gps-label, .audio-label { color: #666; font-size: 11px; }
        .status-btn { background: transparent; border: none; padding: 4px 8px; border-radius: 8px; }
        .status-btn:hover { background: #333; }
        .status-btn:active { background: #444; }
        .status-text { font-size: 14px; color: #888; }
        .status-success { color: #33D17A; }
        .status-warning { color: #FFB000; }
        .status-error { color: #ff4444; }
        .title-label { font-size: 18px; font-weight: bold; color: #FFB000; padding: 12px 0 0 12px; }
        .signal-text { font-size: 11px; color: #888; font-weight: bold; }
        .signal-value { font-size: 11px; font-weight: bold; }
        .rssi-bar { background: #2a2a2a; border: 1px solid #444; border-radius: 4px; }
        .rssi-bar.empty { opacity: 0.3; }
        .rssi-bar.bar-tx progress { background: #FF4444; }
        .rssi-bar.bar-rx progress { background: #FFB000; }
        .ptt-button { min-width: 90px; min-height: 100px; border-radius: 12px; background: #333; border: 2px solid #555; }
        .ptt-button:active { background: #444; border-color: #FFB000; }
        .ptt-icon { color: #888; }
        .ptt-button:active .ptt-icon { color: #FFB000; }
        .ptt-label { font-size: 11px; font-weight: bold; color: #888; }
        .ptt-button:active .ptt-label { color: #FFB000; }
        .squelch-value { font-size: 14px; font-weight: bold; color: #888; }
        .channel-section-title { font-size: 14px; color: #888; }
        .channel-list { background: #2a2a2a; border-radius: 8px; border: 1px solid #444; }
        .channel-row { background: transparent; }
        .channel-row:hover { background: #333; }
        .channel-row-selected { background: #3a3a3a; }
        .aprs-list { background: #2a2a2a; border-radius: 8px; border: 1px solid #444; }
        .aprs-message-row { background: transparent; border-bottom: 1px solid #333; padding: 8px; }
        .aprs-message-row:hover { background: #333; }
        .chat-row-sent { background: transparent; }
        .chat-row-received { background: transparent; }
        .aprs-callsign { font-size: 14px; font-weight: bold; }
        .aprs-to-callsign { font-size: 14px; color: #888; }
        .aprs-timestamp { font-size: 12px; color: #666; }
        .aprs-comment { font-size: 13px; color: #888; font-style: italic; }
        .aprs-message-body { font-size: 14px; color: #aaa; }
        .aprs-empty-text { font-size: 14px; color: #666; padding: 32px; }
        .locator-display { font-size: 18px; font-family: monospace; min-width: 1px; }
        .coords-display { font-size: 14px; color: #888; font-family: monospace; min-width: 1px; }
        .user-marker { background: transparent; }
        .user-marker-dot { color: #1a73e8; }
        .aprs-marker { background: rgba(30, 30, 30, 0.8); border-radius: 4px; padding: 2px 4px; }
        .aprs-symbol { font-size: 14px; color: #FFB000; }
        .aprs-callsign { font-size: 11px; color: #deddda; }
        .map-recenter-btn { background: rgba(30, 30, 30, 0.8); border-radius: 20px; min-width: 40px; min-height: 40px; }
        .map-recenter-btn:hover { background: rgba(50, 50, 50, 0.9); }
        .map-recenter-btn:active { background: rgba(70, 70, 70, 0.9); }
        .beacon-button { background: #333; border: 2px solid #555; border-radius: 12px; }
        .beacon-button:hover { background: #444; border-color: #FFB000; }
        .beacon-button:active { background: #444; border-color: #FFB000; }
        .beacon-button:disabled { opacity: 0.5; }
        .beacon-button:disabled .beacon-icon { color: #666; }
        .beacon-button:disabled .beacon-label { color: #666; }
        .beacon-icon { color: #888; }
        .beacon-button:active .beacon-icon { color: #FFB000; }
        .beacon-label { font-size: 14px; font-weight: bold; color: #888; }
        .beacon-button:active .beacon-label { color: #FFB000; }
        .beacon-status { font-size: 12px; color: #666; min-height: 20px; }
        .chat-bubble { border-radius: 16px; max-width: 260px; padding: 6px; box-shadow: 0 4px 12px rgba(0,0,0,0.8); }
        .chat-bubble-sent { background: #4a4228; border: 1px solid #FFB000; border-bottom-right-radius: 4px; }
        .chat-bubble-received { background: #454545; border: 1px solid #888; border-bottom-left-radius: 4px; }
        .chat-timestamp { font-size: 9px !important; color: #555555 !important; font-weight: lighter; }
        .chat-messages-box { background: transparent; }
        .chat-scroll-frame { background: #1a1a1a; border-radius: 8px; box-shadow: inset 0 0 20px rgba(0,0,0,0.5); }
    "#);
    
    gtk::style_context_add_provider_for_display(
        &gtk::prelude::RootExt::display(&window),
        &css_provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION
    );
    
    // Wrap content in a Box with strict width constraint
    let fixed_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    fixed_container.set_size_request(340, 700);
    fixed_container.set_hexpand(false);
    fixed_container.set_vexpand(false);
    fixed_container.set_halign(gtk::Align::Center);
    fixed_container.set_valign(gtk::Align::Start);
    
    fixed_container.append(&header_bar);
    fixed_container.append(&toast_overlay);
    
    window.set_content(Some(&fixed_container));
    window.show();
    
    // Force window size after content is set - GTK may expand during layout
    window.set_size_request(360, 700);
    window.set_default_size(370, 700);
    

}
