//! Map module using libshumate with VectorRenderer for GNOME tileserver
//!
//! Based on GNOME Maps implementation pattern:
//! - VectorRenderer for vector tile rendering
//! - TileDownloader for fetching tiles from gnome tileserver
//! - MapLayer for display
//! - MarkerLayer for APRS station markers

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gtk::prelude::*;
use libshumate::{
    prelude::LocationExt,
    Map, MapLayer, Marker, MarkerLayer,
    TileDownloader, VectorRenderer, Viewport,
};

use crate::aprs::APRSMessage;

// ============================================================================
// Constants
// ============================================================================

/// GNOME tileserver URL for vector tiles (from GNOME Maps)
const GNOME_TILE_URL: &str = "https://tileserver.gnome.org/data/v3/{z}/{x}/{y}.pbf";

/// Default map zoom level
const DEFAULT_ZOOM: f64 = 10.0;

/// Default map center (Switzerland)
const DEFAULT_LAT: f64 = 46.8;
const DEFAULT_LON: f64 = 8.2;

// ============================================================================
// Map Style (simplified Mapbox GL JSON style)
// ============================================================================

/// Generate a Mapbox GL JSON style for the vector tiles
/// Based on GNOME Maps style generator (simplified for PocketModem)
fn generate_map_style(is_dark: bool) -> String {
    let bg_color = if is_dark { "#1a1a1a" } else { "#f8f9fa" };
    let land_color = if is_dark { "#2d2d2d" } else { "#e8e0d8" };
    let water_color = if is_dark { "#2a4a6a" } else { "#a8d4e6" };
    let road_color = if is_dark { "#4a4a4a" } else { "#ffffff" };
    let text_color = if is_dark { "#e0e0e0" } else { "#333333" };

    format!(r#"{{
  "version": 8,
  "name": "PocketModem {}",
  "sources": {{
    "vector-tiles": {{
      "type": "vector",
      "tiles": ["{}"],
      "minzoom": 0,
      "maxzoom": 14
    }}
  }},
  "layers": [
    {{
      "id": "background",
      "type": "background",
      "paint": {{"background-color": "{}"}}
    }},
    {{
      "id": "land",
      "type": "fill",
      "source": "vector-tiles",
      "source-layer": "land",
      "paint": {{"fill-color": "{}"}}
    }},
    {{
      "id": "water",
      "type": "fill",
      "source": "vector-tiles",
      "source-layer": "water",
      "paint": {{"fill-color": "{}"}}
    }},
    {{
      "id": "roads",
      "type": "line",
      "source": "vector-tiles",
      "source-layer": "transportation",
      "filter": ["==", "$type", "LineString"],
      "paint": {{
        "line-color": "{}",
        "line-width": 2,
        "line-opacity": 0.8
      }}
    }},
    {{
      "id": "places",
      "type": "symbol",
      "source": "vector-tiles",
      "source-layer": "place",
      "layout": {{
        "text-field": "{{name:latin}}",
        "text-size": 12,
        "text-color": "{}"
      }}
    }}
  ]
}}"#, if is_dark { "Dark" } else { "Light" }, GNOME_TILE_URL, bg_color, land_color, water_color, road_color, text_color)
}

// ============================================================================
// Map Station (APRS marker data)
// ============================================================================

/// APRS station data for map display
#[derive(Debug, Clone)]
pub struct MapStation {
    pub callsign: String,
    pub lat: f64,
    pub lon: f64,
    pub symbol: String,
    pub comment: String,
    pub timestamp: u32,
}

impl Default for MapStation {
    fn default() -> Self {
        Self {
            callsign: String::new(),
            lat: 0.0,
            lon: 0.0,
            symbol: String::new(),
            comment: String::new(),
            timestamp: 0,
        }
    }
}

// ============================================================================
// Map Manager
// ============================================================================

/// Manages the libshumate map view, markers, and tile sources
pub struct MapManager {
    /// The libshumate Map widget
    map: Map,
    /// Viewport for controlling view
    viewport: Viewport,
    /// Layer for the map tiles
    map_layer: Option<MapLayer>,
    /// Layer for markers
    marker_layer: MarkerLayer,
    /// Cache of station markers
    station_markers: HashMap<String, Marker>,
    /// Current user position
    user_lat: Option<f64>,
    user_lon: Option<f64>,
    /// Vector renderer (to keep alive)
    vector_renderer: Option<VectorRenderer>,
}

