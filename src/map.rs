//! Map module using libshumate with VectorRenderer for GNOME tileserver
//!
//! Based on GNOME Maps implementation pattern:
//! - VectorRenderer for vector tile rendering
//! - TileDownloader for fetching tiles from gnome tileserver
//! - MapLayer for display
//! - MarkerLayer for APRS station markers

use std::collections::HashMap;

use gtk::prelude::*;
use libshumate::{
    prelude::LocationExt,
    Map, MapLayer, Marker, MarkerLayer,
    TileDownloader, VectorRenderer, Viewport,
};

use crate::aprs::APRSMessage;
use crate::aprs_icons::APRSIconRenderer;

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

/// Map margins for easier carousel swiping (in pixels)
const MAP_MARGIN: i32 = 8;

// ============================================================================
// Map Style (based on GNOME Maps defs.js)
// ============================================================================

/// GNOME Maps color palette (from defs.js)
mod gnome_colors {
    pub fn background(is_dark: bool) -> &'static str {
        if is_dark { "#191a19" } else { "#deddda" }
    }
    
    pub fn foreground(is_dark: bool) -> &'static str {
        if is_dark { "#deddda" } else { "#3d3846" }
    }
    
    pub fn water(is_dark: bool) -> &'static str {
        if is_dark { "#0f2f5e" } else { "#99c1f1" }
    }
    
    pub fn land(is_dark: bool) -> &'static str {
        if is_dark { "#242524" } else { "#e8e5de" }
    }
    
    pub fn landuse(is_dark: bool) -> &'static str {
        if is_dark { "#1e211e" } else { "#e8e5de" }
    }
    
    pub fn grass(is_dark: bool) -> &'static str {
        if is_dark { "#334034" } else { "#adccb3" }
    }
    
    pub fn wood(is_dark: bool) -> &'static str {
        if is_dark { "#29342a" } else { "#a3c2a9" }
    }
    
    pub fn road_main(is_dark: bool) -> &'static str {
        if is_dark { "#493727" } else { "#e9cf75" }
    }
    
    pub fn road_secondary(is_dark: bool) -> &'static str {
        if is_dark { "#453324" } else { "#ebd68a" }
    }
    
    pub fn road_street(is_dark: bool) -> &'static str {
        if is_dark { "#413f39" } else { "#d7d2bc" }
    }
    
    pub fn road_path(is_dark: bool) -> &'static str {
        if is_dark { "#25242a" } else { "#bebdc8" }
    }
    
    pub fn building(is_dark: bool) -> &'static str {
        if is_dark { "#3d3d3d" } else { "#c4c0b8" }
    }
    
    pub fn text_secondary(is_dark: bool) -> &'static str {
        if is_dark { "#c0bfbc" } else { "#5e5c64" }
    }
}

