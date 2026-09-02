// build.rs - Compile RADE C library
//
// Note: FFI bindings are defined manually in lib.rs to avoid bindgen dependency.
// This only compiles the C library.

use std::path::PathBuf;
use std::env;

fn main() {
    let src_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let c_dir = src_dir.join("src").join("c");
    
    println!("cargo:warning=Building RADE C library from {:?}", c_dir);
    
    // Link against system opus (macOS: /opt/homebrew/lib, Linux: /usr/lib or /usr/lib/aarch64-linux-gnu)
    println!("cargo:rustc-link-search=native=/opt/homebrew/lib");
    println!("cargo:rustc-link-search=native=/usr/lib");
    println!("cargo:rustc-link-search=native=/usr/lib/aarch64-linux-gnu");
    println!("cargo:rustc-link-lib=opus");
    
    // Compile the RADE C library
    let mut builder = cc::Build::new();
    builder
        .file(c_dir.join("nnet.c"))
        .file(c_dir.join("freq.c"))
        .file(c_dir.join("rade_api.c"))
        .file(c_dir.join("rade_enc.c"))
        .file(c_dir.join("rade_dec.c"))
        .file(c_dir.join("rade_enc_data.c"))
        .file(c_dir.join("rade_dec_data.c"))
        .flag("-DIS_BUILDING_RADE=1")
        .flag("-DFLOAT_APPROX=1")
        .flag("-DDISABLE_FARGAN=1")
        .flag("-DDISABLE_PITCH=1")
        .flag("-DDISABLE_PLC=1")
        .flag("-O3")
        .flag("-fPIC")
        .include(&c_dir);

    #[cfg(target_arch = "x86_64")]
    {
        builder.flag("-msse").flag("-msse2").flag("-msse4.1");
    }
    #[cfg(target_arch = "aarch64")]
    {
        builder.flag("-ffp-contract=fast");
        builder.flag("-march=armv8-a");  // Enable ARMv8-A features
    }

    builder.compile("rade_core");

    // Print rebuild hints for C source files
    println!("cargo:rerun-if-changed=src/c/rade_api.h");
    println!("cargo:rerun-if-changed=src/c/rade_enc.c");
    println!("cargo:rerun-if-changed=src/c/rade_dec.c");
    println!("cargo:rerun-if-changed=src/c/rade_enc_data.c");
    println!("cargo:rerun-if-changed=src/c/rade_dec_data.c");
    println!("cargo:rerun-if-changed=src/c/rade_core.h");
    println!("cargo:rerun-if-changed=src/c/rade_enc.h");
    println!("cargo:rerun-if-changed=src/c/rade_dec.h");
    println!("cargo:rerun-if-changed=src/c/rade_enc_data.h");
    println!("cargo:rerun-if-changed=src/c/rade_dec_data.h");
    println!("cargo:rerun-if-changed=src/c/rade_constants.h");
    println!("cargo:rerun-if-changed=src/c/nnet.h");
    println!("cargo:rerun-if-changed=src/c/opus_types.h");
    println!("cargo:rerun-if-changed=src/c/os_support.h");
}