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
        
        // Store raw RGBA data
        
        let stride = ((info.width * 4 + 3) & !3) as i32;
        ImageSurface::create_for_data(buf, Format::ARgb32, info.width as i32, info.height as i32, stride)
            .map_err(|e| e.to_string())
    }

    /// Get icon for symbol
    /// table_id: '/' (primary), '\\' (secondary), or '#'/0-9 (overlay)
    pub fn get_icon(&mut self, table_id: Option<char>, code: Option<char>, size: i32) -> ImageSurface {
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
        // APRS symbol table mapping (indices 0-127):
        // Standard APRS symbol table order (matches aprs-symbols spritesheet)
        // Based on: http://www.aprs.org/symbols/symbols-new.txt
        //
        // Row 0 (0-15):   0-9, :, ;, <, =, >, ?, @, A
        // Row 1 (16-31):  B-O (16 symbols)
        // Row 2 (32-47):  P-Z, [, \\, ], ^, _, `, a-b (16 symbols)
        // Row 3 (48-63):  c-n (16 symbols)
        // Row 4 (64-79):  o-z, more special chars
        // Row 5 (80-95):  overlay table start
        // etc.
        
        let idx = match c {
            // Row 0: 0-9 and special characters
            '0'..='9' => c as usize - '0' as usize,  // 0-9 -> indices 0-9
            ':' => 10,
            ';' => 11,
            '<' => 12,
            '=' => 13,
            '>' => 14,
            '?' => 15,
            '@' => 16,
            'A' => 17,
            
            // Row 1: B-O
            'B' => 18, 'C' => 19, 'D' => 20, 'E' => 21, 'F' => 22,
            'G' => 23, 'H' => 24, 'I' => 25, 'J' => 26, 'K' => 27,
            'L' => 28, 'M' => 29, 'N' => 30, 'O' => 31,
            
            // Row 2: P-Z and special
            'P' => 32, 'Q' => 33, 'R' => 34, 'S' => 35, 'T' => 36,
            'U' => 37, 'V' => 38, 'W' => 39, 'X' => 40, 'Y' => 41,
            'Z' => 42,
            '[' => 43, '\\' => 44, ']' => 45, '^' => 46, '_' => 47,
            '`' => 48, 'a' => 49, 'b' => 50,
            
            // Row 3: c-n
            'c' => 51, 'd' => 52, 'e' => 53, 'f' => 54, 'g' => 55,
            'h' => 56, 'i' => 57, 'j' => 58, 'k' => 59, 'l' => 60,
            'm' => 61, 'n' => 62,
            
            // Row 4: o-z and truck/house
            'o' => 63, 'p' => 64, 'q' => 65, 'r' => 66, 's' => 67,
            't' => 68, 'u' => 69, 'v' => 70, 'w' => 71, 'x' => 72,
            'y' => 73, 'z' => 74,
            
            // APRS special symbols that might appear
            '#' => 75,  // Number in circle / DX cluster
            '$' => 76,  // Money/sign
            '%' => 77,  // Bank/ATM
            '&' => 78,  // Arrow
            '*' => 79,  // Star
            '(' => 80,  // Left paren
            ')' => 81,  // Right paren
            '+' => 82,  // Plus
            ',' => 83,  // Comma
            '-' => 84,  // Dash
            '.' => 85,  // Dot
            '/' => 86,  // Forward slash (already used as table ID, but also a symbol)
            
            // Fallback
            _ => 63,  // Default to house-like symbol
        };
        idx
    }

    fn render_icon(&mut self, table: usize, idx: usize, size: i32) -> ImageSurface {
        let surf = ImageSurface::create(Format::ARgb32, size, size)
            .expect("icon surface");
        let cr = Context::new(&surf).expect("cairo context");
        
        if let Some(ref mut sprite) = self.sprites[table] {
            let w = sprite.width() as i32;
            let h = sprite.height() as i32;
            let sprite_size = w / COLS as i32;
            let col = idx % COLS;
            let row = idx / COLS;
            let sx = col as f64 * sprite_size as f64;
            let sy = row as f64 * sprite_size as f64;
            
            // Read sprite data (clone to get owned copy)
            let sprite_data = match sprite.data() {
                Ok(d) => d.to_vec(),
                Err(e) => {
                    eprintln!("[aprs_icons] Failed to read sprite data: {}", e);
                    return surf;
                }
            };
            
            // Create sprite surface from owned data
            let stride = ((w * 4 + 3) & !3) as i32;
            let sprite_surf = match ImageSurface::create_for_data(
                sprite_data,
                Format::ARgb32,
                w,
                h,
                stride
            ) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[aprs_icons] Failed to create sprite surface: {}", e);
                    return surf;
                }
            };
            
            // Scale: sprite_size -> size
            cr.scale(size as f64 / sprite_size as f64, size as f64 / sprite_size as f64);
            cr.set_source_surface(&sprite_surf, -sx, -sy);
            cr.paint().ok();
        } else {
            // Fallback: colored circle
            let (r, g, b) = Self::fallback_color(idx);
            cr.set_source_rgb(r, g, b);
            cr.paint().ok();
        }
        
        surf
    }
    
    // Keep old implementation in comments for reference:
    /*
    fn render_icon_OLD(&self, table: usize, idx: usize, size: i32) -> ImageSurface {
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
            // Add a border so it's visible
            cr.set_source_rgb(r * 0.5, g * 0.5, b * 0.5);
            cr.set_line_width(2.0);
            cr.arc(cx, cx, cx * 0.8, 0.0, 2.0 * std::f64::consts::PI);
            cr.stroke().ok();
        }
        
        surf
    }
    */

    #[allow(dead_code)]
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