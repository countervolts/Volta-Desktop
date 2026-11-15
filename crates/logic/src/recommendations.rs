//! Port of `src/lib/recommendations.ts`: the explainable hybrid album ranker
//! behind Home's "Made for you" shelf.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Datelike, Local, Timelike, Utc};
use indexmap::IndexMap;

use crate::interactions::{EngagementProfile, FeatureVector, coengagement_similarity, keyword_affinity};
use crate::learned_ranker::{RankerModel, blend_learned_score};
use crate::listening_history::{ListeningContext, ListeningProfile, ListeningStats, WeightMap, daypart_for_hour};
use crate::model::{AlbumRecord, Song};
use crate::recommendation_tuning::{RecommendationTuning, normalize as normalize_tuning, weights as tuning_weights};

const ROTATION_EXPOSURE: f64 = 0.6;
const ROTATION_PENALTY: f64 = 0.55;
const DAY_MS: f64 = 86_400_000.0;

type Map = IndexMap<String, f64>;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Signals {
    pub artist: f64,
    pub genre: f64,
    pub album: f64,
    pub recent: f64,
    pub frequent: f64,
    pub favorite: f64,
    pub context: f64,
    pub freshness: f64,
    pub era: f64,
    pub history: f64,
    pub time: f64,
    pub transition: f64,
    pub duration: f64,
    pub skip_penalty: f64,
    pub engagement: f64,
    pub engagement_artist: f64,
    pub engagement_genre: f64,
    pub search: f64,
    pub aversion: f64,
    pub cooccurrence: f64,
    pub coengagement: f64,
    pub loyalty: f64,
    pub replay: f64,
    pub rediscovery: f64,
    pub novelty: f64,
    pub exposure: f64,
    pub discovery: f64,
    pub overlooked: f64,
    pub fatigue: f64,
    pub taste_fit: f64,
    pub learned: f64,
}

impl Signals {
    /// The feature vector, keyed like the web's signals object.
    pub fn features(&self) -> FeatureVector {
        [
            ("artist", self.artist),
            ("genre", self.genre),
            ("album", self.album),
            ("recent", self.recent),
            ("frequent", self.frequent),
            ("favorite", self.favorite),
            ("context", self.context),
            ("freshness", self.freshness),
            ("era", self.era),
            ("history", self.history),
            ("time", self.time),
            ("transition", self.transition),
            ("duration", self.duration),
            ("skipPenalty", self.skip_penalty),
            ("engagement", self.engagement),
            ("engagementArtist", self.engagement_artist),
            ("engagementGenre", self.engagement_genre),
            ("search", self.search),
            ("aversion", self.aversion),
            ("cooccurrence", self.cooccurrence),
            ("coengagement", self.coengagement),
            ("loyalty", self.loyalty),
            ("replay", self.replay),
            ("rediscovery", self.rediscovery),
            ("novelty", self.novelty),
            ("exposure", self.exposure),
            ("discovery", self.discovery),
            ("overlooked", self.overlooked),
            ("fatigue", self.fatigue),
            ("tasteFit", self.taste_fit),
            ("learned", self.learned),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }
}

#[derive(Debug, Clone)]
pub struct AlbumRecommendation {
    pub album: AlbumRecord,
    pub score: f64,
    pub reason: String,
    pub signals: Signals,
    pub features: FeatureVector,
}

#[derive(Default)]
pub struct Input<'a> {
    pub candidates: &'a [AlbumRecord],
    pub discovery_candidates: &'a [AlbumRecord],
    pub library_pool: &'a [AlbumRecord],
    pub recently_played: &'a [AlbumRecord],
    pub frequently_played: &'a [AlbumRecord],
    pub recently_added: &'a [AlbumRecord],
    pub favorite_album_ids: HashSet<String>,
    pub context_songs: &'a [Song],
    pub exclude_album_ids: HashSet<String>,
    pub listening_profile: Option<&'a ListeningProfile>,
    pub engagement_profile: Option<&'a EngagementProfile>,
    pub ranker_model: Option<&'a RankerModel>,
    pub tuning: Option<RecommendationTuning>,
    pub now: Option<DateTime<Local>>,
    pub limit: Option<usize>,
}

#[derive(Default)]
struct AlbumProfile {
    artists: Map,
    genres: Map,
    albums: Map,
    recent: Map,
    frequent: Map,
    freshness: Map,
    favorite_album_ids: HashSet<String>,
    year_total: f64,
    year_weight: f64,
    context_artists: Map,
    context_genres: Map,
}

struct Scored {
    rec: AlbumRecommendation,
    artist_key: String,
    genre_keys: Vec<String>,
    exposure: f64,
}

struct Seeds {
    albums: Vec<String>,
    artists: Vec<String>,
    genres: Vec<String>,
    shuffle_bias: f64,
}

