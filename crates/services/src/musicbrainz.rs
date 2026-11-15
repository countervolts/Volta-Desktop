//! MusicBrainz recording search for song credits (`lookupSongCredits`).
//! Sent only when the listener asks; carries the song title and artist and a
//! generic user agent, never library addresses or accounts.

use std::time::Duration;

use volta_logic::model::Song;
use volta_logic::song_credits::{SongCredits, credits_from_metadata, credits_from_musicbrainz, merge_credits, musicbrainz_query};

pub fn lookup_song_credits(song: &Song) -> Option<SongCredits> {
    let metadata = credits_from_metadata(song);
    if metadata.as_ref().is_some_and(|m| m.credits.len() > 1) {
        return metadata;
    }
    let query = musicbrainz_query(song)?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent(concat!("Volta/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let mut url = url::Url::parse("https://musicbrainz.org/ws/2/recording/").ok()?;
    url.query_pairs_mut()
        .append_pair("fmt", "json")
        .append_pair("limit", "8")
        .append_pair("inc", "artist-credits+recording-rels")
        .append_pair("query", &query);
    let response = client.get(url).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body: serde_json::Value = response.json().ok()?;
    let external = credits_from_musicbrainz(&body, song)?;
    Some(merge_credits(external, metadata))
}
