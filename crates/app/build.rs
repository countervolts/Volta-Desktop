//! Embeds `assets/**` (icons, images) into the binary. The generated
//! table goes to OUT_DIR, which lives in the external target dir.
use std::{env, fs, path::Path};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let root = root.canonicalize().expect("assets dir");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut entries = Vec::new();
    for dir in ["icons", "images"] {
        let path = root.join(dir);
        println!("cargo:rerun-if-changed={}", path.display());
        let mut files: Vec<_> = fs::read_dir(&path).unwrap().flatten().map(|e| e.path()).collect();
        files.sort();
        for file in files {
            let name = file.file_name().unwrap().to_string_lossy().to_string();
            if name.ends_with(".svg") || name.ends_with(".png") {
                entries.push(format!(
                    "    (\"{dir}/{name}\", include_bytes!(r\"{}\")),\n",
                    file.display()
                ));
            }
        }
    }
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("embedded_assets.rs");
    fs::write(
        out,
        format!("pub static EMBEDDED: &[(&str, &[u8])] = &[\n{}];\n", entries.concat()),
    )
    .unwrap();

    // Windows: the app icon as resource 1 (taskbar, Alt+Tab, Explorer).
    let ico = root.join("app-icon/volta.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon_with_id(ico.to_str().expect("utf-8 icon path"), "1");
        res.compile().expect("embed Windows icon resource");
    }
}
