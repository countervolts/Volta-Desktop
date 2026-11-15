//! Keyboard shortcuts: the window `keydown` handler from App.tsx.
//!
//! gpui reports logical keys; the web matches physical `KeyboardEvent.code`.
//! For the keys the bindings use, the unshifted key maps 1:1 to a code.

use gpui::{App, Entity, Keystroke, Window};
use volta_logic::shortcuts::Binding;

use crate::app_state::{AppState, Page, Panel, Route};

/// `KeyboardEvent.code` for a gpui key name.
pub fn key_code(key: &str) -> Option<String> {
    let code = match key {
        "space" => "Space",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "[" => "BracketLeft",
        "]" => "BracketRight",
        "," => "Comma",
        "." => "Period",
        "/" | "?" => "Slash",
        ";" => "Semicolon",
        "'" => "Quote",
        "-" => "Minus",
        "=" | "+" => "Equal",
        "escape" => "Escape",
        "enter" => "Enter",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "backspace" => "Backspace",
        "\\" => "Backslash",
        "`" => "Backquote",
        "tab" => "Tab",
        _ => "",
    };
    if !code.is_empty() {
        return Some(code.to_string());
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Some(format!("Key{}", c.to_ascii_uppercase())),
        (Some(c), None) if c.is_ascii_digit() => Some(format!("Digit{c}")),
        _ if key.len() > 1 && key.starts_with('f') && key[1..].chars().all(|c| c.is_ascii_digit()) => {
            Some(key.to_uppercase())
        }
        _ => None,
    }
}

/// `bindingFromKeyboardEvent`: mod is Ctrl or Meta.
pub fn binding_from(keystroke: &Keystroke) -> Option<Binding> {
    let m = &keystroke.modifiers;
    Some(Binding {
        code: key_code(&keystroke.key)?,
        r#mod: m.control || m.platform,
        shift: m.shift,
        alt: m.alt,
    })
}

