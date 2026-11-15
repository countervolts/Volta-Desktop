//! Library record shapes. Port of the types at the top of `src/lib/navidrome.ts`.
//!
//! Field names serialize in camelCase so Subsonic JSON and the web's persisted
//! JSON (session queue, pins, history) round-trip unchanged.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Navidrome,
    Local,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayGain {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_gain: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_gain: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_peak: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_peak: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Song {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub play_count: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bit_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_count: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starred: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub musical_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_gain: Option<ReplayGain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_modified: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_artwork_url: Option<String>,
}

impl Song {
    pub fn is_local(&self) -> bool {
        self.source == Some(Source::Local)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumRecord {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub play_count: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub song_count: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub song: Option<Vec<Song>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starred: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_artwork_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_modified: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist_image_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_count: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<Vec<AlbumRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starred: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_artwork_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_modified: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub song_count: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<Vec<Song>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readonly: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Genre {
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub song_count: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_count: Option<f64>,
}
