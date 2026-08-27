//! Settings management with GSettings persistence
//!
//! Provides:
//! - GSettings-backed persistent storage (Flatpak-compatible)
//! - CHIRP CSV import/export for channel backup
//! - Schema migrations for future updates

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use gio::prelude::SettingsExt;
use crate::aprs::{DirectMessage, DirectMessageStatus, MessageThread};

/// Current schema version - increment when adding new settings
const CURRENT_SCHEMA_VERSION: u32 = 4;

/// Default values for settings
pub mod defaults {
    pub const FREQUENCY: u32 = 144200;
    pub const SQUELCH: u8 = 4;
    pub const TX_POWER_HIGH: bool = false;
    pub const PRE_EMPHASIS: bool = false;
    pub const DE_EMPHASIS: bool = false;
    pub const HIGH_PASS_FILTER: bool = true;
    pub const LOW_PASS_FILTER: bool = true;
    pub const MIC_GAIN: &str = "none";
    pub const LAST_CHANNEL_INDEX: i32 = -1;
}

// ============================================================================
// Data Types
// ============================================================================

/// Duplex mode for a channel
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Duplex {
    /// Simplex (same TX/RX frequency)
    Simplex,
    /// Positive offset (+600 kHz for 2m)
    Plus,
    /// Negative offset (-600 kHz for 2m)
    Minus,
    /// Split (independent TX/RX frequencies)
    Split,
}

impl Default for Duplex {
    fn default() -> Self {
        Duplex::Simplex
    }
}

impl std::fmt::Display for Duplex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Duplex::Simplex => write!(f, ""),
            Duplex::Plus => write!(f, "+"),
            Duplex::Minus => write!(f, "-"),
            Duplex::Split => write!(f, "split"),
        }
    }
}

impl std::str::FromStr for Duplex {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "" | "simplex" => Ok(Duplex::Simplex),
            "+" | "plus" => Ok(Duplex::Plus),
            "-" | "minus" => Ok(Duplex::Minus),
            "split" => Ok(Duplex::Split),
            _ => Err(format!("Invalid duplex: {}", s)),
        }
    }
}

/// Tone mode for a channel
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToneMode {
    None,
    Tone,
    Tsql,
}

impl Default for ToneMode {
    fn default() -> Self {
        ToneMode::None
    }
}

impl std::fmt::Display for ToneMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToneMode::None => write!(f, ""),
            ToneMode::Tone => write!(f, "Tone"),
            ToneMode::Tsql => write!(f, "TSQL"),
        }
    }
}

impl std::str::FromStr for ToneMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "" | "none" => Ok(ToneMode::None),
            "tone" | "ctcss" => Ok(ToneMode::Tone),
            "tsql" | "tsq" => Ok(ToneMode::Tsql),
            _ => Err(format!("Invalid tone mode: {}", s)),
        }
    }
}

/// Power level for TX
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerLevel {
    High,
    Low,
}

/// Tile source for offline map
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TileSource {
    Online,   // OSM tiles (requires network)
    Offline,  // MBTiles file (offline)
}

impl Default for TileSource {
    fn default() -> Self {
        TileSource::Online
    }
}

impl std::fmt::Display for TileSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TileSource::Online => write!(f, "Online (OSM)"),
            TileSource::Offline => write!(f, "Offline (MBTiles)"),
        }
    }
}

impl std::str::FromStr for TileSource {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "online" | "osm" | "" => Ok(TileSource::Online),
            "offline" | "mbtiles" | "local" => Ok(TileSource::Offline),
            _ => Err(format!("Invalid tile source: {}", s)),
        }
    }
}

impl Default for PowerLevel {
    fn default() -> Self {
        PowerLevel::High
    }
}

impl std::fmt::Display for PowerLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PowerLevel::High => write!(f, "High"),
            PowerLevel::Low => write!(f, "Low"),
        }
    }
}

impl std::str::FromStr for PowerLevel {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "high" | "1" => Ok(PowerLevel::High),
            "low" | "0" => Ok(PowerLevel::Low),
            _ => Err(format!("Invalid power level: {}", s)),
        }
    }
}

