//! Port of `src/lib/listening-history.ts`: listening events (max 4000, 180
//! days), stored as `{version: 1, events}` under
//! `volta-listening-history:v1:<server>:<user>`, and the profile summary the
//! recommender and Stats read.

use std::collections::HashSet;

use chrono::{Datelike, Local, TimeZone, Timelike};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::js::encode_uri_component;
use crate::model::Song;

pub const HISTORY_VERSION: u32 = 1;
pub const MAX_HISTORY_EVENTS: usize = 4000;
pub const MAX_HISTORY_AGE_DAYS: f64 = 180.0;
pub const SESSION_GAP_MS: f64 = 20.0 * 60.0 * 1000.0;
pub const DAY_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Start,
    Resume,
    Pause,
    Skip,
    Complete,
    Stop,
    Seek,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Daypart {
    Overnight,
    Morning,
    Afternoon,
    Evening,
    LateNight,
}

/// `daypartForHour`.
pub fn daypart_for_hour(hour: u32) -> Daypart {
    match hour {
        0..=5 => Daypart::Overnight,
        6..=11 => Daypart::Morning,
        12..=17 => Daypart::Afternoon,
        18..=21 => Daypart::Evening,
        _ => Daypart::LateNight,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListeningEvent {
    pub version: u32,
    pub id: String,
    pub at: f64,
    pub kind: EventKind,
    pub session_id: String,
    pub session_index: u32,
    pub song_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default)]
    pub duration_seconds: f64,
    pub delta_seconds: f64,
    pub total_seconds: f64,
    #[serde(default)]
    pub position_seconds: f64,
    pub completion_ratio: f64,
    pub source: String,
    #[serde(default)]
    pub queue_index: u32,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default = "default_repeat")]
    pub repeat: String,
    pub daypart: Daypart,
    pub hour: u32,
    pub weekday: u32,
    #[serde(default)]
    pub weekend: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seek_delta_seconds: Option<f64>,
}

fn default_repeat() -> String {
    "off".into()
}

#[derive(Debug, Clone, Default)]
pub struct EventInput {
    pub kind: Option<EventKind>,
    pub at: Option<f64>,
    pub session_id: String,
    pub session_index: u32,
    pub duration_seconds: Option<f64>,
    pub delta_seconds: Option<f64>,
    pub total_seconds: Option<f64>,
    pub position_seconds: Option<f64>,
    /// `navidrome` | `local`.
    pub source: String,
    pub queue_index: u32,
    pub shuffle: bool,
    /// `off` | `all` | `one`.
    pub repeat: String,
    pub reason: Option<String>,
    pub seek_delta_seconds: Option<f64>,
}

pub fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

fn local(at: f64) -> chrono::DateTime<Local> {
    Local.timestamp_millis_opt(at as i64).single().unwrap_or_else(Local::now)
}

fn safe_text(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
}

/// `entityKey`: trimmed, lower-cased, whitespace collapsed.
pub fn entity_key(value: Option<&str>) -> String {
    value.unwrap_or("").trim().to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn event_id() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let bytes: [u8; 16] = rng.random();
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-4{}-{}-{}", &hex[0..8], &hex[8..12], &hex[13..16], &hex[16..20], &hex[20..32])
}

