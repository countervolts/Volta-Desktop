//! Port of `src/shortcuts.ts`: the 32 shortcut definitions (generated from
//! the web source), persisted bindings (`volta-keyboard-shortcuts`), labels,
//! and matching by physical key code (`KeyboardEvent.code`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SHORTCUT_STORAGE_KEY: &str = "volta-keyboard-shortcuts";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub code: String,
    #[serde(default)]
    pub r#mod: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub alt: bool,
}

#[derive(Debug, Clone)]
pub struct Def {
    pub id: &'static str,
    pub category: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub default: Binding,
}

fn b(code: &str, r#mod: bool, shift: bool, alt: bool) -> Binding {
    Binding { code: code.to_string(), r#mod, shift, alt }
}

/// `SHORTCUT_DEFINITIONS`, in the web's order.
pub fn definitions() -> Vec<Def> {
    vec![
    Def { id: "playPause", category: "Playback", label: "Play or pause", description: "Toggle the current track.", default: b("Space", false, false, false) },
    Def { id: "previousTrack", category: "Playback", label: "Previous track", description: "Go back, or restart the current track when near the beginning.", default: b("BracketLeft", false, false, false) },
    Def { id: "nextTrack", category: "Playback", label: "Next track", description: "Play the next track in the queue.", default: b("BracketRight", false, false, false) },
    Def { id: "seekBackward", category: "Playback", label: "Seek backward", description: "Move back ten seconds.", default: b("KeyJ", false, false, false) },
    Def { id: "seekForward", category: "Playback", label: "Seek forward", description: "Move forward ten seconds.", default: b("KeyL", false, false, false) },
    Def { id: "volumeDown", category: "Playback", label: "Lower volume", description: "Lower volume by five percent.", default: b("Minus", false, false, false) },
    Def { id: "volumeUp", category: "Playback", label: "Raise volume", description: "Raise volume by five percent.", default: b("Equal", false, false, false) },
    Def { id: "mute", category: "Playback", label: "Mute or unmute", description: "Toggle audio mute.", default: b("KeyM", false, false, false) },
    Def { id: "shuffle", category: "Playback", label: "Toggle shuffle", description: "Turn shuffle on or off.", default: b("KeyS", false, false, false) },
    Def { id: "repeat", category: "Playback", label: "Cycle repeat", description: "Cycle off, repeat all, and repeat one.", default: b("KeyR", false, false, false) },
    Def { id: "favorite", category: "Playback", label: "Favorite current track", description: "Add or remove the current track from favorites.", default: b("KeyF", false, false, false) },
    Def { id: "focusSearch", category: "Search and navigation", label: "Search library", description: "Open library search and focus its field.", default: b("KeyK", true, false, false) },
    Def { id: "focusFilter", category: "Search and navigation", label: "Filter current view", description: "Focus the filter for the current library view.", default: b("Slash", false, false, false) },
    Def { id: "toggleSidebar", category: "Search and navigation", label: "Collapse or expand navigation", description: "Shrink the sidebar to an icon rail, or restore it. On a narrow window it opens or closes the navigation drawer.", default: b("KeyB", true, false, false) },
    Def { id: "goHome", category: "Search and navigation", label: "Go to Home", description: "Open the Home view.", default: b("Digit1", true, false, false) },
    Def { id: "goRecent", category: "Search and navigation", label: "Go to New in Your Library", description: "Open recently added albums.", default: b("Digit2", true, false, false) },
    Def { id: "goPlayed", category: "Search and navigation", label: "Go to Recently Played", description: "Open recently played albums.", default: b("Digit3", true, false, false) },
    Def { id: "goAlbums", category: "Search and navigation", label: "Go to Albums", description: "Open the album library.", default: b("Digit4", true, false, false) },
    Def { id: "goArtists", category: "Search and navigation", label: "Go to Artists", description: "Open the artist library.", default: b("Digit5", true, false, false) },
    Def { id: "goSongs", category: "Search and navigation", label: "Go to Songs", description: "Open the song library.", default: b("Digit6", true, false, false) },
    Def { id: "goFolders", category: "Search and navigation", label: "Go to Folders", description: "Browse the library by folder.", default: b("Digit7", true, false, false) },
    Def { id: "goFavorites", category: "Search and navigation", label: "Go to Favorite Songs", description: "Open your favorite tracks.", default: b("Digit8", true, false, false) },
    Def { id: "goPlaylists", category: "Search and navigation", label: "Go to Playlists", description: "Open your playlists.", default: b("Digit9", true, false, false) },
    Def { id: "goBack", category: "Search and navigation", label: "Go back", description: "Return to the previous library view.", default: b("ArrowLeft", true, false, false) },
    Def { id: "refresh", category: "Search and navigation", label: "Refresh current view", description: "Reload the current library data.", default: b("KeyR", true, false, false) },
    Def { id: "scrollTop", category: "Search and navigation", label: "Scroll to top", description: "Return the current library view to its top.", default: b("Home", false, false, false) },
    Def { id: "toggleQueue", category: "Player and panels", label: "Show or hide queue", description: "Open the Playing Next panel.", default: b("KeyQ", false, false, false) },
    Def { id: "toggleLyrics", category: "Player and panels", label: "Show or hide lyrics", description: "Open the lyrics panel.", default: b("KeyY", false, false, false) },
    Def { id: "openPlayer", category: "Player and panels", label: "Open Now Playing", description: "Expand the full-screen player.", default: b("KeyO", false, false, false) },
    Def { id: "closeOverlays", category: "Player and panels", label: "Close panel or player", description: "Close the full player, panel, sidebar, or menu.", default: b("Escape", false, false, false) },
    Def { id: "openSettings", category: "Application", label: "Open Settings", description: "Open Volta settings.", default: b("Comma", true, false, false) },
    Def { id: "openShortcuts", category: "Application", label: "Show keyboard shortcuts", description: "Open this searchable shortcut reference and editor.", default: b("Slash", false, true, false) },
    ]
}

/// `readShortcutBindings`: stored bindings by id, defaults for anything
/// missing or malformed. Returned in definition order.
pub fn read_bindings(raw: &str) -> Vec<(&'static str, Binding)> {
    let parsed: Option<Value> = serde_json::from_str(raw).ok();
    definitions()
        .into_iter()
        .map(|def| {
            let stored = parsed
                .as_ref()
                .and_then(|v| v.get(def.id))
                .filter(|c| c.get("code").and_then(Value::as_str).is_some())
                .map(|c| Binding {
                    code: c["code"].as_str().unwrap_or_default().to_string(),
                    r#mod: c.get("mod").and_then(Value::as_bool).unwrap_or(false),
                    shift: c.get("shift").and_then(Value::as_bool).unwrap_or(false),
                    alt: c.get("alt").and_then(Value::as_bool).unwrap_or(false),
                });
            (def.id, stored.unwrap_or(def.default))
        })
        .collect()
}

/// `CODE_LABELS` + `codeLabel`.
pub fn code_label(code: &str) -> String {
    let label = match code {
        "Space" => "Space",
        "ArrowLeft" => "←",
        "ArrowRight" => "→",
        "ArrowUp" => "↑",
        "ArrowDown" => "↓",
        "BracketLeft" => "[",
        "BracketRight" => "]",
        "Comma" => ",",
        "Period" => ".",
        "Slash" => "/",
        "Semicolon" => ";",
        "Quote" => "'",
        "Minus" => "−",
        "Equal" => "=",
        "Escape" => "Esc",
        "Enter" => "Enter",
        "Home" => "Home",
        "End" => "End",
        "PageUp" => "Page Up",
        "PageDown" => "Page Down",
        "Backspace" => "Backspace",
        _ => "",
    };
    if !label.is_empty() {
        return label.to_string();
    }
    if let Some(rest) = code.strip_prefix("Key") {
        return rest.to_uppercase();
    }
    if let Some(rest) = code.strip_prefix("Digit") {
        return rest.to_string();
    }
    if let Some(rest) = code.strip_prefix("Numpad") {
        return format!("Num {rest}");
    }
    code.to_string()
}

/// `formatShortcut`, with the platform's modifier label.
pub fn format_shortcut(binding: &Binding, mod_label: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if binding.r#mod {
        parts.push(mod_label.to_string());
    }
    if binding.shift {
        parts.push("Shift".into());
    }
    if binding.alt {
        parts.push("Alt".into());
    }
    parts.push(code_label(&binding.code));
    parts.join(" ")
}

