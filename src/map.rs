//! APRS Map module using GTK4 DrawingArea for tile rendering
//!
//! This replaces libshumate with a lightweight approach:
//! 1. Fetches OSM tiles directly via HTTP
//! 2. Uses GTK4 DrawingArea for 2D rendering with built-in cairo
//! 3. No Clutter or WebKit dependencies

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

use gtk::prelude::*;
use gtk::{self as gtk4, DrawingArea};
use cairo;

use crate::aprs::APRSMessage;

// ============================================================================
// Map State (shared between UI thread and tile fetching thread)
// ============================================================================

/// Thread-safe map state
pub struct MapState {
    /// User's GPS position
    pub user_lat: f64,
    pub user_lon: f64,
    /// APRS stations (call_sign -> (lat, lon))
    pub stations: HashMap<String, (f64, f64)>,
    /// Zoom level (0-18)
    pub zoom: u8,
    /// Whether we have valid position
    pub has_position: bool,
}

impl Default for MapState {
    fn default() -> Self {
        Self {
            user_lat: 45.0,
            user_lon: -75.0,
            stations: HashMap::new(),
            zoom: 10,
            has_position: false,
        }
    }
}

/// Cached tile data (raw RGBA bytes)
pub struct CachedTile {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

// ============================================================================
// Map Manager
// ============================================================================

/// Manages the map view, tiles, and APRS markers using GTK4
pub struct MapManager {
    /// The GTK DrawingArea for rendering (created lazily in view_cloned)
    drawing_area: Option<DrawingArea>,
    /// Shared map state
    state: Arc<Mutex<MapState>>,
    /// Cached tiles (tile_x, tile_y, zoom) -> CachedTile
    tile_cache: Arc<Mutex<HashMap<(i32, i32, u8), CachedTile>>>,
    /// Pending redraw requests (checked in UI timer)
    needs_redraw: Arc<std::sync::atomic::AtomicBool>,
}

impl MapManager {
    /// Create a new map manager
    pub fn new() -> Self {
        let state = Arc::new(Mutex::new(MapState::default()));
        let tile_cache = Arc::new(Mutex::new(HashMap::new()));
        let needs_redraw = Arc::new(std::sync::atomic::AtomicBool::new(false));

        eprintln!("[map] MapManager created with simple tile rendering");

        Self {
            drawing_area: None,
            state,
            tile_cache,
            needs_redraw,
        }
    }

    /// Initialize the map manager
    pub fn initialize(&mut self) {
        // Drawing area will be created lazily in view_cloned()
        // Start tile prefetching
        self.prefetch_tiles();
    }

    /// Set the user's GPS position
    pub fn set_user_position(&mut self, lat: f64, lon: f64) {
        {
            let mut state = self.state.lock().unwrap();
            state.user_lat = lat;
            state.user_lon = lon;
            state.has_position = true;
            state.zoom = 12;
        }

        if let Some(ref da) = self.drawing_area {
            da.queue_draw();
        }

        // Start tile fetching in background
        self.prefetch_tiles();
    }

    /// Center map on a location
    pub fn center_on(&mut self, lat: f64, lon: f64, zoom: f64) {
        {
            let mut state = self.state.lock().unwrap();
            state.user_lat = lat;
            state.user_lon = lon;
            state.zoom = zoom as u8;
            state.has_position = true;
        }

        if let Some(ref da) = self.drawing_area {
            da.queue_draw();
        }

        self.prefetch_tiles();
    }

    /// Center on user's GPS position
    pub fn center_on_user(&mut self) {
        {
            let mut state = self.state.lock().unwrap();
            state.zoom = 12;
        }

        if let Some(ref da) = self.drawing_area {
            da.queue_draw();
        }
    }

    /// Add or update an APRS station
    pub fn update_station(&mut self, msg: &APRSMessage) {
        if msg.position_lat == 0.0 && msg.position_lon == 0.0 {
            return;
        }

        {
            let mut state = self.state.lock().unwrap();
            state.stations.insert(
                msg.from_callsign.clone(),
                (msg.position_lat, msg.position_lon),
            );
        }

        if let Some(ref da) = self.drawing_area {
            da.queue_draw();
        }
    }

