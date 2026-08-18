//! APRS Icon Rendering using hessu/aprs-symbols spritesheets
//!
//! Uses the standard APRS symbol spritesheets from:
//! https://github.com/hessu/aprs-symbols
//!
//! Format:
//! - aprs-symbols-SIZE-0.png = Primary table (/)
//! - aprs-symbols-SIZE-1.png = Secondary table (\)
//! - aprs-symbols-SIZE-2.png = Overlay characters
//!
//! Each spritesheet has 16 symbols per row, symbols are SIZE pixels each.

use cairo::{Context, Format, ImageSurface, SurfacePattern, Filter};
use std::collections::HashMap;
use std::sync::Mutex;

/// Symbols per row in spritesheet
const COLS: usize = 16;

/// APRS icon renderer
pub struct APRSIconRenderer {
    icon_cache: Mutex<HashMap<String, ImageSurface>>,
    /// [primary_table, secondary_table, overlays]
    sprites: [Option<ImageSurface>; 3],
}

impl APRSIconRenderer {
    pub fn new() -> Self {
        Self {
            icon_cache: Mutex::new(HashMap::new()),
            sprites: Self::load_sprites(),
        }
    }

    fn load_sprites() -> [Option<ImageSurface>; 3] {
        let mut sprites: [Option<ImageSurface>; 3] = [None, None, None];
        
        let paths = [
            "symbols/aprs-symbols/png",
            "/usr/share/aprs-symbols",
        ];
        
        // Try sizes from large to small for best quality
        for size in [128, 64, 48, 32, 24] {
            let all_loaded = sprites.iter().all(|s| s.is_some());
            if all_loaded { break; }
            
            for (table, suffix) in [(0, "-0.png"), (1, "-1.png"), (2, "-2.png")] {
                if sprites[table].is_some() { continue; }
                
                let name = format!("aprs-symbols-{}{}", size, suffix);
                for base in &paths {
                    let path = format!("{}/{}", base, name);
                    if let Ok(surf) = Self::load_png(&path) {
                        eprintln!("[aprs_icons] Loaded: {}", path);
                        sprites[table] = Some(surf);
                        break;
                    }
                }
            }
        }
        
        let loaded = sprites.iter().filter(|s| s.is_some()).count();
        eprintln!("[aprs_icons] Sprites loaded: {}/3", loaded);
        
        sprites
    }

    fn load_png(path: &str) -> Result<ImageSurface, String> {
        use std::fs::File;
        use std::io::BufReader;
        
        let file = File::open(path).map_err(|e| e.to_string())?;
        let dec = png::Decoder::new(BufReader::new(file));
        let mut reader = dec.read_info().map_err(|e| e.to_string())?;
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
        
        // Convert RGBA to premultiplied ARGB
        let mut argb = Vec::with_capacity(info.width as usize * info.height as usize * 4);
        for px in buf.chunks(4) {
            if px.len() >= 4 {
                argb.push(px[3]); // A
                argb.push(px[0]); // R  
                argb.push(px[1]); // G
                argb.push(px[2]); // B
            }
        }
        
        let stride = ((info.width * 4 + 3) & !3) as i32;
        ImageSurface::create_for_data(argb, Format::ARgb32, info.width as i32, info.height as i32, stride)
            .map_err(|e| e.to_string())
    }

    /// Get icon for symbol (table_id: '/' or '\\', code: the APRS symbol character)
    pub fn get_icon(&self, table_id: Option<char>, code: Option<char>, size: i32) -> ImageSurface {
        let table = match table_id.unwrap_or('/') {
            '\\' => 1,
            _ => 0,
        };
        let sym = code.unwrap_or('?');
        let idx = Self::symbol_index(sym);
        
        let key = format!("{}/{}/{}", table, idx, size);
        
        if let Some(cached) = self.icon_cache.lock().unwrap().get(&key) {
            return cached.clone();
        }
        
        let surf = self.render_icon(table, idx, size);
        self.icon_cache.lock().unwrap().insert(key, surf.clone());
        surf
    }

    fn symbol_index(c: char) -> usize {
        match c {
            '0'..='9' => c as usize - '0' as usize,
            'A'..='Z' => 10 + c as usize - 'A' as usize,
            'a'..='z' => 36 + c as usize - 'a' as usize,
            _ => 63,
        }
    }

    fn render_icon(&self, table: usize, idx: usize, size: i32) -> ImageSurface {
        let surf = ImageSurface::create(Format::ARgb32, size, size)
            .expect("icon surface");
        let cr = Context::new(&surf).expect("cairo context");
        
        if let Some(ref sprite) = self.sprites[table] {
            let sprite_size = sprite.width() / COLS as i32;
            let col = idx % COLS;
            let row = idx / COLS;
            let sx = col as f64 * sprite_size as f64;
            let sy = row as f64 * sprite_size as f64;
            
            let scale = size as f64 / sprite_size as f64;
            cr.scale(scale, scale);
            
            let pattern = SurfacePattern::create(sprite);
            pattern.set_filter(Filter::Best);
            let mut mat = cairo::Matrix::identity();
            mat.translate(-sx, -sy);
            pattern.set_matrix(mat);
            
            cr.set_source(&pattern);
            cr.paint().ok();
        } else {
            // Fallback: colored circle
            let (r, g, b) = Self::fallback_color(idx);
            let cx = size as f64 / 2.0;
            cr.set_source_rgb(r, g, b);
            cr.arc(cx, cx, cx * 0.8, 0.0, 2.0 * std::f64::consts::PI);
            cr.fill().ok();
        }
        
        surf
    }

    fn fallback_color(idx: usize) -> (f64, f64, f64) {
        match idx / 16 {
            0 => (1.0, 0.75, 0.0),     // 0-9: yellow
            1 => (0.0, 0.0, 0.8),      // A-J: blue
            2 => (0.0, 0.8, 0.0),      // K-Z: green
            3 => (1.0, 0.0, 0.0),      // a-j: red
            4 => (1.0, 0.55, 0.0),     // k-z: orange
            _ => (0.5, 0.5, 0.5),
        }
    }
}

impl Default for APRSIconRenderer {
    fn default() -> Self { Self::new() }
}