/// `createListeningEvent`.
pub fn create_listening_event(song: &Song, input: &EventInput) -> ListeningEvent {
    let at = input.at.filter(|v| v.is_finite()).unwrap_or_else(now_ms);
    let date = local(at);
    let hour = date.hour();
    let duration = input.duration_seconds.filter(|v| v.is_finite()).unwrap_or(song.duration.unwrap_or(0.0)).max(0.0);
    let upper = if duration > 0.0 { duration } else { f64::MAX };
    let total = input.total_seconds.filter(|v| v.is_finite()).unwrap_or(0.0).max(0.0).clamp(0.0, upper);
    let delta = input.delta_seconds.filter(|v| v.is_finite()).unwrap_or(0.0).max(0.0).clamp(0.0, total);
    let position = input.position_seconds.filter(|v| v.is_finite()).unwrap_or(total).max(0.0);
    let completion = if duration > 0.0 { (total / duration).clamp(0.0, 1.0) } else { 0.0 };
    let weekday = date.weekday().num_days_from_sunday();
    ListeningEvent {
        version: HISTORY_VERSION,
        id: event_id(),
        at,
        kind: input.kind.unwrap_or(EventKind::Start),
        session_id: input.session_id.clone(),
        session_index: input.session_index,
        song_id: song.id.clone(),
        title: song.title.clone(),
        artist: safe_text(song.artist.as_deref()),
        artist_id: safe_text(song.artist_id.as_deref().or(song.album_artist.as_deref())),
        album: safe_text(song.album.as_deref()),
        album_id: safe_text(song.album_id.as_deref()),
        cover_art: safe_text(song.cover_art.as_deref()),
        image_url: None,
        genre: safe_text(song.genre.as_deref()),
        duration_seconds: duration,
        delta_seconds: delta,
        total_seconds: total,
        position_seconds: position,
        completion_ratio: completion,
        source: input.source.clone(),
        queue_index: input.queue_index,
        shuffle: input.shuffle,
        repeat: input.repeat.clone(),
        daypart: daypart_for_hour(hour),
        hour,
        weekday,
        weekend: weekday == 0 || weekday == 6,
        reason: safe_text(input.reason.as_deref()),
        seek_delta_seconds: input.seek_delta_seconds.filter(|v| v.is_finite()),
    }
}

/// `listeningHistoryKey`.
pub fn listening_history_key(server: &str, username: &str) -> String {
    format!("volta-listening-history:v1:{}:{}", encode_uri_component(server), encode_uri_component(username))
}

fn prune(events: Vec<ListeningEvent>, now: f64) -> Vec<ListeningEvent> {
    let cutoff = now - MAX_HISTORY_AGE_DAYS * DAY_MS;
    let kept: Vec<ListeningEvent> = events.into_iter().filter(|e| e.at >= cutoff && e.at <= now + DAY_MS).collect();
    let skip = kept.len().saturating_sub(MAX_HISTORY_EVENTS);
    kept.into_iter().skip(skip).collect()
}

/// `readListeningHistory` from the stored JSON.
pub fn read_listening_history(raw: &str, now: f64) -> Vec<ListeningEvent> {
    let Ok(value) = serde_json::from_str::<Value>(raw) else { return Vec::new() };
    if value.get("version").and_then(Value::as_u64) != Some(HISTORY_VERSION as u64) {
        return Vec::new();
    }
    let Some(items) = value.get("events").and_then(Value::as_array) else { return Vec::new() };
    let events = items.iter().filter_map(|item| serde_json::from_value::<ListeningEvent>(item.clone()).ok()).filter(|e| e.version == HISTORY_VERSION).collect();
    prune(events, now)
}

/// `writeListeningHistory`: the JSON to store.
pub fn write_listening_history(events: Vec<ListeningEvent>, now: f64) -> (Vec<ListeningEvent>, String) {
    let next = prune(events, now);
    let json = serde_json::json!({"version": HISTORY_VERSION, "events": next}).to_string();
    (next, json)
}

