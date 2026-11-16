//! Persisted key names and value validators. Port of `src/app/app-storage.ts`
//! (plus the volume reader from `App.tsx`).
//!
//! Every reader takes the raw stored string (what `localStorage.getItem`
//! returned, or `""` when absent) so the same functions work on the SQLite
//! `kv` table and on the in-memory session store.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::js::{encode_uri_component, js_number, js_round};

pub const LOCAL_FAVORITES_KEY: &str = "volta-local-favorites";
pub const LOCAL_ALBUM_FAVORITES_KEY: &str = "volta-local-album-favorites";
pub const LOCAL_ARTIST_FAVORITES_KEY: &str = "volta-local-artist-favorites";
pub const REMEMBERED_CREDENTIALS_KEY: &str = "volta-remembered-credentials";
/// Session-only (sessionStorage on the web).
pub const SESSION_CREDENTIALS_KEY: &str = "volta-session-credentials";
pub const INTERFACE_SCALE_KEY: &str = "volta-interface-scale";
pub const EXTERNAL_LYRICS_KEY: &str = "volta-external-lyrics";
pub const LYRICS_BLUR_KEY: &str = "volta-lyrics-blur";
pub const INFINITE_PLAY_COUNT_KEY: &str = "volta-infinite-play-count";
pub const INFINITE_PLAY_MODE_KEY: &str = "volta-infinite-play-mode";
pub const INFINITE_PLAY_ENABLED_KEY: &str = "volta-infinite-play-enabled";
pub const LISTENING_HISTORY_ENABLED_KEY: &str = "volta-listening-history-enabled";
pub const LISTENING_HISTORY_PERSIST_KEY: &str = "volta-listening-history-persist";
pub const CROSSFADE_KEY: &str = "volta-crossfade-seconds";
pub const VOLUME_SCROLL_STEP_KEY: &str = "volta-volume-scroll-step";
pub const VOLUME_SHIFT_SCROLL_STEP_KEY: &str = "volta-volume-shift-scroll-step";
pub const TRANSITION_MODE_KEY: &str = "volta-transition-mode";
pub const NORMALIZATION_KEY: &str = "volta-normalization";
pub const PINS_KEY: &str = "volta-pins";
pub const ACCOUNTS_KEY: &str = "volta-accounts";
/// Session-only (sessionStorage on the web).
pub const LOGIN_DRAFT_KEY: &str = "volta-login-draft";
pub const LOCAL_SOURCE_MODE_KEY: &str = "volta-source-mode";
pub const SEARCH_HISTORY_KEY: &str = "volta-search-history";
pub const SHARE_PROVIDER_KEY: &str = "volta-share-provider";
pub const FRAME_MONITOR_KEY: &str = "volta-frame-monitor";
/// Native-only: "true" opens the developer console window at launch.
pub const CONSOLE_AT_STARTUP_KEY: &str = "volta-console-at-startup";
/// Native-only: "false" turns the daily release check off.
pub const UPDATE_CHECK_KEY: &str = "volta-update-check";
pub const UPDATE_SKIPPED_KEY: &str = "volta-update-skipped-version";
pub const UPDATE_LAST_CHECK_KEY: &str = "volta-update-last-check";
pub const REWARD_WINDOW_MS: u64 = 6 * 60 * 60 * 1000;

// Keys written inline in App.tsx.
pub const THEME_KEY: &str = "volta-theme";
pub const SIDEBAR_COLLAPSED_KEY: &str = "volta-sidebar-collapsed";
pub const FLOATING_SIDEBAR_KEY: &str = "volta-floating-sidebar";
pub const SEPARATE_PINNED_KEY: &str = "volta-separate-pinned-entities";
pub const SERVER_KEY: &str = "volta-server";
pub const USERNAME_KEY: &str = "volta-username";
pub const REMEMBER_ME_KEY: &str = "volta-remember-me";
pub const WARN_BEFORE_LEAVE_KEY: &str = "volta-warn-before-leave";
pub const KEYBOARD_SHORTCUTS_KEY: &str = "volta-keyboard-shortcuts";
pub const ANIMATED_ARTWORK_KEY: &str = "volta-animated-artwork";
pub const ANIMATED_ARTWORK_MODE_KEY: &str = "volta-animated-artwork-mode";