/// Generate a Mapbox GL JSON style for the vector tiles
/// Matching GNOME Maps style exactly
fn generate_map_style(is_dark: bool) -> String {
    let bg = gnome_colors::background(is_dark);
    let fg = gnome_colors::foreground(is_dark);
    let fg_secondary = gnome_colors::text_secondary(is_dark);
    let water = gnome_colors::water(is_dark);
    let land = gnome_colors::land(is_dark);
    let landuse = gnome_colors::landuse(is_dark);
    let grass = gnome_colors::grass(is_dark);
    let wood = gnome_colors::wood(is_dark);
    let road = gnome_colors::road_main(is_dark);
    let street = gnome_colors::road_street(is_dark);
    let secondary = gnome_colors::road_secondary(is_dark);
    let path = gnome_colors::road_path(is_dark);
    let building = gnome_colors::building(is_dark);
    let name = if is_dark { "Dark" } else { "Light" };

    // Build style matching GNOME Maps exactly
    // Using proper Mapbox Style expressions
    format!(r#"{{
  "version": 8,
  "name": "GNOME Maps {}",
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
      "id": "landuse-residential",
      "type": "fill",
      "source": "vector-tiles",
      "source-layer": "landuse",
      "filter": ["in", "class", "residential", "commercial", "industrial", "retail", "education", "hospital"],
      "paint": {{"fill-color": "{}"}}
    }},
    {{
      "id": "landuse-grass",
      "type": "fill",
      "source": "vector-tiles",
      "source-layer": "landcover",
      "filter": ["in", "class", "grass", "pitch"],
      "paint": {{"fill-color": "{}", "fill-opacity": 0.6}}
    }},
    {{
      "id": "landuse-wood",
      "type": "fill",
      "source": "vector-tiles",
      "source-layer": "landcover",
      "filter": ["in", "class", "wood", "forest"],
      "paint": {{"fill-color": "{}", "fill-opacity": 0.5}}
    }},
    {{
      "id": "water",
      "type": "fill",
      "source": "vector-tiles",
      "source-layer": "water",
      "paint": {{"fill-color": "{}"}}
    }},
    {{
      "id": "transportation-path",
      "type": "line",
      "source": "vector-tiles",
      "source-layer": "transportation",
      "filter": ["in", "class", "path", "track", "footway", "steps", "cycleway"],
      "paint": {{
        "line-color": "{}",
        "line-width": 0.5,
        "line-dasharray": [1.5, 2],
        "line-cap": "round"
      }}
    }},
    {{
      "id": "transportation-street",
      "type": "line",
      "source": "vector-tiles",
      "source-layer": "transportation",
      "filter": ["in", "class", "tertiary", "minor", "service"],
      "paint": {{
        "line-color": "{}",
        "line-width": 0.75,
        "line-opacity": 0.9,
        "line-cap": "round"
      }}
    }},
    {{
      "id": "transportation-secondary",
      "type": "line",
      "source": "vector-tiles",
      "source-layer": "transportation",
      "filter": ["==", "class", "secondary"],
      "paint": {{
        "line-color": "{}",
        "line-width": 1.25,
        "line-opacity": 0.9,
        "line-cap": "round"
      }}
    }},
    {{
      "id": "transportation-road",
      "type": "line",
      "source": "vector-tiles",
      "source-layer": "transportation",
      "filter": ["in", "class", "primary", "trunk", "motorway"],
      "paint": {{
        "line-color": "{}",
        "line-width": 1.5,
        "line-opacity": 0.9,
        "line-cap": "round"
      }}
    }},
    {{
      "id": "buildings",
      "type": "fill",
      "source": "vector-tiles",
      "source-layer": "building",
      "minzoom": 15,
      "paint": {{"fill-color": "{}"}}
    }},
    {{
      "id": "place-city",
      "type": "symbol",
      "source": "vector-tiles",
      "source-layer": "place",
      "filter": ["==", "class", "city"],
      "layout": {{
        "text-field": ["coalesce", ["get", "name:en"], ["get", "name"]],
        "text-font": ["Adwaita Sans Bold"],
        "text-size": 20,
        "text-padding": 10
      }},
      "paint": {{
        "text-color": "{}"
      }}
    }},
    {{
      "id": "place-town-village",
      "type": "symbol",
      "source": "vector-tiles",
      "source-layer": "place",
      "filter": ["in", "class", "town", "village"],
      "layout": {{
        "text-field": ["coalesce", ["get", "name:en"], ["get", "name"]],
        "text-font": ["Adwaita Sans Bold"],
        "text-size": 14,
        "text-padding": 10
      }},
      "paint": {{
        "text-color": "{}"
      }}
    }},
    {{
      "id": "poi",
      "type": "symbol",
      "source": "vector-tiles",
      "source-layer": "poi",
      "minzoom": 16,
      "layout": {{
        "text-field": ["coalesce", ["get", "name:en"], ["get", "name"]],
        "text-font": ["Adwaita Sans Italic"],
        "text-size": 11,
        "text-offset": [0, 0.7],
        "text-padding": 10
      }},
      "paint": {{
        "text-color": "{}"
      }}
    }},
    {{
      "id": "road-name",
      "type": "symbol",
      "source": "vector-tiles",
      "source-layer": "transportation_name",
      "minzoom": 14,
      "layout": {{
        "text-field": ["coalesce", ["get", "name:en"], ["get", "name"]],
        "text-font": ["Adwaita Sans"],
        "text-size": 10,
        "symbol-placement": "line",
        "text-offset": [0, 1]
      }},
      "paint": {{
        "text-color": "{}"
      }}
    }}
  ]
}}"#, name, GNOME_TILE_URL, bg, land, landuse, grass, wood, water, path, street, secondary, road, building, fg, fg, fg, fg)
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
    /// User position marker (GPS dot)
    user_marker: Option<Marker>,
    /// Vector renderer (to keep alive)
    vector_renderer: Option<VectorRenderer>,
    /// Track if viewport changes should update user marker position
    viewport_listener_id: Option<glib::SignalHandlerId>,
    /// Whether the map has been centered on user position yet
    has_centered_on_user: bool,
    /// Current dark mode state (for marker styling)
    dark_mode: bool,
    /// APRS icon renderer
    icon_renderer: APRSIconRenderer,
}

