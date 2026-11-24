//! Make the opt-in linker script available to downstream firmware.

use std::env;

fn main() {
    println!("cargo::rerun-if-changed=fm1-assets.ld");
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(fm1_assets_xip)");
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("pi32v2") {
        println!("cargo::rustc-cfg=fm1_assets_xip");
        println!(
            "cargo::rustc-link-search=native={}",
            env!("CARGO_MANIFEST_DIR")
        );
    }
}
