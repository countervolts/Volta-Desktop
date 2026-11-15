//! Port of `src/lib/interactions.ts`: engagement events (max 2500 events,
//! 800 impressions, 240 days) stored as `{version: 1, events}` under
//! `volta-engagement:v1:<server>:<user>`, and the taste profile.

use std::collections::HashSet;

use chrono::{Datelike, Local, TimeZone, Timelike};
use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::js::encode_uri_component;
use crate::listening_history::{Daypart, daypart_for_hour, entity_key as engage_key_opt, now_ms};

pub const ENGAGEMENT_VERSION: u32 = 1;
const MAX_ENGAGEMENT_EVENTS: usize = 2500;
const MAX_IMPRESSION_EVENTS: usize = 800;
const MAX_ENGAGEMENT_AGE_DAYS: f64 = 240.0;
const COENGAGEMENT_SESSION_GAP_MS: f64 = 45.0 * 60.0 * 1000.0;
const HALF_LIFE_DAYS: f64 = 30.0;
const DAY_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;

const STOPWORDS: [&str; 31] = [
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "with", "at", "by", "from", "is", "it", "my", "me", "we", "you",
    "your", "volta", "song", "songs", "album", "albums", "artist", "artists", "music", "track", "tracks",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EngagementKind {
    AlbumView,
    ArtistView,
    GenreView,
    FavoriteAdd,
    FavoriteRemove,
    QueueAdd,
    PlayNext,
    AlbumPlay,
    AlbumShuffle,
    PlaylistAdd,
    Search,
    RecommendationImpression,
    RecommendationClick,
    Dislike,
    Undislike,
    ArtistMute,
    ArtistUnmute,
}

impl EngagementKind {
    /// `ENGAGEMENT_WEIGHTS`.
    pub fn weight(self) -> f64 {
        use EngagementKind::*;
        match self {
            AlbumView => 0.55,
            ArtistView => 0.45,
            GenreView => 0.35,
            FavoriteAdd => 3.2,
            FavoriteRemove => -1.1,
            QueueAdd => 1.15,
            PlayNext => 1.45,
            AlbumPlay => 2.3,
            AlbumShuffle => 1.5,
            PlaylistAdd => 2.7,
            Search => 0.4,
            RecommendationImpression => 0.0,
            RecommendationClick => 1.7,
            Dislike => -3.4,
            Undislike => 0.9,
            ArtistMute => -4.5,
            ArtistUnmute => 1.2,
        }
    }

    pub fn as_str(self) -> String {
        serde_json::to_value(self).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntityType {
    Song,
    Album,
    Artist,
    Genre,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngagementEntity {
    #[serde(rename = "type")]
    pub kind: Option<EntityType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<f64>,
}

pub type FeatureVector = IndexMap<String, f64>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngagementEvent {
    pub version: u32,
    pub id: String,
    pub at: f64,
    pub kind: EngagementKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<EngagementEntity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<EngagementEntity>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<Vec<FeatureVector>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<f64>,
    pub daypart: Daypart,
    pub hour: u32,
    pub weekday: u32,
    #[serde(default)]
    pub weekend: bool,
}

#[derive(Debug, Clone, Default)]
pub struct EngagementInput {
    pub kind: Option<EngagementKind>,
    pub at: Option<f64>,
    pub entity: Option<EngagementEntity>,
    pub items: Option<Vec<EngagementEntity>>,
    pub features: Option<Vec<FeatureVector>>,
    pub text: Option<String>,
    pub value: Option<f64>,
    pub position: Option<f64>,
}

pub fn engage_key(value: Option<&str>) -> String {
    engage_key_opt(value)
}

pub fn engagement_storage_key(server: &str, username: &str) -> String {
    format!("volta-engagement:v1:{}:{}", encode_uri_component(server), encode_uri_component(username))
}

/// `tokenize`: lower-case alphanumeric runs of 2+ chars, minus stopwords.
pub fn tokenize(value: &str) -> Vec<String> {
    value
        .to_lowercase()
        .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
        .filter(|t| t.len() >= 2 && !STOPWORDS.contains(t))
        .map(str::to_string)
        .collect()
}

fn make_id() -> String {
    use rand::Rng;
    let bytes: [u8; 16] = rand::rng().random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `createEngagementEvent`.
pub fn create_engagement_event(input: EngagementInput) -> EngagementEvent {
    let at = input.at.filter(|v| v.is_finite()).unwrap_or_else(now_ms);
    let date = Local.timestamp_millis_opt(at as i64).single().unwrap_or_else(Local::now);
    let (hour, weekday) = (date.hour(), date.weekday().num_days_from_sunday());
    EngagementEvent {
        version: ENGAGEMENT_VERSION,
        id: make_id(),
        at,
        kind: input.kind.unwrap_or(EngagementKind::AlbumView),
        entity: input.entity,
        items: input.items,
        features: input.features,
        text: input.text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
        value: input.value.filter(|v| v.is_finite()),
        position: input.position.filter(|v| v.is_finite()),
        daypart: daypart_for_hour(hour),
        hour,
        weekday,
        weekend: weekday == 0 || weekday == 6,
    }
}

fn prune(events: Vec<EngagementEvent>, now: f64) -> Vec<EngagementEvent> {
    let cutoff = now - MAX_ENGAGEMENT_AGE_DAYS * DAY_MS;
    let mut fresh: Vec<EngagementEvent> = events.into_iter().filter(|e| e.at >= cutoff && e.at <= now + DAY_MS).collect();
    fresh.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
    let (impressions, others): (Vec<_>, Vec<_>) = fresh.into_iter().partition(|e| e.kind == EngagementKind::RecommendationImpression);
    let skip = impressions.len().saturating_sub(MAX_IMPRESSION_EVENTS);
    let mut all: Vec<EngagementEvent> = others.into_iter().chain(impressions.into_iter().skip(skip)).collect();
    all.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
    let skip = all.len().saturating_sub(MAX_ENGAGEMENT_EVENTS);
    all.into_iter().skip(skip).collect()
}

fn valid(event: &EngagementEvent) -> bool {
    let entity_ok = |e: &EngagementEntity| e.kind.is_some() && (e.id.is_some() || e.name.is_some());
    event.version == ENGAGEMENT_VERSION
        && event.entity.as_ref().is_none_or(entity_ok)
        && event.items.as_ref().is_none_or(|items| items.iter().all(entity_ok))
}

/// `readEngagement`.
pub fn read_engagement(raw: &str, now: f64) -> Vec<EngagementEvent> {
    let Ok(value) = serde_json::from_str::<Value>(raw) else { return Vec::new() };
    if value.get("version").and_then(Value::as_u64) != Some(ENGAGEMENT_VERSION as u64) {
        return Vec::new();
    }
    let Some(items) = value.get("events").and_then(Value::as_array) else { return Vec::new() };
    prune(items.iter().filter_map(|i| serde_json::from_value::<EngagementEvent>(i.clone()).ok()).filter(valid).collect(), now)
}

/// `writeEngagement`: the pruned list and its JSON.
pub fn write_engagement(events: Vec<EngagementEvent>, now: f64) -> (Vec<EngagementEvent>, String) {
    let next = prune(events, now);
    let json = serde_json::json!({"version": ENGAGEMENT_VERSION, "events": next}).to_string();
    (next, json)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EngagementStats {
    pub impressions: f64,
    pub last_shown_at: f64,
    pub views: u32,
    pub favorites: u32,
    pub unfavorites: u32,
    pub queue_adds: u32,
    pub play_nexts: u32,
    pub album_plays: u32,
    pub album_shuffles: u32,
    pub playlist_adds: u32,
    pub clicks: u32,
    pub dislikes: u32,
    pub mutes: u32,
    pub searches: u32,
    pub last_at: f64,
    pub first_at: f64,
}

impl EngagementStats {
    /// Not `Default`: `first_at` starts at +∞.
    fn empty() -> Self {
        Self { first_at: f64::INFINITY, ..Default::default() }
    }

    fn record(&mut self, event: &EngagementEvent, weight: f64) {
        use EngagementKind::*;
        match event.kind {
            AlbumView | ArtistView | GenreView => self.views += 1,
            FavoriteAdd => self.favorites += 1,
            FavoriteRemove => self.unfavorites += 1,
            QueueAdd => self.queue_adds += 1,
            PlayNext => self.play_nexts += 1,
            AlbumPlay => self.album_plays += 1,
            AlbumShuffle => self.album_shuffles += 1,
            PlaylistAdd => self.playlist_adds += 1,
            RecommendationClick => self.clicks += 1,
            Dislike => self.dislikes += 1,
            ArtistMute => self.mutes += 1,
            Search => self.searches += 1,
            _ => {}
        }
        let name = event.kind.as_str();
        if weight > 0.0 || name.starts_with("dislike") || name.ends_with("mute") {
            self.last_at = self.last_at.max(event.at);
            self.first_at = self.first_at.min(event.at);
        }
    }
}

pub type ValueMap = IndexMap<String, f64>;

#[derive(Debug, Clone, Default)]
pub struct EngagementProfile {
    pub events: usize,
    pub albums: IndexMap<String, EngagementStats>,
    pub artists: IndexMap<String, EngagementStats>,
    pub genres: IndexMap<String, EngagementStats>,
    pub songs: IndexMap<String, EngagementStats>,
    pub album_affinity: ValueMap,
    pub artist_affinity: ValueMap,
    pub genre_affinity: ValueMap,
    pub song_affinity: ValueMap,
    pub album_aversion: ValueMap,
    pub artist_aversion: ValueMap,
    pub genre_aversion: ValueMap,
    pub keywords: ValueMap,
    pub mutes: IndexSet<String>,
    pub by_daypart: IndexMap<Daypart, f64>,
    pub album_coengagement: IndexMap<String, ValueMap>,
    pub artist_coengagement: IndexMap<String, ValueMap>,
    pub measured_at: f64,
}

fn add_value(map: &mut ValueMap, key: &str, value: f64) {
    if key.is_empty() || !value.is_finite() {
        return;
    }
    *map.entry(key.to_string()).or_insert(0.0) += value;
}

fn add_coengagement(map: &mut IndexMap<String, ValueMap>, left: &str, right: &str, weight: f64) {
    if left.is_empty() || right.is_empty() || left == right || weight <= 0.0 {
        return;
    }
    *map.entry(left.to_string()).or_default().entry(right.to_string()).or_insert(0.0) += weight;
}

fn decay(age_days: f64, half_life: f64) -> f64 {
    if age_days <= 0.0 { 1.0 } else { 0.5f64.powf(age_days / half_life) }
}

fn stats_for<'a>(map: &'a mut IndexMap<String, EngagementStats>, key: &str) -> Option<&'a mut EngagementStats> {
    if key.is_empty() {
        return None;
    }
    Some(map.entry(key.to_string()).or_insert_with(EngagementStats::empty))
}

fn genre_list(genre: Option<&str>) -> Vec<String> {
    genre.unwrap_or("").split([',', ';', '|', '/']).map(|g| engage_key(Some(g))).filter(|g| !g.is_empty()).collect()
}

/// `summarizeEngagement`.
pub fn summarize_engagement(events: &[EngagementEvent], now: f64) -> EngagementProfile {
    use EngagementKind::*;
    let mut profile = EngagementProfile { events: events.len(), measured_at: now, ..Default::default() };
    for event in events {
        if event.kind == RecommendationImpression {
            let entities: Vec<&EngagementEntity> = match (&event.items, &event.entity) {
                (Some(items), _) => items.iter().collect(),
                (None, Some(e)) => vec![e],
                _ => Vec::new(),
            };
            for entity in entities {
                if entity.kind != Some(EntityType::Album) {
                    continue;
                }
                if let Some(stats) = stats_for(&mut profile.albums, &engage_key(entity.id.as_deref())) {
                    stats.impressions += decay(((now - event.at) / DAY_MS).max(0.0), 14.0);
                    stats.last_shown_at = stats.last_shown_at.max(event.at);
                }
            }
            continue;
        }
        let base = event.kind.weight();
        let age = ((now - event.at) / DAY_MS).max(0.0);
        let weighted = base * decay(age, if event.kind == Search { 14.0 } else { HALF_LIFE_DAYS });
        *profile.by_daypart.entry(event.daypart).or_insert(0.0) += weighted.abs() + 0.15;
        if event.kind == Search
            && let Some(text) = event.text.as_deref() {
                for token in tokenize(text) {
                    add_value(&mut profile.keywords, &token, decay(age, 14.0));
                }
            }
        let Some(entity) = event.entity.as_ref() else { continue };
        let album_id = if entity.kind == Some(EntityType::Album) { engage_key(entity.id.as_deref()) } else { engage_key(entity.album_id.as_deref()) };
        let artist_id = {
            let key = engage_key(entity.artist_id.as_deref());
            if !key.is_empty() {
                key
            } else if entity.kind == Some(EntityType::Artist) {
                engage_key(entity.id.as_deref())
            } else {
                String::new()
            }
        };
        let genres = genre_list(entity.genre.as_deref());
        let key = {
            let id = engage_key(entity.id.as_deref());
            if id.is_empty() { engage_key(entity.name.as_deref()) } else { id }
        };
        let (stats_map, affinity, aversion) = match entity.kind {
            Some(EntityType::Album) => (&mut profile.albums, Some(&mut profile.album_affinity), Some(&mut profile.album_aversion)),
            Some(EntityType::Artist) => (&mut profile.artists, Some(&mut profile.artist_affinity), Some(&mut profile.artist_aversion)),
            Some(EntityType::Genre) => (&mut profile.genres, Some(&mut profile.genre_affinity), Some(&mut profile.genre_aversion)),
            _ => (&mut profile.songs, None, None),
        };
        if let Some(stats) = stats_for(stats_map, &key) {
            stats.record(event, weighted);
        }
        match (affinity, aversion) {
            (Some(affinity), Some(aversion)) => {
                if weighted >= 0.0 {
                    add_value(affinity, &key, weighted);
                } else {
                    add_value(aversion, &key, weighted.abs());
                }
            }
            _ => {
                if weighted > 0.0 {
                    add_value(&mut profile.song_affinity, &key, weighted);
                }
            }
        }
        if event.kind == ArtistMute && !artist_id.is_empty() {
            profile.mutes.insert(format!("artist:{artist_id}"));
        }
        if event.kind == ArtistUnmute && !artist_id.is_empty() {
            profile.mutes.shift_remove(&format!("artist:{artist_id}"));
        }
        if weighted > 0.0 {
            match entity.kind {
                Some(EntityType::Album) => {
                    add_value(&mut profile.artist_affinity, &artist_id, weighted * 0.55);
                    for genre in &genres {
                        add_value(&mut profile.genre_affinity, genre, weighted * 0.32);
                    }
                }
                Some(EntityType::Song) => {
                    add_value(&mut profile.album_affinity, &album_id, weighted * 0.5);
                    add_value(&mut profile.artist_affinity, &artist_id, weighted * 0.5);
                    for genre in &genres {
                        add_value(&mut profile.genre_affinity, genre, weighted * 0.3);
                    }
                }
                _ => {}
            }
        } else if weighted < 0.0 {
            match entity.kind {
                Some(EntityType::Album) => add_value(&mut profile.artist_aversion, &artist_id, weighted.abs() * 0.45),
                Some(EntityType::Genre) => add_value(&mut profile.genre_aversion, &key, weighted.abs()),
                _ => {}
            }
        }
    }

    let mut acted: Vec<&EngagementEvent> =
        events.iter().filter(|e| e.entity.as_ref().is_some_and(|en| en.kind == Some(EntityType::Album)) && e.kind.weight() > 0.0).collect();
    acted.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
    let mut sessions: Vec<Vec<&EngagementEvent>> = Vec::new();
    for event in acted {
        let start_new = match sessions.last() {
            None => true,
            Some(current) => event.at - current.last().map_or(0.0, |e| e.at) > COENGAGEMENT_SESSION_GAP_MS,
        };
        if start_new {
            sessions.push(vec![event]);
        } else {
            sessions.last_mut().unwrap().push(event);
        }
    }
    for bucket in sessions {
        let mut unique: IndexMap<String, (String, f64)> = IndexMap::new();
        for event in bucket {
            let entity = event.entity.as_ref().unwrap();
            let album = engage_key(entity.id.as_deref());
            if album.is_empty() || unique.contains_key(&album) {
                continue;
            }
            unique.insert(album, (engage_key(entity.artist_id.as_deref()), event.at));
        }
        let entries: Vec<(&String, &(String, f64))> = unique.iter().collect();
        for l in 0..entries.len() {
            for r in l + 1..entries.len() {
                let gap = (entries[l].1.1 - entries[r].1.1).abs() / (60.0 * 60.0 * 1000.0);
                let weight = 1.0 / (1.0 + gap * 0.25);
                add_coengagement(&mut profile.album_coengagement, entries[l].0, entries[r].0, weight);
                add_coengagement(&mut profile.album_coengagement, entries[r].0, entries[l].0, weight);
                if !entries[l].1.0.is_empty() && !entries[r].1.0.is_empty() {
                    add_coengagement(&mut profile.artist_coengagement, &entries[l].1.0, &entries[r].1.0, weight);
                }
            }
        }
    }
    profile
}

/// `normalizedAffinity`: value / max, clamped to 0..1.
pub fn normalized_affinity(map: &ValueMap, key: &str) -> f64 {
    if key.is_empty() || map.is_empty() {
        return 0.0;
    }
    let maximum = map.values().cloned().fold(0.0, f64::max);
    if maximum > 0.0 { (map.get(key).copied().unwrap_or(0.0) / maximum).clamp(0.0, 1.0) } else { 0.0 }
}

pub fn normalized_genre_affinity(map: &ValueMap, keys: &[String]) -> f64 {
    keys.iter().map(|k| normalized_affinity(map, k)).fold(0.0, f64::max)
}

/// `coengagementSimilarity`.
pub fn coengagement_similarity(profile: &EngagementProfile, seed: &str, candidate: &str) -> f64 {
    if seed.is_empty() || candidate.is_empty() || seed == candidate {
        return 0.0;
    }
    let Some(row) = profile.album_coengagement.get(seed) else { return 0.0 };
    let direct = row.get(candidate).copied().unwrap_or(0.0);
    if direct == 0.0 {
        return 0.0;
    }
    let seed_total: f64 = row.values().sum();
    let candidate_total: f64 = profile.album_coengagement.get(candidate).map(|r| r.values().sum()).unwrap_or(0.0);
    let denominator = (seed_total.max(1.0) * candidate_total.max(1.0)).sqrt();
    (direct / denominator).clamp(0.0, 1.0) * direct / (direct + 2.0)
}

/// `keywordAffinity`.
pub fn keyword_affinity(profile: &EngagementProfile, text: &str) -> f64 {
    if text.is_empty() || profile.keywords.is_empty() {
        return 0.0;
    }
    tokenize(text).iter().map(|t| normalized_affinity(&profile.keywords, t)).fold(0.0, f64::max)
}

/// Mute keys present in a profile (`profile.mutes`).
pub fn muted_artists(profile: &EngagementProfile) -> HashSet<String> {
    profile.mutes.iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album_event(kind: EngagementKind, id: &str, artist: &str, at: f64) -> EngagementEvent {
        create_engagement_event(EngagementInput {
            kind: Some(kind),
            at: Some(at),
            entity: Some(EngagementEntity {
                kind: Some(EntityType::Album),
                id: Some(id.into()),
                artist_id: Some(artist.into()),
                genre: Some("Jazz".into()),
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    #[test]
    fn profile_weights_affinity_aversion_and_coengagement() {
        let now = 1_800_000_000_000.0;
        let events = vec![
            album_event(EngagementKind::FavoriteAdd, "a1", "x", now),
            album_event(EngagementKind::AlbumPlay, "a2", "y", now + 60_000.0),
            album_event(EngagementKind::Dislike, "a3", "z", now),
        ];
        let p = summarize_engagement(&events, now);
        assert!((p.album_affinity["a1"] - 3.2).abs() < 1e-9);
        assert!((p.artist_affinity["x"] - 3.2 * 0.55).abs() < 1e-9);
        assert!((p.genre_affinity["jazz"] - (3.2 + 2.3) * 0.32).abs() < 1e-9);
        assert!((p.album_aversion["a3"] - 3.4).abs() < 1e-9);
        assert!(p.album_coengagement["a1"]["a2"] > 0.99);
        assert!(coengagement_similarity(&p, "a1", "a2") > 0.0);
        assert_eq!(normalized_affinity(&p.album_affinity, "a1"), 1.0);
        assert_eq!(tokenize("The Blue Hour album"), vec!["blue", "hour"]);
    }

    #[test]
    fn storage_round_trip_caps_impressions() {
        let now = 1_800_000_000_000.0;
        let events: Vec<EngagementEvent> = (0..900)
            .map(|i| {
                create_engagement_event(EngagementInput {
                    kind: Some(EngagementKind::RecommendationImpression),
                    at: Some(now - i as f64 * 1000.0),
                    ..Default::default()
                })
            })
            .collect();
        let (kept, json) = write_engagement(events, now);
        assert_eq!(kept.len(), 800);
        assert_eq!(read_engagement(&json, now).len(), 800);
    }
}
