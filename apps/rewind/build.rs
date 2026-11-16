use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

const ICON: &str = "assets/rewind.ico";
const KITS: &str = "Windows Kits\\10\\bin";

fn newest_rc() -> Option<PathBuf> {
    let roots = ["ProgramFiles(x86)", "ProgramFiles"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(|root| Path::new(&root).join(KITS));
    roots
        .filter_map(|root| std::fs::read_dir(root).ok())
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path().join("x64").join("rc.exe"))
        .filter(|path| path.is_file())
        .max()
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed={ICON}");
    if std::env::var("CARGO_CFG_TARGET_OS")? != "windows" {
        return Ok(());
    }
    let Some(rc) = newest_rc() else {
        println!("cargo:warning=rc.exe not found, the executable will have no icon");
        return Ok(());
    };
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let out = PathBuf::from(std::env::var("OUT_DIR")?);
    let icon = manifest
        .join(ICON)
        .display()
        .to_string()
        .replace('\\', "\\\\");
    let script = out.join("rewind.rc");
    let resource = out.join("rewind.res");
    std::fs::write(&script, format!("1 ICON \"{icon}\"\n"))?;
    let status = Command::new(rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&resource)
        .arg(&script)
        .status()?;
    if !status.success() {
        return Err(format!("rc.exe failed with {status}").into());
    }
    println!("cargo:rustc-link-arg-bins={}", resource.display());
    Ok(())
}