    /// Get the widget wrapped in a constrained container
    pub fn view_cloned(&mut self) -> gtk4::Widget {
        // Create DrawingArea for 2D rendering
        let drawing_area = DrawingArea::new();
        drawing_area.set_size_request(330, 600);
        drawing_area.set_hexpand(false);
        drawing_area.set_vexpand(false);
        drawing_area.set_halign(gtk::Align::Center);
        drawing_area.add_css_class("map-area");
        
        // Store in manager for queue_draw access
        self.drawing_area = Some(drawing_area.clone());
        
        // Set up drawing callback
        self.setup_drawing(&drawing_area);
        
        // Wrap in AdwClamp to limit width to 340px
        let clamp = adw::Clamp::new();
        clamp.set_size_request(330, 620);
        clamp.set_hexpand(false);
        clamp.set_vexpand(false);
        clamp.set_maximum_size(340);
        clamp.set_tightening_threshold(340);
        
        // Constrain drawing area to exact 340x600
        drawing_area.set_size_request(330, 600);
        drawing_area.set_hexpand(false);
        drawing_area.set_vexpand(false);
        
        clamp.set_child(Some(&drawing_area));
        clamp.upcast()
    }
    
    /// Check if map has any stations
    pub fn has_stations(&self) -> bool {
        !self.state.lock().unwrap().stations.is_empty()
    }
    
    /// Check if a redraw is pending from background tile loading
    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw.load(std::sync::atomic::Ordering::SeqCst)
    }
    
    /// Request a redraw on the drawing area
    pub fn request_redraw(&self) {
        if let Some(ref da) = self.drawing_area {
            da.queue_draw();
        }
    }

    /// Set up drawing callback on the given drawing area
    fn setup_drawing(&self, drawing_area: &DrawingArea) {
        let state = Arc::clone(&self.state);
        let tile_cache = Arc::clone(&self.tile_cache);
        let needs_redraw_flag = Arc::clone(&self.needs_redraw);

        drawing_area.set_draw_func(move |area, cr, width, height| {
            // Clamp width to 340 to prevent map from expanding UI
            let max_width = 340.0;
            let draw_width = (width as f64).min(max_width);
            let draw_height = height as f64;
            
            // Save context and clip to constrained area
            cr.save();
            cr.rectangle(0.0, 0.0, draw_width, draw_height);
            cr.clip();
            
            // Clear with map background
            cr.set_source_rgba(0.15, 0.17, 0.2, 1.0);
            cr.paint();

            let map_state = state.lock().unwrap();

            // Draw map tiles (use constrained dimensions)
            draw_tiles(&tile_cache, map_state.user_lat, map_state.user_lon,
                      map_state.zoom, draw_width as i32, draw_height as i32, cr);

            // Draw APRS markers
            for (callsign, (lat, lon)) in &map_state.stations {
                draw_aprs_marker(lat, lon, map_state.user_lat, map_state.user_lon,
                                map_state.zoom, draw_width as i32, draw_height as i32, cr, callsign);
            }

            // Draw user position
            if map_state.has_position {
                draw_user_position(draw_width, draw_height, cr);
            }
            
            cr.restore();
            
            if needs_redraw_flag.load(std::sync::atomic::Ordering::SeqCst) {
                needs_redraw_flag.store(false, std::sync::atomic::Ordering::SeqCst);
                area.queue_draw();
            }
        });
    }

