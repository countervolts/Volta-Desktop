//! Display and audio helpers from `src/lib/navidrome.ts` and `player-ui.tsx`.

use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

use crate::model::{AlbumRecord, Song};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Normalization {
    #[default]
    Off,
    Track,
    Album,
}

impl Normalization {
    pub fn from_stored(raw: &str) -> Self {
        match raw {
            "track" => Self::Track,
            "album" => Self::Album,
            _ => Self::Off,
        }
    }
}

/// `duration(seconds)`: `m:ss`.
pub fn duration(seconds: Option<f64>) -> String {
    let value = match seconds {
        Some(s) if s.is_finite() => s.floor().max(0.0) as u64,
        _ => 0,
    };
    format!("{}:{:02}", value / 60, value % 60)
}

pub fn album_name(album: &AlbumRecord) -> &str {
    album
        .name
        .as_deref()
        .filter(|s| !s.is_empty())
        .or(album.title.as_deref().filter(|s| !s.is_empty()))
        .unwrap_or("Untitled album")
}

static NON_ALNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}]+").unwrap());
static ARTIST_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?s)(.*?)(?:\s+[-–—]\s*|\s*[:;|]\s*)(.+)$").unwrap());

fn normalize_local_album_label(value: &str) -> String {
    let folded: String = value.nfkd().collect::<String>().to_lowercase().replace('$', "s");
    NON_ALNUM.replace_all(&folded, "").into_owned()
}

/// `localAlbumName`: strips up to four repeated `Artist - ` prefixes.
pub fn local_album_name(album: &str, artist: Option<&str>) -> String {
    let value = album.trim();
    let normalized_artist = normalize_local_album_label(artist.unwrap_or(""));
    if value.is_empty() || normalized_artist.is_empty() {
        return value.to_string();
    }
    let strip_one = |current: &str| -> String {
        let Some(captures) = ARTIST_PREFIX.captures(current) else {
            return current.to_string();
        };
        if normalize_local_album_label(&captures[1]) != normalized_artist {
            return current.to_string();
        }
        let rest = captures[2].trim();
        if rest.is_empty() { current.to_string() } else { rest.to_string() }
    };
    let mut result = value.to_string();
    for _ in 0..4 {
        let stripped = strip_one(&result);
        if stripped == result {
            break;
        }
        result = stripped;
    }
    result
}

/// Source metadata only; this does not promise a bit-perfect output path.
pub fn is_lossless(song: Option<&Song>) -> bool {
    let Some(song) = song else { return false };
    let extension = song
        .suffix
        .as_deref()
        .map(|s| s.to_lowercase().trim_start_matches('.').to_string())
        .unwrap_or_default();
    let codec = song.codec.as_deref().map(|c| c.trim().to_lowercase()).unwrap_or_default();
    let content_type = song
        .content_type
        .as_deref()
        .map(|c| c.to_lowercase().split(';').next().unwrap_or("").trim().to_string())
        .unwrap_or_default();
    let lossless_m4a = matches!(extension.as_str(), "m4a" | "mp4")
        && song.bit_depth.is_some_and(|d| d.is_finite() && d > 0.0);
    matches!(codec.as_str(), "alac" | "flac" | "wav" | "wave" | "aif" | "aiff")
        || matches!(extension.as_str(), "flac" | "alac" | "wav" | "wave" | "aif" | "aiff")
        || lossless_m4a
        || matches!(
            content_type.as_str(),
            "audio/flac"
                | "audio/x-flac"
                | "audio/alac"
                | "audio/x-alac"
                | "audio/wav"
                | "audio/wave"
                | "audio/x-wav"
                | "audio/vnd.wave"
                | "audio/aiff"
                | "audio/x-aiff"
        )
}

