//! Map module using OpenStreetMap tiles with GTK4 DrawingArea for rendering
//!
//! Features:
//! - OpenStreetMap tile rendering
//! - User position marker (green)
//! - APRS stations with official APRS symbol icons from aprs-symbols submodule
//! - Background tile loading with placeholder display

use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::hash::{Hash, Hasher};
use std::fmt;
use std::path::Path;

/// Tile ID for OpenStreetMap tiles
#[derive(Clone, Copy, PartialEq, Eq, Hash)] 
pub struct TileId {
    pub x: u32,
    pub y: u32,
    pub z: u8,
}

impl TileId {
    pub fn new(x: u32, y: u32, z: u8) -> Self {
        Self { x, y, z }
    }
}

impl fmt::Debug for TileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TileId({}, {}, {})", self.x, self.y, self.z)
    }
}

impl Default for TileId {
    fn default() -> Self {
        Self { x: 0, y: 0, z: 0 }
    }
}

/// APRS symbol info
#[derive(Debug, Clone)]
pub struct APRSSymbol {
    /// Symbol table (0 = primary, 1 = secondary, 2 = overlay)
    pub table: u8,
    /// Symbol code within the table (0-89)
    pub code: u8,
}

impl APRSSymbol {
    /// Parse APRS symbol from two characters
    /// Returns None if invalid
    pub fn from_chars(table_char: char, code_char: char) -> Option<Self> {
        let table = match table_char {
            '/' => 0,  // Primary table
            '\\' => 1, // Secondary table
            _ => return None,
        };
        
        // Symbol codes are ASCII, map to 0-89
        let code = code_char as u8;
        if code < 32 || code > 126 {
            return None;
        }
        
        Some(Self { table, code })
    }
    
    /// Get row and column in the spritesheet for this symbol
    /// Each symbol is 24x24 pixels, spritesheet has 16 symbols per row
    pub fn sprite_position(&self) -> Option<(u32, u32)> {
        if self.table > 2 {
            return None;
        }
        
        // Symbol code maps to position
        // Map ASCII to 0-89 index: 'A' to 'Z' = 0-25, '0' to '9' = 26-35, 'a' to 'z' = 36-61
        let index = match self.code {
            b'A'..=b'Z' => (self.code - b'A') as u32,
            b'0'..=b'9' => 26 + (self.code - b'0') as u32,
            b'a'..=b'z' => 36 + (self.code - b'a') as u32,
            _ => return None,
        };
        
        if index >= 88 {
            return None; // Out of bounds
        }
        
        let col = index % 16;
        let row = index / 16;
        let table_row = self.table as u32;
        
        Some((table_row * 32 + row * 24, col * 24))
    }
}

/// APRS station data for map display
#[derive(Debug, Clone)]
pub struct MapStation {
    pub callsign: String,
    pub lat: f64,
    pub lon: f64,
    pub symbol: APRSSymbol,
    pub comment: String,
    pub timestamp: u32,
}

/// Map state
#[derive(Debug, Clone)]
pub struct MapState {
    /// User's current GPS position
    pub user_lat: Option<f64>,
    pub user_lon: Option<f64>,
    /// APRS stations to display
    pub stations: HashMap<String, MapStation>,
    /// Flag to request map redraw
    pub needs_redraw: bool,
    /// Zoom level (1-18)
    pub zoom: u8,
    /// Pan offset in pixels from center
    pub pan_x: f64,
    pub pan_y: f64,
}

impl Default for MapState {
    fn default() -> Self {
        Self {
            user_lat: None,
            user_lon: None,
            stations: HashMap::new(),
            needs_redraw: true,
            zoom: 12,
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

/// Map manager
pub struct MapManager {
    state: Arc<Mutex<MapState>>,
    tile_cache: Arc<Mutex<HashMap<TileId, Arc<image::RgbaImage>>>>,
    /// APRS symbol spritesheet cache
    aprs_symbols: Arc<Mutex<Option<image::RgbaImage>>>,
    pending_tiles: Arc<Mutex<Vec<TileId>>>,
}

impl MapManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MapState::default())),
            tile_cache: Arc::new(Mutex::new(HashMap::new())),
            aprs_symbols: Arc::new(Mutex::new(None)),
            pending_tiles: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn set_user_position(&mut self, lat: f64, lon: f64) {
        let mut state = self.state.lock().unwrap();
        state.user_lat = Some(lat);
        state.user_lon = Some(lon);
        state.needs_redraw = true;
    }

