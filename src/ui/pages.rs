//! Page builders for PocketModem UI

use std::sync::{Arc, Mutex};

use adw::prelude::*;
use gtk::prelude::*;

use crate::settings::SettingsManager;
use crate::radio::KV4PRadio;
use crate::gps::GpsManager;
use crate::audio::AudioManager;
use crate::map::MapManager;

use super::components::*;

/// Map margins for carousel swiping (in pixels)
const MAP_MARGIN: i32 = 8;

// ============================================================================
// Header Bar
// ============================================================================

pub fn header_bar() -> (adw::HeaderBar, gtk::ToggleButton) {
    let header_bar = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("PocketModem", ""))
        .build();
    
    let settings_btn = gtk::ToggleButton::new();
    settings_btn.set_icon_name("emblem-system-symbolic");
    settings_btn.add_css_class("flat");
    settings_btn.set_tooltip_text(Some("Settings"));
    header_bar.pack_end(&settings_btn);
    
    (header_bar, settings_btn)
}

// ============================================================================
// Main Page (VFO + S-meter + Channels + PTT)
// ============================================================================

pub fn main_page(
    radio: &Arc<Mutex<KV4PRadio>>,
    audio: &Arc<Mutex<AudioManager>>,
    _gps: &Arc<Mutex<GpsManager>>,
    settings: &SettingsManager,
) -> (gtk::Box, gtk::Entry, gtk::ProgressBar, gtk::Label) {
    let saved_freq = settings.frequency();
    
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    
    // Status indicators
    let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 32);
    status_row.set_halign(gtk::Align::Center);
    status_row.set_margin_top(16);
    status_row.set_margin_bottom(16);
    
    let (modem_btn, modem_led) = status_button(
        "network-wireless-symbolic",
        "○",
        "MODEM",
        &["status-icon-red"],
        "modem-label",
    );
    let (gps_btn, gps_led) = status_button(
        "location-services-active-symbolic",
        "○",
        "GPS",
        &["gps-led-off"],
        "gps-label",
    );
    let (audio_btn, audio_led) = status_button(
        "audio-volume-medium-symbolic",
        "○",
        "AUDIO",
        &["status-icon-gray"],
        "audio-label",
    );
    
    status_row.append(&modem_btn);
    status_row.append(&gps_btn);
    status_row.append(&audio_btn);
    page.append(&status_row);
    
    // Frequency display
    let freq_entry = gtk::Entry::new();
    freq_entry.set_text(&format!("{}.{:03}", saved_freq / 1000, saved_freq % 1000));
    gtk::prelude::EntryExt::set_alignment(&freq_entry, 0.5);
    freq_entry.add_css_class("freq-display");
    freq_entry.set_margin_start(16);
    freq_entry.set_margin_end(16);
    freq_entry.set_margin_top(8);
    freq_entry.set_margin_bottom(4);
    freq_entry.set_input_purpose(gtk::InputPurpose::Number);
    
    // Connect frequency entry activation
    let radio_clone = Arc::clone(radio);
    let settings_for_freq = settings as *const SettingsManager as *mut SettingsManager;
    freq_entry.connect_activate(move |entry| {
        let text = entry.text().to_string();
        if let Ok(freq_mhz) = text.parse::<f64>() {
            let khz = (freq_mhz * 1000.0) as u32;
            let radio = Arc::clone(&radio_clone);
            
            unsafe { (*settings_for_freq).set_frequency(khz); }
            
            std::thread::spawn(move || {
                if let Ok(r) = radio.lock() {
                    let _ = r.set_frequency(khz);
                }
            });
            
            entry.set_text(&format!("{}.{:03}", khz / 1000, khz % 1000));
        }
    });
    page.append(&freq_entry);
    
    // RSSI / S-meter
    let rssi_sbar = gtk::ProgressBar::new();
    rssi_sbar.set_fraction(0.0);
    rssi_sbar.add_css_class("rssi-bar");
    
    let smeter_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    smeter_box.set_halign(gtk::Align::Center);
    smeter_box.set_margin_start(24);
    smeter_box.set_margin_end(24);
    smeter_box.set_margin_top(12);
    smeter_box.set_margin_bottom(20);
    
    let signal_label = gtk::Label::new(Some("SIGNAL"));
    signal_label.add_css_class("signal-text");
    signal_label.set_width_request(60);
    
    let signal_value = gtk::Label::new(None);
    signal_value.add_css_class("signal-value");
    signal_value.set_markup(&format!("<span color='#FFB000'>{}</span>", "--%"));
    signal_value.set_width_request(70);
    
    smeter_box.append(&signal_label);
    smeter_box.append(&rssi_sbar);
    smeter_box.append(&signal_value);
    page.append(&smeter_box);
    
    // Channel section
    let channel_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    channel_container.set_margin_start(16);
    channel_container.set_margin_end(16);
    
    let channel_header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    channel_header.set_margin_bottom(8);
    
    let channel_title = gtk::Label::new(Some("<b>Channels</b>"));
    channel_title.set_markup("<b>Channels</b>");
    channel_title.set_halign(gtk::Align::Start);
    channel_title.add_css_class("channel-section-title");
    
    let channel_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    channel_spacer.set_hexpand(true);
    
    let add_channel_btn = gtk::Button::new();
    add_channel_btn.set_icon_name("list-add-symbolic");
    add_channel_btn.add_css_class("flat");
    add_channel_btn.set_tooltip_text(Some("Add current frequency"));
    
    channel_header.append(&channel_title);
    channel_header.append(&channel_spacer);
    channel_header.append(&add_channel_btn);
    
    let no_channels_row = adw::ActionRow::builder()
        .title("No channels")
        .subtitle("Tap + to add")
        .build();
    no_channels_row.set_sensitive(false);
    
    let channel_list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    channel_list.add_css_class("channel-list");
    
    let channel_count = settings.channels().len();
    no_channels_row.set_visible(channel_count == 0);
    channel_list.append(&no_channels_row);
    
    let channel_scroll = scroll_container(80, 200);
    channel_scroll.set_child(Some(&channel_list));
    
    channel_container.append(&channel_header);
    channel_container.append(&channel_scroll);
    page.append(&channel_container);
    
    // PTT Button
    let (ptt_btn, ptt_label) = ptt_button();
    
    // PTT gesture
    let radio_pressed = Arc::clone(radio);
    let audio_pressed = Arc::clone(audio);
    let radio_released = Arc::clone(radio);
    let audio_released = Arc::clone(audio);
    let settings_for_ptt = settings as *const SettingsManager as *mut SettingsManager;
    let freq_clone = freq_entry.clone();
    
    let gesture = gtk::GestureClick::new();
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    gesture.set_button(0);
    
    gesture.connect_pressed({
        let r = radio_pressed.clone();
        let a = audio_pressed.clone();
        let label = ptt_label.clone();
        let freq = freq_clone.clone();
        let settings = settings_for_ptt;
        move |_, _, _, _| {
            label.set_text("TX");
            let rx_freq = unsafe { (*settings).frequency() };
            freq.set_text(&format!("{}.{:03}", rx_freq / 1000, rx_freq % 1000));
            freq.add_css_class("tx-frequency");
            
            if let Ok(mut rad) = r.lock() {
                let _ = rad.set_frequency(rx_freq);
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
        let freq = freq_clone.clone();
        let settings = settings_for_ptt;
        move |_, _, _, _| {
            if let Ok(mut aud) = a.lock() { let _ = aud.stop_capture(); }
            let rx_freq = unsafe { (*settings).frequency() };
            if let Ok(mut rad) = r.lock() { let _ = rad.ptt_off(); }
            freq.remove_css_class("tx-frequency");
            freq.set_text(&format!("{}.{:03}", rx_freq / 1000, rx_freq % 1000));
            label.set_text("PTT");
        }
    });
    
    ptt_btn.add_controller(gesture);
    page.append(&ptt_btn);
    
    (page, freq_entry, rssi_sbar, signal_value)
}

// ============================================================================
// APRS Page
// ============================================================================

pub fn aprs_page() -> (gtk::Box, gtk::Label, gtk::Box) {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.set_margin_start(8);
    page.set_margin_end(8);
    page.set_margin_top(MAP_MARGIN);
    page.set_margin_bottom(MAP_MARGIN);
    
    let header = gtk::Label::new(Some("<b>APRS Messages</b>"));
    header.set_markup("<b>APRS Messages</b>");
    header.set_halign(gtk::Align::Start);
    header.set_margin_start(8);
    header.set_margin_top(8);
    header.set_margin_bottom(8);
    
    let list_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list_box.add_css_class("aprs-list");
    
    let empty_label = gtk::Label::new(Some("No APRS messages"));
    empty_label.set_halign(gtk::Align::Center);
    empty_label.add_css_class("aprs-empty-text");
    list_box.append(&empty_label);
    
    let scroll = scroll_container(200, 0);
    scroll.set_child(Some(&list_box));
    
    page.append(&header);
    page.append(&scroll);
    
    (page, empty_label, list_box)
}

// ============================================================================
// Map Page
// ============================================================================

pub fn map_page(map_manager: &Arc<Mutex<MapManager>>) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.set_margin_top(MAP_MARGIN);
    page.set_margin_bottom(MAP_MARGIN);
    page.set_margin_start(4);
    page.set_margin_end(4);
    
    let map_view = {
        let mm = map_manager.lock().unwrap();
        mm.view().clone()
    };
    
    map_view.set_hexpand(false);
    map_view.set_vexpand(true);
    map_view.set_valign(gtk::Align::Fill);
    
    // Re-center button
    let recenter_btn = gtk::Button::new();
    recenter_btn.set_icon_name("find-location-symbolic");
    recenter_btn.add_css_class("map-recenter-btn");
    recenter_btn.set_tooltip_text(Some("Center on my location"));
    recenter_btn.set_halign(gtk::Align::End);
    recenter_btn.set_valign(gtk::Align::End);
    recenter_btn.set_margin_end(12);
    recenter_btn.set_margin_bottom(12);
    recenter_btn.set_size_request(40, 40);
    
    let map_mgr = Arc::clone(map_manager);
    recenter_btn.connect_clicked(move |_| {
        if let Ok(mut mm) = map_mgr.lock() {
            mm.center_on_user();
        }
    });
    
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&map_view));
    overlay.add_overlay(&recenter_btn);
    
    page.append(&overlay);
    
    page
}

