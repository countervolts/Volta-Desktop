//! Port of `src/lib/song-credits.ts`: credits from tags, and from a
//! MusicBrainz recording search (the request itself lives in services).

use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

use crate::model::Song;

#[derive(Debug, Clone, PartialEq)]
pub struct SongCredit {
    pub role: String,
    pub people: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreditsSource {
    Metadata,
    MusicBrainz,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SongCredits {
    pub source: CreditsSource,
    pub credits: Vec<SongCredit>,
    pub recording_title: String,
}

/// NFKD, strip marks, lower-case, non-alphanumerics to single spaces.
fn normalized(value: Option<&str>) -> String {
    let folded: String = value
        .unwrap_or("")
        .nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase();
    let mut out = String::new();
    let mut gap = false;
    for c in folded.chars() {
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

fn same(left: Option<&str>, right: Option<&str>) -> bool {
    let (a, b) = (normalized(left), normalized(right));
    !a.is_empty() && !b.is_empty() && (a == b || a.contains(&b) || b.contains(&a))
}

fn split_people(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or("")
        .split([';', '\u{0}', '•'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// Ordered role → people map with per-role de-duplication.
#[derive(Default)]
struct Roles(Vec<SongCredit>);

impl Roles {
    fn add(&mut self, role: &str, person: &str) {
        match self.0.iter_mut().find(|c| c.role == role) {
            Some(credit) => {
                if !credit.people.iter().any(|p| p == person) {
                    credit.people.push(person.to_string());
                }
            }
            None => self.0.push(SongCredit { role: role.to_string(), people: vec![person.to_string()] }),
        }
    }
}

/// `creditsFromMetadata`.
pub fn credits_from_metadata(song: &Song) -> Option<SongCredits> {
    let mut roles = Roles::default();
    for person in split_people(song.artist.as_deref()) {
        roles.add("Primary artist", &person);
    }
    if !same(song.album_artist.as_deref(), song.artist.as_deref()) {
        for person in split_people(song.album_artist.as_deref()) {
            roles.add("Album artist", &person);
        }
    }
    for person in split_people(song.composer.as_deref()) {
        roles.add("Composer", &person);
    }
    (!roles.0.is_empty()).then(|| SongCredits {
        source: CreditsSource::Metadata,
        recording_title: song.title.clone(),
        credits: roles.0,
    })
}

fn artist_names(recording: &Value) -> Vec<String> {
    recording
        .get("artist-credit")
        .and_then(Value::as_array)
        .map(|credits| {
            credits
                .iter()
                .filter_map(|c| {
                    c.pointer("/artist/name").and_then(Value::as_str).or_else(|| c.get("name").and_then(Value::as_str))
                })
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn score_recording(recording: &Value, song: &Song) -> f64 {
    let mut score = recording.get("score").and_then(Value::as_f64).unwrap_or(0.0);
    if same(recording.get("title").and_then(Value::as_str), Some(&song.title)) {
        score += 120.0;
    }
    if artist_names(recording).iter().any(|a| same(Some(a), song.artist.as_deref())) {
        score += 80.0;
    }
    if let (Some(duration), Some(length)) = (song.duration.filter(|d| *d != 0.0), recording.get("length").and_then(Value::as_f64)) {
        let difference = (length / 1000.0 - duration).abs();
        score += (25.0 - difference * 3.0).max(0.0);
    }
    score
}

/// `displayRole`: `-`/`_` to spaces, capitalise each word.
fn display_role(role: &str) -> String {
    let spaced = role.replace(['-', '_'], " ");
    let mut out = String::new();
    let mut start = true;
    for c in spaced.chars() {
        if start && c.is_alphanumeric() {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        start = !(c.is_alphanumeric() || c == '_');
    }
    out
}

/// `creditsFromMusicBrainz`: the best recording whose title and artist both
/// match; primary artists then each relation role.
pub fn credits_from_musicbrainz(response: &Value, song: &Song) -> Option<SongCredits> {
    let recordings = response.get("recordings").and_then(Value::as_array)?;
    let best = recordings
        .iter()
        .filter(|r| {
            same(r.get("title").and_then(Value::as_str), Some(&song.title))
                && artist_names(r).iter().any(|a| same(Some(a), song.artist.as_deref()))
        })
        .max_by(|a, b| score_recording(a, song).partial_cmp(&score_recording(b, song)).unwrap_or(std::cmp::Ordering::Equal))?;
    let mut roles = Roles::default();
    for artist in artist_names(best) {
        roles.add("Primary artist", &artist);
    }
    for relation in best.get("relations").and_then(Value::as_array).into_iter().flatten() {
        let (Some(kind), Some(person)) = (
            relation.get("type").and_then(Value::as_str),
            relation.pointer("/artist/name").and_then(Value::as_str),
        ) else {
            continue;
        };
        roles.add(&display_role(kind), person);
    }
    Some(SongCredits {
        source: CreditsSource::MusicBrainz,
        recording_title: best.get("title").and_then(Value::as_str).unwrap_or(&song.title).to_string(),
        credits: roles.0,
    })
}

/// Merges metadata credits into MusicBrainz ones (the end of `lookupSongCredits`).
pub fn merge_credits(external: SongCredits, metadata: Option<SongCredits>) -> SongCredits {
    let Some(metadata) = metadata else { return external };
    let mut roles = Roles::default();
    for credit in metadata.credits.iter().chain(external.credits.iter()) {
        for person in &credit.people {
            roles.add(&credit.role, person);
        }
    }
    SongCredits { credits: roles.0, ..external }
}

/// The MusicBrainz query string for a song (`recording:"…" AND artist:"…"`).
pub fn musicbrainz_query(song: &Song) -> Option<String> {
    let artist = song.artist.as_deref().filter(|a| !a.is_empty())?;
    (!song.title.is_empty()).then(|| format!("recording:\"{}\" AND artist:\"{}\"", song.title, artist))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn song(title: &str, artist: &str) -> Song {
        Song { id: "blue".into(), title: title.into(), artist: Some(artist.into()), ..Default::default() }
    }

    #[test]
    fn metadata_credits_before_lookup() {
        let mut s = song("Blue Hour", "Miles & The City");
        s.album_artist = Some("Miles & The City".into());
        s.composer = Some("Rhea Lane; Jordan Lee".into());
        let credits = credits_from_metadata(&s).unwrap();
        assert_eq!(credits.source, CreditsSource::Metadata);
        assert_eq!(credits.recording_title, "Blue Hour");
        assert_eq!(
            credits.credits,
            vec![
                SongCredit { role: "Primary artist".into(), people: vec!["Miles & The City".into()] },
                SongCredit { role: "Composer".into(), people: vec!["Rhea Lane".into(), "Jordan Lee".into()] },
            ]
        );
    }

    #[test]
    fn keeps_only_best_title_and_artist_match() {
        let mut s = song("Blue Hour", "Miles & The City");
        s.duration = Some(182.0);
        let response = json!({"recordings": [
            {"title": "Another Song", "score": 100, "artist-credit": [{"name": "Different Artist"}]},
            {"title": "Blue Hour", "length": 182000, "score": 88,
             "artist-credit": [{"artist": {"name": "Miles & The City"}}],
             "relations": [
                {"type": "producer", "artist": {"name": "Rhea Lane"}},
                {"type": "mix", "artist": {"name": "Jordan Lee"}},
                {"type": "producer", "artist": {"name": "Rhea Lane"}}
             ]}
        ]});
        let credits = credits_from_musicbrainz(&response, &s).unwrap();
        assert_eq!(credits.source, CreditsSource::MusicBrainz);
        assert_eq!(
            credits.credits,
            vec![
                SongCredit { role: "Primary artist".into(), people: vec!["Miles & The City".into()] },
                SongCredit { role: "Producer".into(), people: vec!["Rhea Lane".into()] },
                SongCredit { role: "Mix".into(), people: vec!["Jordan Lee".into()] },
            ]
        );
    }

    #[test]
    fn rejects_title_only_match() {
        let response = json!({"recordings": [{"title": "Blue Hour", "artist-credit": [{"name": "Elsewhere"}]}]});
        assert!(credits_from_musicbrainz(&response, &song("Blue Hour", "Miles & The City")).is_none());
    }
}