/// A single radio memory channel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    /// Channel number/location (1-999)
    pub location: u16,
    /// User-friendly name
    pub name: String,
    /// Receive frequency in kHz
    pub rx_freq_khz: u32,
    /// Duplex mode (simplex, +offset, -offset, split)
    pub duplex: Duplex,
    /// TX offset in kHz (used for +/- duplex)
    pub offset_khz: u32,
    /// TX frequency in kHz (used for split duplex)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_freq_khz: Option<u32>,
    /// Tone mode
    pub tone_mode: ToneMode,
    /// RX tone frequency in Hz (CTCSS)
    pub rtone_hz: f32,
    /// TX tone frequency in Hz (CTCSS)
    pub ctone_hz: f32,
    /// Mode: "FM" or "NFM"
    pub mode: String,
    /// Power level
    pub power: PowerLevel,
    /// Squelch level (0-8, 0 = open, 8 = max)
    pub squelch: u8,
    /// Optional comment/notes
    pub comment: String,
}

impl Default for Channel {
    fn default() -> Self {
        Channel {
            location: 1,
            name: String::new(),
            rx_freq_khz: 144200,
            duplex: Duplex::Simplex,
            offset_khz: 600,
            tx_freq_khz: None,
            tone_mode: ToneMode::None,
            rtone_hz: 88.5,
            ctone_hz: 88.5,
            mode: "FM".to_string(),
            power: PowerLevel::High,
            squelch: defaults::SQUELCH,
            comment: String::new(),
        }
    }
}

/// Application settings
#[derive(Debug, Clone)]
pub struct AppSettings {
    pub frequency: u32,
    pub squelch: u8,
    pub tx_power_high: bool,
    pub pre_emphasis: bool,
    pub de_emphasis: bool,
    pub high_pass_filter: bool,
    pub low_pass_filter: bool,
    pub mic_gain: String,
    pub channels: Vec<Channel>,
    pub last_channel_index: i32,
    pub tile_source: TileSource,
    pub offline_tiles_path: Option<String>,
    pub schema_version: u32,
    // APRS TX Configuration
    pub aprs_callsign: String,
    pub aprs_ssid: u8,
    pub aprs_symbol_table: char,
    pub aprs_symbol_code: char,
    pub aprs_comment: String,
    pub aprs_tx_enabled: bool,
    pub aprs_beacon_dest: String,  // Destination for beacon (default: "APRS")
    // APRS Messages Storage
    pub aprs_messages: Vec<DirectMessage>,
    // Sequential message ID counter (for message numbering)
    aprs_message_counter: u32,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            frequency: defaults::FREQUENCY,
            squelch: defaults::SQUELCH,
            tx_power_high: defaults::TX_POWER_HIGH,
            pre_emphasis: defaults::PRE_EMPHASIS,
            de_emphasis: defaults::DE_EMPHASIS,
            high_pass_filter: defaults::HIGH_PASS_FILTER,
            low_pass_filter: defaults::LOW_PASS_FILTER,
            mic_gain: defaults::MIC_GAIN.to_string(),
            channels: Vec::new(),
            last_channel_index: defaults::LAST_CHANNEL_INDEX,
            tile_source: TileSource::default(),
            offline_tiles_path: None,
            schema_version: CURRENT_SCHEMA_VERSION,
            // APRS TX defaults - no prefilled values, empty by default
            aprs_callsign: String::new(),
            aprs_ssid: 0,
            aprs_symbol_table: '/',
            aprs_symbol_code: '>',  // walking person (primary table)
            aprs_comment: String::new(),
            aprs_tx_enabled: false,
            aprs_beacon_dest: "APRS".to_string(),
            aprs_messages: Vec::new(),
            aprs_message_counter: 0,
        }
    }
}

// ============================================================================
// Settings Manager
// ============================================================================

/// Manages application settings with GSettings persistence
pub struct SettingsManager {
    settings: gio::Settings,
    cached: AppSettings,
    _thread_safe: std::marker::PhantomData<*const ()>,
}

impl SettingsManager {
    /// Create a new settings manager, loading from GSettings
    pub fn new() -> Self {
        let settings = gio::Settings::new("org.pocketmodem.pocket-modem");
        let mut manager = SettingsManager {
            settings,
            cached: AppSettings::default(),
            _thread_safe: std::marker::PhantomData,
        };
        
        // Run migrations if needed
        manager.migrate_if_needed();
        
        // Load current values
        manager.cached = manager.load();
        
        // Initialize APRS message counter from stored messages to avoid ID collision
        manager.cached.aprs_message_counter = manager.get_max_aprs_id();
        eprintln!("[settings] APRS message counter initialized to {}", manager.cached.aprs_message_counter);
        
        manager
    }