impl MapManager {
    /// Create a new map manager
    pub fn new() -> Self {
        // Create the Map widget
        let map = Map::new();
        
        // Constrain size to fit 330px wide UI
        map.set_size_request(330, 400);
        map.set_hexpand(false);
        map.set_vexpand(false);
        
        // Get viewport
        let viewport = map.viewport().expect("Map should have a Viewport");
        
        // Set zoom limits
        viewport.set_max_zoom_level(18);
        viewport.set_min_zoom_level(2);
        
        // Set default position using builder pattern
        let vp = Viewport::builder()
            .zoom_level(DEFAULT_ZOOM)
            .latitude(DEFAULT_LAT)
            .longitude(DEFAULT_LON)
            .build();
        // Note: Viewport is already created by Map::new(), so we use the setters
        viewport.set_zoom_level(DEFAULT_ZOOM);
        
        eprintln!("[map] Created libshumate Map with VectorRenderer");
        eprintln!("[map] Tile source: {}", GNOME_TILE_URL);

        // Create marker layer
        let marker_layer = MarkerLayer::new(&viewport);

        Self {
            map,
            viewport,
            map_layer: None,
            marker_layer,
            station_markers: HashMap::new(),
            user_lat: None,
            user_lon: None,
            vector_renderer: None,
        }
    }

    /// Initialize the map with vector renderer
    pub fn initialize(&mut self) {
        // Check if already initialized
        if self.map_layer.is_some() {
            return;
        }

        // Generate map style (using light mode for now)
        let style_json = generate_map_style(false);
        
        // Create vector renderer with the style
        match VectorRenderer::new("vector-tiles", &style_json) {
            Ok(renderer) => {
                eprintln!("[map] VectorRenderer created successfully");
                
                // Create tile downloader
                let downloader = TileDownloader::new(GNOME_TILE_URL);
                eprintln!("[map] TileDownloader created");
                
                // Connect renderer to downloader
                // Note: VectorRenderer handles the data source internally via style
                
                // Create map layer to display the tiles
                let map_layer = MapLayer::new(&renderer, &self.viewport);
                
                // Add layer to map
                self.map.add_layer(&map_layer);
                
                // Add marker layer
                self.map.add_layer(&self.marker_layer);
                
                // Clone renderer before moving into vector_renderer
                let renderer_for_viewport = renderer.clone();
                
                self.map_layer = Some(map_layer);
                self.vector_renderer = Some(renderer);
                
                // Set reference map source for viewport
                self.viewport.set_reference_map_source(Some(&renderer_for_viewport));
                
                eprintln!("[map] Map initialized with vector tiles");
            }
            Err(e) => {
                eprintln!("[map] Failed to create VectorRenderer: {:?}", e);
            }
        }
    }

    /// Get the map widget for embedding in UI
    pub fn view(&self) -> &Map {
        &self.map
    }

    /// Set the user's GPS position
    pub fn set_user_position(&mut self, lat: f64, lon: f64) {
        self.user_lat = Some(lat);
        self.user_lon = Some(lon);
        
        // Pan to user position
        self.map.center_on(lat, lon);
        eprintln!("[map] Centered on user position: {:.4}, {:.4}", lat, lon);
    }

    /// Center map on a specific location
    pub fn center_on(&self, lat: f64, lon: f64, zoom: f64) {
        self.viewport.set_zoom_level(zoom);
        self.map.center_on(lat, lon);
    }

    /// Update or add an APRS station marker
    pub fn update_station(&mut self, msg: &APRSMessage) {
        // Skip if no valid position
        if msg.position_lat == 0.0 && msg.position_lon == 0.0 {
            return;
        }

        let key = msg.from_callsign.clone();

        // Remove existing marker if updating
        if let Some(old_marker) = self.station_markers.remove(&key) {
            self.marker_layer.remove_marker(&old_marker);
        }

        // Create widget for marker
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        container.add_css_class("aprs-marker");

        // Get APRS symbol
        let symbol_char = get_aprs_symbol(msg);
        let symbol_label = gtk::Label::new(Some(&symbol_char.to_string()));
        symbol_label.add_css_class("aprs-symbol");

        let call_label = gtk::Label::new(Some(&msg.from_callsign));
        call_label.add_css_class("aprs-callsign");

        container.append(&symbol_label);
        container.append(&call_label);

        // Create marker with child widget using builder
        let marker = Marker::builder()
            .latitude(msg.position_lat)
            .longitude(msg.position_lon)
            .child(&container)
            .build();

        self.marker_layer.add_marker(&marker);
        self.station_markers.insert(key, marker);
        
        eprintln!("[map] Added APRS station: {} at ({:.4}, {:.4})", 
                  msg.from_callsign, msg.position_lat, msg.position_lon);
    }

    /// Get user position
    pub fn get_user_position(&self) -> Option<(f64, f64)> {
        self.user_lat.zip(self.user_lon)
    }

    /// Check if map has any stations
    pub fn has_stations(&self) -> bool {
        !self.station_markers.is_empty()
    }
}