/// Runs the shortcut bound to `keystroke`. `typing` is true while a text
/// field has focus: then only shortcuts with the modifier fire (the web's
/// input guard). Returns whether a shortcut ran.
pub fn handle(state: &Entity<AppState>, keystroke: &Keystroke, typing: bool, window: &mut Window, cx: &mut App) -> bool {
    let Some(pressed) = binding_from(keystroke) else { return false };
    // The scale preview owns ← / → (Shift: 10 %) and Escape.
    if state.read(cx).scale_preview.is_some() {
        let step = if keystroke.modifiers.shift { 10.0 } else { 1.0 };
        match keystroke.key.as_str() {
            "left" => crate::components::scale_preview::adjust(state, -step, cx),
            "right" => crate::components::scale_preview::adjust(state, step, cx),
            "escape" => crate::components::scale_preview::cancel(state, cx),
            "enter" => crate::components::scale_preview::keep(state, cx),
            _ => return false,
        }
        return true;
    }
    if state.read(cx).recording_shortcut.is_some() {
        state.update(cx, |s, cx| s.record_shortcut(pressed, cx));
        return true;
    }
    let bindings = state.read(cx).shortcuts.clone();
    let Some((id, binding)) = bindings.iter().find(|(_, b)| *b == pressed) else { return false };
    // Menus and dialogs keep their keys; Escape still closes them.
    let overlay = state.read(cx).menu.is_some() || state.read(cx).dialog.is_some();
    if overlay && *id != "closeOverlays" {
        return false;
    }
    if typing && !binding.r#mod {
        return false;
    }
    let player = state.read(cx).player.clone();
    let go = |page: Page, cx: &mut App| state.update(cx, |s, cx| s.navigate(Route::page(page), cx));
    match *id {
        "playPause" => player.update(cx, |p, cx| p.toggle(cx)),
        "previousTrack" => player.update(cx, |p, cx| p.previous(cx)),
        "nextTrack" => player.update(cx, |p, cx| p.next(cx)),
        "seekBackward" => player.update(cx, |p, cx| {
            let t = p.position - 10.0;
            p.seek(t, cx)
        }),
        "seekForward" => player.update(cx, |p, cx| {
            let t = p.position + 10.0;
            p.seek(t, cx)
        }),
        "volumeDown" => player.update(cx, |p, cx| {
            let v = p.volume - 0.05;
            p.set_volume(v, cx)
        }),
        "volumeUp" => player.update(cx, |p, cx| {
            let v = p.volume + 0.05;
            p.set_volume(v, cx)
        }),
        "mute" => player.update(cx, |p, cx| p.toggle_mute(cx)),
        "shuffle" => player.update(cx, |p, cx| p.toggle_shuffle(cx)),
        "repeat" => player.update(cx, |p, cx| p.cycle_repeat(cx)),
        "favorite" => {
            if let Some(song) = player.read(cx).current_song().cloned() {
                state.update(cx, |s, cx| s.toggle_song_favorite(&song, cx));
            }
        }
        "focusSearch" => state.update(cx, |s, cx| {
            s.mobile_sidebar = true;
            s.navigate(Route::page(Page::Search), cx);
            s.focus_request = Some(crate::app_state::FocusTarget::Search);
        }),
        "focusFilter" => state.update(cx, |s, cx| {
            s.focus_request = Some(crate::app_state::FocusTarget::Filter);
            cx.notify();
        }),
        "toggleSidebar" => state.update(cx, |s, cx| {
            if crate::theme::viewport_css_width(window) <= 640.0 {
                s.mobile_sidebar = !s.mobile_sidebar;
                cx.notify();
            } else {
                s.toggle_sidebar(cx);
            }
        }),
        "goHome" => go(Page::Home, cx),
        "goRecent" => go(Page::Recent, cx),
        "goPlayed" => go(Page::Played, cx),
        "goAlbums" => go(Page::Albums, cx),
        "goArtists" => go(Page::Artists, cx),
        "goSongs" => go(Page::Songs, cx),
        "goFolders" => go(Page::Folders, cx),
        "goFavorites" => go(Page::Favorites, cx),
        "goPlaylists" => go(Page::Playlists, cx),
        "goBack" => state.update(cx, |s, cx| s.back(cx)),
        "refresh" => state.update(cx, |s, cx| s.refresh(cx)),
        "scrollTop" => state.update(cx, |s, cx| {
            s.scroll_top_request = true;
            cx.notify();
        }),
        "toggleQueue" => state.update(cx, |s, cx| s.toggle_panel(Panel::Queue, cx)),
        "toggleLyrics" => state.update(cx, |s, cx| s.toggle_panel(Panel::Lyrics, cx)),
        "openPlayer" => state.update(cx, |s, cx| {
            if s.player.read(cx).current_song().is_some() {
                s.full_player_open = true;
                cx.notify();
            }
        }),
        "openSettings" => state.update(cx, |s, cx| {
            s.dialog = None;
            s.navigate(Route::page(Page::Settings), cx);
        }),
        "openShortcuts" => state.update(cx, |s, cx| {
            s.dialog = Some(crate::app_state::Dialog::Shortcuts);
            cx.notify();
        }),
        "closeOverlays" => state.update(cx, |s, cx| {
            s.full_player_open = false;
            s.panel = None;
            s.mobile_sidebar = false;
            s.menu = None;
            s.open_select = None;
            s.dialog = None;
            cx.notify();
        }),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_web_codes() {
        assert_eq!(key_code("space").as_deref(), Some("Space"));
        assert_eq!(key_code("k").as_deref(), Some("KeyK"));
        assert_eq!(key_code("4").as_deref(), Some("Digit4"));
        assert_eq!(key_code("[").as_deref(), Some("BracketLeft"));
        assert_eq!(key_code("escape").as_deref(), Some("Escape"));
        assert_eq!(key_code("f5").as_deref(), Some("F5"));
        assert_eq!(key_code("shift"), None);
    }
}
