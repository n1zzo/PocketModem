//! Squelch Integration Test
//! 
//! Tests squelch setting and reading back from the device.
//! 
//! BUILD on target machine (ht):
//!   cd ~/PocketModem && cargo build --release
//!
//! RUN with optional serial port:
//!   ./target/release/pocket-modem --test-squelch
//!   ./target/release/pocket-modem --test-squelch /dev/ttyUSB0
//!
//! The test will:
//! 1. Connect to the radio
//! 2. Set various squelch levels (0, 2, 4, 6, 8, 9)
//! 3. Read back the echoed squelch from DeviceState
//! 4. Report PASS/FAIL for each level

extern crate pocket_modem;

use pocket_modem::{KV4PRadio, SerialConfig};
use std::time::Duration;

fn main() {
    println!("=== Squelch Integration Test ===\n");
    
    // Get serial port from args or detect
    let port = std::env::args()
        .nth(1)
        .unwrap_or_else(|| detect_port());
    
    println!("Using port: {}\n", port);
    
    let config = SerialConfig {
        port,
        baudrate: 115200,
        timeout_ms: 500,
    };
    
    let mut radio = KV4PRadio::new(config);
    
    // Connect
    println!("Connecting...");
    match radio.open() {
        Ok(Some(version)) => {
            println!("  ✓ Connected: firmware v{}, RF module: {:?}\n", 
                     version.firmware_version, version.rf_module_type);
        }
        Ok(None) => {
            println!("  ✗ No device response");
            std::process::exit(1);
        }
        Err(e) => {
            println!("  ✗ Connection failed: {}", e);
            std::process::exit(1);
        }
    }
    
    // Wait for initial state exchange
    std::thread::sleep(Duration::from_millis(300));
    
    // Test squelch levels
    let levels = [0, 2, 4, 6, 8, 9];
    let mut passed = 0;
    let mut failed = 0;
    
    println!("Testing squelch levels:");
    println!("  Set -> Echoed -> Status");
    println!("  -----------------------");
    
    for level in levels {
        if let Err(e) = radio.set_squelch(level) {
            println!("  {:2} -> ERROR: {}", level, e);
            failed += 1;
            continue;
        }
        
        // Wait for device to process and echo back
        std::thread::sleep(Duration::from_millis(400));
        
        if let Some(state) = radio.device_state() {
            let echoed = state.squelch;
            if echoed == level {
                println!("  {:2} -> {:2}     ✓ PASS", level, echoed);
                passed += 1;
            } else {
                println!("  {:2} -> {:2}     ✗ FAIL (expected {})", level, echoed, level);
                failed += 1;
            }
        } else {
            println!("  {:2} -> (no device state)", level);
            failed += 1;
        }
        
        std::thread::sleep(Duration::from_millis(150));
    }
    
    println!("\n=== Results ===");
    println!("  Passed: {}", passed);
    println!("  Failed: {}", failed);
    
    radio.close();
    
    if failed > 0 {
        println!("\n⚠️  Some tests FAILED - check firmware squelch handling");
        std::process::exit(1);
    } else {
        println!("\n✓ All tests PASSED");
    }
}

fn detect_port() -> String {
    println!("Detecting serial port...");
    if let Ok(entries) = std::fs::read_dir("/dev/serial/by-id/") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // Look for common USB-serial device patterns
            if name.contains("KV4P") || name.contains("CP210") || name.contains("FTDI") || name.contains("CH340") {
                println!("  Found: {}", name);
                return format!("/dev/serial/by-id/{}", name);
            }
        }
    }
    println!("  No device found in /dev/serial/by-id/, using /dev/ttyUSB0");
    "/dev/ttyUSB0".to_string()
}