impl Default for MapManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Get APRS symbol character based on message type
fn get_aprs_symbol(msg: &APRSMessage) -> char {
    match msg.msg_type {
        crate::aprs::APRSType::Position => '>',
        crate::aprs::APRSType::Object => 'O',
        crate::aprs::APRSType::Weather => '@',
        crate::aprs::APRSType::Message => '*',
        _ => '?',
    }
}

// ============================================================================
// Backward Compatibility (for current main.rs)
// ============================================================================

impl MapManager {
    /// Legacy method for current main.rs compatibility
    pub fn load_aprs_symbols(&mut self, _path: &std::path::Path) -> Result<(), String> {
        // APRS symbol spritesheets not used with libshumate vector renderer
        // Symbols are rendered via the vector tile style
        Ok(())
    }

    /// Legacy method - returns map state
    pub fn get_state(&self) -> MapState {
        MapState {
            user_lat: self.user_lat,
            user_lon: self.user_lon,
            stations: self.station_markers.keys().map(|k| (k.clone(), MapStation::default())).collect(),
            needs_redraw: false,
            zoom: self.viewport.zoom_level() as u8,
            pan_x: 0.0,
            pan_y: 0.0,
        }
    }

    /// Legacy method - no-op for libshumate
    pub fn load_visible_tiles(&self, _start_x: i32, _start_y: i32, _tiles_x: i32, _tiles_y: i32, _zoom: u32) {
        // Tiles are loaded automatically by libshumate
    }

    /// Legacy method
    pub fn request_redraw(&mut self) {
        // Redraw is handled automatically by libshumate
    }

    /// Legacy method
    pub fn get_tile_cache(&self) -> Arc<Mutex<HashMap<crate::map::TileId, Arc<image::RgbaImage>>>> {
        Arc::new(Mutex::new(HashMap::new()))
    }

    /// Legacy method
    pub fn get_aprs_symbols(&self) -> Arc<Mutex<Option<image::RgbaImage>>> {
        Arc::new(Mutex::new(None))
    }

    /// Legacy method
    pub fn needs_redraw(&self) -> bool {
        false
    }

    /// Legacy method
    pub fn set_zoom(&mut self, zoom: u8) {
        self.viewport.set_zoom_level(zoom as f64);
    }

    /// Legacy method
    pub fn pan(&mut self, dx: f64, dy: f64) {
        if let Some((lat, lon)) = self.get_user_position() {
            // Simple pan by adjusting center
            let zoom = self.viewport.zoom_level();
            let scale = 1.0 / (2_f64.powf(zoom) * 256.0);
            self.map.center_on(lat + dy * scale, lon + dx * scale);
        }
    }
    
    /// Legacy method
    pub fn center_on_user(&mut self) {
        if let Some((lat, lon)) = self.get_user_position() {
            self.map.center_on(lat, lon);
        }
    }

    /// Legacy method
    pub fn get_zoom(&self) -> u8 {
        self.viewport.zoom_level() as u8
    }

    /// Legacy method
    pub fn get_stations(&self) -> Vec<MapStation> {
        Vec::new()
    }
}

// MapState for backward compatibility
#[derive(Debug, Clone)]
pub struct MapState {
    pub user_lat: Option<f64>,
    pub user_lon: Option<f64>,
    pub stations: HashMap<String, MapStation>,
    pub needs_redraw: bool,
    pub zoom: u8,
    pub pan_x: f64,
    pub pan_y: f64,
}

impl Default for MapState {
    fn default() -> Self {
        Self {
            user_lat: None,
            user_lon: None,
            stations: HashMap::new(),
            needs_redraw: false,
            zoom: DEFAULT_ZOOM as u8,
            pan_x: 0.0,
            pan_y: 0.0,
        }
    }
}

impl MapState {
    pub fn get_user_position(&self) -> Option<(f64, f64)> {
        self.user_lat.zip(self.user_lon)
    }
}

// TileId for backward compatibility
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileId {
    pub x: u32,
    pub y: u32,
    pub z: u8,
}

// Lat/lon to tile conversion for compatibility
pub fn lat_lon_to_tile(lat: f64, lon: f64, zoom: u32) -> (f64, f64) {
    let n = 2.0_f64.powi(zoom as i32);
    let x = (lon + 180.0) / 360.0 * n;
    let lat_rad = lat.to_radians();
    let y = (1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    (x, y)
}

// MapWidget for backward compatibility (just wraps the Map)
pub struct MapWidget {
    pub widget: Map,
}

impl MapWidget {
    pub fn new() -> Self {
        let map = Map::new();
        map.set_size_request(330, 400);
        Self { widget: map }
    }
}

impl Default for MapWidget {
    fn default() -> Self {
        Self::new()
    }
}