    /// Load all settings from GSettings
    pub fn load(&self) -> AppSettings {
        // Load channels as JSON string
        let channels_json = self.settings.string("channels");
        let channels: Vec<Channel> = serde_json::from_str(&channels_json).unwrap_or_default();
        
        // Load tile source (default to Online if not set)
        let tile_source_str = self.settings.string("tile-source");
        let tile_source: TileSource = tile_source_str.to_string().parse().unwrap_or_default();
        
        // Load offline tiles path
        let offline_tiles_path = if self.settings.string("offline-tiles-path").len() > 0 {
            Some(self.settings.string("offline-tiles-path").to_string())
        } else {
            None
        };
        
        // Load APRS settings
        let aprs_callsign = self.settings.string("aprs-callsign").to_string();
        let aprs_ssid = self.settings.int("aprs-ssid") as u8;
        let aprs_symbol_table = self.settings.string("aprs-symbol-table").chars().next().unwrap_or('/');
        let aprs_symbol_code = self.settings.string("aprs-symbol-code").chars().next().unwrap_or('\'');
        let aprs_comment = self.settings.string("aprs-comment").to_string();
        let aprs_tx_enabled = self.settings.boolean("aprs-tx-enabled");
        let aprs_beacon_dest = self.settings.string("aprs-beacon-dest").to_string();

        AppSettings {
            frequency: self.settings.int("frequency") as u32,
            squelch: self.settings.int("squelch") as u8,
            tx_power_high: self.settings.boolean("tx-power-high"),
            pre_emphasis: self.settings.boolean("pre-emphasis"),
            de_emphasis: self.settings.boolean("de-emphasis"),
            high_pass_filter: self.settings.boolean("high-pass-filter"),
            low_pass_filter: self.settings.boolean("low-pass-filter"),
            mic_gain: self.settings.string("mic-gain").to_string(),
            channels,
            last_channel_index: self.settings.int("last-channel-index"),
            tile_source,
            offline_tiles_path,
            schema_version: self.settings.int("schema-version") as u32,
            // APRS TX settings
            aprs_callsign,
            aprs_ssid,
            aprs_symbol_table,
            aprs_symbol_code,
            aprs_comment,
            aprs_tx_enabled,
            aprs_beacon_dest: if aprs_beacon_dest.is_empty() { "APRS".to_string() } else { aprs_beacon_dest },
            // APRS Messages
            aprs_messages: self.load_aprs_messages_from_settings(),
            // Message counter - will be set from stored messages
            aprs_message_counter: 0,
        }
    }
    
    /// Save all settings to GSettings
    pub fn save(&self) {
        self.settings.set_int("frequency", self.cached.frequency as i32).ok();
        self.settings.set_int("squelch", self.cached.squelch as i32).ok();
        self.settings.set_boolean("tx-power-high", self.cached.tx_power_high).ok();
        self.settings.set_boolean("pre-emphasis", self.cached.pre_emphasis).ok();
        self.settings.set_boolean("de-emphasis", self.cached.de_emphasis).ok();
        self.settings.set_boolean("high-pass-filter", self.cached.high_pass_filter).ok();
        self.settings.set_boolean("low-pass-filter", self.cached.low_pass_filter).ok();
        self.settings.set_string("mic-gain", &self.cached.mic_gain).ok();
        self.settings.set_int("last-channel-index", self.cached.last_channel_index).ok();
    }

    /// Persist channel list to GSettings
    pub fn save_channels(&self) {
        let channels_json = serde_json::to_string(&self.cached.channels).unwrap_or_else(|_| "[]".to_string());
        self.settings.set_string("channels", &channels_json).ok();
        self.settings.set_int("schema-version", CURRENT_SCHEMA_VERSION as i32).ok();
    }

    /// Reset all settings to defaults
    pub fn reset_to_defaults(&mut self) {
        self.cached = AppSettings::default();
        self.save();
        self.save_channels();
        self.save_aprs_messages();
    }

    // ========================================================================
    // Individual setters (auto-save)
    // ========================================================================

    pub fn set_frequency(&mut self, freq: u32) {
        self.cached.frequency = freq;
        self.settings.set_int("frequency", freq as i32).ok();
    }

    pub fn set_squelch(&mut self, level: u8) {
        let level = level.min(8);
        self.cached.squelch = level;
        self.settings.set_int("squelch", level as i32).ok();
    }

    pub fn set_tx_power_high(&mut self, high: bool) {
        self.cached.tx_power_high = high;
        self.settings.set_boolean("tx-power-high", high).ok();
    }

    pub fn set_pre_emphasis(&mut self, enabled: bool) {
        self.cached.pre_emphasis = enabled;
        self.settings.set_boolean("pre-emphasis", enabled).ok();
    }

    pub fn set_de_emphasis(&mut self, enabled: bool) {
        self.cached.de_emphasis = enabled;
        self.settings.set_boolean("de-emphasis", enabled).ok();
    }

