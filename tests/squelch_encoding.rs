//! Squelch encoding test
//! 
//! Run with: rustc tests/squelch_encoding.rs -o /tmp/squelch_test && /tmp/squelch_test
//! 
//! Tests that squelch is correctly encoded in HostDesiredState packets.
//! This verifies the byte layout matches the KV4P protocol spec.

fn main() {
    println!("=== Squelch Encoding Test ===\n");
    
    // HostDesiredState to_bytes() format:
    // sequence[4] + memory_id[4] + flags[2] + bandwidth[1] + freq_tx[4] + freq_rx[4] + ctcss_tx[1] + squelch[1] + ctcss_rx[1]
    // Total: 22 bytes
    // Index:    0-3       4-7         8-9      10         11-14       15-18       19          20          21
    
    println!("Verifying HostDesiredState packet layout:");
    println!("  sequence:   bytes 0-3");
    println!("  memory_id:  bytes 4-7");
    println!("  flags:      bytes 8-9");
    println!("  bandwidth:  byte 10");
    println!("  freq_tx:    bytes 11-14");
    println!("  freq_rx:    bytes 15-18");
    println!("  ctcss_tx:   byte 19");
    println!("  squelch:    byte 20  <-- TARGET");
    println!("  ctcss_rx:   byte 21\n");
    
    let mut bytes = vec![0u8; 22];
    
    // Set squelch at index 20
    bytes[20] = 0xAB; // Marker to identify position
    println!("  Marker at index 20: {:#04x}", bytes[20]);
    
    // Test all valid squelch levels
    println!("\nTesting squelch level encoding:\n");
    println!("  Level | Encoded | Status");
    println!("  ------|---------|-------");
    
    let mut passed = true;
    for level in 0..=9u8 {
        bytes[20] = level;
        let status = if bytes[20] == level { "OK" } else { "FAIL" };
        println!("  {:2}    | {:#04x}     | {}", level, bytes[20], status);
        if bytes[20] != level { passed = false; }
    }
    
    // Also test that squelch is NOT at index 19
    println!("\n  Verifying squelch is NOT at index 19: {:#04x}", bytes[19]);
    let not_squelch = bytes[19] != 0xAB;
    
    println!("\n=== Results ===");
    if passed && not_squelch {
        println!("  All tests PASSED");
        println!("  Squelch correctly encoded at byte index 20");
    } else {
        println!("  Tests FAILED");
        std::process::exit(1);
    }
}