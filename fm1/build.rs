//! Build the startup archive and expose the FM-1 linker script for target firmware.

use std::{env, fs, path::PathBuf, process::Command};

fn run(command: &mut Command) -> std::io::Result<()> {
    let result = command.status()?;
    assert!(result.success(), "{command:?} failed ({result})");
    Ok(())
}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    println!("cargo::rerun-if-changed=runtime");
    println!("cargo::rerun-if-env-changed=PI32V2_BIN");
    println!("cargo::rustc-check-cfg=cfg(fm1)");
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("pi32v2") {
        return Ok(());
    }
    println!("cargo::rustc-cfg=fm1");
    let bin = PathBuf::from(
        env::var_os("PI32V2_BIN").ok_or("set PI32V2_BIN to the pi32v2 LLVM bin directory")?,
    );
    let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("Cargo did not set OUT_DIR")?);
    let mut ar = Command::new(bin.join("llvm-ar"));
    ar.arg("crs").arg(out.join("libfm1_rt.a"));
    for file in ["crt0.S", "shim.c"] {
        let object = out.join(file).with_extension("o");
        run(Command::new(bin.join("clang"))
            .args([
                "--target=pi32v2",
                "-mcpu=ac791n",
                "-O2",
                "-ffreestanding",
                "-c",
            ])
            .arg(PathBuf::from("runtime").join(file))
            .arg("-o")
            .arg(&object))?;
        ar.arg(object);
    }
    run(&mut ar)?;
    fs::copy("runtime/fm1.ld", out.join("fm1.ld"))?;
    println!("cargo::rustc-link-search=native={}", out.display());
    println!("cargo::rustc-link-lib=static=fm1_rt");
    // pi32v2 implements atomic RMW operations and some arithmetic in compiler-rt.
    let result = Command::new(bin.join("clang"))
        .arg("--print-resource-dir")
        .output()?;
    assert!(result.status.success(), "clang --print-resource-dir failed");
    let resources = String::from_utf8(result.stdout)?;
    let builtins = PathBuf::from(resources.trim()).join("lib/pi32v2");
    assert!(
        builtins.join("libclang_rt.builtins.a").is_file(),
        "missing pi32v2 compiler-rt builtins in {}",
        builtins.display()
    );
    println!("cargo::rustc-link-search=native={}", builtins.display());
    println!("cargo::rustc-link-lib=static=clang_rt.builtins");
    Ok(())
}
