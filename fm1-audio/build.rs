//! Assemble the FM-1 mixer with the same LLVM tools as the BSP.
use std::{env, io, path::PathBuf, process::Command};

fn run(command: &mut Command) -> io::Result<()> {
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{command:?} failed ({status})")))
    }
}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    println!("cargo::rustc-check-cfg=cfg(fm1_dsp)");
    println!("cargo::rerun-if-changed=src/mix/kernels.S");
    println!("cargo::rerun-if-env-changed=PI32V2_BIN");
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("pi32v2")
        || env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("none")
        || env::var_os("CARGO_FEATURE_DSP").is_none()
    {
        return Ok(());
    }
    let bin = PathBuf::from(env::var_os("PI32V2_BIN").ok_or("set PI32V2_BIN as for fm1")?);
    let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("Cargo did not set OUT_DIR")?);
    let object = out.join("kernels.o");
    run(Command::new(bin.join("clang"))
        .args([
            "--target=pi32v2",
            "-mcpu=ac791n",
            "-c",
            "src/mix/kernels.S",
            "-o",
        ])
        .arg(&object))?;
    run(Command::new(bin.join("llvm-ar"))
        .arg("crs")
        .arg(out.join("libfm1_audio_mix.a"))
        .arg(object))?;
    println!("cargo::rustc-cfg=fm1_dsp");
    println!("cargo::rustc-link-search=native={}", out.display());
    println!("cargo::rustc-link-lib=static=fm1_audio_mix");
    Ok(())
}