    pub fn set_high_pass_filter(&mut self, enabled: bool) {
        self.cached.high_pass_filter = enabled;
        self.settings.set_boolean("high-pass-filter", enabled).ok();
    }

    pub fn set_low_pass_filter(&mut self, enabled: bool) {
        self.cached.low_pass_filter = enabled;
        self.settings.set_boolean("low-pass-filter", enabled).ok();
    }

    pub fn set_mic_gain(&mut self, level: &str) {
        self.cached.mic_gain = level.to_string();
        self.settings.set_string("mic-gain", level).ok();
    }

    pub fn set_last_channel_index(&mut self, index: i32) {
        self.cached.last_channel_index = index;
        self.settings.set_int("last-channel-index", index).ok();
    }
    
    pub fn set_tile_source(&mut self, source: TileSource) {
        self.cached.tile_source = source;
        let source_str = match source {
            TileSource::Online => "online",
            TileSource::Offline => "offline",
        };
        self.settings.set_string("tile-source", source_str).ok();
    }
    
    pub fn set_offline_tiles_path(&mut self, path: Option<&str>) {
        self.cached.offline_tiles_path = path.map(String::from);
        if let Some(p) = path {
            self.settings.set_string("offline-tiles-path", p).ok();
        } else {
            self.settings.set_string("offline-tiles-path", "").ok();
        }
    }
    
    // ========================================================================
    // APRS TX Settings
    // ========================================================================
    
    pub fn set_aprs_callsign(&mut self, callsign: &str) {
        self.cached.aprs_callsign = callsign.to_uppercase();
        self.settings.set_string("aprs-callsign", &self.cached.aprs_callsign).ok();
    }
    
    pub fn set_aprs_ssid(&mut self, ssid: u8) {
        self.cached.aprs_ssid = ssid;
        self.settings.set_int("aprs-ssid", ssid as i32).ok();
    }
    
    pub fn set_aprs_symbol_table(&mut self, table: char) {
        self.cached.aprs_symbol_table = table;
        self.settings.set_string("aprs-symbol-table", &table.to_string()).ok();
    }
    
    pub fn set_aprs_symbol_code(&mut self, code: char) {
        self.cached.aprs_symbol_code = code;
        self.settings.set_string("aprs-symbol-code", &code.to_string()).ok();
    }
    
    pub fn set_aprs_comment(&mut self, comment: &str) {
        self.cached.aprs_comment = comment.to_string();
        self.settings.set_string("aprs-comment", &self.cached.aprs_comment).ok();
    }
    
    pub fn set_aprs_tx_enabled(&mut self, enabled: bool) {
        self.cached.aprs_tx_enabled = enabled;
        self.settings.set_boolean("aprs-tx-enabled", enabled).ok();
    }
    
    pub fn set_aprs_beacon_dest(&mut self, dest: &str) {
        self.cached.aprs_beacon_dest = dest.to_uppercase();
        self.settings.set_string("aprs-beacon-dest", &self.cached.aprs_beacon_dest).ok();
    }
    
    /// Get the full APRS callsign with SSID (e.g., "KD4LCD-9")
    pub fn aprs_full_callsign(&self) -> String {
        if self.cached.aprs_ssid > 0 {
            format!("{}-{}", self.cached.aprs_callsign, self.cached.aprs_ssid)
        } else {
            self.cached.aprs_callsign.clone()
        }
    }
    
    // ========================================================================
    // APRS Messages Storage
    // ========================================================================
    
    /// Add a new APRS direct message
    pub fn add_aprs_message(&mut self, mut msg: DirectMessage) {
        // Set the from_callsign to our callsign if not already set
        if msg.from_callsign.is_empty() {
            msg.from_callsign = self.aprs_full_callsign();
        }
        self.cached.aprs_messages.push(msg);
        self.save_aprs_messages();
    }
    
    /// Update the status of an APRS message by ID
    pub fn update_aprs_message_status(&mut self, msg_id: &str, status: DirectMessageStatus) -> bool {
        if let Some(msg) = self.cached.aprs_messages.iter_mut().find(|m| m.id == msg_id) {
            msg.status = status;
            self.save_aprs_messages();
            true
        } else {
            false
        }
    }
    
    /// Get all APRS messages
    pub fn aprs_messages(&self) -> &[DirectMessage] {
        &self.cached.aprs_messages
    }
    