    /// Update or add an APRS station
    /// symbol_table and symbol_code are the APRS symbol characters
    pub fn update_station(&mut self, callsign: &str, lat: f64, lon: f64, symbol_table: char, symbol_code: char, comment: &str, timestamp: u32) {
        let mut state = self.state.lock().unwrap();
        
        let symbol = APRSSymbol::from_chars(symbol_table, symbol_code)
            .unwrap_or(APRSSymbol { table: 0, code: b'?' });
        
        let is_new = !state.stations.contains_key(callsign);
        
        state.stations.insert(callsign.to_string(), MapStation {
            callsign: callsign.to_string(),
            lat,
            lon,
            symbol,
            comment: comment.to_string(),
            timestamp,
        });
        
        if is_new {
            state.needs_redraw = true;
        }
    }

    pub fn needs_redraw(&self) -> bool {
        self.state.lock().unwrap().needs_redraw
    }

    pub fn request_redraw(&mut self) {
        let mut state = self.state.lock().unwrap();
        state.needs_redraw = false;
    }

    pub fn get_state(&self) -> MapState {
        self.state.lock().unwrap().clone()
    }
    
    pub fn get_user_position(&self) -> Option<(f64, f64)> {
        let state = self.state.lock().unwrap();
        state.user_lat.zip(state.user_lon)
    }

    pub fn get_zoom(&self) -> u8 {
        self.state.lock().unwrap().zoom
    }
    
    pub fn get_stations(&self) -> Vec<MapStation> {
        let state = self.state.lock().unwrap();
        state.stations.values().cloned().collect()
    }
    
    pub fn set_zoom(&mut self, zoom: u8) {
        let mut state = self.state.lock().unwrap();
        state.zoom = zoom.clamp(1, 18);
        state.needs_redraw = true;
    }
    
    pub fn pan(&mut self, dx: f64, dy: f64) {
        let mut state = self.state.lock().unwrap();
        state.pan_x += dx;
        state.pan_y += dy;
        state.needs_redraw = true;
    }
    
    pub fn center_on_user(&mut self) {
        let mut state = self.state.lock().unwrap();
        state.pan_x = 0.0;
        state.pan_y = 0.0;
        state.needs_redraw = true;
    }
    
    /// Get tile cache
    pub fn get_tile_cache(&self) -> Arc<Mutex<HashMap<TileId, Arc<image::RgbaImage>>>> {
        Arc::clone(&self.tile_cache)
    }
    
    /// Get APRS symbols spritesheet (lazy loaded)
    pub fn get_aprs_symbols(&self) -> Arc<Mutex<Option<image::RgbaImage>>> {
        Arc::clone(&self.aprs_symbols)
    }
    
    /// Load APRS symbols spritesheet
    pub fn load_aprs_symbols(&self, path: &Path) -> Result<(), String> {
        let mut cache = self.aprs_symbols.lock().unwrap();
        
        if cache.is_some() {
            return Ok(()); // Already loaded
        }
        
        // Try different spritesheet sizes
        let sizes = [24, 48, 64, 128];
        for size in sizes {
            let spritesheet_path = path.join(format!("png/aprs-symbols-{}-{}.png", size, 0));
            if spritesheet_path.exists() {
                match image::open(&spritesheet_path) {
                    Ok(img) => {
                        let rgba = img.to_rgba8();
                        eprintln!("[map] Loaded APRS symbols: {}x{}, size={}", 
                            rgba.width(), rgba.height(), size);
                        *cache = Some(rgba);
                        return Ok(());
                    }
                    Err(e) => {
                        eprintln!("[map] Failed to load {}: {}", spritesheet_path.display(), e);
                    }
                }
            }
        }
        
        Err("Could not find APRS symbols spritesheet".to_string())
    }
    
    /// Start background tile loading for visible tiles
    pub fn load_visible_tiles(&self, start_x: i32, start_y: i32, tiles_x: i32, tiles_y: i32, zoom: u32) {
        let cache = Arc::clone(&self.tile_cache);
        let pending = Arc::clone(&self.pending_tiles);
        
        std::thread::spawn(move || {
            for ty in 0..tiles_y {
                for tx in 0..tiles_x {
                    let tile_x = start_x + tx;
                    let tile_y = start_y + ty;
                    if tile_x < 0 || tile_y < 0 { continue; }
                    let max_tile = 2_i32.pow(zoom) - 1;
                    if tile_x > max_tile || tile_y > max_tile { continue; }
                    
                    let tile_id = TileId { x: tile_x as u32, y: tile_y as u32, z: zoom as u8 };
                    
                    // Skip if already loaded
                    if cache.lock().unwrap().contains_key(&tile_id) {
                        continue;
                    }
                    
                    // Load tile
                    let _ = load_tile_sync(tile_id, &cache);
                }
            }
        });
    }
}

