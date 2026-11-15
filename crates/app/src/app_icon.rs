//! The Volta app icon on each platform.
//!
//! - **Linux/Wayland**: compositors show a window's icon by matching its
//!   `app_id` (`xyz.voltamusic.Volta`) to a `.desktop` file whose `Icon=` names
//!   an icon-theme entry. Unpackaged builds register their own desktop entry
//!   and hicolor icons under `$XDG_DATA_HOME` on startup (rewritten only when
//!   the content changes, e.g. the binary moved). Set
//!   `VOLTA_NO_DESKTOP_ENTRY=1` to skip this, as packages that ship their own
//!   entry should.
//! - **Linux/X11**: the window also carries the icon pixels (`_NET_WM_ICON`).
//! - **Windows**: `build.rs` embeds `volta.ico` as icon resource 1, which gpui
//!   loads for the window class (taskbar, Alt+Tab, Explorer).
//! - **macOS**: the Dock icon comes from the `.app` bundle's `volta.icns`; see
//!   `[package.metadata.bundle]` in `Cargo.toml`. A bare binary run from a
//!   terminal shows the generic executable icon.

pub const APP_ID: &str = "xyz.voltamusic.Volta";

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
const ICONS: &[(u32, &[u8])] = &[
    (512, include_bytes!("../../../assets/app-icon/volta-512.png")),
    (256, include_bytes!("../../../assets/app-icon/volta-256.png")),
    (128, include_bytes!("../../../assets/app-icon/volta-128.png")),
    (64, include_bytes!("../../../assets/app-icon/volta-64.png")),
    (48, include_bytes!("../../../assets/app-icon/volta-48.png")),
    (32, include_bytes!("../../../assets/app-icon/volta-32.png")),
];

/// The window icon for X11 (ignored on Wayland, macOS and Windows).
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub fn window_icon() -> Option<std::sync::Arc<image::RgbaImage>> {
    let (_, bytes) = ICONS.iter().find(|(size, _)| *size == 128)?;
    image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok().map(|image| std::sync::Arc::new(image.into_rgba8()))
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub fn window_icon() -> Option<std::sync::Arc<image::RgbaImage>> {
    None
}

/// Registers the desktop entry and icons in the background (Linux only).
pub fn register_desktop_entry() {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        if std::env::var_os("VOLTA_NO_DESKTOP_ENTRY").is_some() || std::env::var_os("FLATPAK_ID").is_some() {
            return;
        }
        std::thread::spawn(|| {
            if let Err(error) = install_desktop_entry() {
                log::warn!("desktop entry: {error}");
            }
        });
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn install_desktop_entry() -> std::io::Result<()> {
    use std::path::PathBuf;

    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .ok_or_else(|| std::io::Error::other("no XDG_DATA_HOME or HOME"))?;

    // A system package already provides the entry and icons.
    let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    if data_dirs.split(':').any(|dir| PathBuf::from(dir).join(format!("applications/{APP_ID}.desktop")).exists()) {
        return Ok(());
    }
    install_into(&data_home, &std::env::current_exe()?)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn install_into(data_home: &std::path::Path, exe: &std::path::Path) -> std::io::Result<()> {
    let mut changed = false;
    for (size, bytes) in ICONS {
        let path = data_home.join(format!("icons/hicolor/{size}x{size}/apps/{APP_ID}.png"));
        changed |= write_if_changed(&path, bytes)?;
    }

    // Desktop Entry spec: quote the path, escaping `"`, `` ` ``, `$` and `\`.
    let mut quoted = String::from("\"");
    for c in exe.to_string_lossy().chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Volta\n\
         GenericName=Music Player\n\
         Comment=Listen to your Navidrome and local music\n\
         Exec={quoted}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=AudioVideo;Audio;Player;\n\
         StartupWMClass={APP_ID}\n\
         StartupNotify=true\n"
    );
    changed |= write_if_changed(&data_home.join(format!("applications/{APP_ID}.desktop")), entry.as_bytes())?;

    if changed {
        // Desktop shells watch these directories; no cache rebuild needed
        // (writing a user icon-theme.cache would go stale for other apps).
        log::info!("registered desktop entry {APP_ID}");
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn write_if_changed(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<bool> {
    if std::fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Write-then-rename so a desktop shell never reads a half-written file.
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(true)
}

#[cfg(all(test, any(target_os = "linux", target_os = "freebsd")))]
mod tests {
    use super::*;

    #[test]
    fn installs_entry_and_icons() {
        let dir = std::env::temp_dir().join(format!("volta-icon-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        install_into(&dir, std::path::Path::new("/opt/My $App/volta")).unwrap();
        let entry = std::fs::read_to_string(dir.join(format!("applications/{APP_ID}.desktop"))).unwrap();
        assert!(entry.contains("Exec=\"/opt/My \\$App/volta\"\n"), "{entry}");
        assert!(entry.contains(&format!("Icon={APP_ID}\n")));
        assert!(dir.join(format!("icons/hicolor/256x256/apps/{APP_ID}.png")).exists());
        assert!(window_icon().is_some_and(|icon| icon.width() == 128));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
