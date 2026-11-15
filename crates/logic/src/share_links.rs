//! Port of `src/lib/share-links.ts` (pure parts): search URLs, iTunes query,
//! and choosing an exact Apple Music result.

use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

use crate::js::encode_uri_component;
use crate::storage::ShareProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareKind {
    Song,
    Album,
    Artist,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShareTarget {
    pub kind: ShareKind,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShareResolution {
    pub url: String,
    pub exact: bool,
}

/// NFKD, strip marks, lower-case, drop bracketed parts, non-alphanumerics to spaces.
fn normalize(value: Option<&str>) -> String {
    let folded: String = value
        .unwrap_or("")
        .nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase();
    // `/[\[(][^\])]*[\])]/g` → " "
    let mut stripped = String::new();
    let mut depth = false;
    for c in folded.chars() {
        if !depth && (c == '[' || c == '(') {
            depth = true;
            stripped.push(' ');
        } else if depth && (c == ']' || c == ')') {
            depth = false;
        } else if !depth {
            stripped.push(c);
        }
    }
    let mut out = String::new();
    let mut gap = false;
    for c in stripped.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

fn matches(left: Option<&str>, right: Option<&str>) -> bool {
    let (a, b) = (normalize(left), normalize(right));
    !a.is_empty() && !b.is_empty() && (a == b || a.contains(&b) || b.contains(&a))
}

pub fn query_for(target: &ShareTarget) -> String {
    let album = if target.kind == ShareKind::Song { target.album.as_deref() } else { None };
    [target.artist.as_deref(), Some(target.title.as_str()), album]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn provider_search_url(provider: ShareProvider, target: &ShareTarget) -> String {
    let query = encode_uri_component(&query_for(target));
    match provider {
        ShareProvider::Spotify => format!("https://open.spotify.com/search/{query}"),
        ShareProvider::AppleMusic => format!("https://music.apple.com/us/search?term={query}"),
    }
}

pub fn itunes_entity(kind: ShareKind) -> &'static str {
    match kind {
        ShareKind::Song => "song",
        ShareKind::Album => "album",
        ShareKind::Artist => "musicArtist",
    }
}

fn field<'a>(result: &'a Value, key: &str) -> Option<&'a str> {
    result.get(key).and_then(Value::as_str)
}

fn is_exact(target: &ShareTarget, result: &Value) -> bool {
    match target.kind {
        ShareKind::Song => {
            matches(Some(&target.title), field(result, "trackName")) && matches(target.artist.as_deref(), field(result, "artistName"))
        }
        ShareKind::Album => {
            matches(Some(&target.title), field(result, "collectionName"))
                && matches(target.artist.as_deref(), field(result, "artistName"))
        }
        ShareKind::Artist => matches(Some(&target.title), field(result, "artistName")),
    }
}

/// The exact Apple Music URL from an iTunes search payload, or the search
/// fallback.
pub fn resolve_from_itunes(target: &ShareTarget, payload: &Value) -> ShareResolution {
    let fallback = ShareResolution { url: provider_search_url(ShareProvider::AppleMusic, target), exact: false };
    let results: Vec<&Value> = payload
        .get("results")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter(|r| is_exact(target, r)).collect())
        .unwrap_or_default();
    let best = match target.duration.filter(|d| *d != 0.0) {
        Some(duration) => results.into_iter().min_by(|a, b| {
            let gap = |r: &Value| (r.get("trackTimeMillis").and_then(Value::as_f64).unwrap_or(0.0) / 1000.0 - duration).abs();
            gap(a).partial_cmp(&gap(b)).unwrap_or(std::cmp::Ordering::Equal)
        }),
        None => results.into_iter().next(),
    };
    let key = match target.kind {
        ShareKind::Song => "trackViewUrl",
        ShareKind::Album => "collectionViewUrl",
        ShareKind::Artist => "artistViewUrl",
    };
    match best.and_then(|r| field(r, key)) {
        Some(url) => ShareResolution { url: url.to_string(), exact: true },
        None => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn song() -> ShareTarget {
        ShareTarget {
            kind: ShareKind::Song,
            title: "Blue Hour (Remastered)".into(),
            artist: Some("Miles & The City".into()),
            album: Some("Blue Hour".into()),
            duration: Some(182.0),
        }
    }

    #[test]
    fn search_urls_per_provider() {
        let t = song();
        assert_eq!(query_for(&t), "Miles & The City Blue Hour (Remastered) Blue Hour");
        assert_eq!(
            provider_search_url(ShareProvider::Spotify, &t),
            "https://open.spotify.com/search/Miles%20%26%20The%20City%20Blue%20Hour%20(Remastered)%20Blue%20Hour"
        );
        assert!(provider_search_url(ShareProvider::AppleMusic, &t).starts_with("https://music.apple.com/us/search?term=Miles"));
    }

    #[test]
    fn picks_exact_match_closest_in_length() {
        let payload = json!({"results": [
            {"trackName": "Blue Hour", "artistName": "Other", "trackViewUrl": "https://x/other"},
            {"trackName": "Blue Hour", "artistName": "Miles & The City", "trackTimeMillis": 200000, "trackViewUrl": "https://x/far"},
            {"trackName": "Blue Hour", "artistName": "Miles and The City", "trackTimeMillis": 183000, "trackViewUrl": "https://x/near"},
            {"trackName": "Blue Hour", "artistName": "Miles & The City", "trackTimeMillis": 181000, "trackViewUrl": "https://x/nearest"}
        ]});
        let resolved = resolve_from_itunes(&song(), &payload);
        assert_eq!(resolved, ShareResolution { url: "https://x/nearest".into(), exact: true });
        let none = resolve_from_itunes(&song(), &json!({"results": []}));
        assert!(!none.exact);
    }
}
