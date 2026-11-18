//! Embeds `assets/**` (icons, images) into the binary. The generated
//! table goes to OUT_DIR, which lives in the external target dir.
use std::{env, fs, path::Path};

/// Build facts shown in Settings → About and used by the nightly update check.
fn build_info() {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    // CI passes the commit through GITHUB_SHA; local builds ask git.
    let sha = env::var("GITHUB_SHA").ok().filter(|s| !s.is_empty()).or_else(|| git(&["rev-parse", "HEAD"])).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some();
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let rustc = std::process::Command::new(env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=VOLTA_GIT_SHA={sha}");
    println!("cargo:rustc-env=VOLTA_GIT_DIRTY={dirty}");
    println!("cargo:rustc-env=VOLTA_BUILD_UNIX={unix}");
    println!("cargo:rustc-env=VOLTA_RUSTC={rustc}");
    println!("cargo:rustc-env=VOLTA_TARGET={}", env::var("TARGET").unwrap_or_default());
    println!("cargo:rustc-env=VOLTA_PROFILE={}", env::var("PROFILE").unwrap_or_default());
    // "stable" (tagged release), "nightly" (CI build of main) or "dev" (everything else).
    println!("cargo:rerun-if-env-changed=VOLTA_CHANNEL");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rustc-env=VOLTA_CHANNEL={}", env::var("VOLTA_CHANNEL").unwrap_or_else(|_| "dev".into()));
    println!("cargo:rerun-if-changed=../../.git/HEAD");

    // Versions of the libraries worth knowing about in a bug report, read
    // from the lockfile this binary was built against.
    const WANTED: &[&str] = &[
        "gpui-pre", "gpui-component", "gpui-pre-apple", "gpui-pre-windows", "gpui-pre-wgpu", "wgpu", "symphonia", "symphonia-adapter-libopus",
        "cpal", "rubato", "rusqlite", "libsqlite3-sys", "reqwest", "rustls", "image", "resvg", "tiny-skia", "souvlaki", "zbus", "rfd", "serde_json", "chrono",
    ];
    let lock = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock.display());
    let mut found: Vec<(String, Vec<String>)> = Vec::new();
    if let Ok(text) = fs::read_to_string(&lock) {
        let mut name = String::new();
        for line in text.lines() {
            if let Some(n) = line.strip_prefix("name = \"") {
                name = n.trim_end_matches('"').to_string();
            } else if let Some(v) = line.strip_prefix("version = \"") {
                if WANTED.contains(&name.as_str()) {
                    let v = v.trim_end_matches('"').to_string();
                    match found.iter_mut().find(|(n, _)| *n == name) {
                        Some((_, versions)) => versions.push(v),
                        None => found.push((name.clone(), vec![v])),
                    }
                }
            }
        }
    }
    let deps: Vec<String> = found.iter().map(|(n, v)| format!("{n}={}", v.join("/"))).collect();
    println!("cargo:rustc-env=VOLTA_DEPS={}", deps.join(";"));
}

fn main() {
    build_info();
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
