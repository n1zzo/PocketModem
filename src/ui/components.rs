//! Reusable UI components for PocketModem

use adw::prelude::{ActionRowExt, PreferencesRowExt};
use gtk::{self, prelude::*};

// ============================================================================
// Status Button Component
// ============================================================================

/// Create a status indicator button (MODEM, GPS, AUDIO)
///
/// Returns (button, led_label) where led_label can be updated by the update loop
pub fn status_button(
    icon_name: &str,
    led_text: &str,
    label_text: &str,
    led_css_classes: &[&str],
    label_css_class: &str,
) -> (gtk::Button, gtk::Label) {
    let btn = gtk::Button::new();
    btn.add_css_class("flat");
    btn.add_css_class("status-btn");
    
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(28);
    
    let led = gtk::Label::new(Some(led_text));
    for cls in led_css_classes {
        led.add_css_class(cls);
    }
    
    let label = gtk::Label::new(Some(label_text));
    label.add_css_class("status-text");
    label.add_css_class(label_css_class);
    
    b.append(&icon);
    b.append(&led);
    b.append(&label);
    btn.set_child(Some(&b));
    
    (btn, led)
}

// ============================================================================
// Detail Window Components
// ============================================================================

/// Create a simple modal detail window with a title, content box, and close button
pub fn detail_window(
    title: &str,
    width: i32,
    height: i32,
    content: gtk::Box,
    close_callback: impl Fn() + 'static,
) -> gtk::Window {
    let window = gtk::Window::builder()
        .title(title)
        .default_width(width)
        .default_height(height)
        .decorated(false)
        .modal(true)
        .build();
    
    let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
    b.set_margin_start(12);
    b.set_margin_end(12);
    
    let title_label = gtk::Label::new(Some(title));
    title_label.add_css_class("title-label");
    
    let close_btn = gtk::Button::with_label("Close");
    let win = window.clone();
    close_btn.connect_clicked(move |_| { 
        win.hide(); 
        close_callback();
    });
    
    b.append(&title_label);
    b.append(&content);
    b.append(&close_btn);
    window.set_child(Some(&b));
    
    window
}

/// Create an action row with label and value
pub fn action_row(title: &str, initial_value: &str) -> (adw::ActionRow, gtk::Label) {
    let row = adw::ActionRow::new();
    row.set_title(title);
    
    let value = gtk::Label::new(Some(initial_value));
    value.add_css_class("status-text");
    row.add_suffix(&value);
    
    (row, value)
}

// ============================================================================
// Channel Row Component
// ============================================================================

/// Create a channel list item row
pub fn channel_row(
    channel: &crate::settings::Channel,
    is_selected: bool,
    on_click: impl Fn() + 'static,
    on_edit: impl Fn() + 'static,
) -> gtk::Box {
    use crate::settings::{Duplex, ToneMode};
    
    let freq_mhz = channel.rx_freq_khz as f64 / 1000.0;
    
    let mut subtitle_parts = Vec::new();
    match channel.duplex {
        Duplex::Plus => subtitle_parts.push(format!("(+{:.3})", channel.offset_khz as f64 / 1000.0)),
        Duplex::Minus => subtitle_parts.push(format!("(-{:.3})", channel.offset_khz as f64 / 1000.0)),
        Duplex::Split => subtitle_parts.push(format!("(split)"))
,
        Duplex::Simplex => {}
    }
    match channel.tone_mode {
        ToneMode::Tone => subtitle_parts.push(format!("[{}]", channel.rtone_hz)),
        ToneMode::Tsql => subtitle_parts.push(format!("[{}, {}]", channel.rtone_hz, channel.ctone_hz)),
        ToneMode::None => {}
    }
    
    let subtitle = if subtitle_parts.is_empty() {
        format!("{:.3} MHz", freq_mhz)
    } else {
        format!("{:.3} MHz {}", freq_mhz, subtitle_parts.join(" "))
    };
    
    let row_container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row_container.set_valign(gtk::Align::Center);
    row_container.add_css_class("channel-row");
    if is_selected {
        row_container.add_css_class("channel-row-selected");
    }
    
    let row = adw::ActionRow::builder()
        .title(&channel.name)
        .subtitle(&subtitle)
        .build();
    
    let edit_btn = gtk::Button::new();
    edit_btn.set_icon_name("document-edit-symbolic");
    edit_btn.add_css_class("flat");
    
    let row_clone = row.clone();
    row.add_controller({
        let click = gtk::GestureClick::new();
        click.set_button(1);
        click.connect_pressed(move |_, _, _, _| {
            on_click();
            gtk::prelude::WidgetExt::activate(&row_clone);
        });
        click
    });
    
    edit_btn.connect_clicked(move |_| { on_edit(); });
    
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    
    row_container.append(&row);
    row_container.append(&spacer);
    row_container.append(&edit_btn);
    
    row_container
}