    /// Get messages for a specific thread (recipient)
    pub fn aprs_messages_for_thread(&self, thread_id: &str) -> Vec<&DirectMessage> {
        // Match messages where the other party is the given thread_id
        // Either they messaged us (from_callsign == thread_id) or we messaged them (to_callsign == thread_id)
        self.cached.aprs_messages.iter()
            .filter(|m| m.from_callsign == thread_id || m.to_callsign == thread_id)
            .collect()
    }
    
    /// Build message threads from stored messages
    pub fn aprs_threads(&self) -> Vec<MessageThread> {
        MessageThread::build_threads(&self.cached.aprs_messages)
    }
    
    /// Get a message by ID
    pub fn get_aprs_message(&self, msg_id: &str) -> Option<&DirectMessage> {
        self.cached.aprs_messages.iter().find(|m| m.id == msg_id)
    }
    
    /// Get a mutable message by ID
    pub fn get_aprs_message_mut(&mut self, msg_id: &str) -> Option<&mut DirectMessage> {
        self.cached.aprs_messages.iter_mut().find(|m| m.id == msg_id)
    }
    
    /// Increment retry count for a message
    pub fn increment_aprs_message_retries(&mut self, msg_id: &str) -> bool {
        if let Some(msg) = self.cached.aprs_messages.iter_mut().find(|m| m.id == msg_id) {
            msg.retries += 1;
            self.save_aprs_messages();
            true
        } else {
            false
        }
    }
    
    pub fn update_message_last_retry(&mut self, msg_id: &str, timestamp: u64) -> bool {
        if let Some(msg) = self.cached.aprs_messages.iter_mut().find(|m| m.id == msg_id) {
            msg.last_retry_timestamp = timestamp;
            self.save_aprs_messages();
            true
        } else {
            false
        }
    }
    
    /// Mark ACK as sent for a received message
    pub fn mark_message_ack_sent(&mut self, msg_id: &str) -> bool {
        if let Some(msg) = self.cached.aprs_messages.iter_mut().find(|m| m.id == msg_id) {
            msg.mark_ack_sent();
            self.save_aprs_messages();
            true
        } else {
            false
        }
    }
    
    /// Generate a new unique message ID using UUID v4
    /// Generate a sequential numeric message ID for outgoing messages
    /// Format: 001, 002, 003, etc. (zero-padded to 3 digits, wraps at 999)
    pub fn generate_aprs_message_id(&mut self) -> String {
        self.cached.aprs_message_counter = (self.cached.aprs_message_counter % 999) + 1;
        format!("{:03}", self.cached.aprs_message_counter)
    }
    
    /// Generate a unique UUID for internal message tracking
    pub fn generate_message_uuid(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }
    
    /// Calculate storage size of APRS messages in bytes
    pub fn aprs_messages_storage_size(&self) -> usize {
        serde_json::to_string(&self.cached.aprs_messages)
            .map(|s| s.len())
            .unwrap_or(0)
    }
    
    /// Get the maximum APRS message ID from stored messages
    /// Used to initialize counter to avoid ID collision after restart
    /// Load APRS messages from GSettings
    fn load_aprs_messages_from_settings(&self) -> Vec<DirectMessage> {
        let messages_json = self.settings.string("aprs-messages");
        serde_json::from_str(&messages_json).unwrap_or_default()
    }
    
    /// Get the maximum APRS message ID from stored messages
    /// Used to initialize counter to avoid ID collision after restart
    fn get_max_aprs_id(&self) -> u32 {
        let messages = self.load_aprs_messages_from_settings();
        let mut max_id = 0u32;
        for msg in messages {
            // Parse APRS ID (e.g., "001" -> 1)
            if let Ok(id) = msg.aprs_id.parse::<u32>() {
                if id > max_id {
                    max_id = id;
                }
            }
        }
        max_id
    }
    