/// `bindingSignature`, for conflict detection.
pub fn binding_signature(binding: &Binding) -> String {
    format!(
        "{}+{}+{}+{}",
        binding.code,
        if binding.r#mod { "mod" } else { "" },
        if binding.shift { "shift" } else { "" },
        if binding.alt { "alt" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thirty_two_definitions_with_web_defaults() {
        let defs = definitions();
        assert_eq!(defs.len(), 32);
        assert_eq!(defs[0].id, "playPause");
        assert_eq!(defs[0].default, b("Space", false, false, false));
        let search = defs.iter().find(|d| d.id == "focusSearch").unwrap();
        assert_eq!(format_shortcut(&search.default, "Super"), "Super K");
        let shortcuts = defs.iter().find(|d| d.id == "openShortcuts").unwrap();
        assert_eq!(format_shortcut(&shortcuts.default, "⌘"), "Shift /");
    }

    #[test]
    fn stored_bindings_override_and_bad_entries_fall_back() {
        let raw = r#"{"playPause": {"code": "KeyP", "mod": true}, "nextTrack": {"nope": 1}}"#;
        let bindings = read_bindings(raw);
        assert_eq!(bindings[0].1, b("KeyP", true, false, false));
        let next = bindings.iter().find(|(id, _)| *id == "nextTrack").unwrap();
        assert_eq!(next.1, b("BracketRight", false, false, false));
        assert_eq!(read_bindings("garbage").len(), 32);
        assert_eq!(code_label("Digit4"), "4");
        assert_eq!(code_label("Minus"), "−");
    }
}