// ============================================================================
// Settings Page
// ============================================================================

pub fn settings_page(
    radio: &Arc<Mutex<KV4PRadio>>,
    settings: &SettingsManager,
) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    
    let settings_box = gtk::Box::new(gtk::Orientation::Vertical, 16);
    settings_box.set_margin_top(12);
    settings_box.set_margin_start(12);
    settings_box.set_margin_end(12);
    settings_box.set_margin_bottom(12);
    
    // Modem section
    let modem_group = adw::PreferencesGroup::builder()
        .title("Modem")
        .build();
    
    let saved_squelch = settings.squelch();
    let radio_sq = Arc::clone(radio);
    let settings_sq = settings as *const SettingsManager as *mut SettingsManager;
    let last_sent_sq: Arc<std::sync::atomic::AtomicU8> = Arc::new(std::sync::atomic::AtomicU8::new(saved_squelch));
    
    let adj = gtk::Adjustment::new(saved_squelch as f64, 0.0, 8.0, 1.0, 0.0, 0.0);
    let squelch_row = adw::SpinRow::builder()
        .title("Squelch Level")
        .subtitle("Signal threshold for audio output")
        .adjustment(&adj)
        .build();
    squelch_row.set_digits(0);
    squelch_row.set_numeric(true);
    squelch_row.set_snap_to_ticks(true);
    
    let sent_clone = Arc::clone(&last_sent_sq);
    let r_clone = Arc::clone(&radio_sq);
    let s_clone = settings_sq;
    adj.connect_value_changed(move |adj| {
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
    
    // TX Power toggle
    let radio_tg = Arc::clone(radio);
    let settings_tg = settings as *const SettingsManager as *mut SettingsManager;
    let initial_high = settings.tx_power_high();
    
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
    
    toggle_group.add(toggle_low);
    toggle_group.add(toggle_high);
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
    
    // Audio section
    let audio_group = adw::PreferencesGroup::builder()
        .title("Audio")
        .build();
    
    let settings_audio = settings as *const SettingsManager as *mut SettingsManager;
    let pre_emph_row = adw::SwitchRow::builder()
        .title("Pre-Emphasis")
        .subtitle("Boost high frequencies before TX")
        .active(settings.pre_emphasis())
        .build();
    pre_emph_row.connect_notify_local(Some("active"), move |row, _| {
        unsafe { (*settings_audio).set_pre_emphasis(row.is_active()); }
    });
    audio_group.add(&pre_emph_row);
    
    let settings_deemph = settings as *const SettingsManager as *mut SettingsManager;
    let de_emph_row = adw::SwitchRow::builder()
        .title("De-Emphasis")
        .subtitle("Reduce high frequencies on RX")
        .active(settings.de_emphasis())
        .build();
    de_emph_row.connect_notify_local(Some("active"), move |row, _| {
        unsafe { (*settings_deemph).set_de_emphasis(row.is_active()); }
    });
    audio_group.add(&de_emph_row);
    
    settings_box.append(&audio_group);
    
    // About section
    let about_group = adw::PreferencesGroup::builder()
        .title("About")
        .build();
    
    let version_row = adw::ActionRow::builder()
        .title("PocketModem")
        .subtitle("Version 1.0.0 - KV4P Radio Controller")
        .build();
    about_group.add(&version_row);
    
    settings_box.append(&about_group);
    
    scroll.set_child(Some(&settings_box));
    page.append(&scroll);
    
    page
}