    /// Format storage size for display
    pub fn aprs_messages_storage_size_display(&self) -> String {
        let bytes = self.aprs_messages_storage_size();
        if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        }
    }
    
    /// Save APRS messages to GSettings
    pub fn save_aprs_messages(&self) {
        let messages_json = serde_json::to_string(&self.cached.aprs_messages)
            .unwrap_or_else(|_| "[]".to_string());
        self.settings.set_string("aprs-messages", &messages_json).ok();
    }
    
    /// Reload APRS messages from GSettings
    /// Use this to ensure cached data is fresh after modifications
    pub fn reload_aprs_messages(&mut self) {
        self.cached.aprs_messages = self.load_aprs_messages_from_settings();
    }
    
    /// Initialize APRS settings with defaults (for first-time setup)
    pub fn initialize_aprs_settings(&mut self) {
        // Set sensible defaults for APRS if not already set
        if self.cached.aprs_callsign.is_empty() {
            // Leave empty - user needs to configure their callsign
            eprintln!("[settings] APRS callsign not set, user should configure in settings");
        }
    }
    
    /// Clear all APRS messages
    pub fn clear_aprs_messages(&mut self) {
        self.cached.aprs_messages.clear();
        self.settings.set_string("aprs-messages", "[]").ok();
        eprintln!("[settings] APRS messages cleared");
    }
    
    // ========================================================================
    // Getters
    // ========================================================================

    pub fn frequency(&self) -> u32 { self.cached.frequency }
    pub fn squelch(&self) -> u8 { self.cached.squelch }
    pub fn tx_power_high(&self) -> bool { self.cached.tx_power_high }
    pub fn pre_emphasis(&self) -> bool { self.cached.pre_emphasis }
    pub fn de_emphasis(&self) -> bool { self.cached.de_emphasis }
    pub fn high_pass_filter(&self) -> bool { self.cached.high_pass_filter }
    pub fn low_pass_filter(&self) -> bool { self.cached.low_pass_filter }
    pub fn mic_gain(&self) -> &str { &self.cached.mic_gain }
    pub fn last_channel_index(&self) -> i32 { self.cached.last_channel_index }
    pub fn schema_version(&self) -> u32 { self.cached.schema_version }
    pub fn tile_source(&self) -> TileSource { self.cached.tile_source }
    pub fn offline_tiles_path(&self) -> Option<&str> { self.cached.offline_tiles_path.as_deref() }
    
    // APRS TX getters
    pub fn aprs_callsign(&self) -> &str { &self.cached.aprs_callsign }
    pub fn aprs_ssid(&self) -> u8 { self.cached.aprs_ssid }
    pub fn aprs_symbol_table(&self) -> char { self.cached.aprs_symbol_table }
    pub fn aprs_symbol_code(&self) -> char { self.cached.aprs_symbol_code }
    pub fn aprs_comment(&self) -> &str { &self.cached.aprs_comment }
    pub fn aprs_tx_enabled(&self) -> bool { self.cached.aprs_tx_enabled }
    pub fn aprs_beacon_dest(&self) -> &str { &self.cached.aprs_beacon_dest }

    pub fn channels(&self) -> &[Channel] {
        &self.cached.channels
    }

    pub fn channels_mut(&mut self) -> &mut Vec<Channel> {
        &mut self.cached.channels
    }

    pub fn get_channel(&self, index: usize) -> Option<&Channel> {
        self.cached.channels.get(index)
    }

    pub fn get_channel_by_location(&self, location: u16) -> Option<&Channel> {
        self.cached.channels.iter().find(|c| c.location == location)
    }

    // ========================================================================
    // Channel mutations
    // ========================================================================

    pub fn add_channel(&mut self, channel: Channel) {
        self.cached.channels.push(channel);
        self.save_channels();
    }

    pub fn update_channel(&mut self, index: usize, channel: Channel) {
        if index < self.cached.channels.len() {
            self.cached.channels[index] = channel;
            self.save_channels();
        }
    }

    pub fn delete_channel(&mut self, index: usize) {
        if index < self.cached.channels.len() {
            self.cached.channels.remove(index);
            self.save_channels();
        }
    }

    pub fn clear_channels(&mut self) {
        self.cached.channels.clear();
        self.save_channels();
    }

    pub fn set_channels(&mut self, channels: Vec<Channel>) {
        self.cached.channels = channels;
        self.save_channels();
    }

    // ========================================================================
    // Migrations
    // ========================================================================

    fn migrate_if_needed(&mut self) {
        let stored_version = self.settings.int("schema-version") as u32;
        if stored_version < CURRENT_SCHEMA_VERSION {
            eprintln!("[settings] Migrating from schema v{} to v{}", 
                      stored_version, CURRENT_SCHEMA_VERSION);
            self.settings.set_int("schema-version", CURRENT_SCHEMA_VERSION as i32).ok();
        }
    }

    // ========================================================================
    // CHIRP CSV Import/Export
    // ========================================================================

    /// Standard CHIRP CSV columns we support (plus PocketModem extensions)
    const CHIRP_COLUMNS: &'static str = 
        "Location,Name,Frequency,Duplex,Offset,Tone,rToneFreq,cToneFreq,Mode,Power,Comment";
    
    /// PocketModem extended columns (appended after standard columns)
    const PM_EXT_COLUMNS: &'static str = "Squelch";

    /// Export all channels to CHIRP CSV format
    pub fn export_csv(&self, path: &Path) -> std::io::Result<()> {
        let mut csv = String::new();
        
        // Header comment
        csv.push_str("# PocketModem channels - CHIRP CSV format\n");
        csv.push_str("# https://github.com/nizzo/pocket-modem\n");
        csv.push_str(&format!("# Exported: {}\n", chrono_lite_date()));
        csv.push_str("#\n");
        csv.push_str("# Columns: ");
        csv.push_str(Self::CHIRP_COLUMNS);
        csv.push_str("\n");
        csv.push('\n');
        
        // Header row
        csv.push_str(Self::CHIRP_COLUMNS);
        csv.push_str("\n");

        // Channel rows
        for ch in &self.cached.channels {
            let freq = format_freq_khz(ch.rx_freq_khz);
            let offset = format_freq_khz(ch.offset_khz);

            // Format power for CHIRP (watt value)
            let power_str = match ch.power {
                PowerLevel::High => "1W",
                PowerLevel::Low => "0.5W",
            };

            // Handle tone mode
            let (tone_mode, rtone_str, ctone_str) = match ch.tone_mode {
                ToneMode::None => ("".to_string(), String::new(), String::new()),
                ToneMode::Tone => ("Tone".to_string(), format!("{:.1}", ch.rtone_hz), String::new()),
                ToneMode::Tsql => ("TSQL".to_string(), 
                    format!("{:.1}", ch.rtone_hz),
                    format!("{:.1}", ch.ctone_hz)),
            };

            csv.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{},{}\n",
                ch.location,
                escape_csv(&ch.name),
                freq,
                ch.duplex,
                offset,
                tone_mode,
                rtone_str,
                ctone_str,
                ch.mode,
                power_str,
                escape_csv(&ch.comment),
                ch.squelch,
            ));
        }

        fs::write(path, csv)
    }

    /// Import channels from CHIRP CSV file
    pub fn import_csv(&mut self, path: &Path) -> std::io::Result<Vec<Channel>> {
        let content = fs::read_to_string(path)?;
        self.import_csv_string(&content)
    }

    /// Import channels from CSV string content
    pub fn import_csv_string(&mut self, content: &str) -> std::io::Result<Vec<Channel>> {
        let mut channels = Vec::new();
        let mut found_header = false;
        let mut line_num = 0;

        for line in content.lines() {
            line_num += 1;
            let line = line.trim();
            
            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Parse header line (first non-comment line)
            if !found_header {
                // Validate it's a CHIRP header
                if !line.contains("Location") || !line.contains("Frequency") {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("Line {}: Not a valid CHIRP CSV file", line_num),
                    ));
                }
                found_header = true;
                continue;
            }

            // Parse data line
            match parse_csv_line(line) {
                Ok(ch) => {
                    channels.push(ch);
                }
                Err(e) => {
                    eprintln!("[settings] Warning: Line {}: {}", line_num, e);
                }
            }
        }

        if channels.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "No valid channels found in CSV",
            ));
        }

        // Update stored channels
        self.cached.channels = channels.clone();
        self.save_channels();

        Ok(channels)
    }
}