    /// Prefetch tiles for current view in background
    fn prefetch_tiles(&self) {
        let state = Arc::clone(&self.state);
        let tile_cache = Arc::clone(&self.tile_cache);
        let needs_redraw = Arc::clone(&self.needs_redraw);

        thread::spawn(move || {
            let (center_lat, center_lon, zoom) = {
                let s = state.lock().unwrap();
                (s.user_lat, s.user_lon, s.zoom)
            };

            // Calculate tile coordinates
            let (tile_x, tile_y) = lat_lon_to_tile(center_lat, center_lon, zoom);

            // Fetch nearby tiles
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let tx = tile_x + dx;
                    let ty = tile_y + dy;

                    if tx < 0 || ty < 0 { continue; }

                    let key = (tx, ty, zoom);

                    // Check if tile is cached
                    {
                        let cache = tile_cache.lock().unwrap();
                        if cache.contains_key(&key) {
                            continue;
                        }
                    }

                    // Fetch tile from OSM
                    if let Ok(tile_data) = fetch_tile(tx, ty, zoom) {
                        // Convert PNG to RGBA
                        if let Some(cached) = create_tile_from_png(&tile_data) {
                            let mut cache = tile_cache.lock().unwrap();
                            cache.insert(key, cached);
                            
                            // Set flag for UI thread to pick up
                            needs_redraw.store(true, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                }
            }
        });
    }
}

impl Default for MapManager {
    fn default() -> Self {
        let mut m = Self::new();
        m.initialize();
        m
    }
}

// ============================================================================
// Tile Fetching
// ============================================================================

/// Fetch a tile from OpenStreetMap
fn fetch_tile(x: i32, y: i32, zoom: u8) -> Result<Vec<u8>, String> {
    let url = format!(
        "https://tile.openstreetmap.org/{}/{}/{}.png",
        zoom, x, y
    );

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .get(&url)
        .header("User-Agent", "PocketModem/1.0")
        .send()
        .map_err(|e| e.to_string())?;

    if response.status().is_success() {
        response.bytes().map(|b| b.to_vec()).map_err(|e| e.to_string())
    } else {
        Err(format!("HTTP {}", response.status()))
    }
}

/// Convert lat/lon to tile coordinates at zoom level
fn lat_lon_to_tile(lat: f64, lon: f64, zoom: u8) -> (i32, i32) {
    let lat_rad = lat * std::f64::consts::PI / 180.0;
    let n = (1 << zoom) as f64;
    let x = ((lon + 180.0) / 360.0 * n) as i32;
    let y = ((1.0 - lat_rad.tanh().ln() / std::f64::consts::PI) / 2.0 * n) as i32;
    (x, y)
}

/// Create a CachedTile from PNG data
fn create_tile_from_png(png_data: &[u8]) -> Option<CachedTile> {
    use image::GenericImageView;

    let img = image::load_from_memory(png_data).ok()?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();

    // Convert to owned Vec<u8> (BGRA format for cairo)
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let pixel = rgba.get_pixel(x, y);
            // BGRA format for cairo
            data.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }

    Some(CachedTile {
        data,
        width,
        height,
    })
}

// ============================================================================
// Drawing Functions
// ============================================================================

/// Draw map tiles centered on position
fn draw_tiles(
    tile_cache: &Arc<Mutex<HashMap<(i32, i32, u8), CachedTile>>>,
    center_lat: f64,
    center_lon: f64,
    zoom: u8,
    viewport_width: i32,
    viewport_height: i32,
    cr: &cairo::Context,
) {
    let (tile_x, tile_y) = lat_lon_to_tile(center_lat, center_lon, zoom);

    let lat_rad = center_lat * std::f64::consts::PI / 180.0;
    let n = (1 << zoom) as f64;

    let center_tile_px_x = (center_lon + 180.0) / 360.0 * n - tile_x as f64;
    let center_tile_px_y = (1.0 - lat_rad.tanh().ln() / std::f64::consts::PI) / 2.0 * n - tile_y as f64;

    let offset_x = center_tile_px_x * 256.0;
    let offset_y = center_tile_px_y * 256.0;

    let cache = tile_cache.lock().unwrap();
    let center_x = viewport_width as f64 / 2.0;
    let center_y = viewport_height as f64 / 2.0;

    for dy in -2..=2 {
        for dx in -2..=2 {
            let tx = tile_x + dx;
            let ty = tile_y + dy;

            if tx < 0 || ty < 0 { continue; }

            let key = (tx, ty, zoom);

            let tile_px_x = dx as f64 * 256.0 - offset_x + center_x;
            let tile_px_y = dy as f64 * 256.0 - offset_y + center_y;

            // Check if we have the tile
            if let Some(tile) = cache.get(&key) {
                // Draw the tile image using cairo image surface
                draw_tile_image(cr, tile_px_x, tile_px_y, &tile.data, tile.width, tile.height);
            } else {
                // Draw placeholder for missing tile
                cr.set_source_rgba(0.15, 0.17, 0.2, 0.8);
                cr.rectangle(tile_px_x, tile_px_y, 256.0, 256.0);
                cr.fill();
            }
        }
    }
}