impl MapManager {
    /// Create a new map manager
    pub fn new() -> Self {
        // Create the Map widget
        let map = Map::new();
        
        // Fill available space (no fixed size)
        map.set_hexpand(false);
        map.set_vexpand(true);
        
        // Get viewport
        let viewport = map.viewport().expect("Map should have a Viewport");
        
        // Set zoom limits
        viewport.set_max_zoom_level(18);
        viewport.set_min_zoom_level(2);
        
        // Set default position
        viewport.set_zoom_level(DEFAULT_ZOOM);
        
        eprintln!("[map] Created libshumate Map with VectorRenderer");

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
            user_marker: None,
            vector_renderer: None,
            viewport_listener_id: None,
            has_centered_on_user: false,
            dark_mode: true,
            icon_renderer: APRSIconRenderer::new(),
        }
    }

    /// Initialize the map with vector renderer
    pub fn initialize(&mut self, dark_mode: bool) {
        self.dark_mode = dark_mode;
        // Check if already initialized
        if self.map_layer.is_some() {
            self.update_style_internal(dark_mode);
            return;
        }

        // Generate map style based on theme
        let style_json = generate_map_style(dark_mode);
        
        // Create vector renderer with the style
        match VectorRenderer::new("vector-tiles", &style_json) {
            Ok(renderer) => {
                eprintln!("[map] VectorRenderer created successfully");
                eprintln!("[map] Style: {}", if dark_mode { "dark" } else { "light" });
                
                // Create tile downloader
                let _downloader = TileDownloader::new(GNOME_TILE_URL);
                eprintln!("[map] TileDownloader created");
                
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
    
    /// Update map style when theme changes (public API)
    pub fn update_style(&mut self, dark_mode: bool) {
        self.update_style_internal(dark_mode);
    }
    
    /// Internal style update - recreates the renderer for full style change
    fn update_style_internal(&mut self, dark_mode: bool) {
        // Update dark mode state
        self.dark_mode = dark_mode;
        
        // Generate new style
        let style_json = generate_map_style(dark_mode);
        eprintln!("[map] Updating style to {} mode", if dark_mode { "dark" } else { "light" });
        
        // Remove old layers
        if let Some(ref layer) = self.map_layer {
            self.map.remove_layer(layer);
        }
        
        // Recreate vector renderer with new style
        match VectorRenderer::new("vector-tiles", &style_json) {
            Ok(renderer) => {
                // Create new map layer
                let map_layer = MapLayer::new(&renderer, &self.viewport);
                self.map.add_layer(&map_layer);
                
                self.map_layer = Some(map_layer);
                self.vector_renderer = Some(renderer.clone());
                
                // Set as reference map source
                self.viewport.set_reference_map_source(Some(&renderer));
                
                eprintln!("[map] Style updated successfully");
            }
            Err(e) => {
                eprintln!("[map] Failed to update style: {:?}", e);
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
        
        // Update or create user marker
        if let Some(marker) = &mut self.user_marker {
            // Update existing marker position
            marker.set_latitude(lat);
            marker.set_longitude(lon);
        } else {
            // Create new user marker - yellow circle with dynamic border
            let drawing_area = gtk::DrawingArea::new();
            drawing_area.set_size_request(16, 16);
            
            // Capture dark mode for closure
            let is_dark = self.dark_mode;
            drawing_area.set_draw_func(move |_area, cr, _width, _height| {
                // Draw yellow circle
                cr.set_source_rgb(1.0, 0.69, 0.0); // #FFB000
                cr.arc(8.0, 8.0, 7.0, 0.0, 2.0 * std::f64::consts::PI);
                cr.fill().expect("Failed to fill circle");
                
                // Draw border - white in dark mode, black in light mode
                if is_dark {
                    cr.set_source_rgb(1.0, 1.0, 1.0); // White
                } else {
                    cr.set_source_rgb(0.0, 0.0, 0.0); // Black
                }
                cr.set_line_width(1.5);
                cr.arc(8.0, 8.0, 7.0, 0.0, 2.0 * std::f64::consts::PI);
                cr.stroke().expect("Failed to stroke circle");
            });
            
            let marker = Marker::builder()
                .latitude(lat)
                .longitude(lon)
                .child(&drawing_area)
                .build();
            
            self.marker_layer.add_marker(&marker);
            self.user_marker = Some(marker);
            eprintln!("[map] Created user position marker (yellow circle, dark={})", is_dark);
        }
        
        // Only center on map on first position, then let user pan freely
        if !self.has_centered_on_user {
            self.map.center_on(lat, lon);
            self.has_centered_on_user = true;
            eprintln!("[map] Centered on initial user position: {:.4}, {:.4}", lat, lon);
        } else {
            eprintln!("[map] Updated user position (not centering): {:.4}, {:.4}", lat, lon);
        }
    }

    /// Force center the map on user's current position (for re-center button)
    pub fn center_on_user(&mut self) {
        if let (Some(lat), Some(lon)) = (self.user_lat, self.user_lon) {
            self.map.center_on(lat, lon);
            eprintln!("[map] Re-centered on user: {:.4}, {:.4}", lat, lon);
        }
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

        // Icon size + banner height (2 lines of text)
        let icon_size = 32;
        let banner_height = 28;
        let total_width = 48;  // Wider for callsign + distance
        let total_height = icon_size + banner_height;

        // Get the APRS icon surface
        let icon_surface = self.icon_renderer.get_icon(
            msg.symbol_table_id,
            msg.symbol_code,
            icon_size,
        );
        
        // Create a DrawingArea to render the icon
        let drawing_area = gtk::DrawingArea::new();
        drawing_area.set_size_request(total_width, total_height);
        drawing_area.set_hexpand(false);
        drawing_area.set_vexpand(false);

        // Take ownership of the surface for the closure
        let surface = icon_surface.clone();

        // Icon dimensions
        let icon_width = icon_size as f64;
        let icon_height = icon_size as f64;
        
        // Clone data for closures
        let comment_for_tooltip = if msg.comment.is_empty() { String::new() } else { msg.comment.clone() };
        let callsign_for_draw = msg.from_callsign.clone();
        
        // Calculate distance from user position (in km)
        let distance_text = if let (Some(user_lat), Some(user_lon)) = (self.user_lat, self.user_lon) {
            let dist = Self::haversine_distance(user_lat, user_lon, msg.position_lat, msg.position_lon);
            format!("{:.1}km", dist)
        } else {
            String::new()
        };

        let surface_for_closure = surface.clone();
        
        drawing_area.set_draw_func(move |area, cr, width, height| {
            // Draw the APRS icon
            let scale_x = width as f64 / icon_width;
            let scale_y = height as f64 / icon_height;
            let scale = scale_x.min(scale_y);
            
            let scaled_width = icon_width * scale;
            let scaled_height = icon_height * scale;
            let offset_x = (width as f64 - scaled_width) / 2.0;
            let offset_y = (height as f64 - scaled_height) / 2.0;
            
            cr.set_source_surface(&surface_for_closure, offset_x, offset_y);
            cr.paint().ok();
            
            // Draw callsign + distance banner below icon
            let w = width as f64;
            let h = height as f64;
            
            // Banner background
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.7);
            cr.rectangle(0.0, icon_height as f64, w, banner_height as f64);
            cr.fill().ok();
            
            // Text settings
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
            cr.set_font_size(8.0);
            
            // Line 1: callsign (centered)
            let callsign_x = (w - callsign_for_draw.len() as f64 * 5.0) / 2.0;
            cr.move_to(callsign_x, icon_height as f64 + 11.0);
            cr.show_text(&callsign_for_draw).ok();
            
            // Line 2: distance (centered)
            let dist_x = (w - distance_text.len() as f64 * 5.0) / 2.0;
            cr.move_to(dist_x, icon_height as f64 + 22.0);
            cr.show_text(&distance_text).ok();
        });

        // Create tooltip with station info
        let tooltip_text = format!("{} - {}", msg.from_callsign, 
            if comment_for_tooltip.is_empty() { "No comment" } else { &comment_for_tooltip });
        drawing_area.set_tooltip_text(Some(&tooltip_text));

        // Create marker with icon
        let marker = Marker::builder()
            .latitude(msg.position_lat)
            .longitude(msg.position_lon)
            .child(&drawing_area)
            .build();

        self.marker_layer.add_marker(&marker);
        
        // Schedule draw after widget is realized
        let weak_drawing_area = drawing_area.downgrade();
        drawing_area.add_tick_callback(move |area, _clock| {
            if let Some(da) = weak_drawing_area.upgrade() {
                da.queue_allocate();
                da.queue_draw();
            }
            glib::ControlFlow::Break
        });

        self.station_markers.insert(key, marker);
    }

    /// Get user position
    pub fn get_user_position(&self) -> Option<(f64, f64)> {
        self.user_lat.zip(self.user_lon)
    }

    /// Check if map has any stations
    pub fn has_stations(&self) -> bool {
        !self.station_markers.is_empty()
    }
    
    /// Clean up resources
    pub fn cleanup(&mut self) {
        // Remove user marker if present
        if let Some(marker) = self.user_marker.take() {
            self.marker_layer.remove_marker(&marker);
        }
    }
    
    /// Calculate distance between two coordinates using Haversine formula (in km)
    fn haversine_distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
        let r = 6371.0; // Earth radius in km
        let d_lat = (lat2 - lat1).to_radians();
        let d_lon = (lon2 - lon1).to_radians();
        let a = (d_lat / 2.0).sin().powi(2) 
              + lat1.to_radians().cos() * lat2.to_radians().cos() * (d_lon / 2.0).sin().powi(2);
        let c = 2.0 * a.sqrt().asin();
        r * c
    }
}

impl Default for MapManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Backward Compatibility
// ============================================================================

impl MapManager {
    /// Legacy method
    pub fn load_aprs_symbols(&mut self, _path: &std::path::Path) -> Result<(), String> {
        Ok(())
    }

    /// Legacy method
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

    /// Legacy method
    pub fn load_visible_tiles(&self, _start_x: i32, _start_y: i32, _tiles_x: i32, _tiles_y: i32, _zoom: u32) {}

    /// Legacy method
    pub fn request_redraw(&mut self) {}

    /// Legacy method
    pub fn get_tile_cache(&self) -> std::sync::Arc<std::sync::Mutex<HashMap<crate::map::TileId, std::sync::Arc<image::RgbaImage>>>> {
        std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))
    }

    /// Legacy method
    pub fn get_aprs_symbols(&self) -> std::sync::Arc<std::sync::Mutex<Option<image::RgbaImage>>> {
        std::sync::Arc::new(std::sync::Mutex::new(None))
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
            let zoom = self.viewport.zoom_level();
            let scale = 1.0 / (2_f64.powf(zoom) * 256.0);
            self.map.center_on(lat + dy * scale, lon + dx * scale);
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
        map.set_size_request(360, -1);
        Self { widget: map }
    }
}

impl Default for MapWidget {
    fn default() -> Self {
        Self::new()
    }
}