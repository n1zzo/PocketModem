// Build script for PocketModem
//
// Compiles GLib GSettings schema during build using glib-compile-schemas

fn main() {
    // Find glib-compile-schemas
    let schema_dir = "data/glib-2.0/schemas";
    
    // Try to compile schemas (best effort - doesn't fail build if not available)
    let result = std::process::Command::new("glib-compile-schemas")
        .arg(schema_dir)
        .output();

    match result {
        Ok(output) => {
            if output.status.success() {
                println!("cargo:warning=GSettings schema compiled successfully");
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("cargo:warning=Schema compilation warning: {}", stderr);
            }
        }
        Err(e) => {
            println!("cargo:warning=glib-compile-schemas not found: {}. Schema will be compiled at install time.", e);
        }
    }
    
    // Tell cargo to rerun if schema files change
    println!("cargo:rerun-if-changed={}/org.pocketmodem.pocket-modem.gschema.xml", schema_dir);
}