// ============================================================================
// Thread safety
unsafe impl Send for SettingsManager {}
unsafe impl Sync for SettingsManager {}

// Helper Functions
// ============================================================================

fn escape_csv(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn format_freq_khz(khz: u32) -> String {
    let mhz = khz / 1000;
    let khz_part = khz % 1000;
    format!("{}.{:03}", mhz, khz_part)
}

fn parse_freq_mhz(s: &str) -> Result<u32, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(0);
    }
    
    // Handle "MHz" suffix
    let s = s.trim_end_matches("MHz").trim();
    
    if let Ok(val) = s.parse::<f64>() {
        return Ok((val * 1000.0) as u32);
    }
    
    Err(format!("Invalid frequency: {}", s))
}

fn parse_csv_line(line: &str) -> Result<Channel, String> {
    let parts: Vec<&str> = split_csv_line(line);
    
    if parts.len() < 4 {
        return Err("Not enough columns".to_string());
    }

    let location: u16 = parts[0].trim().parse()
        .map_err(|_| format!("Invalid location: {}", parts[0]))?;
    
    let name = parts[1].trim().to_string();
    
    let rx_freq_khz = parse_freq_mhz(parts[2])
        .map_err(|_| format!("Invalid frequency: {}", parts[2]))?;
    
    let duplex: Duplex = parts[3].trim().parse()
        .unwrap_or(Duplex::Simplex);
    
    let offset_khz = if parts.len() > 4 && !parts[4].trim().is_empty() {
        parse_freq_mhz(parts[4]).unwrap_or(0)
    } else {
        0
    };

    let rtone_hz = if parts.len() > 6 && !parts[6].trim().is_empty() {
        parts[6].trim().parse().unwrap_or(88.5)
    } else {
        88.5
    };

    let ctone_hz = if parts.len() > 7 && !parts[7].trim().is_empty() {
        parts[7].trim().parse().unwrap_or(rtone_hz)
    } else {
        rtone_hz
    };

    let (tone_mode, rtone, ctone) = if parts.len() > 5 && !parts[5].trim().is_empty() {
        let mode_str = parts[5].trim().to_lowercase();
        let mode = match mode_str.as_str() {
            "tone" | "ctcss" => ToneMode::Tone,
            "tsql" | "tsq" => ToneMode::Tsql,
            _ => ToneMode::None,
        };
        (mode, rtone_hz, ctone_hz)
    } else {
        (ToneMode::None, 88.5, 88.5)
    };

    let mode = if parts.len() > 8 && !parts[8].trim().is_empty() {
        parts[8].trim().to_string()
    } else {
        "FM".to_string()
    };

    let power: PowerLevel = if parts.len() > 9 && !parts[9].trim().is_empty() {
        parts[9].trim().parse().unwrap_or(PowerLevel::High)
    } else {
        PowerLevel::High
    };

    let comment = if parts.len() > 10 {
        parts[10].trim().to_string()
    } else {
        String::new()
    };
    
    // Squelch (column 11, PocketModem extension)
    let squelch = if parts.len() > 11 && !parts[11].trim().is_empty() {
        parts[11].trim().parse::<u8>().unwrap_or(defaults::SQUELCH).min(8)
    } else {
        defaults::SQUELCH
    };

    Ok(Channel {
        location,
        name,
        rx_freq_khz,
        duplex,
        offset_khz,
        tx_freq_khz: None,
        tone_mode,
        rtone_hz: rtone,
        ctone_hz: ctone,
        mode,
        power,
        squelch,
        comment,
    })
}

