//! Squelch Test - Run on ht with hardware connected
//!
//! Build: cargo build --bin squelch_test
//! Run:   ./target/release/squelch_test /dev/ttyUSB0

use std::time::Duration;

mod audio;
mod gps;
mod kiss;
mod radio;

use radio::{KV4PRadio, SerialConfig};

fn main() {
    let port = std::env::args().nth(1).unwrap_or_else(|| "/dev/ttyUSB0".to_string());
    println!("=== Squelch Test on {}\n", port);

    let config = SerialConfig {
        port,
        baudrate: 115200,
        timeout_ms: 500,
    };

    let mut radio = KV4PRadio::new(config);

    // Connect
    print!("Connecting... ");
    match radio.open() {
        Ok(Some(v)) => println!("OK (fw=v{}, rf={:?})", v.firmware_version, v.rf_module_type),
        Ok(None) => { println!("NO RESPONSE"); return; }
        Err(e) => { println!("FAILED: {}", e); return; }
    }

    std::thread::sleep(Duration::from_millis(300));

    println!("\nSetting squelch levels:\n");
    
    for level in [0, 3, 6, 9] {
        radio.set_squelch(level).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        
        let echoed = radio.device_state().map(|s| s.squelch);
        match echoed {
            Some(e) if e == level => println!("  {} -> {} ✓", level, e),
            Some(e) => println!("  {} -> {} MISMATCH", level, e),
            None => println!("  {} -> (no echo)", level),
        }
    }

    radio.close();
    println!("\nDone.");
}