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
        let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| "?".to_string());
        eprintln!("[aprs_icons] CWD: {}", cwd);
        
        let mut sprites: [Option<ImageSurface>; 3] = [None, None, None];
        
        // Try multiple relative and absolute paths
        let exe_path = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf()));
        let mut paths = vec![
            "symbols/aprs-symbols/png".to_string(),
            "./symbols/aprs-symbols/png".to_string(),
            "../symbols/aprs-symbols/png".to_string(),
        ];
        
        // Add exe directory as fallback
        // Exe is at: /path/to/PocketModem/target/release/pocket-modem
        // Symbols are at: /path/to/PocketModem/symbols/aprs-symbols/png/
        if let Some(ref exe_dir) = exe_path {
            // ../../ from target/release to PocketModem root
            if let Some(parent) = exe_dir.parent() {
                let project_root = parent.parent().unwrap_or(parent);
                paths.push(format!("{}/symbols/aprs-symbols/png", project_root.display()));
                eprintln!("[aprs_icons] exe_dir={}, project_root={}", exe_dir.display(), project_root.display());
            }
        }
        
        // Add common system paths
        paths.push("/usr/share/aprs-symbols".to_string());
        paths.push("/usr/local/share/aprs-symbols".to_string());
        
        // Try sizes from large to small for best quality
        for size in [128, 64, 48, 32, 24] {
            let all_loaded = sprites.iter().all(|s| s.is_some());
            if all_loaded { break; }
            
            for (table, suffix) in [(0, "-0.png"), (1, "-1.png"), (2, "-2.png")] {
                if sprites[table].is_some() { continue; }
                
                let name = format!("aprs-symbols-{}{}", size, suffix);
                for base in &paths {
                    let path = format!("{}/{}", base, name);
                    match Self::load_png(&path) {
                        Ok(surf) => {
                            eprintln!("[aprs_icons] Loaded table {} from {}", table, path);
                            sprites[table] = Some(surf);
                            break;
                        }
                        Err(e) => {
                            eprintln!("[aprs_icons] Failed {}: {}", path, e);
                        }
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

    /// Get icon for symbol
    /// table_id: '/' (primary), '\\' (secondary), or '#'/0-9 (overlay)
    pub fn get_icon(&self, table_id: Option<char>, code: Option<char>, size: i32) -> ImageSurface {
        let table = match table_id.unwrap_or('/') {
            '/' => 0,     // Primary table
            '\\' => 1,    // Secondary table  
            '#' => 2,     // Overlay characters
            '0'..='9' => 2, // Overlay on base table - use overlay sprites
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
        // APRS symbol table mapping:
        // 0-9: indices 0-9
        // A-Z: indices 10-35 (10 + 0-25)
        // a-x: indices 36-61 (36 + 0-23), skipping y and z
        // >: index 62 (truck)
        // \: index 63 (house)
        
        let ascii = c as u32;
        if ascii >= 0x30 && ascii <= 0x39 {
            (ascii - 0x30) as usize  // 0-9
        } else if ascii >= 0x41 && ascii <= 0x5A {
            10 + (ascii - 0x41) as usize  // A-Z
        } else if ascii >= 0x61 && ascii <= 0x78 {
            36 + (ascii - 0x61) as usize  // a-x (36-60)
        } else if c == '>' {
            62
        } else if c == '\\' {
            63
        } else {
            63  // Default fallback
        }
    }

    fn render_icon(&self, table: usize, idx: usize, size: i32) -> ImageSurface {
        let surf = ImageSurface::create(Format::ARgb32, size, size)
            .expect("icon surface");
        let cr = Context::new(&surf).expect("cairo context");
        
        eprintln!("[aprs_icons] render_icon: table={}, idx={}, size={}", table, idx, size);
        
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
            eprintln!("[aprs_icons] Using FALLBACK for table={}, idx={}", table, idx);
            let (r, g, b) = Self::fallback_color(idx);
            let cx = size as f64 / 2.0;
            cr.set_source_rgb(r, g, b);
            cr.arc(cx, cx, cx * 0.8, 0.0, 2.0 * std::f64::consts::PI);
            cr.fill().ok();
            // Add a border so it's visible
            cr.set_source_rgb(r * 0.5, g * 0.5, b * 0.5);
            cr.set_line_width(2.0);
            cr.arc(cx, cx, cx * 0.8, 0.0, 2.0 * std::f64::consts::PI);
            cr.stroke().ok();
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