fn normalize(value: Option<&str>) -> String {
    value.unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn or_str<'a>(a: Option<&'a str>, b: Option<&'a str>) -> Option<&'a str> {
    a.filter(|s| !s.is_empty()).or(b)
}

fn artist_key(album: &AlbumRecord) -> String {
    normalize(or_str(album.artist_id.as_deref(), album.artist.as_deref()))
}

fn album_key(album: &AlbumRecord) -> String {
    album.id.trim().to_string()
}

fn genre_keys(value: Option<&str>) -> Vec<String> {
    let mut seen = indexmap::IndexSet::new();
    for g in value.unwrap_or("").split([',', ';', '|', '/']) {
        let k = normalize(Some(g));
        if !k.is_empty() {
            seen.insert(k);
        }
    }
    seen.into_iter().collect()
}

fn clamp(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

fn add(map: &mut Map, key: &str, value: f64) {
    if key.is_empty() || !value.is_finite() || value <= 0.0 {
        return;
    }
    *map.entry(key.to_string()).or_insert(0.0) += value;
}

fn positional_weight(index: usize, length: usize, base: f64) -> f64 {
    if length == 0 {
        return 0.0;
    }
    base * (0.42 + 0.58 * (-(index as f64) / (length as f64 * 0.42).max(1.0)).exp())
}

/// Per-call cache of map maxima (`affinityLookup`).
#[derive(Default)]
struct Lookup {
    maxima: std::cell::RefCell<HashMap<usize, f64>>,
}

impl Lookup {
    fn value(&self, map: Option<&Map>, key: &str) -> f64 {
        let Some(map) = map.filter(|_| !key.is_empty()) else { return 0.0 };
        let id = map as *const Map as usize;
        let maximum = *self.maxima.borrow_mut().entry(id).or_insert_with(|| map.values().cloned().fold(0.0, f64::max));
        if maximum > 0.0 { clamp(map.get(key).copied().unwrap_or(0.0) / maximum) } else { 0.0 }
    }

    fn genres(&self, map: Option<&Map>, keys: &[String]) -> f64 {
        keys.iter().map(|k| self.value(map, k)).fold(0.0, f64::max)
    }
}

fn stat_completion(stats: Option<&ListeningStats>) -> f64 {
    match stats {
        Some(s) if s.completion_samples > 0 => clamp(s.completion_ratio_total / s.completion_samples as f64),
        _ => 0.0,
    }
}

fn stat_affinity(stats: Option<&ListeningStats>, now: f64) -> f64 {
    let Some(s) = stats.filter(|s| s.starts > 0) else { return 0.0 };
    let age_days = ((now - s.last_played_at) / DAY_MS).max(0.0);
    let recency = if s.last_played_at != 0.0 { (-age_days / 21.0).exp() } else { 0.0 };
    let volume = clamp((s.starts as f64).ln_1p() / 12f64.ln_1p());
    let replay = clamp((s.starts as f64 - 1.0) / 6.0);
    let skip_penalty = clamp(s.early_skips as f64 / (s.starts.max(1)) as f64);
    clamp(volume * 0.28 + recency * 0.3 + stat_completion(Some(s)) * 0.25 + replay * 0.12 - skip_penalty * 0.3)
}

fn context_affinity(source: Option<&ListeningContext>, key: &str, now: f64) -> f64 {
    let Some(source) = source.filter(|_| !key.is_empty()) else { return 0.0 };
    let map = &source.albums;
    let Some(stats) = map.get(key).filter(|s| s.starts > 0) else { return 0.0 };
    let maximum = map.values().map(|v| v.starts).max().unwrap_or(0);
    let volume = if maximum > 0 { clamp((stats.starts as f64).ln_1p() / (maximum as f64).ln_1p()) } else { 0.0 };
    clamp(volume * 0.58 + stat_completion(Some(stats)) * 0.27 + stat_affinity(Some(stats), now) * 0.15) * stats.starts as f64
        / (stats.starts as f64 + 3.0)
}

fn transition_affinity(profile: Option<&ListeningProfile>, from: Option<&str>, to: &str) -> f64 {
    let (Some(profile), Some(from)) = (profile, from.filter(|s| !s.is_empty())) else { return 0.0 };
    if to.is_empty() {
        return 0.0;
    }
    let Some(row) = profile.album_transitions.get(from) else { return 0.0 };
    let maximum = row.values().cloned().fold(0.0, f64::max);
    if maximum > 0.0 { clamp(row.get(to).copied().unwrap_or(0.0) / maximum) } else { 0.0 }
}

/// FNV-1a over the UTC date and id (`stableRotation`).
fn stable_rotation(value: &str, now: &DateTime<Local>) -> f64 {
    let key = format!("{}:{value}", now.with_timezone(&Utc).format("%Y-%m-%d"));
    let mut hash: u32 = 2166136261;
    for unit in key.encode_utf16() {
        hash ^= unit as u32;
        hash = hash.wrapping_mul(16777619);
    }
    hash as f64 / 4294967295.0
}

fn song_artist_key(song: &Song) -> String {
    normalize(or_str(song.artist_id.as_deref(), or_str(song.album_artist.as_deref(), song.artist.as_deref())))
}

fn cosine(map: Option<&WeightMap>, seed: &str, candidate: &str) -> f64 {
    let Some(map) = map else { return 0.0 };
    if seed.is_empty() || candidate.is_empty() || seed == candidate {
        return 0.0;
    }
    let Some(row) = map.get(seed) else { return 0.0 };
    let direct = row.get(candidate).copied().unwrap_or(0.0);
    if direct == 0.0 {
        return 0.0;
    }
    let seed_total: f64 = row.values().sum();
    let candidate_total: f64 = map.get(candidate).map(|r| r.values().sum()).unwrap_or(0.0);
    clamp(direct / (seed_total.max(1.0) * candidate_total.max(1.0)).sqrt()) * direct / (direct + 2.0)
}

fn best_cosine(map: Option<&WeightMap>, seeds: &[&String], candidate: &str) -> f64 {
    seeds.iter().map(|s| cosine(map, s, candidate)).fold(0.0, f64::max)
}

fn top_keys(map: &Map, limit: usize) -> Vec<String> {
    let mut entries: Vec<(&String, &f64)> = map.iter().filter(|(_, v)| **v > 0.0).collect();
    entries.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
    entries.into_iter().take(limit).map(|(k, _)| k.clone()).collect()
}

#[derive(Clone, Copy)]
enum Target {
    Taste,
    Recent,
    Frequent,
    Freshness,
}

fn add_album_signal(profile: &mut AlbumProfile, album: &AlbumRecord, weight: f64, target: Target) {
    let id = album_key(album);
    match target {
        Target::Recent => add(&mut profile.recent, &id, weight),
        Target::Frequent => add(&mut profile.frequent, &id, weight),
        Target::Freshness => add(&mut profile.freshness, &id, weight),
        Target::Taste => {
            add(&mut profile.albums, &id, weight);
            add(&mut profile.artists, &artist_key(album), weight);
            for g in genre_keys(album.genre.as_deref()) {
                add(&mut profile.genres, &g, weight);
            }
            if let Some(year) = album.year.filter(|y| y.is_finite()) {
                profile.year_total += year * weight;
                profile.year_weight += weight;
            }
        }
    }
}

/// `mergeAlbums`: first-seen order, later non-null fields win.
pub fn merge_albums(groups: &[&[AlbumRecord]]) -> Vec<AlbumRecord> {
    let mut albums: IndexMap<String, AlbumRecord> = IndexMap::new();
    for group in groups {
        for album in group.iter() {
            let id = album_key(album);
            if id.is_empty() {
                continue;
            }
            match albums.get_mut(&id) {
                None => {
                    albums.insert(id, album.clone());
                }
                Some(existing) => {
                    let mut merged = serde_json::to_value(&*existing).unwrap_or_default();
                    if let (Some(target), Ok(serde_json::Value::Object(source))) = (merged.as_object_mut(), serde_json::to_value(album)) {
                        for (k, v) in source {
                            if !v.is_null() {
                                target.insert(k, v);
                            }
                        }
                    }
                    if let Ok(next) = serde_json::from_value(merged) {
                        *existing = next;
                    }
                }
            }
        }
    }
    albums.into_values().collect()
}

fn build_profile(input: &Input) -> AlbumProfile {
    let mut profile = AlbumProfile { favorite_album_ids: input.favorite_album_ids.clone(), ..Default::default() };
    let add_list = |profile: &mut AlbumProfile, albums: &[AlbumRecord], base: f64, target: Target| {
        for (i, album) in albums.iter().enumerate() {
            add_album_signal(profile, album, positional_weight(i, albums.len(), base), target);
        }
    };
    add_list(&mut profile, input.recently_played, 1.18, Target::Taste);
    add_list(&mut profile, input.recently_played, 1.05, Target::Recent);
    add_list(&mut profile, input.frequently_played, 1.1, Target::Taste);
    add_list(&mut profile, input.frequently_played, 1.0, Target::Frequent);
    add_list(&mut profile, input.recently_added, 1.0, Target::Freshness);
    let metadata: HashMap<String, AlbumRecord> =
        merge_albums(&[input.candidates, input.discovery_candidates, input.library_pool]).into_iter().map(|a| (a.id.clone(), a)).collect();
    let favorites: Vec<String> = profile.favorite_album_ids.iter().cloned().collect();
    for id in favorites {
        match metadata.get(&id) {
            Some(album) => add_album_signal(&mut profile, album, 1.45, Target::Taste),
            None => add(&mut profile.albums, &id, 1.45),
        }
    }
    for (i, song) in input.context_songs.iter().enumerate() {
        let weight = if i == 0 { 1.35 } else { (0.92 - i as f64 * 0.1).max(0.35) };
        add(&mut profile.context_artists, &song_artist_key(song), weight);
        for g in genre_keys(song.genre.as_deref()) {
            add(&mut profile.context_genres, &g, weight);
        }
    }
    profile
}

fn build_seeds(input: &Input, profile: &AlbumProfile, now: f64) -> Seeds {
    let mut albums = profile.albums.clone();
    let mut artists = profile.artists.clone();
    let mut genres = profile.genres.clone();
    if let Some(history) = input.listening_profile {
        for (id, s) in &history.albums {
            add(&mut albums, id, stat_affinity(Some(s), now) * 1.8);
        }
        for (id, s) in &history.artists {
            add(&mut artists, id, stat_affinity(Some(s), now) * 1.8);
        }
        for (id, s) in &history.genres {
            add(&mut genres, id, stat_affinity(Some(s), now) * 1.6);
        }
    }
    let add_normalized = |target: &mut Map, source: Option<&Map>, scale: f64| {
        let Some(source) = source.filter(|s| !s.is_empty()) else { return };
        let maximum = source.values().cloned().fold(0.0, f64::max);
        if maximum <= 0.0 {
            return;
        }
        for (k, v) in source {
            add(target, k, (v / maximum) * scale + v * 0.05);
        }
    };
    let engagement = input.engagement_profile;
    add_normalized(&mut albums, engagement.map(|e| &e.album_affinity), 1.4);
    add_normalized(&mut artists, engagement.map(|e| &e.artist_affinity), 1.4);
    add_normalized(&mut genres, engagement.map(|e| &e.genre_affinity), 1.3);
    let (mut plays, mut shuffles) = (0.0, 0.0);
    if let Some(e) = engagement {
        for s in e.albums.values() {
            plays += s.album_plays as f64;
            shuffles += s.album_shuffles as f64;
        }
    }
    let total = plays + shuffles;
    Seeds {
        albums: top_keys(&albums, 14),
        artists: top_keys(&artists, 14),
        genres: top_keys(&genres, 12),
        shuffle_bias: if total > 4.0 { clamp(shuffles / total) } else { 0.35 },
    }
}

fn genre_overlap(left: &[String], right: &[String]) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let common = left.iter().filter(|g| right.contains(g)).count() as f64;
    let union: HashSet<&String> = left.iter().chain(right.iter()).collect();
    common / (union.len().max(1)) as f64
}

fn similarity(left: &Scored, right: &Scored) -> f64 {
    if !left.artist_key.is_empty() && left.artist_key == right.artist_key { 1.0 } else { genre_overlap(&left.genre_keys, &right.genre_keys) * 0.48 }
}

fn format_genre(album: &AlbumRecord) -> String {
    let first = album.genre.as_deref().unwrap_or("").split([',', ';', '|', '/']).next().unwrap_or("").trim().to_string();
    if first.is_empty() { "this sound".into() } else { first }
}

fn reason_for(album: &AlbumRecord, s: &Signals, artist_key: &str, seeds: &Seeds, adventure: f64) -> String {
    let artist = album.artist.as_deref().map(str::trim).filter(|a| !a.is_empty()).unwrap_or("this artist");
    let genre = format_genre(album);
    if s.favorite >= 1.0 {
        return "Because you saved this album".into();
    }
    if s.overlooked > 0.7 && s.taste_fit >= 0.15 {
        return format!("Overlooked {genre}, close to your taste");
    }
    if s.exposure == 0.0 {
        if s.cooccurrence >= 0.3 || s.coengagement >= 0.3 {
            return "Close to what you play, with no recorded listens".into();
        }
        if s.context >= 0.5 {
            return format!("Unplayed {genre}, close to what you have on now");
        }
        if adventure >= 0.6 {
            return format!("A lesser-played {genre} discovery");
        }
        return "In your collection, with no recorded listens".into();
    }
    if s.exposure <= 0.25 && adventure >= 0.4 {
        if s.history > 0.4 {
            return format!("You liked {artist}, but barely played this");
        }
        return format!("A {genre} record you have only brushed past");
    }
    
    if s.engagement >= 0.6 && s.engagement_artist >= 0.5 {
        format!("You keep coming back to {artist}")
    } else if s.engagement >= 0.55 {
        "From an album you engaged with".into()
    } else if s.context > 0.72 && !artist_key.is_empty() {
        format!("More from {artist}")
    } else if s.time > 0.72 {
        "A fit for your listening time".into()
    } else if s.coengagement >= 0.5 || s.cooccurrence >= 0.5 {
        "Because it sits beside what you play".into()
    } else if s.search >= 0.6 {
        "Matches what you have searched for".into()
    } else if s.rediscovery >= 0.55 {
        "A favorite you have not visited in a while".into()
    } else if s.history > 0.72 {
        "From your listening history".into()
    } else if s.replay >= 0.6 {
        format!("A {artist} record you replay")
    } else if s.frequent >= 0.58 {
        "From your heavy rotation".into()
    } else if s.recent >= 0.58 {
        "A familiar record you may want back".into()
    } else if s.genre >= 0.58 {
        format!("Because you play {genre}")
    } else if s.freshness >= 0.62 {
        "A new arrival in your collection".into()
    } else if s.exposure >= 0.6 {
        format!("A {artist} album you have played a lot")
    } else if s.novelty >= 0.6 && !seeds.genres.is_empty() {
        format!("A different corner of {genre}")
    } else {
        "A fresh turn from your collection".into()
    }
}

/// `rankAlbumRecommendations`.
pub fn rank_album_recommendations(input: &Input) -> Vec<AlbumRecommendation> {
    let limit = input.limit.unwrap_or(8);
    if limit == 0 {
        return Vec::new();
    }
    let now_dt = input.now.unwrap_or_else(Local::now);
    let now = now_dt.timestamp_millis() as f64;
    let tuning = normalize_tuning(&input.tuning.unwrap_or_default());
    let w = tuning_weights(&tuning);
    let lookup = Lookup::default();
    let profile = build_profile(input);
    let seeds = build_seeds(input, &profile, now);
    let engagement = input.engagement_profile;
    let lp = input.listening_profile;
    let hour = now_dt.hour();
    let weekday = now_dt.weekday().num_days_from_sunday();
    let daypart = daypart_for_hour(hour);
    let current_album = input.context_songs.first().and_then(|s| s.album_id.as_deref());
    let taste_year = (profile.year_weight > 0.0).then(|| profile.year_total / profile.year_weight);
    let seed_albums: Vec<&String> = seeds.albums.iter().take(8).collect();
    let seed_artists: Vec<&String> = seeds.artists.iter().take(8).collect();
    let adventure = tuning.adventurousness;
    let discovery_weight = adventure.powf(1.5);

    let pool = merge_albums(&[input.candidates, input.discovery_candidates, input.library_pool]);
    let mut scored: Vec<Scored> = pool
        .into_iter()
        .filter(|a| !input.exclude_album_ids.contains(&album_key(a)) && !engagement.is_some_and(|e| e.mutes.contains(&format!("artist:{}", artist_key(a)))))
        .map(|album| {
            let id = album_key(&album);
            let artist = artist_key(&album);
            let genres = genre_keys(album.genre.as_deref());
            let artist_affinity = lookup.value(Some(&profile.artists), &artist);
            let genre_affinity = lookup.genres(Some(&profile.genres), &genres);
            let album_affinity = lookup.value(Some(&profile.albums), &id);
            let recent = lookup.value(Some(&profile.recent), &id);
            let frequent = lookup.value(Some(&profile.frequent), &id);
            let freshness = lookup.value(Some(&profile.freshness), &id);
            let context = lookup.value(Some(&profile.context_artists), &artist).max(lookup.genres(Some(&profile.context_genres), &genres));
            let favorite = if profile.favorite_album_ids.contains(&id) || album.starred.is_some() { 1.0 } else { 0.0 };

            let album_stats = lp.and_then(|p| p.albums.get(&id));
            let artist_stats = lp.and_then(|p| p.artists.get(&artist));
            let history = stat_affinity(album_stats, now);
            let history_artist = stat_affinity(artist_stats, now);
            let history_genre = genres.iter().map(|g| stat_affinity(lp.and_then(|p| p.genres.get(g)), now)).fold(0.0, f64::max);
            let historical_taste = history.max(history_artist).max(history_genre);

            let time = context_affinity(lp.and_then(|p| p.by_daypart.get(&daypart)), &id, now)
                .max(context_affinity(lp.and_then(|p| p.by_hour.get(&hour)), &id, now) * 0.92)
                .max(context_affinity(lp.and_then(|p| p.by_weekday.get(&weekday)), &id, now) * 0.78);
            let transition = transition_affinity(lp, current_album, &id);

            let track_seconds = match (album.duration, album.song_count) {
                (Some(d), Some(c)) if d != 0.0 && c != 0.0 => d / c,
                _ => 0.0,
            };
            let avg = lp.map_or(0.0, |p| p.average_track_seconds);
            let duration = if track_seconds > 0.0 && avg != 0.0 { clamp(1.0 - (track_seconds - avg).abs() / 120.0) } else { 0.0 };
            let skip_penalty = album_stats.map_or(0.0, |s| clamp(s.early_skips as f64 / (s.starts as f64 + 3.0)));
            let artist_skip_penalty = artist_stats.map_or(0.0, |s| clamp(s.early_skips as f64 / (s.starts as f64 + 8.0)));
            let era = match (taste_year, album.year.filter(|y| y.is_finite())) {
                (Some(t), Some(y)) => clamp(1.0 - (y - t).abs() / 28.0),
                _ => 0.0,
            };
            let abandonment = album_stats.map_or(0.0, |s| s.average_position_ratio).max(artist_stats.map_or(0.0, |s| s.average_position_ratio));
            let loyalty = clamp(album_stats.map_or(0.0, |s| s.distinct_days as f64 / 8.0).max(artist_stats.map_or(0.0, |s| s.distinct_days as f64 / 12.0)));
            let replay = artist_stats.map_or(0.0, |s| clamp((s.starts as f64 - s.sessions as f64) / (s.starts.max(1)) as f64));
            let rediscovery = clamp((album_affinity * 0.5 + recent * 0.5) * if recent > 0.0 && recent < 0.2 { 1.0 } else { 0.35 });

            let engagement_album = lookup.value(engagement.map(|e| &e.album_affinity), &id);
            let engagement_artist = lookup.value(engagement.map(|e| &e.artist_affinity), &artist);
            let engagement_genre = lookup.genres(engagement.map(|e| &e.genre_affinity), &genres);
            let engagement_score = engagement_album.max(engagement_artist * 0.7).max(engagement_genre * 0.5);
            let aversion = lookup
                .value(engagement.map(|e| &e.album_aversion), &id)
                .max(lookup.value(engagement.map(|e| &e.artist_aversion), &artist))
                .max(lookup.genres(engagement.map(|e| &e.genre_aversion), &genres))
                .max(skip_penalty * 0.5)
                .max(artist_skip_penalty * 0.4)
                .max(if abandonment > 0.85 { 0.35 } else { 0.0 });
            let search = engagement.map_or(0.0, |e| {
                keyword_affinity(
                    e,
                    &format!(
                        "{} {} {}",
                        or_str(album.name.as_deref(), album.title.as_deref()).unwrap_or(""),
                        album.artist.as_deref().unwrap_or(""),
                        album.genre.as_deref().unwrap_or("")
                    ),
                )
            });
            let other_seeds: Vec<&String> = seed_albums.iter().copied().filter(|s| **s != id).collect();
            let cooccurrence = best_cosine(lp.map(|p| &p.album_cooccurrence), &other_seeds, &id);
            let coengagement = engagement.map_or(0.0, |e| other_seeds.iter().map(|s| coengagement_similarity(e, s, &id)).fold(0.0, f64::max));
            let other_artists: Vec<&String> = seed_artists.iter().copied().filter(|s| **s != artist).collect();
            let artist_cooccurrence = best_cosine(lp.map(|p| &p.artist_cooccurrence), &other_artists, &artist);

            let familiarity = clamp(
                artist_affinity * 0.4 + genre_affinity * 0.32 + album_affinity * 0.18 + historical_taste * 0.3 + engagement_score * 0.25,
            );
            let novelty = clamp(1.0 - familiarity);
            let recent_penalty = recent * 0.17;

            let album_seconds = album.duration.filter(|d| *d > 0.0).unwrap_or(2400.0);
            let album_tracks = album.song_count.filter(|c| *c > 0.0).unwrap_or(10.0);
            let local_passes = album_stats.map_or(0.0, |s| s.listened_seconds) / album_seconds;
            let server_plays = album.play_count.filter(|p| p.is_finite()).map_or(0.0, |p| p.max(0.0));
            let exposure = clamp(
                (1.0 - (-local_passes / 2.0).exp())
                    .max((album_stats.map_or(0.0, |s| s.starts as f64) / (album_tracks * 8.0)).min(0.25))
                    .max(1.0 - (-server_plays / 4.0).exp())
                    .max(if recent > 0.0 { 0.12 } else { 0.0 })
                    .max(if frequent > 0.0 { 0.35 + frequent * 0.35 } else { 0.0 }),
            );
            let seen = engagement.and_then(|e| e.albums.get(&normalize(Some(&id))));
            let impressions = seen.map_or(0.0, |s| s.impressions);
            let views = seen.map_or(0.0, |s| s.views as f64);
            let overlooked = (1.0 - exposure) / (1.0 + views + impressions).sqrt();
            let shown_age_days = match seen {
                Some(s) if s.last_shown_at != 0.0 => ((now - s.last_shown_at) / DAY_MS).max(0.0),
                _ => f64::INFINITY,
            };
            let acted_since_shown = seen.is_some_and(|s| s.last_at > s.last_shown_at);
            let fatigue = (1.0 - (-impressions / 3.0).exp()) * (-shown_age_days / 3.0).exp() * if acted_since_shown { 0.25 } else { 1.0 };
            let unexposed = clamp(1.0 - exposure).sqrt();

            let taste_fit = clamp(
                artist_affinity * 0.26
                    + genre_affinity * 0.24
                    + historical_taste * 0.18
                    + engagement_score * 0.16
                    + cooccurrence * 0.2
                    + coengagement * 0.16
                    + artist_cooccurrence * 0.14
                    + era * 0.06
                    + duration * 0.05
                    + search * 0.08,
            );
            let discovery = unexposed
                * (taste_fit * (0.85 - adventure * 0.2)
                    + overlooked * 0.2
                    + novelty * adventure * 0.08
                    + stable_rotation(&id, &now_dt) * (0.025 + adventure * 0.055));

            let prior = artist_affinity * 0.24 * w.artist
                + genre_affinity * 0.19 * w.genre
                + album_affinity * 0.1 * w.album
                + frequent * 0.1 * w.frequent
                + recent * 0.06 * w.recent
                + favorite * 0.12 * w.favorite
                + context * 0.11
                + freshness * 0.05 * w.freshness
                + era * 0.03 * w.era
                + historical_taste * 0.17 * w.history
                + time * 0.1
                + transition * 0.08
                + duration * 0.03
                + engagement_score * 0.2 * w.engagement
                + engagement_artist * 0.12 * w.engagement
                + engagement_genre * 0.09 * w.engagement
                + search * 0.07
                + cooccurrence * 0.14 * w.cooccurrence
                + coengagement * 0.12 * w.cooccurrence
                + artist_cooccurrence * 0.08 * w.cooccurrence
                + loyalty * 0.06 * w.loyalty
                + replay * 0.05 * w.replay
                + rediscovery * 0.04 * w.rediscovery
                - recent_penalty * w.recent_penalty
                - skip_penalty * 0.16 * w.skip_penalty
                - aversion * 0.3 * w.aversion;
            let discovery_bias = seeds.shuffle_bias * 0.05 * (0.4 + novelty) - (1.0 - seeds.shuffle_bias) * 0.02 * novelty;
            let mut signals = Signals {
                artist: artist_affinity,
                genre: genre_affinity,
                album: album_affinity,
                recent,
                frequent,
                favorite,
                context,
                freshness,
                era,
                history: historical_taste,
                time,
                transition,
                duration,
                skip_penalty,
                engagement: engagement_score,
                engagement_artist,
                engagement_genre,
                search,
                aversion,
                cooccurrence,
                coengagement,
                loyalty,
                replay,
                rediscovery,
                novelty,
                exposure,
                discovery,
                overlooked,
                fatigue,
                taste_fit,
                learned: 0.0,
            };
            let features = signals.features();
            let relevance = prior + discovery_bias;
            let rotation_penalty = if exposure >= ROTATION_EXPOSURE { discovery_weight * ROTATION_PENALTY } else { 0.0 };
            let base = relevance * (1.0 - discovery_weight) + discovery * discovery_weight - rotation_penalty
                + overlooked * (0.04 + adventure * 0.12)
                - fatigue * (0.12 + adventure * 0.18)
                - aversion * discovery_weight * 0.35 * w.aversion;
            let learned = (blend_learned_score(base, input.ranker_model, &features) - base) * w.learned;
            signals.learned = learned;
            let reason = reason_for(&album, &signals, &artist, &seeds, tuning.adventurousness);
            // The web mutates `signals.learned` after aliasing it as `features`.
            let mut features = features;
            features.insert("learned".into(), learned);
            Scored {
                rec: AlbumRecommendation { album, score: base + learned, reason, signals, features },
                artist_key: artist,
                genre_keys: genres,
                exposure,
            }
        })
        .collect();

    if scored.is_empty() {
        return Vec::new();
    }
    let discovery_target = crate::js::js_round(limit.min(scored.len()) as f64 * adventure * 0.75) as usize;
    let mut discoveries = 0;
    let mut genre_counts: HashMap<String, f64> = HashMap::new();
    let mut genre_taste = profile.genres.clone();
    if let Some(p) = lp {
        for (g, s) in &p.genres {
            add(&mut genre_taste, g, stat_affinity(Some(s), now));
        }
    }
    if let Some(e) = engagement {
        for (g, v) in &e.genre_affinity {
            add(&mut genre_taste, g, *v);
        }
    }
    let genre_total: f64 = genre_taste.values().sum();
    let eligible = |item: &Scored| {
        item.exposure < 0.3 && item.rec.signals.aversion < 0.35 && (item.rec.signals.taste_fit >= 0.08 || genre_total == 0.0)
    };
    let mut selected: Vec<Scored> = Vec::new();
    let mut artist_counts: HashMap<String, f64> = HashMap::new();
    while !scored.is_empty() && selected.len() < limit {
        let needs_discovery = discoveries < discovery_target
            && limit - selected.len() <= discovery_target - discoveries
            && scored.iter().any(&eligible);
        let mut best_index = 0;
        let mut best_value = f64::NEG_INFINITY;
        for (index, candidate) in scored.iter().enumerate() {
            if needs_discovery && !eligible(candidate) {
                continue;
            }
            let redundancy = selected.iter().map(|s| similarity(candidate, s)).fold(0.0, f64::max);
            let artist_count = if candidate.artist_key.is_empty() { 0.0 } else { artist_counts.get(&candidate.artist_key).copied().unwrap_or(0.0) };
            let coverage = if genre_total > 0.0 {
                candidate
                    .genre_keys
                    .iter()
                    .map(|g| {
                        let target = genre_taste.get(g).copied().unwrap_or(0.0) / genre_total;
                        let actual = genre_counts.get(g).copied().unwrap_or(0.0) / selected.len().max(1) as f64;
                        (target - actual).max(0.0)
                    })
                    .sum()
            } else {
                0.0
            };
            let value = candidate.rec.score + coverage * 0.16 * w.diversity - redundancy * 0.14 * w.diversity - artist_count * 0.11 * w.diversity;
            if value > best_value || (value == best_value && candidate.rec.album.id < scored[best_index].rec.album.id) {
                best_value = value;
                best_index = index;
            }
        }
        let best = scored.remove(best_index);
        if eligible(&best) {
            discoveries += 1;
        }
        let share = 1.0 / best.genre_keys.len().max(1) as f64;
        for g in &best.genre_keys {
            *genre_counts.entry(g.clone()).or_insert(0.0) += share;
        }
        *artist_counts.entry(best.artist_key.clone()).or_insert(0.0) += 1.0;
        selected.push(best);
    }
    selected.into_iter().map(|s| s.rec).collect()
}

/// `songAlbumCandidates`.
pub fn song_album_candidates(songs: &[Song]) -> Vec<AlbumRecord> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for song in songs {
        let id = or_str(song.album_id.as_deref(), Some(song.id.as_str())).unwrap_or("").trim().to_string();
        if id.is_empty() || !seen.insert(id.clone()) {
            continue;
        }
        out.push(AlbumRecord {
            id,
            name: Some(song.album.clone().filter(|a| !a.is_empty()).unwrap_or_else(|| song.title.clone())),
            artist: or_str(song.artist.as_deref(), song.album_artist.as_deref()).map(str::to_string),
            artist_id: song.artist_id.clone(),
            cover_art: song.cover_art.clone(),
            year: song.year,
            genre: song.genre.clone(),
            song_count: Some(1.0),
            duration: song.duration,
            source: song.source,
            ..Default::default()
        });
    }
    out
}

