//! CSS styles for PocketModem UI

/// Application CSS styles
/// 
/// These styles define the visual appearance of PocketModem:
/// - Dark theme with amber (#FFB000) accent color
/// - Status indicators with LED-like appearance
/// - Frequency display with LED segment aesthetic
/// - PTT button with active state highlighting
pub const APP_CSS: &str = r#"
    /* Frequency display - LED segment style */
    .freq-display {
        font-size: 48px;
        font-family: monospace;
        font-weight: bold;
        color: #FFB000;
        background: #1E1E1E;
        border: 3px solid #444;
        border-radius: 12px;
        text-shadow: 0 0 8px rgba(255, 176, 0, 0.5);
        box-shadow: inset 0 0 16px rgba(0, 0, 0, 0.8);
        caret-color: #FFB000;
        padding: 8px;
    }
    .freq-display:focus {
        border-color: #FFB000;
    }
    .tx-frequency {
        color: #ff4444;
        border-color: #ff4444;
    }
    
    /* Status icons */
    .status-icon-green {
        font-size: 14px;
        color: #33D17A;
    }
    .status-icon-red {
        font-size: 14px;
        color: #ff4444;
    }
    .status-icon-gray {
        font-size: 14px;
        color: #666;
    }
    
    /* GPS LED states */
    .gps-led-on {
        color: #33D17A;
        font-size: 14px;
    }
    .gps-led-off {
        color: #666;
        font-size: 14px;
    }
    .gps-led-searching {
        color: #FFB000;
        font-size: 14px;
    }
    
    /* Status labels */
    .modem-label, .gps-label, .audio-label {
        color: #666;
        font-size: 11px;
    }
    
    /* Status buttons */
    .status-btn {
        background: transparent;
        border: none;
        padding: 4px 8px;
        border-radius: 8px;
    }
    .status-btn:hover {
        background: #333;
    }
    
    /* Status text values */
    .status-text {
        font-size: 14px;
        color: #888;
    }
    .status-success {
        color: #33D17A;
    }
    .status-warning {
        color: #FFB000;
    }
    .status-error {
        color: #ff4444;
    }
    
    /* Title labels */
    .title-label {
        font-size: 18px;
        font-weight: bold;
        color: #FFB000;
        padding: 12px 0 0 12px;
    }
    
    /* Signal meter */
    .signal-text {
        font-size: 11px;
        color: #888;
        font-weight: bold;
    }
    .signal-value {
        font-size: 11px;
        font-weight: bold;
    }
    .rssi-bar {
        background: #2a2a2a;
        border: 1px solid #444;
        border-radius: 4px;
    }
    .rssi-bar.empty {
        opacity: 0.3;
    }
    .rssi-bar.bar-tx progress {
        background: #FF4444;
    }
    .rssi-bar.bar-rx progress {
        background: #FFB000;
    }
    
    /* PTT button */
    .ptt-button {
        min-width: 90px;
        min-height: 100px;
        border-radius: 12px;
        background: #333;
        border: 2px solid #555;
    }
    .ptt-button:active {
        background: #444;
        border-color: #FFB000;
    }
    .ptt-icon {
        color: #888;
    }
    .ptt-button:active .ptt-icon {
        color: #FFB000;
    }
    .ptt-label {
        font-size: 11px;
        font-weight: bold;
        color: #888;
    }
    .ptt-button:active .ptt-label {
        color: #FFB000;
    }
    
    /* Channel list */
    .channel-section-title {
        font-size: 14px;
        color: #888;
    }
    .channel-list {
        background: #2a2a2a;
        border-radius: 8px;
        border: 1px solid #444;
    }
    .channel-row {
        background: transparent;
    }
    .channel-row:hover {
        background: #333;
    }
    .channel-row-selected {
        background: #3a3a3a;
    }
    
    /* APRS list */
    .aprs-list {
        background: #2a2a2a;
        border-radius: 8px;
        border: 1px solid #444;
    }
    .aprs-message-row {
        background: transparent;
        border-bottom: 1px solid #333;
        padding: 8px;
    }
    .aprs-message-row:hover {
        background: #333;
    }
    .aprs-callsign {
        font-size: 14px;
        font-weight: bold;
    }
    .aprs-to-callsign {
        font-size: 14px;
        color: #888;
    }
    .aprs-timestamp {
        font-size: 12px;
        color: #666;
    }
    .aprs-comment {
        font-size: 13px;
        color: #888;
        font-style: italic;
    }
    .aprs-message-body {
        font-size: 14px;
        color: #aaa;
    }
    .aprs-empty-text {
        font-size: 14px;
        color: #666;
        padding: 32px;
    }
    
    /* Map */
    .map-recenter-btn {
        background: rgba(30, 30, 30, 0.8);
        border-radius: 20px;
        min-width: 40px;
        min-height: 40px;
    }
    .map-recenter-btn:hover {
        background: rgba(50, 50, 50, 0.9);
    }
    .map-recenter-btn:active {
        background: rgba(70, 70, 70, 0.9);
    }
    
    /* Locator/coordinates display */
    .locator-display {
        font-size: 18px;
        font-family: monospace;
        min-width: 1px;
    }
    .coords-display {
        font-size: 14px;
        color: #888;
        font-family: monospace;
        min-width: 1px;
    }
"#;