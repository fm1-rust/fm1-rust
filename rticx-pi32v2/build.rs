//! Assemble the interrupt entry stubs with the same LLVM as the BSP.
use std::{env, path::PathBuf};

fn main() -> Result<(), Box<dyn core::error::Error>> {
    println!("cargo::rustc-check-cfg=cfg(pi32v2)");
    println!("cargo::rerun-if-changed=src/interrupt.S");
    println!("cargo::rerun-if-env-changed=PI32V2_BIN");
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("pi32v2") {
        println!("cargo::rustc-cfg=pi32v2");
        let bin = PathBuf::from(env::var_os("PI32V2_BIN").ok_or("set PI32V2_BIN")?);
        cc::Build::new()
            .compiler(bin.join("clang"))
            .archiver(bin.join("llvm-ar"))
            .no_default_flags(true)
            .flag("--target=pi32v2")
            .flag("-mcpu=ac791n")
            .file("src/interrupt.S")
            .try_compile("rticx_pi32v2_irq")?;
    }
    Ok(())
}