/// `appendListeningEvent`: returns the new list and the JSON to store.
pub fn append_listening_event(raw: &str, event: ListeningEvent, now: f64) -> (Vec<ListeningEvent>, String) {
    let mut existing = read_listening_history(raw, now);
    if !existing.iter().any(|e| e.id == event.id) {
        existing.push(event);
    }
    write_listening_history(existing, now)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListeningStats {
    pub starts: u32,
    pub resumes: u32,
    pub pauses: u32,
    pub skips: u32,
    pub completes: u32,
    pub stops: u32,
    pub seeks: u32,
    pub listened_seconds: f64,
    pub completion_ratio_total: f64,
    pub completion_samples: u32,
    pub early_skips: u32,
    pub late_skips: u32,
    pub forward_seek_seconds: f64,
    pub backward_seek_seconds: f64,
    pub last_played_at: f64,
    pub first_played_at: f64,
    pub sessions: u32,
    pub distinct_days: u32,
    pub average_position_ratio: f64,
    pub position_ratio_total: f64,
    pub position_ratio_samples: u32,
    session_ids: HashSet<String>,
    day_ids: HashSet<String>,
}

impl ListeningStats {
    /// Not `Default`: `first_played_at` starts at +∞.
    fn empty() -> Self {
        Self { first_played_at: f64::INFINITY, ..Default::default() }
    }

    fn update(&mut self, event: &ListeningEvent) {
        match event.kind {
            EventKind::Start => self.starts += 1,
            EventKind::Resume => self.resumes += 1,
            EventKind::Pause => self.pauses += 1,
            EventKind::Skip => self.skips += 1,
            EventKind::Complete => self.completes += 1,
            EventKind::Stop => self.stops += 1,
            EventKind::Seek => {
                self.seeks += 1;
                let delta = event.seek_delta_seconds.unwrap_or(0.0);
                if delta >= 0.0 {
                    self.forward_seek_seconds += delta;
                } else {
                    self.backward_seek_seconds += delta.abs();
                }
            }
        }
        self.listened_seconds += event.delta_seconds.max(0.0);
        if matches!(event.kind, EventKind::Skip | EventKind::Complete | EventKind::Stop) {
            self.completion_ratio_total += event.completion_ratio;
            self.completion_samples += 1;
            if event.duration_seconds > 0.0 {
                self.position_ratio_total += (event.position_seconds / event.duration_seconds).clamp(0.0, 1.0);
                self.position_ratio_samples += 1;
            }
            if event.kind == EventKind::Skip {
                if event.completion_ratio < 0.35 {
                    self.early_skips += 1;
                } else {
                    self.late_skips += 1;
                }
            }
        }
        if matches!(event.kind, EventKind::Start | EventKind::Resume) {
            self.last_played_at = self.last_played_at.max(event.at);
            self.first_played_at = self.first_played_at.min(event.at);
        }
        self.session_ids.insert(event.session_id.clone());
        let utc = chrono::DateTime::from_timestamp_millis(event.at as i64).unwrap_or_default();
        self.day_ids.insert(utc.format("%Y-%m-%d").to_string());
    }

    fn finish(&mut self) {
        self.sessions = self.session_ids.len() as u32;
        if !self.first_played_at.is_finite() {
            self.first_played_at = 0.0;
        }
        self.distinct_days = self.day_ids.len() as u32;
        self.average_position_ratio =
            if self.position_ratio_samples > 0 { self.position_ratio_total / self.position_ratio_samples as f64 } else { 0.0 };
    }
}

pub type StatsMap = IndexMap<String, ListeningStats>;
pub type WeightMap = IndexMap<String, IndexMap<String, f64>>;

#[derive(Debug, Clone, Default)]
pub struct ListeningContext {
    pub songs: StatsMap,
    pub albums: StatsMap,
    pub artists: StatsMap,
    pub genres: StatsMap,
}

/// `split(/[,;|/]+/).map(entityKey).filter(Boolean)`.
pub fn genre_keys(genre: Option<&str>) -> Vec<String> {
    genre.unwrap_or("").split([',', ';', '|', '/']).map(|g| entity_key(Some(g))).filter(|g| !g.is_empty()).collect()
}

impl ListeningContext {
    fn update(&mut self, event: &ListeningEvent) {
        fn bump(map: &mut StatsMap, key: &str, event: &ListeningEvent) {
            if key.is_empty() {
                return;
            }
            map.entry(key.to_string()).or_insert_with(ListeningStats::empty).update(event);
        }
        bump(&mut self.songs, &event.song_id, event);
        if let Some(album) = event.album_id.as_deref() {
            bump(&mut self.albums, album, event);
        }
        let artist = entity_key(event.artist_id.as_deref().or(event.artist.as_deref()));
        bump(&mut self.artists, &artist, event);
        for genre in genre_keys(event.genre.as_deref()) {
            bump(&mut self.genres, &genre, event);
        }
    }

    fn finish(&mut self) {
        for map in [&mut self.songs, &mut self.albums, &mut self.artists, &mut self.genres] {
            map.values_mut().for_each(ListeningStats::finish);
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ListeningProfile {
    pub events: usize,
    pub sessions: usize,
    pub songs: StatsMap,
    pub albums: StatsMap,
    pub artists: StatsMap,
    pub genres: StatsMap,
    pub by_daypart: IndexMap<Daypart, ListeningContext>,
    pub by_hour: IndexMap<u32, ListeningContext>,
    pub by_weekday: IndexMap<u32, ListeningContext>,
    pub song_transitions: WeightMap,
    pub album_transitions: WeightMap,
    pub song_cooccurrence: WeightMap,
    pub album_cooccurrence: WeightMap,
    pub artist_cooccurrence: WeightMap,
    pub genre_cooccurrence: WeightMap,
    pub hour_histogram: [f64; 24],
    pub weekday_histogram: [f64; 7],
    pub average_track_seconds: f64,
    pub average_completion_ratio: f64,
}

fn add_transition(map: &mut WeightMap, from: Option<&str>, to: Option<&str>) {
    let (Some(from), Some(to)) = (from.filter(|s| !s.is_empty()), to.filter(|s| !s.is_empty())) else { return };
    if from == to {
        return;
    }
    *map.entry(from.to_string()).or_default().entry(to.to_string()).or_insert(0.0) += 1.0;
}

fn add_cooccurrence(map: &mut WeightMap, left: &str, right: &str, weight: f64) {
    if left.is_empty() || right.is_empty() || left == right || weight <= 0.0 {
        return;
    }
    *map.entry(left.to_string()).or_default().entry(right.to_string()).or_insert(0.0) += weight;
    *map.entry(right.to_string()).or_default().entry(left.to_string()).or_insert(0.0) += weight;
}

/// `summarizeListening`.
pub fn summarize_listening(events: &[ListeningEvent]) -> ListeningProfile {
    let mut profile = ListeningProfile {
        events: events.len(),
        sessions: events.iter().map(|e| e.session_id.as_str()).collect::<HashSet<_>>().len(),
        ..Default::default()
    };
    let mut overall = ListeningContext::default();
    let (mut duration_total, mut duration_samples, mut completion_total, mut completion_samples) = (0.0, 0u32, 0.0, 0u32);
    for event in events {
        overall.update(event);
        profile.by_daypart.entry(event.daypart).or_default().update(event);
        profile.by_hour.entry(event.hour).or_default().update(event);
        profile.by_weekday.entry(event.weekday).or_default().update(event);
        let audible = event.delta_seconds.max(0.0);
        if (event.hour as usize) < 24 {
            profile.hour_histogram[event.hour as usize] += audible;
        }
        if (event.weekday as usize) < 7 {
            profile.weekday_histogram[event.weekday as usize] += audible;
        }
        if event.kind == EventKind::Start && event.duration_seconds > 0.0 {
            duration_total += event.duration_seconds;
            duration_samples += 1;
        }
        if matches!(event.kind, EventKind::Skip | EventKind::Complete | EventKind::Stop) {
            completion_total += event.completion_ratio;
            completion_samples += 1;
        }
    }

    let mut starts: Vec<&ListeningEvent> = events.iter().filter(|e| e.kind == EventKind::Start).collect();
    starts.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
    let mut previous: IndexMap<String, (String, Option<String>, f64)> = IndexMap::new();
    let mut by_session: IndexMap<String, Vec<&ListeningEvent>> = IndexMap::new();
    for event in &starts {
        if let Some((song, album, at)) = previous.get(&event.session_id)
            && event.at - at <= SESSION_GAP_MS {
                add_transition(&mut profile.song_transitions, Some(song), Some(&event.song_id));
                add_transition(&mut profile.album_transitions, album.as_deref(), event.album_id.as_deref());
            }
        previous.insert(event.session_id.clone(), (event.song_id.clone(), event.album_id.clone(), event.at));
        by_session.entry(event.session_id.clone()).or_default().push(event);
    }
    for bucket in by_session.values() {
        let (mut albums, mut artists, mut genres, mut songs): (IndexMap<String, f64>, IndexMap<String, f64>, IndexMap<String, f64>, IndexMap<String, f64>) =
            Default::default();
        let keep = |map: &mut IndexMap<String, f64>, key: &str, weight: f64| {
            if key.is_empty() {
                return;
            }
            let slot = map.entry(key.to_string()).or_insert(0.0);
            *slot = slot.max(weight);
        };
        for (index, event) in bucket.iter().enumerate() {
            let weight = 1.0 / (1.0 + index as f64 * 0.08);
            if let Some(album) = event.album_id.as_deref() {
                keep(&mut albums, album, weight);
            }
            keep(&mut artists, &entity_key(event.artist_id.as_deref().or(event.artist.as_deref())), weight);
            keep(&mut songs, &event.song_id, weight);
            for genre in genre_keys(event.genre.as_deref()) {
                keep(&mut genres, &genre, weight);
            }
        }
        let pair = |map: &mut WeightMap, entries: &IndexMap<String, f64>| {
            let list: Vec<(&String, &f64)> = entries.iter().collect();
            for l in 0..list.len() {
                for r in l + 1..list.len() {
                    add_cooccurrence(map, list[l].0, list[r].0, list[l].1 * list[r].1);
                }
            }
        };
        pair(&mut profile.album_cooccurrence, &albums);
        pair(&mut profile.artist_cooccurrence, &artists);
        pair(&mut profile.genre_cooccurrence, &genres);
        pair(&mut profile.song_cooccurrence, &songs);
    }

    overall.finish();
    profile.by_daypart.values_mut().for_each(ListeningContext::finish);
    profile.by_hour.values_mut().for_each(ListeningContext::finish);
    profile.by_weekday.values_mut().for_each(ListeningContext::finish);
    profile.songs = overall.songs;
    profile.albums = overall.albums;
    profile.artists = overall.artists;
    profile.genres = overall.genres;
    profile.average_track_seconds = if duration_samples > 0 { duration_total / duration_samples as f64 } else { 0.0 };
    profile.average_completion_ratio = if completion_samples > 0 { completion_total / completion_samples as f64 } else { 0.0 };
    profile
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, title: &str, artist: &str, album: &str, duration: f64) -> Song {
        Song {
            id: id.into(),
            title: title.into(),
            artist: Some(artist.into()),
            artist_id: Some(artist.to_lowercase().replace(' ', "-")),
            album: Some(album.into()),
            album_id: Some(album.to_lowercase().replace(' ', "-")),
            genre: Some("Ambient".into()),
            duration: Some(duration),
            ..Default::default()
        }
    }

    #[test]
    fn preserves_outcomes_and_time_context() {
        let morning = Local::now().date_naive().and_hms_opt(8, 0, 0).unwrap().and_local_timezone(Local).unwrap().timestamp_millis() as f64;
        let first = song("song-1", "First", "North Star", "Anchor", 180.0);
        let second = song("song-2", "Second", "Quiet Field", "Field Notes", 200.0);
        let input = |kind, at: f64, index, queue, shuffle| EventInput {
            kind: Some(kind),
            at: Some(at),
            session_id: "session-1".into(),
            session_index: index,
            source: "navidrome".into(),
            queue_index: queue,
            shuffle,
            repeat: "off".into(),
            ..Default::default()
        };
        let events = vec![
            create_listening_event(&first, &input(EventKind::Start, morning, 0, 0, false)),
            create_listening_event(
                &first,
                &EventInput {
                    duration_seconds: Some(180.0),
                    delta_seconds: Some(180.0),
                    total_seconds: Some(180.0),
                    reason: Some("ended".into()),
                    ..input(EventKind::Complete, morning + 180_000.0, 0, 0, false)
                },
            ),
            create_listening_event(&second, &input(EventKind::Start, morning + 240_000.0, 1, 1, true)),
            create_listening_event(
                &second,
                &EventInput {
                    duration_seconds: Some(200.0),
                    delta_seconds: Some(10.0),
                    total_seconds: Some(10.0),
                    position_seconds: Some(10.0),
                    reason: Some("manual-next".into()),
                    ..input(EventKind::Skip, morning + 250_000.0, 1, 1, true)
                },
            ),
        ];
        let now = morning + 300_000.0;
        let mut raw = String::new();
        for event in events {
            raw = append_listening_event(&raw, event, now).1;
        }
        let stored = read_listening_history(&raw, now);
        let profile = summarize_listening(&stored);
        assert_eq!(stored.len(), 4);
        assert_eq!(profile.songs["song-1"].completes, 1);
        assert_eq!(profile.songs["song-2"].early_skips, 1);
        assert_eq!(profile.by_hour[&8].songs["song-1"].starts, 1);
        assert_eq!(profile.album_transitions["anchor"]["field-notes"], 1.0);
        assert_eq!(profile.average_track_seconds, 190.0);
    }
}