pub const MAX_ACCOUNTS: usize = 12;
pub const DEFAULT_VOLUME: f64 = 0.8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMaterial {
    pub salt: String,
    pub token: String,
}

/// Same shape for `SavedAccount` and `StoredCredentials`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCredentials {
    pub server: String,
    pub username: String,
    pub auth: AuthMaterial,
}

pub type SavedAccount = StoredCredentials;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginDraft {
    pub server: String,
    pub username: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PinKind {
    Album,
    Artist,
    Playlist,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
    pub kind: PinKind,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionMode {
    Off,
    Crossfade,
    Automix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareProvider {
    AppleMusic,
    Spotify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemePreference {
    #[default]
    Dark,
    Light,
    HighContrast,
    System,
}

impl ThemePreference {
    /// `safeRead(localStorage, "volta-theme") || "dark"`. Unknown values fall
    /// back to dark, which is what the CSS does for an unmatched `data-theme`.
    pub fn from_stored(raw: &str) -> Self {
        match raw {
            "light" => Self::Light,
            "high-contrast" => Self::HighContrast,
            "system" => Self::System,
            _ => Self::Dark,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::HighContrast => "high-contrast",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumePreference {
    pub volume: f64,
    pub muted: bool,
    pub previous_volume: f64,
}

impl Default for VolumePreference {
    fn default() -> Self {
        Self {
            volume: DEFAULT_VOLUME,
            muted: false,
            previous_volume: DEFAULT_VOLUME,
        }
    }
}

fn non_empty_str(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| !s.is_empty())
}

fn credentials_from_value(value: &Value) -> Option<StoredCredentials> {
    let server = value.get("server")?.as_str()?;
    let username = value.get("username")?.as_str()?;
    let auth = value.get("auth")?;
    let salt = non_empty_str(auth.get("salt")?)?;
    let token = non_empty_str(auth.get("token")?)?;
    Some(StoredCredentials {
        server: server.to_string(),
        username: username.to_string(),
        auth: AuthMaterial {
            salt: salt.to_string(),
            token: token.to_string(),
        },
    })
}

/// `readStoredCredentials`
pub fn read_stored_credentials(raw: &str) -> Option<StoredCredentials> {
    if raw.is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(raw).ok()?;
    credentials_from_value(&value)
}

/// `readAccounts`
pub fn read_accounts(raw: &str) -> Vec<SavedAccount> {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Array(items)) => items.iter().filter_map(credentials_from_value).collect(),
        _ => Vec::new(),
    }
}

/// `writeAccounts`: the stored list is capped at 12.
pub fn write_accounts(accounts: &[SavedAccount]) -> String {
    let capped = &accounts[..accounts.len().min(MAX_ACCOUNTS)];
    serde_json::to_string(capped).unwrap_or_else(|_| "[]".into())
}

/// `accountKey`
pub fn account_key(server: &str, username: &str) -> String {
    format!("{server}\u{1f}{username}")
}

/// `rememberAccount` in App.tsx: newest first, deduped, capped at 12.
pub fn remember_account(current: &[SavedAccount], account: SavedAccount) -> Vec<SavedAccount> {
    let key = account_key(&account.server, &account.username);
    let mut next = vec![account];
    next.extend(
        current
            .iter()
            .filter(|item| account_key(&item.server, &item.username) != key)
            .cloned(),
    );
    next.truncate(MAX_ACCOUNTS);
    next
}

/// `forgetAccount` in App.tsx.
pub fn forget_account(current: &[SavedAccount], server: &str, username: &str) -> Vec<SavedAccount> {
    let key = account_key(server, username);
    current
        .iter()
        .filter(|item| account_key(&item.server, &item.username) != key)
        .cloned()
        .collect()
}

/// `readLoginDraft`. Returns the draft and the value to write back
/// (`None` means remove the key).
pub fn read_login_draft(raw: &str) -> (LoginDraft, Option<String>) {
    match serde_json::from_str::<Value>(raw) {
        Ok(value) if !value.is_null() => {
            let draft = LoginDraft {
                server: value
                    .get("server")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                username: value
                    .get("username")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            };
            let written = serde_json::to_string(&draft).ok();
            (draft, written)
        }
        // JSON.parse("") throws; `null.server` throws as well.
        _ => (
            LoginDraft {
                server: String::new(),
                username: String::new(),
            },
            None,
        ),
    }
}

pub fn pins_storage_key(server: &str, username: &str) -> String {
    format!(
        "{PINS_KEY}:{}:{}",
        encode_uri_component(server),
        encode_uri_component(username)
    )
}

/// `readPins`
pub fn read_pins(raw: &str) -> Vec<Pin> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|pin| {
            let id = pin.get("id")?.as_str()?;
            let name = pin.get("name")?.as_str()?;
            let kind = match pin.get("kind")?.as_str()? {
                "album" => PinKind::Album,
                "artist" => PinKind::Artist,
                "playlist" => PinKind::Playlist,
                _ => return None,
            };
            Some(Pin {
                kind,
                id: id.to_string(),
                name: name.to_string(),
                cover_art: pin.get("coverArt").and_then(Value::as_str).map(str::to_string),
                image_url: pin.get("imageUrl").and_then(Value::as_str).map(str::to_string),
            })
        })
        .collect()
}

pub fn pin_key(pin: &Pin) -> String {
    let kind = match pin.kind {
        PinKind::Album => "album",
        PinKind::Artist => "artist",
        PinKind::Playlist => "playlist",
    };
    format!("{kind}:{}", pin.id)
}

/// `clampInterfaceScale`
pub fn clamp_interface_scale(value: f64) -> f64 {
    js_round(value).clamp(70.0, 150.0)
}

/// `readInterfaceScale`: percent, 70–150, default 100.
pub fn read_interface_scale(raw: &str) -> f64 {
    let value = js_number(raw);
    if value.is_finite() && (70.0..=150.0).contains(&value) {
        clamp_interface_scale(value)
    } else {
        100.0
    }
}

/// `readTransitionMode`, including the legacy crossfade migration.
pub fn read_transition_mode(mode_raw: &str, crossfade_raw: &str) -> TransitionMode {
    match mode_raw {
        "automix" => return TransitionMode::Automix,
        "crossfade" => return TransitionMode::Crossfade,
        "off" => return TransitionMode::Off,
        _ => {}
    }
    let seconds = js_number(crossfade_raw);
    if seconds.is_finite() && seconds > 0.0 {
        TransitionMode::Crossfade
    } else {
        TransitionMode::Off
    }
}

/// `readSearchHistory`: string entries only, at most 8.
pub fn read_search_history(raw: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .map(|items| items.into_iter().filter_map(|i| i.as_str().map(str::to_string)).take(8).collect())
        .unwrap_or_default()
}

/// `rememberSearch`: newest first, case-insensitive dedupe, capped at 8.
pub fn remember_search(history: &[String], value: &str) -> Vec<String> {
    let next = value.trim();
    if next.is_empty() {
        return history.to_vec();
    }
    let lower = next.to_lowercase();
    std::iter::once(next.to_string())
        .chain(history.iter().filter(|item| item.to_lowercase() != lower).cloned())
        .take(8)
        .collect()
}

pub fn read_share_provider(raw: &str) -> ShareProvider {
    if raw == "spotify" {
        ShareProvider::Spotify
    } else {
        ShareProvider::AppleMusic
    }
}

/// `readLocalFavoriteSet`: keeps keys with the prefix whose value is `true`.
pub fn read_local_favorite_set(raw: &str, prefix: &str) -> Vec<String> {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(map)) => map
            .iter()
            .filter(|(key, value)| key.starts_with(prefix) && **value == Value::Bool(true))
            .map(|(key, _)| key.clone())
            .collect(),
        _ => Vec::new(),
    }
}