fn split_csv_line(line: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut in_quotes = false;
    let mut field_start = 0;
    
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                result.push(&line[field_start..i]);
                field_start = i + 1;
            }
            _ => {}
        }
    }
    result.push(&line[field_start..]);
    result
}

fn chrono_lite_date() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let days = secs / 86400;
    // Simple date: days since epoch (rough approximation)
    format!("{}", days)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_duplex_parsing() {
        assert_eq!("".parse::<Duplex>().unwrap(), Duplex::Simplex);
        assert_eq!("+".parse::<Duplex>().unwrap(), Duplex::Plus);
        assert_eq!("-".parse::<Duplex>().unwrap(), Duplex::Minus);
        assert_eq!("split".parse::<Duplex>().unwrap(), Duplex::Split);
    }

    #[test]
    fn test_tone_mode_parsing() {
        assert_eq!("".parse::<ToneMode>().unwrap(), ToneMode::None);
        assert_eq!("Tone".parse::<ToneMode>().unwrap(), ToneMode::Tone);
        assert_eq!("TSQL".parse::<ToneMode>().unwrap(), ToneMode::Tsql);
    }

    #[test]
    fn test_power_parsing() {
        assert_eq!("High".parse::<PowerLevel>().unwrap(), PowerLevel::High);
        assert_eq!("Low".parse::<PowerLevel>().unwrap(), PowerLevel::Low);
        assert_eq!("1W".parse::<PowerLevel>().unwrap(), PowerLevel::High);
        assert_eq!("0.5W".parse::<PowerLevel>().unwrap(), PowerLevel::Low);
    }

    #[test]
    fn test_freq_parsing() {
        assert_eq!(parse_freq_mhz("144.200").unwrap(), 144200);
        assert_eq!(parse_freq_mhz("144.200 MHz").unwrap(), 144200);
        assert_eq!(parse_freq_mhz("430.000").unwrap(), 430000);
    }

    #[test]
    fn test_csv_split() {
        let line = r#"1,"Test, Channel",144.200,+,,88.5,,FM,High,"Comment, with, commas""#;
        let parts = split_csv_line(line);
        assert_eq!(parts.len(), 11);
        assert_eq!(parts[1], "Test, Channel");
        assert_eq!(parts[10], "Comment, with, commas");
    }

    #[test]
    fn test_format_freq() {
        assert_eq!(format_freq_khz(144200), "144.200");
        assert_eq!(format_freq_khz(430000), "430.000");
        assert_eq!(format_freq_khz(145000), "145.000");
    }
}