// ============================================================================
// PTT Button Component  
// ============================================================================

/// Create a PTT (Push-to-Talk) button
pub fn ptt_button() -> (gtk::Button, gtk::Label) {
    let btn = gtk::Button::new();
    btn.add_css_class("ptt-button");
    btn.set_margin_start(20);
    btn.set_margin_end(20);
    btn.set_margin_bottom(8);
    btn.set_valign(gtk::Align::End);
    btn.set_tooltip_text(Some("Hold to transmit"));
    
    let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
    b.set_halign(gtk::Align::Center);
    b.set_valign(gtk::Align::Center);
    
    let icon = gtk::Image::from_icon_name("media-record-symbolic");
    icon.set_pixel_size(40);
    icon.add_css_class("ptt-icon");
    
    let label = gtk::Label::new(Some("PTT"));
    label.add_css_class("ptt-label");
    
    b.append(&icon);
    b.append(&label);
    btn.set_child(Some(&b));
    
    (btn, label)
}

// ============================================================================
// Scroll Container
// ============================================================================

/// Create a scroll container with vertical scrolling
pub fn scroll_container(min_height: i32, max_height: i32) -> gtk::ScrolledWindow {
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_min_content_height(min_height);
    scroll.set_max_content_height(max_height);
    scroll
}

// ============================================================================
// APRS Message Row
// ============================================================================

use crate::aprs::APRSMessage;
use crate::utils::{escape_markup, calculate_distance_display, bearing_to_compass, calculate_distance_bearing};

/// Create an APRS message row widget
pub fn aprs_message_row(msg: &APRSMessage, my_lat: f64, my_lon: f64) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
    row.set_margin_top(8);
    row.set_margin_bottom(8);
    row.add_css_class("aprs-message-row");
    
    // Header: from → to callsigns
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.set_halign(gtk::Align::Start);
    
    let from_label = gtk::Label::new(None);
    from_label.set_markup(&format!("<span color='#FFB000'>{}</span>", escape_markup(&msg.from_callsign)));
    from_label.add_css_class("aprs-callsign");
    
    let to_label = gtk::Label::new(None);
    to_label.set_markup(&format!("→ <span color='#FFB000'>{}</span>", escape_markup(&msg.to_callsign)));
    to_label.add_css_class("aprs-to-callsign");
    
    let time_label = gtk::Label::new(None);
    let time_str = if let Some(t) = msg.timestamp {
        format!("{:02}:{:02}", (t / 3600) % 24, (t / 60) % 60)
    } else {
        "--:--".to_string()
    };
    time_label.set_text(&time_str);
    time_label.add_css_class("aprs-timestamp");
    
    header.append(&from_label);
    header.append(&to_label);
    header.append(&time_label);
    
    // Content
    let content = gtk::Label::new(None);
    content.set_halign(gtk::Align::Start);
    content.set_valign(gtk::Align::Start);
    content.set_wrap(true);
    content.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    
    match msg.msg_type {
        crate::aprs::APRSType::Position => {
            if msg.position_lat != 0.0 || msg.position_lon != 0.0 {
                let dist_text = calculate_distance_display(my_lat, my_lon, msg.position_lat, msg.position_lon);
                let bearing_text = if let Some((_, bearing)) = calculate_distance_bearing(my_lat, my_lon, msg.position_lat, msg.position_lon) {
                    bearing_to_compass(bearing)
                } else {
                    "--°".to_string()
                };
                
                let comment = if msg.comment.is_empty() { 
                    String::new() 
                } else { 
                    format!(" - {}", escape_markup(&msg.comment)) 
                };
                content.set_markup(&format!("<span color='#33D17A'>📍 {} {}</span>{}", dist_text, bearing_text, comment));
            } else {
                content.set_text(&msg.comment);
                content.add_css_class("aprs-comment");
            }
        }
        crate::aprs::APRSType::Message => {
            let body = msg.msg_body.as_deref().unwrap_or("");
            content.set_markup(&format!(
                "<span color='#888888'>Message to {}:</span>\n{}", 
                msg.to_callsign_msg.as_deref().unwrap_or(&msg.to_callsign), 
                escape_markup(body)
            ));
            content.add_css_class("aprs-message-body");
        }
        _ => {
            content.set_text(&msg.comment);
            content.add_css_class("aprs-comment");
        }
    }
    
    row.append(&header);
    row.append(&content);
    
    row
}