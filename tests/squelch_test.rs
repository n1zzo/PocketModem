//! Squelch setting test
//! 
//! Run on target machine: cargo run --release --test squelch_test
//! 
//! Tests that squelch setting is properly sent and echoed back.

use std::time::Duration;

fn main() {
    println!("=== Squelch Setting Test ===\n");
    
    // Detect serial port
    let port = std::env::args()
        .nth(1)
        .unwrap_or_else(|| {
            println!("No serial port specified. Looking for device...");
            // Try to find KV4P device
            let entries = std::fs::read_dir("/dev/serial/by-id/").unwrap();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.contains("KV4P") || name.contains("CP210") {
                    println!("Found: {}", name);
                    return format!("/dev/serial/by-id/{}", name);
                }
            }
            // Fallback to ttyUSB0
            println!("No KV4P device found, using /dev/ttyUSB0");
            "/dev/ttyUSB0".to_string()
        });
    
    println!("Using port: {}\n", port);
    
    let config = pocket_modem::SerialConfig {
        port,
        baudrate: 115200,
        timeout_ms: 500,
    };
    
    let mut radio = pocket_modem::KV4PRadio::new(config);
    
    // Connect
    println!("Connecting...");
    match radio.open() {
        Ok(Some(version)) => {
            println!("  Connected! Firmware v{}, RF module: {:?}\n", 
                     version.firmware_version, version.rf_module_type);
        }
        Ok(None) => {
            println!("  No device response");
            return;
        }
        Err(e) => {
            println!("  Failed: {}", e);
            return;
        }
    }
    
    // Small delay for device to process
    std::thread::sleep(Duration::from_millis(200));
    
    // Test squelch levels
    let levels = [0, 2, 4, 6, 8, 9];
    
    println!("Setting squelch levels and reading back:\n");
    println!("  Set -> Echoed");
    println!("  -------------");
    
    for level in levels {
        radio.set_squelch(level).unwrap();
        std::thread::sleep(Duration::from_millis(500)); // Wait for device response
        
        // Read device state to see what squelch the device has
        if let Some(state) = radio.device_state() {
            let echoed = state.squelch;
            let status = if echoed == level { "OK" } else { "MISMATCH!" };
            println!("  {:2} -> {:2}  [{}]", level, echoed, status);
        } else {
            println!("  {:2} -> (no device state)", level);
        }
        
        std::thread::sleep(Duration::from_millis(200));
    }
    
    println!("\nTest complete.");
    radio.close();
}