impl Default for MapManager {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tile coordinate conversion utilities
// =============================================================================

/// Convert lat/lon to tile x/y at given zoom level
pub fn lat_lon_to_tile(lat: f64, lon: f64, zoom: u32) -> (f64, f64) {
    let n = 2.0_f64.powi(zoom as i32);
    let x = (lon + 180.0) / 360.0 * n;
    let lat_rad = lat.to_radians();
    let y = (1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    (x, y)
}

// =============================================================================
// Tile downloading
// =============================================================================

static HTTP_CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();

fn get_http_client() -> &'static reqwest::blocking::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default()
    })
}

/// Load a tile synchronously (blocking)
pub fn load_tile_sync(tile_id: TileId, cache: &Arc<Mutex<HashMap<TileId, Arc<image::RgbaImage>>>>) -> Option<Arc<image::RgbaImage>> {
    // Check cache first
    {
        let c = cache.lock().unwrap();
        if let Some(tile) = c.get(&tile_id) {
            return Some(Arc::clone(tile));
        }
    }
    
    // Download tile
    let url = format!(
        "https://tile.openstreetmap.org/{}/{}/{}.png",
        tile_id.z, tile_id.x, tile_id.y
    );
    
    match get_http_client().get(&url).send() {
        Ok(response) => {
            match response.bytes() {
                Ok(bytes) => {
                    match image::load_from_memory(&bytes) {
                        Ok(img) => {
                            let rgba = img.to_rgba8();
                            let tile = Arc::new(rgba);
                            
                            // Cache the tile
                            let mut c = cache.lock().unwrap();
                            c.insert(tile_id, Arc::clone(&tile));
                            
                            return Some(tile);
                        }
                        Err(e) => eprintln!("[map] image error: {:?}", e),
                    }
                }
                Err(e) => eprintln!("[map] bytes error: {:?}", e),
            }
        }
        Err(e) => eprintln!("[map] HTTP error: {:?}", e),
    }
    
    None
}

// =============================================================================
// Map widget
// =============================================================================

use gtk::prelude::*;
use cairo::{Context, Format, ImageSurface};

/// Custom map widget that renders OSM tiles
pub struct MapWidget {
    pub widget: gtk::DrawingArea,
}

impl MapWidget {
    pub fn new() -> Self {
        let area = gtk::DrawingArea::new();
        area.set_size_request(330, 400);
        area.set_hexpand(false);
        area.set_vexpand(false);
        
        Self { widget: area }
    }
    
