//! LRCLIB fallback lyrics (`lrclibLyrics`), only when "External lyrics
//! lookup" is on. Search first for the closest synced match, then `/api/get`.

use std::time::Duration;

use volta_logic::lyrics::{Lyrics, lrclib_artist_name, parse_lrc, parse_plain_lyrics, pick_synced_lrclib_result};
use volta_logic::model::Song;

const CLIENT: &str = "Volta Desktop Player";

pub fn lrclib_lyrics(song: &Song) -> Option<Lyrics> {
    let artist = song.album_artist.as_deref().or(song.artist.as_deref()).filter(|a| !a.is_empty())?;
    if song.title.is_empty() || song.artist.as_deref().is_none_or(str::is_empty) {
        return None;
    }
    let artist = lrclib_artist_name(artist);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("Volta/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let mut search = url::Url::parse("https://lrclib.net/api/search").ok()?;
    search.query_pairs_mut().append_pair("track_name", &song.title).append_pair("artist_name", &artist);
    if let Ok(response) = client.get(search).header("Lrclib-Client", CLIENT).send()
        && response.status().is_success()
            && let Ok(records) = response.json::<serde_json::Value>()
                && let Some(lyrics) = pick_synced_lrclib_result(&records, song) {
                    return Some(lyrics);
                }
    let mut get = url::Url::parse("https://lrclib.net/api/get").ok()?;
    {
        let mut q = get.query_pairs_mut();
        q.append_pair("track_name", &song.title).append_pair("artist_name", &artist);
        if let Some(album) = song.album.as_deref().filter(|a| !a.is_empty()) {
            q.append_pair("album_name", album);
        }
        if let Some(duration) = song.duration.filter(|d| *d > 0.0 && *d <= 3600.0) {
            q.append_pair("duration", &format!("{}", duration.round() as i64));
        }
    }
    let response = client.get(get).header("Lrclib-Client", CLIENT).send().ok()?;
    if response.status().as_u16() == 404 || !response.status().is_success() {
        return None;
    }
    let record: serde_json::Value = response.json().ok()?;
    record
        .get("syncedLyrics")
        .and_then(|v| v.as_str())
        .and_then(parse_lrc)
        .or_else(|| record.get("plainLyrics").and_then(|v| v.as_str()).and_then(parse_plain_lyrics))
}