pub fn library_path_key(server: &str, username: &str, suffix: &str) -> String {
    format!(
        "volta-private-path:{}:{}:{suffix}",
        encode_uri_component(server),
        encode_uri_component(username)
    )
}

pub fn local_library_path_key(suffix: &str) -> String {
    library_path_key("local", "local-library", suffix)
}

pub fn last_played_song_key(server: &str, username: &str) -> String {
    format!(
        "volta-last-played:{}:{}",
        encode_uri_component(server),
        encode_uri_component(username)
    )
}

pub fn volume_key(server: &str, username: &str) -> String {
    format!(
        "volta-volume:{}:{}",
        encode_uri_component(server),
        encode_uri_component(username)
    )
}

pub fn local_playback_key(directory_name: &str) -> String {
    format!("volta-local-last-played:{}", encode_uri_component(directory_name))
}

pub fn local_volume_key(directory_name: &str) -> String {
    format!("volta-local-volume:{}", encode_uri_component(directory_name))
}

/// `readStoredVolume` in App.tsx, including the legacy bare-number format.
pub fn read_stored_volume(raw: &str) -> Option<VolumePreference> {
    if raw.is_empty() {
        return None;
    }
    let legacy = js_number(raw);
    if legacy.is_finite() && (0.0..=1.0).contains(&legacy) {
        return Some(VolumePreference {
            volume: legacy,
            muted: legacy == 0.0,
            previous_volume: if legacy != 0.0 { legacy } else { DEFAULT_VOLUME },
        });
    }
    let value: Value = serde_json::from_str(raw).ok()?;
    if value.is_null() {
        return None;
    }
    let volume = json_number(value.get("volume"));
    if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
        return None;
    }
    let previous = json_number(value.get("previousVolume"));
    Some(VolumePreference {
        volume,
        muted: truthy(value.get("muted")) || volume == 0.0,
        previous_volume: if previous.is_finite() && previous > 0.0 && previous <= 1.0 {
            previous
        } else if volume != 0.0 {
            volume
        } else {
            DEFAULT_VOLUME
        },
    })
}