    /// Draw the map using Cairo
    pub fn draw(&self, cr: &Context, width: i32, height: i32, state: &MapState, 
                tile_cache: &Arc<Mutex<HashMap<TileId, Arc<image::RgbaImage>>>>,
                aprs_symbols: &Arc<Mutex<Option<image::RgbaImage>>>) {
        let tile_size = 256.0;
        let zoom = state.zoom as u32;
        
        // Default center if no GPS
        let (center_lat, center_lon) = state.get_user_position().unwrap_or((46.0, 8.0));
        let (cx, cy) = lat_lon_to_tile(center_lat, center_lon, zoom);
        
        // Calculate tiles to show
        let tiles_x = (width as f64 / tile_size).ceil() as i32 + 2;
        let tiles_y = (height as f64 / tile_size).ceil() as i32 + 2;
        
        // Starting tile (top-left corner)
        let start_x = (cx - tiles_x as f64 / 2.0).floor() as i32;
        let start_y = (cy - tiles_y as f64 / 2.0).floor() as i32;
        
        // Pan offset
        let pan_x = state.pan_x;
        let pan_y = state.pan_y;
        
        // Center offset (tiles)
        let center_offset_x = cx.fract() - tiles_x as f64 / 2.0;
        let center_offset_y = cy.fract() - tiles_y as f64 / 2.0;
        
        // Draw background
        cr.set_source_rgb(0.1, 0.1, 0.1);
        let _ = cr.paint();
        
        // Draw tiles from cache
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let tile_x = start_x + tx;
                let tile_y = start_y + ty;
                
                // Skip invalid tiles
                let max_tile = 2_i32.pow(zoom) - 1;
                if tile_x < 0 || tile_y < 0 || tile_x > max_tile || tile_y > max_tile {
                    continue;
                }
                
                let tile_id = TileId { x: tile_x as u32, y: tile_y as u32, z: zoom as u8 };
                
                // Calculate screen position
                let screen_x = (tx as f64 - center_offset_x) * tile_size - pan_x;
                let screen_y = (ty as f64 - center_offset_y) * tile_size - pan_y;
                
                // Try to get tile from cache
                if let Some(tile) = tile_cache.lock().unwrap().get(&tile_id) {
                    Self::draw_tile_image(cr, tile, screen_x, screen_y, tile_size);
                } else {
                    // Draw placeholder
                    cr.set_source_rgb(0.15, 0.15, 0.15);
                    cr.rectangle(screen_x, screen_y, tile_size, tile_size);
                    let _ = cr.fill();
                }
            }
        }
        
        // Draw user position marker (house icon style)
        if let Some((lat, lon)) = state.get_user_position() {
            let (ux, uy) = lat_lon_to_tile(lat, lon, zoom);
            
            let tile_frac_x = (ux - cx) * tile_size + width as f64 / 2.0 + pan_x;
            let tile_frac_y = (uy - cy) * tile_size + height as f64 / 2.0 + pan_y;
            
            // Draw glow
            cr.set_source_rgba(0.2, 0.82, 0.48, 0.3);
            cr.arc(tile_frac_x, tile_frac_y, 16.0, 0.0, 2.0 * std::f64::consts::PI);
            let _ = cr.fill();
            
            cr.set_source_rgba(0.2, 0.82, 0.48, 0.6);
            cr.arc(tile_frac_x, tile_frac_y, 10.0, 0.0, 2.0 * std::f64::consts::PI);
            let _ = cr.fill();
            
            cr.set_source_rgba(0.2, 0.82, 0.48, 1.0);
            cr.arc(tile_frac_x, tile_frac_y, 5.0, 0.0, 2.0 * std::f64::consts::PI);
            let _ = cr.fill();
        }
        
        // Draw APRS station markers with official symbols
        let symbol_size = 24.0; // APRS symbol size
        for station in state.stations.values() {
            let (sx, sy) = lat_lon_to_tile(station.lat, station.lon, zoom);
            
            let tile_frac_x = (sx - cx) * tile_size + width as f64 / 2.0 + pan_x;
            let tile_frac_y = (sy - cy) * tile_size + height as f64 / 2.0 + pan_y;
            
            // Skip if off screen
            if tile_frac_x < -symbol_size || tile_frac_x > width as f64 + symbol_size ||
               tile_frac_y < -symbol_size || tile_frac_y > height as f64 + symbol_size {
                continue;
            }
            
            // Get sprite position for this symbol
            if let Some((row, col)) = station.symbol.sprite_position() {
                // Try to draw from spritesheet
                if let Some(spritesheet) = aprs_symbols.lock().unwrap().as_ref() {
                    let symbol_img = extract_symbol(spritesheet, row, col, 24);
                    if let Some(img) = symbol_img {
                        // Draw the symbol centered on position
                        let x = tile_frac_x - symbol_size / 2.0;
                        let y = tile_frac_y - symbol_size / 2.0;
                        Self::draw_tile_image(cr, &img, x, y, symbol_size);
                        continue;
                    }
                }
            }
            
            // Fallback: draw a simple circle marker
            cr.set_source_rgb(0.0, 0.5, 1.0);
            cr.arc(tile_frac_x, tile_frac_y, 8.0, 0.0, 2.0 * std::f64::consts::PI);
            let _ = cr.fill();
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.set_line_width(1.5);
            let _ = cr.stroke();
        }
    }
    
    /// Draw an RGBA image as a Cairo surface
    fn draw_tile_image(cr: &Context, image: &image::RgbaImage, x: f64, y: f64, size: f64) {
        let (img_width, img_height) = image.dimensions();
        
        // Convert RGBA to ARGB for Cairo
        let mut data: Vec<u8> = Vec::with_capacity((img_width * img_height * 4) as usize);
        for pixel in image.pixels() {
            data.push(pixel[2]);  // B
            data.push(pixel[1]);  // G  
            data.push(pixel[0]);  // R
            data.push(pixel[3]);  // A
        }
        
        let surface = match ImageSurface::create_for_data(
            data,
            Format::ARgb32,
            img_width as i32,
            img_height as i32,
            (img_width * 4) as i32,
        ) {
            Ok(s) => s,
            Err(_) => return,
        };
        
        // Scale to fit target size
        let scale_x = size / img_width as f64;
        let scale_y = size / img_height as f64;
        
        cr.save();
        cr.rectangle(x, y, size, size);
        let _ = cr.clip();
        cr.scale(scale_x, scale_y);
        cr.set_source_surface(&surface, x / scale_x, y / scale_y);
        let _ = cr.paint();
        cr.restore();
    }
}

/// Extract a symbol from the spritesheet
fn extract_symbol(spritesheet: &image::RgbaImage, row: u32, col: u32, size: u32) -> Option<image::RgbaImage> {
    let (sheet_width, sheet_height) = spritesheet.dimensions();
    
    let x = col;
    let y = row;
    
    if x + size > sheet_width || y + size > sheet_height {
        return None;
    }
    
    let mut result = image::RgbaImage::new(size, size);
    
    for dy in 0..size {
        for dx in 0..size {
            let px = x + dx;
            let py = y + dy;
            if let Some(pixel) = spritesheet.get_pixel_checked(px, py) {
                result.put_pixel(dx, dy, *pixel);
            }
        }
    }
    
    Some(result)
}