/// Draw a tile image using cairo
fn draw_tile_image(cr: &cairo::Context, x: f64, y: f64, data: &[u8], width: u32, height: u32) {
    // Create cairo image surface from BGRA data
    let mut rgba_data = Vec::with_capacity(data.len());
    for chunk in data.chunks(4) {
        rgba_data.extend_from_slice(&[chunk[2], chunk[1], chunk[0], chunk[3]]);
    }

    let surface = cairo::ImageSurface::create_for_data(
        rgba_data,
        cairo::Format::ARgb32,
        width as i32,
        height as i32,
        width as i32 * 4,
    );

    if let Ok(surf) = surface {
        cr.set_source_surface(&surf, x, y);
        cr.paint();
    }
}

/// Draw an APRS station marker
fn draw_aprs_marker(
    lat: &f64, lon: &f64,
    center_lat: f64, center_lon: f64,
    zoom: u8,
    viewport_width: i32, viewport_height: i32,
    cr: &cairo::Context,
    _callsign: &str,
) {
    let (tile_x, tile_y) = lat_lon_to_tile(center_lat, center_lon, zoom);
    let (marker_tile_x, marker_tile_y) = lat_lon_to_tile(*lat, *lon, zoom);

    let lat_rad = center_lat * std::f64::consts::PI / 180.0;
    let marker_lat_rad = lat * std::f64::consts::PI / 180.0;
    let n = (1 << zoom) as f64;

    let center_tile_px_x = (center_lon + 180.0) / 360.0 * n - tile_x as f64;
    let center_tile_px_y = (1.0 - lat_rad.tanh().ln() / std::f64::consts::PI) / 2.0 * n - tile_y as f64;

    let marker_tile_px_x = (*lon + 180.0) / 360.0 * n - marker_tile_x as f64;
    let marker_tile_px_y = (1.0 - marker_lat_rad.tanh().ln() / std::f64::consts::PI) / 2.0 * n - marker_tile_y as f64;

    let offset_x = (marker_tile_px_x - center_tile_px_x) * 256.0;
    let offset_y = (marker_tile_px_y - center_tile_px_y) * 256.0;

    let screen_x = viewport_width as f64 / 2.0 + offset_x;
    let screen_y = viewport_height as f64 / 2.0 + offset_y;

    // Only draw if on screen
    if screen_x > -20.0 && screen_x < viewport_width as f64 + 20.0 &&
       screen_y > -20.0 && screen_y < viewport_height as f64 + 20.0 {
        // Draw orange marker
        cr.set_source_rgba(1.0, 0.69, 0.0, 1.0);
        cr.arc(screen_x, screen_y, 6.0, 0.0, 2.0 * std::f64::consts::PI);
        cr.fill();

        // Draw white border
        cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
        cr.set_line_width(2.0);
        cr.arc(screen_x, screen_y, 8.0, 0.0, 2.0 * std::f64::consts::PI);
        cr.stroke();
    }
}

/// Draw user position marker
fn draw_user_position(viewport_width: f64, viewport_height: f64, cr: &cairo::Context) {
    let x = viewport_width / 2.0;
    let y = viewport_height / 2.0;

    // Blue dot
    cr.set_source_rgba(0.2, 0.6, 1.0, 1.0);
    cr.arc(x, y, 8.0, 0.0, 2.0 * std::f64::consts::PI);
    cr.fill();

    // White border
    cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
    cr.set_line_width(2.0);
    cr.arc(x, y, 10.0, 0.0, 2.0 * std::f64::consts::PI);
    cr.stroke();
}