/// `quality(song, activeStream)` in player-ui.tsx.
pub fn quality_label(song: Option<&Song>, compatible_stream: bool) -> &'static str {
    if compatible_stream {
        return "MP3 320 kbps";
    }
    let Some(s) = song else { return "Original Quality" };
    if !is_lossless(Some(s)) {
        return "Original";
    }
    if s.bit_depth.unwrap_or(0.0) > 16.0 || s.sampling_rate.unwrap_or(0.0) > 48000.0 {
        "Hi-Res Lossless"
    } else {
        "Lossless"
    }
}

/// Linear gain for a ReplayGain value in dB, limited so peaks cannot clip.
pub fn replay_gain_factor(song: Option<&Song>, mode: Normalization) -> f64 {
    let Some(song) = song else { return 1.0 };
    let Some(rg) = song.replay_gain.as_ref() else { return 1.0 };
    let (gain, peak) = match mode {
        Normalization::Off => return 1.0,
        Normalization::Track => (rg.track_gain, rg.track_peak),
        Normalization::Album => (rg.album_gain, rg.album_peak),
    };
    let Some(gain) = gain.filter(|g| g.is_finite() && *g != 0.0) else {
        return 1.0;
    };
    let mut factor = 10f64.powf(gain / 20.0);
    if let Some(peak) = peak.filter(|p| p.is_finite())
        && peak * factor > 1.0 {
            factor = 1.0 / peak;
        }
    factor.clamp(0.0, 4.0)
}

/// `formatBytes` (folder-tree.ts): 1024 steps, one decimal above bytes.
pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "—".into();
    }
    let units = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let rounded = if unit == 0 { value.round() } else { (value * 10.0).round() / 10.0 };
    format!("{} {}", if rounded.fract() == 0.0 { format!("{}", rounded as i64) } else { format!("{rounded}") }, units[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ReplayGain;

    #[test]
    fn duration_formats() {
        assert_eq!(duration(Some(0.0)), "0:00");
        assert_eq!(duration(Some(61.9)), "1:01");
        assert_eq!(duration(Some(-3.0)), "0:00");
        assert_eq!(duration(None), "0:00");
        assert_eq!(duration(Some(f64::NAN)), "0:00");
        assert_eq!(duration(Some(3600.0)), "60:00");
    }

    #[test]
    fn local_album_name_strips_artist_prefixes() {
        assert_eq!(local_album_name("Artist - Album", Some("Artist")), "Album");
        assert_eq!(local_album_name("$uicideBoy$ - Long Term", Some("SuicideBoy$")), "Long Term");
        assert_eq!(local_album_name("Jay-Z Album", Some("Jay")), "Jay-Z Album");
        assert_eq!(local_album_name("A: A - B", Some("A")), "B");
        assert_eq!(local_album_name("Album", None), "Album");
    }

    #[test]
    fn lossless_detection() {
        let flac = Song { suffix: Some("FLAC".into()), ..Default::default() };
        assert!(is_lossless(Some(&flac)));
        let alac = Song { suffix: Some("m4a".into()), bit_depth: Some(16.0), ..Default::default() };
        assert!(is_lossless(Some(&alac)));
        let aac = Song { suffix: Some("m4a".into()), ..Default::default() };
        assert!(!is_lossless(Some(&aac)));
        let hires = Song { suffix: Some("flac".into()), bit_depth: Some(24.0), ..Default::default() };
        assert_eq!(quality_label(Some(&hires), false), "Hi-Res Lossless");
        assert_eq!(quality_label(Some(&hires), true), "MP3 320 kbps");
    }

    #[test]
    fn replay_gain_limits_peaks() {
        let song = Song {
            replay_gain: Some(ReplayGain {
                track_gain: Some(6.0),
                track_peak: Some(0.9),
                album_gain: Some(-6.0),
                album_peak: None,
            }),
            ..Default::default()
        };
        assert!((replay_gain_factor(Some(&song), Normalization::Track) - 1.0 / 0.9).abs() < 1e-12);
        assert!((replay_gain_factor(Some(&song), Normalization::Album) - 10f64.powf(-0.3)).abs() < 1e-12);
        assert_eq!(replay_gain_factor(Some(&song), Normalization::Off), 1.0);
    }
}
