//! APRS Map module using libshumate
//!
//! Features:
//! - OSM tiles (online)
//! - APRS station markers with callsign labels  
//! - User position marker (from GPS)

use std::collections::HashMap;

use gtk::prelude::*;
use libshumate::{
    SimpleMap, Map, Viewport, Marker, MarkerLayer,
    MapSourceRegistry, MapSource, prelude::*,
};

use crate::aprs::APRSMessage;

// ============================================================================
// Map Manager
// ============================================================================

/// Manages the map view, markers, and tile sources
pub struct MapManager {
    /// SimpleMap widget with embedded Map
    simple_map: SimpleMap,
    /// The underlying Map widget
    map: Map,
    /// Viewport for controlling the view
    viewport: Viewport,
    /// Layer for markers
    marker_layer: MarkerLayer,
    /// Cache of station markers (call_sign -> marker)
    station_markers: HashMap<String, Marker>,
}

impl MapManager {
    /// Create a new map manager
    pub fn new() -> Self {
        // Create SimpleMap (it creates Map and Viewport internally)
        let simple_map = SimpleMap::new();
        
        // CRITICAL: Constrain map size to fit in 280px wide UI
        // The SimpleMap will expand to fill available space, so we must limit it
        simple_map.set_size_request(280, 350);
        simple_map.set_hexpand(false);
        simple_map.set_vexpand(false);
        
        // Get references to internal components
        let map = simple_map.map().expect("SimpleMap should have a Map");
        let viewport = simple_map.viewport().expect("SimpleMap should have a Viewport");
        
        // Get default OSM tile source from registry and set it
        let registry = MapSourceRegistry::with_defaults();
        registry.populate_defaults();  // Ensure defaults are loaded
        
        // Try to get OSM Mapnik tile source by ID
        // Note: SimpleMap already has a default, but we re-assert it here
        if let Some(source) = registry.by_id(libshumate::MAP_SOURCE_OSM_MAPNIK) {
            map.set_map_source(&source);
            eprintln!("[map] Tile source: OSM Mapnik");
        } else if let Some(source) = registry.item(0) {
            // Fallback: use first available tile source
            if let Ok(map_source) = source.downcast::<libshumate::MapSource>() {
                map.set_map_source(&map_source);
                eprintln!("[map] Tile source: first available");
            }
        } else {
            eprintln!("[map] WARNING: No tile sources available");
        }
        
        // Set initial viewport state to trigger tile loading
        viewport.set_zoom_level(4.0);
        map.go_to(45.0, -75.0);  // Ottawa as default
        eprintln!("[map] Viewport: zoom=4, lat=45, lon=-75");
        
        // Create marker layer for APRS markers
        let marker_layer = MarkerLayer::new(&viewport);
        map.add_layer(&marker_layer);
        
        eprintln!("[map] MapManager created");

        Self {
            simple_map,
            map,
            viewport,
            marker_layer,
            station_markers: HashMap::new(),
        }
    }

    /// Configure tile source
    pub fn set_tile_source(&mut self, online: bool, _mbtiles_path: Option<&str>) {
        if online {
            // Use OSM Mapnik tiles
            let registry = MapSourceRegistry::with_defaults();
            registry.populate_defaults();
            if let Some(source) = registry.by_id(libshumate::MAP_SOURCE_OSM_MAPNIK) {
                self.simple_map.set_map_source(Some(&source));
                eprintln!("[map] Tile source: OSM Mapnik (online)");
            } else {
                eprintln!("[map] Could not get OSM tile source");
            }
        } else {
            eprintln!("[map] Offline tiles: not implemented");
        }
    }

    /// Set the user's GPS position on the map
    pub fn set_user_position(&mut self, lat: f64, lon: f64) {
        // Go to the user's position with animation
        self.map.go_to(lat, lon);
    }

    /// Center map on a location
    pub fn center_on(&self, lat: f64, lon: f64, zoom: f64) {
        // Set zoom first, then go to location
        self.viewport.set_zoom_level(zoom);
        self.map.go_to(lat, lon);
    }
    
    /// Center on user's GPS position
    pub fn center_on_user(&self, lat: f64, lon: f64) {
        self.center_on(lat, lon, 12.0);
    }

    /// Add or update an APRS station marker
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

        // Create marker
        let marker = Marker::new();
        marker.set_location(msg.position_lat, msg.position_lon);

        // Create widget for marker
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        container.add_css_class("aprs-marker");

        let symbol_label = gtk::Label::new(Some(&get_aprs_symbol(msg).to_string()));
        symbol_label.add_css_class("aprs-symbol");

        let call_label = gtk::Label::new(Some(&msg.from_callsign));
        call_label.add_css_class("aprs-callsign");

        container.append(&symbol_label);
        container.append(&call_label);

        marker.set_child(Some(&container));

        self.marker_layer.add_marker(&marker);
        self.station_markers.insert(key, marker);
    }

    /// Remove stale stations
    pub fn cleanup_stale(&mut self, _max_age_secs: u64) {
        // TODO: implement with timestamp tracking
    }

    /// Get the SimpleMap widget
    pub fn view(&self) -> &SimpleMap {
        &self.simple_map
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