/// `favoriteAlbumIdsFromSongs`.
pub fn favorite_album_ids_from_songs(songs: &[Song]) -> HashSet<String> {
    songs.iter().filter_map(|s| s.album_id.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)).collect()
}

/// `topArtistSeeds`.
pub fn top_artist_seeds(listening: Option<&ListeningProfile>, engagement: Option<&EngagementProfile>, now: f64, limit: usize) -> Vec<String> {
    let mut scores = Map::new();
    if let Some(p) = listening {
        for (key, s) in &p.artists {
            if key.is_empty() || key.contains(' ') {
                continue;
            }
            add(&mut scores, key, stat_affinity(Some(s), now) * (1.0 + (s.starts as f64).ln_1p()));
        }
    }
    if let Some(e) = engagement.filter(|e| !e.artist_affinity.is_empty()) {
        let maximum = e.artist_affinity.values().cloned().fold(0.0, f64::max);
        if maximum > 0.0 {
            for (key, v) in &e.artist_affinity {
                if key.is_empty() || key.contains(' ') {
                    continue;
                }
                add(&mut scores, key, (v / maximum) * 1.3 + v * 0.05);
            }
        }
    }
    top_keys(&scores, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: &str, artist: &str, genre: &str) -> AlbumRecord {
        AlbumRecord {
            id: id.into(),
            name: Some(id.into()),
            artist: Some(artist.into()),
            artist_id: Some(artist.into()),
            genre: Some(genre.into()),
            ..Default::default()
        }
    }

    #[test]
    fn favorites_rank_and_mutes_exclude() {
        let pool = vec![album("a", "x", "Jazz"), album("b", "y", "Rock"), album("c", "z", "Jazz")];
        let mut engagement = EngagementProfile::default();
        engagement.mutes.insert("artist:z".into());
        let input = Input {
            candidates: &pool,
            favorite_album_ids: ["a".to_string()].into_iter().collect(),
            engagement_profile: Some(&engagement),
            limit: Some(8),
            ..Default::default()
        };
        let ranked = rank_album_recommendations(&input);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].album.id, "a");
        assert_eq!(ranked[0].reason, "Because you saved this album");
        assert!(ranked.iter().all(|r| r.album.id != "c"));
    }

    #[test]
    fn merge_keeps_first_order_and_fills_fields() {
        let mut richer = album("a", "x", "Jazz");
        richer.starred = Some("2024".into());
        let merged = merge_albums(&[&[album("a", "x", "Jazz"), album("b", "y", "Rock")], &[richer]]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].starred.as_deref(), Some("2024"));
    }
}
