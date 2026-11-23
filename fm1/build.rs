//! Build the startup archive and expose the FM-1 linker script for target firmware.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn run(command: &mut Command) -> std::io::Result<()> {
    let result = command.status()?;
    assert!(result.success(), "{command:?} failed ({result})");
    Ok(())
}

fn archive(bin: &Path, out: &Path, name: &str, files: &[&str]) -> std::io::Result<()> {
    let archive = out.join(name);
    let mut ar = Command::new(bin.join("llvm-ar"));
    ar.arg("crs").arg(&archive);
    for file in files {
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
    // llvm-ar replaces named members but keeps omitted ones on incremental builds.
    if archive.try_exists()? {
        fs::remove_file(&archive)?;
    }
    run(&mut ar)
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
    archive(&bin, &out, "libfm1_rt.a", &["crt0.S", "shim.c"])?;
    if env::var_os("CARGO_FEATURE_CPU1_PROBE").is_some() {
        // Only the CPU1 examples explicitly link this archive, even with --all-features.
        archive(
            &bin,
            &out,
            "libfm1_cpu1_probe.a",
            &["cpu1_entry.S", "cpu1_probe.c", "cpu1_audio.c"],
        )?;
    }
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