pub fn write_volume(preference: &VolumePreference) -> String {
    serde_json::to_string(preference).unwrap_or_default()
}

/// `Number(value)` for a JSON value.
fn json_number(value: Option<&Value>) -> f64 {
    match value {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => js_number(s),
        Some(Value::Array(a)) if a.is_empty() => 0.0,
        Some(_) => f64::NAN,
    }
}

/// `Boolean(value)` for a JSON value.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds(server: &str, user: &str) -> SavedAccount {
        StoredCredentials {
            server: server.into(),
            username: user.into(),
            auth: AuthMaterial {
                salt: "s".into(),
                token: "t".into(),
            },
        }
    }

    #[test]
    fn credentials_require_non_empty_auth() {
        assert!(read_stored_credentials("").is_none());
        assert!(read_stored_credentials("not json").is_none());
        assert!(
            read_stored_credentials(r#"{"server":"a","username":"b","auth":{"salt":"","token":"t"}}"#)
                .is_none()
        );
        assert!(read_stored_credentials(r#"{"server":"a","username":"b"}"#).is_none());
        let ok = read_stored_credentials(
            r#"{"server":"https://music.test","username":"Listener","auth":{"salt":"s","token":"t"},"extra":1}"#,
        )
        .unwrap();
        assert_eq!(ok, creds("https://music.test", "Listener"));
    }

    #[test]
    fn accounts_filter_invalid_and_cap_at_twelve() {
        let raw = r#"[{"server":"a","username":"b","auth":{"salt":"s","token":"t"}},
            {"server":"a","username":2,"auth":{"salt":"s","token":"t"}}, null, 5]"#;
        assert_eq!(read_accounts(raw), vec![creds("a", "b")]);
        assert!(read_accounts("{}").is_empty());
        let many: Vec<_> = (0..20).map(|i| creds("s", &i.to_string())).collect();
        assert_eq!(read_accounts(&write_accounts(&many)).len(), 12);
    }

    #[test]
    fn remember_and_forget_accounts() {
        let list = vec![creds("a", "1"), creds("b", "2")];
        let next = remember_account(&list, creds("b", "2"));
        assert_eq!(next, vec![creds("b", "2"), creds("a", "1")]);
        assert_eq!(forget_account(&next, "a", "1"), vec![creds("b", "2")]);
    }

    #[test]
    fn pins_validate_kind_and_optional_artwork() {
        let raw = r#"[{"kind":"album","id":"1","name":"A","coverArt":"c","imageUrl":3},
            {"kind":"song","id":"2","name":"B"},{"kind":"artist","id":"3"}]"#;
        let pins = read_pins(raw);
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].cover_art.as_deref(), Some("c"));
        assert_eq!(pins[0].image_url, None);
        assert_eq!(pin_key(&pins[0]), "album:1");
    }

    #[test]
    fn interface_scale_defaults_and_clamps() {
        assert_eq!(read_interface_scale(""), 100.0);
        assert_eq!(read_interface_scale("69"), 100.0);
        assert_eq!(read_interface_scale("151"), 100.0);
        assert_eq!(read_interface_scale("124.5"), 125.0);
        assert_eq!(read_interface_scale("abc"), 100.0);
        assert_eq!(clamp_interface_scale(200.0), 150.0);
        assert_eq!(clamp_interface_scale(10.0), 70.0);
    }

    #[test]
    fn transition_mode_migrates_legacy_crossfade() {
        assert_eq!(read_transition_mode("automix", "0"), TransitionMode::Automix);
        assert_eq!(read_transition_mode("", "6"), TransitionMode::Crossfade);
        assert_eq!(read_transition_mode("", "0"), TransitionMode::Off);
        assert_eq!(read_transition_mode("bogus", ""), TransitionMode::Off);
    }

    #[test]
    fn per_account_keys_match_web_encoding() {
        assert_eq!(
            pins_storage_key("https://music.test", "Listener"),
            "volta-pins:https%3A%2F%2Fmusic.test:Listener"
        );
        assert_eq!(
            volume_key("https://music.test", "a b"),
            "volta-volume:https%3A%2F%2Fmusic.test:a%20b"
        );
        assert_eq!(
            local_library_path_key("number"),
            "volta-private-path:local:local-library:number"
        );
    }

    #[test]
    fn login_draft_normalizes() {
        let (draft, write) = read_login_draft(r#"{"server":"x","username":5}"#);
        assert_eq!(draft.server, "x");
        assert_eq!(draft.username, "");
        assert_eq!(write.as_deref(), Some(r#"{"server":"x","username":""}"#));
        assert_eq!(read_login_draft("").1, None);
    }

    #[test]
    fn volume_reads_legacy_and_json() {
        assert_eq!(read_stored_volume(""), None);
        assert_eq!(
            read_stored_volume("0"),
            Some(VolumePreference { volume: 0.0, muted: true, previous_volume: 0.8 })
        );
        assert_eq!(
            read_stored_volume(r#"{"volume":0.5,"muted":false,"previousVolume":2}"#),
            Some(VolumePreference { volume: 0.5, muted: false, previous_volume: 0.5 })
        );
        assert_eq!(read_stored_volume(r#"{"volume":1.5}"#), None);
    }

    #[test]
    fn favorites_and_share_provider() {
        let mut set = read_local_favorite_set(r#"{"local:a":true,"local:b":false,"x:c":true}"#, "local:");
        set.sort();
        assert_eq!(set, vec!["local:a"]);
        assert_eq!(read_share_provider("spotify"), ShareProvider::Spotify);
        assert_eq!(read_share_provider(""), ShareProvider::AppleMusic);
    }

    #[test]
    fn search_history_reads_and_remembers() {
        assert_eq!(read_search_history(r#"["a", 3, "b"]"#), vec!["a", "b"]);
        assert!(read_search_history("nope").is_empty());
        let many: Vec<String> = (0..10).map(|i| format!("q{i}")).collect();
        assert_eq!(read_search_history(&serde_json::to_string(&many).unwrap()).len(), 8);
        let history = vec!["Jazz".to_string(), "blue".to_string()];
        assert_eq!(remember_search(&history, " BLUE "), vec!["BLUE", "Jazz"]);
        assert_eq!(remember_search(&history, "  "), history);
        assert_eq!(remember_search(&many, "new").len(), 8);
    }
}
