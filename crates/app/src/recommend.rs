//! The App.tsx recommendation plumbing: engagement logging (`recordEngagement`),
//! reward attribution and ranker training (`learnFromOutcome`), playback
//! rewards, impression logging, and the cached "Made for you" slate.

use std::collections::HashSet;

use gpui::{AppContext, Context};
use volta_logic::interactions::{
    EngagementEntity, EngagementEvent, EngagementInput, EngagementKind, EngagementProfile, EntityType, create_engagement_event,
    engagement_storage_key, read_engagement, summarize_engagement, write_engagement,
};
use volta_logic::learned_ranker::{
    PendingImpression, RankerModel, RewardTarget, attribute_reward, impression_storage_key, playback_reward, ranker_model_storage_key,
    read_impressions, read_ranker_model, train_ranker, write_impressions, write_ranker_model,
};
use volta_logic::listening_history::{ListeningEvent, ListeningProfile, now_ms, summarize_listening};
use volta_logic::model::{AlbumRecord, Song};
use volta_logic::recommendation_tuning::{RECOMMENDATION_TUNING_KEY, RecommendationTuning, read_tuning, write_tuning};
use volta_logic::recommendations::{AlbumRecommendation, Input, merge_albums, rank_album_recommendations, top_artist_seeds};

use volta_logic::storage as keys;

use crate::app_state::AppState;
use crate::components::notifications::NoticeKind;

/// `REWARD_WINDOW_MS` in App.tsx: playback older than this is not attributed.
const REWARD_WINDOW_MS: f64 = 6.0 * 60.0 * 60.0 * 1000.0;

#[derive(Default)]
pub struct Learning {
    pub engagement: Vec<EngagementEvent>,
    pub engagement_profile: EngagementProfile,
    pub listening_profile: ListeningProfile,
    listening_signature: (usize, String),
    pub ranker: RankerModel,
    impressions: Vec<PendingImpression>,
    /// What `record_impressions` last logged (30-minute bucket, account, slate), so the
    /// per-frame Home render does not re-read and re-parse the engagement log to find
    /// nothing new.
    impression_stamp: Option<(i64, String, String, usize)>,
    rewarded: HashSet<String>,
    pub tuning: RecommendationTuning,
    /// The cached Home slate and the inputs it was ranked from.
    pub slate: Vec<AlbumRecommendation>,
    slate_signature: String,
    pub discovery_albums: Vec<AlbumRecord>,
    discovery_signature: String,
    /// The tune-engine preview and the tuning/slate it was ranked for.
    preview: Vec<AlbumRecommendation>,
    preview_signature: String,
}

pub fn entity_for_song(song: &Song) -> EngagementEntity {
    EngagementEntity {
        kind: Some(EntityType::Song),
        id: Some(song.id.clone()),
        name: Some(song.title.clone()),
        artist_id: song.artist_id.clone().filter(|s| !s.is_empty()).or_else(|| song.album_artist.clone()),
        artist_name: song.artist.clone(),
        album_id: song.album_id.clone(),
        album_name: song.album.clone(),
        genre: song.genre.clone(),
        year: None,
    }
}

pub fn entity_for_album(album: &AlbumRecord) -> EngagementEntity {
    EngagementEntity {
        kind: Some(EntityType::Album),
        id: Some(album.id.clone()),
        name: Some(volta_logic::format::album_name(album).to_string()),
        artist_id: album.artist_id.clone(),
        artist_name: album.artist.clone(),
        genre: album.genre.clone(),
        year: album.year,
        ..Default::default()
    }
}

pub fn target_for_song(song: &Song) -> RewardTarget {
    RewardTarget {
        album_id: song.album_id.clone(),
        artist_id: song.artist_id.clone().filter(|s| !s.is_empty()).or_else(|| song.album_artist.clone()),
    }
}

pub fn target_for_album(album: &AlbumRecord) -> RewardTarget {
    RewardTarget { album_id: Some(album.id.clone()), artist_id: album.artist_id.clone() }
}

fn artist_mute_key(song: &Song) -> String {
    let id = [song.artist_id.as_deref(), song.album_artist.as_deref(), song.artist.as_deref()]
        .into_iter()
        .flatten()
        .find(|s| !s.is_empty())
        .unwrap_or("");
    format!("artist:{}", id.trim().to_lowercase())
}

/// Parity screenshots start from a fresh session, like a new browser context.
fn shooting() -> bool {
    cfg!(feature = "shots") && std::env::var_os("VOLTA_SHOT").is_some()
}

impl AppState {
    /// `listeningStorage`: local storage, or session-only storage when "Keep
    /// history across logins and restarts" is off.
    pub fn listening_persistent(&self) -> bool {
        self.storage.get(keys::LISTENING_HISTORY_PERSIST_KEY) != "false"
    }

    pub fn lget(&self, key: &str) -> String {
        if self.listening_persistent() { self.storage.get(key) } else { self.storage.session_get(key) }
    }

    pub fn lset(&self, key: &str, value: &str) {
        if self.listening_persistent() {
            self.storage.set(key, value)
        } else {
            self.storage.session_set(key, value)
        }
    }

    pub fn lremove(&self, key: &str) {
        self.storage.remove(key);
        self.storage.session_remove(key);
    }

    /// The four per-account learning keys (history, engagement, ranker, impressions).
    pub fn learning_storage_keys(&self) -> Vec<String> {
        let Some(c) = self.client() else { return Vec::new() };
        vec![
            volta_logic::listening_history::listening_history_key(&c.server, &c.username),
            engagement_storage_key(&c.server, &c.username),
            ranker_model_storage_key(&c.server, &c.username),
            impression_storage_key(&c.server, &c.username),
        ]
    }

    /// `changeListeningHistoryPersistence`: moves the stored profile between
    /// local and session storage.
    pub fn set_listening_persistent(&mut self, persistent: bool, cx: &mut Context<Self>) {
        if persistent == self.listening_persistent() {
            return;
        }
        for key in self.learning_storage_keys() {
            let raw = self.lget(&key);
            self.lremove(&key);
            if !raw.is_empty() {
                if persistent { self.storage.set(&key, &raw) } else { self.storage.session_set(&key, &raw) }
            }
        }
        self.storage.set(keys::LISTENING_HISTORY_PERSIST_KEY, if persistent { "true" } else { "false" });
        cx.notify();
    }

    /// Logging out ends a session-only profile.
    pub fn end_listening_session(&self) {
        for key in self.learning_storage_keys() {
            self.storage.session_remove(&key);
        }
    }

    fn learning_enabled(&self) -> bool {
        self.lget(keys::LISTENING_HISTORY_ENABLED_KEY) != "false"
    }

    fn learning_keys(&self) -> Option<(String, String, String)> {
        let c = self.client()?;
        Some((
            engagement_storage_key(&c.server, &c.username),
            ranker_model_storage_key(&c.server, &c.username),
            impression_storage_key(&c.server, &c.username),
        ))
    }

    /// Reads engagement, the ranker and impressions for the signed-in account.
    pub fn load_learning(&mut self) {
        let now = now_ms();
        self.learning.tuning = read_tuning(Some(self.lget(RECOMMENDATION_TUNING_KEY)).filter(|s| !s.is_empty()).as_deref());
        self.learning.rewarded.clear();
        self.learning.slate_signature.clear();
        let enabled = self.learning_enabled();
        match self.learning_keys().filter(|_| enabled) {
            Some((engagement, ranker, impressions)) => {
                self.learning.engagement = read_engagement(&self.lget(&engagement), now);
                self.learning.ranker = read_ranker_model(Some(self.lget(&ranker)).filter(|s| !s.is_empty()).as_deref());
                self.learning.impressions = read_impressions(Some(self.lget(&impressions)).filter(|s| !s.is_empty()).as_deref(), now);
            }
            None => {
                self.learning.engagement.clear();
                self.learning.ranker = RankerModel::default();
                self.learning.impressions.clear();
            }
        }
        self.learning.engagement_profile = summarize_engagement(&self.learning.engagement, now);
        // Events already on disk were rewarded in an earlier session.
        self.learning.rewarded = self.listening_events.iter().map(|e| e.id.clone()).collect();
    }

    /// `recordEngagement`.
    pub fn record_engagement(&mut self, input: EngagementInput) {
        if !self.learning_enabled() {
            return;
        }
        let Some((key, _, _)) = self.learning_keys() else { return };
        let now = now_ms();
        let event = create_engagement_event(input);
        let mut events = read_engagement(&self.lget(&key), now);
        events.push(event);
        let (next, json) = write_engagement(events, now);
        self.lset(&key, &json);
        self.learning.engagement_profile = summarize_engagement(&next, now);
        self.learning.engagement = next;
    }

    pub fn engage(&mut self, kind: EngagementKind, entity: EngagementEntity) {
        self.record_engagement(EngagementInput { kind: Some(kind), entity: Some(entity), ..Default::default() });
    }

    /// `learnFromOutcome`.
    pub fn learn_from_outcome(&mut self, target: RewardTarget, reward: f64, at: Option<f64>) {
        if !self.learning_enabled() {
            return;
        }
        let Some((_, ranker_key, impression_key)) = self.learning_keys() else { return };
        let now = at.unwrap_or_else(now_ms);
        let (examples, remaining) = attribute_reward(&self.learning.impressions, &target, reward, now);
        if examples.is_empty() {
            return;
        }
        let (kept, json) = write_impressions(remaining, now_ms());
        self.lset(&impression_key, &json);
        self.learning.impressions = kept;
        let trained = train_ranker(&self.learning.ranker, &examples, 3);
        self.lset(&ranker_key, &write_ranker_model(&trained));
        self.learning.ranker = trained;
    }

    /// Folds a new playback event into the ranker (the App.tsx effect over
    /// `listeningEvents`).
    pub fn learn_from_playback(&mut self, event: &ListeningEvent) {
        if !self.learning.rewarded.insert(event.id.clone()) || event.at < now_ms() - REWARD_WINDOW_MS {
            return;
        }
        let reward = playback_reward(event.kind, event.completion_ratio);
        if reward != 0.0 {
            self.learn_from_outcome(RewardTarget { album_id: event.album_id.clone(), artist_id: event.artist_id.clone() }, reward, Some(event.at));
        }
        if self.learning.rewarded.len() > 3000 {
            let start = self.listening_events.len().saturating_sub(1500);
            self.learning.rewarded = self.listening_events[start..].iter().map(|e| e.id.clone()).collect();
        }
    }

    /// `resetLearning`.
    pub fn reset_learning(&mut self, cx: &mut Context<Self>) {
        if let Some((engagement, ranker, impressions)) = self.learning_keys() {
            for key in [engagement, ranker, impressions] {
                self.lremove(&key);
            }
        }
        self.learning.impressions.clear();
        self.learning.ranker = RankerModel::default();
        self.learning.engagement.clear();
        self.learning.engagement_profile = EngagementProfile::default();
        self.learning.slate_signature.clear();
        self.notify("Volta's learned profile has been reset.", NoticeKind::Info, cx);
    }

    pub fn set_tuning(&mut self, tuning: RecommendationTuning, cx: &mut Context<Self>) {
        let (normalized, json) = write_tuning(&tuning);
        self.lset(RECOMMENDATION_TUNING_KEY, &json);
        self.learning.tuning = normalized;
        cx.notify();
    }

    pub fn reset_tuning(&mut self, cx: &mut Context<Self>) {
        self.lremove(RECOMMENDATION_TUNING_KEY);
        self.learning.tuning = RecommendationTuning::default();
        cx.notify();
    }

    /// `notInterested`.
    pub fn not_interested_song(&mut self, song: &Song, cx: &mut Context<Self>) {
        self.engage(EngagementKind::Dislike, entity_for_song(song));
        self.learn_from_outcome(target_for_song(song), -1.0, None);
        self.notify("Got it — Volta will show less like this.", NoticeKind::Info, cx);
    }

    pub fn not_interested_album(&mut self, album: &AlbumRecord, cx: &mut Context<Self>) {
        self.engage(EngagementKind::Dislike, entity_for_album(album));
        self.learn_from_outcome(target_for_album(album), -1.0, None);
        self.notify("Got it — Volta will show less like this.", NoticeKind::Info, cx);
    }

    /// `setArtistMuted`.
    pub fn set_artist_muted(&mut self, artist_id: Option<String>, name: Option<String>, muted: bool, cx: &mut Context<Self>) {
        let Some(id) = artist_id.filter(|s| !s.is_empty()) else { return };
        let entity = EngagementEntity { kind: Some(EntityType::Artist), id: Some(id), name: name.clone(), ..Default::default() };
        self.engage(if muted { EngagementKind::ArtistMute } else { EngagementKind::ArtistUnmute }, entity);
        let who = name.unwrap_or_else(|| "this artist".into());
        let message = if muted { format!("Muted {who} in recommendations.") } else { format!("Unmuted {who}.") };
        self.notify(message, NoticeKind::Info, cx);
    }

    pub fn is_song_artist_muted(&self, song: &Song) -> bool {
        self.learning.engagement_profile.mutes.contains(&artist_mute_key(song))
    }

    fn refresh_listening_profile(&mut self) {
        let signature = (self.listening_events.len(), self.listening_events.last().map(|e| e.id.clone()).unwrap_or_default());
        if signature != self.learning.listening_signature {
            self.learning.listening_profile = summarize_listening(&self.listening_events);
            self.learning.listening_signature = signature;
        }
    }

    /// Fetches albums by the listener's top artists (the `discoveryAlbums`
    /// effect) when the seed set changes.
    fn refresh_discovery(&mut self, cx: &mut Context<Self>) {
        let seeds = top_artist_seeds(Some(&self.learning.listening_profile), Some(&self.learning.engagement_profile), now_ms(), 3);
        let signature = seeds.join("|");
        if signature == self.learning.discovery_signature {
            return;
        }
        self.learning.discovery_signature = signature.clone();
        if seeds.is_empty() {
            self.learning.discovery_albums.clear();
            return;
        }
        let Some(client) = self.client() else { return };
        let task = cx.background_spawn(async move {
            let groups: Vec<Vec<AlbumRecord>> = seeds.iter().map(|id| client.artist(id).ok().and_then(|a| a.album).unwrap_or_default()).collect();
            let refs: Vec<&[AlbumRecord]> = groups.iter().map(Vec::as_slice).collect();
            merge_albums(&refs)
        });
        cx.spawn(async move |this, cx| {
            let albums = task.await;
            let _ = this.update(cx, |state, cx| {
                if state.learning.discovery_signature == signature && !albums.is_empty() {
                    state.learning.discovery_albums = albums;
                    state.learning.slate_signature.clear();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The queue context for ranking: up to 4 distinct songs from the current one.
    fn context_songs(&self, cx: &gpui::App) -> Vec<Song> {
        let player = self.player.read(cx);
        let start = player.index.unwrap_or(0);
        let mut seen = HashSet::new();
        player.queue.iter().skip(start).take(4).filter(|s| seen.insert(s.id.clone())).cloned().collect()
    }

    /// `recommendedResults`, re-ranked only when an input changes.
    pub fn refresh_recommendations(&mut self, cx: &mut Context<Self>) {
        // `sourceMode !== "navidrome"`: no Made for you on a local library.
        if self.client().is_none() || self.is_local() {
            self.learning.slate.clear();
            return;
        }
        self.refresh_listening_profile();
        self.refresh_discovery(cx);
        let context = self.context_songs(cx);
        let home = &self.home;
        let signature = format!(
            "{}|{}|{}|{}|{}|{}|{:?}|{}|{}",
            home.generation,
            self.learning.engagement.len(),
            self.learning.ranker.samples,
            self.learning.listening_signature.1,
            self.learning.discovery_albums.len(),
            context.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(","),
            self.learning.tuning,
            home.recommendation_pool.len(),
            home.favorite_album_ids.len(),
        );
        if signature == self.learning.slate_signature {
            return;
        }
        self.learning.slate_signature = signature;
        let mut excluded: HashSet<String> = home.recently_played.iter().take(2).map(|a| a.id.clone()).collect();
        excluded.extend(context.iter().filter_map(|s| s.album_id.clone()));
        let input = Input {
            candidates: &home.recommendation_candidates,
            discovery_candidates: &self.learning.discovery_albums,
            library_pool: &home.recommendation_pool,
            recently_played: &home.recently_played,
            frequently_played: &home.frequent,
            recently_added: &home.recent,
            favorite_album_ids: home.favorite_album_ids.iter().cloned().collect(),
            context_songs: &context,
            exclude_album_ids: excluded,
            listening_profile: Some(&self.learning.listening_profile),
            engagement_profile: Some(&self.learning.engagement_profile),
            ranker_model: Some(&self.learning.ranker),
            tuning: Some(self.learning.tuning),
            now: None,
            limit: Some(self.learning.tuning.slate_size as usize),
        };
        self.learning.slate = rank_album_recommendations(&input);
    }

    /// A slate preview for the tune-engine dialog (`recommendationPreviewContext`).
    /// The web opens it from Settings, where page data is emptied, so only the
    /// snapshotted candidates and pool carry over: the recent, frequent,
    /// newest and favorite lists rank as empty there, and here too.
    pub fn preview_recommendations(&self, tuning: RecommendationTuning) -> Vec<AlbumRecommendation> {
        let home = &self.home;
        rank_album_recommendations(&Input {
            candidates: &home.recommendation_candidates,
            discovery_candidates: &self.learning.discovery_albums,
            library_pool: &home.recommendation_pool,
            listening_profile: Some(&self.learning.listening_profile),
            engagement_profile: Some(&self.learning.engagement_profile),
            ranker_model: Some(&self.learning.ranker),
            tuning: Some(tuning),
            limit: Some(tuning.slate_size as usize),
            ..Default::default()
        })
    }

    /// The engine dialog's live preview, re-ranked only when the tuning or
    /// the Home inputs change.
    pub fn engine_preview(&mut self) -> &[AlbumRecommendation] {
        let signature = format!("{:?}|{}|{}", self.learning.tuning, self.learning.slate_signature, self.home.generation);
        if signature != self.learning.preview_signature {
            self.learning.preview = self.preview_recommendations(self.learning.tuning);
            self.learning.preview_signature = signature;
        }
        &self.learning.preview
    }

    pub fn is_recommended(&self, album_id: &str) -> bool {
        self.learning.slate.iter().any(|r| r.album.id == album_id)
    }

    /// Logs a shelf exposure once per album per 30-minute bucket
    /// (`observeRecommendationVisibility`). Native has no intersection
    /// observer, so a rendered shelf counts every slot as seen.
    pub fn record_impressions(&mut self) {
        if self.learning.slate.is_empty() {
            return;
        }
        let Some((engagement_key, _, impression_key)) = self.learning_keys() else { return };
        let now = now_ms();
        let bucket = (now / (30.0 * 60.0 * 1000.0)).floor() as i64;
        let stamp = (bucket, engagement_key.clone(), self.learning.slate_signature.clone(), self.learning.slate.len());
        if self.learning.impression_stamp.as_ref() == Some(&stamp) {
            return;
        }
        if !self.learning_enabled() {
            return;
        }
        self.learning.impression_stamp = Some(stamp);
        let mut events = read_engagement(&self.lget(&engagement_key), now);
        let known: HashSet<String> = events.iter().map(|e| e.id.clone()).collect();
        let mut added = Vec::new();
        for (position, rec) in self.learning.slate.iter().enumerate() {
            let album = &rec.album;
            let id = format!("seen:{}:{bucket}", album.id);
            if known.contains(&id) {
                continue;
            }
            let entity = EngagementEntity {
                kind: Some(EntityType::Album),
                id: Some(album.id.clone()),
                name: album.name.clone().or_else(|| album.title.clone()),
                artist_id: album.artist_id.clone().or_else(|| album.artist.clone()),
                genre: album.genre.clone(),
                ..Default::default()
            };
            let mut event = create_engagement_event(EngagementInput {
                kind: Some(EngagementKind::RecommendationImpression),
                at: Some(now),
                entity: Some(entity),
                position: Some(position as f64),
                ..Default::default()
            });
            event.id = id.clone();
            events.push(event);
            let mut features = rec.features.clone();
            features.insert("learned".into(), 0.0);
            added.push(PendingImpression {
                id,
                at: now,
                album_id: album.id.trim().to_lowercase(),
                artist_id: album.artist_id.as_deref().or(album.artist.as_deref()).unwrap_or("").trim().to_lowercase(),
                genres: album
                    .genre
                    .as_deref()
                    .unwrap_or("")
                    .split([',', ';', '|', '/'])
                    .map(|g| g.trim().to_lowercase())
                    .filter(|g| !g.is_empty())
                    .collect(),
                features,
                position: position as f64,
            });
        }
        if added.is_empty() {
            return;
        }
        // Exposures persist without re-summarizing, so the slate on screen
        // does not shift underneath the listener (refreshed on the next visit).
        let (_, json) = write_engagement(events, now);
        self.lset(&engagement_key, &json);
        let mut impressions = std::mem::take(&mut self.learning.impressions);
        impressions.extend(added);
        let (kept, json) = write_impressions(impressions, now);
        self.lset(&impression_key, &json);
        self.learning.impressions = kept;
    }

    /// Home visits re-read engagement so recorded exposures take effect.
    pub fn reload_engagement(&mut self) {
        if !self.learning_enabled() {
            return;
        }
        let Some((key, _, _)) = self.learning_keys() else { return };
        let now = now_ms();
        self.learning.engagement = read_engagement(&self.lget(&key), now);
        self.learning.engagement_profile = summarize_engagement(&self.learning.engagement, now);
    }

    /// `playbackSnapshot` JSON, or `None` when there is nothing to save
    /// (empty queue, or local songs).
    pub fn playback_snapshot(&self, cx: &gpui::App) -> Option<String> {
        let p = self.player.read(cx);
        // Server and local sessions are saved apart and never mixed.
        let local = self.is_local();
        if p.queue.is_empty() || p.queue.iter().any(|s| s.is_local() != local) {
            return None;
        }
        let repeat = match p.repeat {
            crate::player::Repeat::Off => "off",
            crate::player::Repeat::All => "all",
            crate::player::Repeat::One => "one",
        };
        Some(
            serde_json::json!({
                "queue": p.queue,
                "currentIndex": p.index.map(|i| i as i64).unwrap_or(-1),
                "position": p.position.max(0.0).floor(),
                "shuffle": p.shuffle,
                "repeat": repeat,
                "original": p.original,
                "wasPlaying": p.playing,
            })
            .to_string(),
        )
    }

    /// Saves the playback snapshot when it changed (the App.tsx effect keyed on
    /// queue, index, whole seconds, shuffle, repeat, quality and playing).
    pub fn save_playback_snapshot(&mut self, cx: &gpui::App) {
        if shooting() {
            return;
        }
        let Some(key) = self.snapshot_key() else { return };
        let Some(json) = self.playback_snapshot(cx) else { return };
        if json != self.last_snapshot {
            self.storage.set(&key, &json);
            self.last_snapshot = json;
        }
    }

    /// `lastPlayedSongKey` or, for a local library, `localPlaybackKey`.
    fn snapshot_key(&self) -> Option<String> {
        if self.is_local() {
            return self.local.library.as_ref().map(|l| keys::local_playback_key(&l.directory_name));
        }
        let client = self.client()?;
        Some(keys::last_played_song_key(&client.server, &client.username))
    }

    /// `readLocalPlaybackSession`: saved ids resolved against the scan.
    fn restore_local_playback(&mut self, value: &serde_json::Value, cx: &mut Context<Self>) {
        let Some(library) = self.local.library.clone() else { return };
        let Some(saved) = value.get("queue").and_then(|q| q.as_array()) else { return };
        let ids: Vec<&str> = saved
            .iter()
            .filter(|s| s.get("title").and_then(|t| t.as_str()).is_some())
            .filter_map(|s| s.get("id").and_then(|i| i.as_str()))
            .collect();
        let by_id: std::collections::HashMap<&str, &Song> = library.tracks.iter().map(|s| (s.id.as_str(), s)).collect();
        let queue: Vec<Song> = ids.iter().filter_map(|id| by_id.get(id).map(|s| (*s).clone())).collect();
        if queue.is_empty() {
            return;
        }
        let saved_index = value.get("currentIndex").and_then(|v| v.as_f64()).unwrap_or(0.0).max(0.0) as usize;
        let index = ids.get(saved_index).and_then(|id| queue.iter().position(|s| s.id == *id)).unwrap_or(0);
        let position = value.get("position").and_then(|v| v.as_f64()).filter(|p| p.is_finite()).map(|p| p.max(0.0)).unwrap_or(0.0);
        let repeat = match value.get("repeat").and_then(|v| v.as_str()) {
            Some("all") => crate::player::Repeat::All,
            Some("one") => crate::player::Repeat::One,
            _ => crate::player::Repeat::Off,
        };
        let flag = |key: &str| value.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
        let original = value.get("original").and_then(|v| v.as_bool()) != Some(false);
        let (shuffle, playing) = (flag("shuffle"), flag("wasPlaying"));
        self.player.update(cx, |p, cx| p.restore_session(queue, index, position, shuffle, repeat, original, playing, cx));
    }

    /// `readPlaybackSession` + `player.restoreSession`, once per account.
    pub fn restore_playback(&mut self, cx: &mut Context<Self>) {
        if shooting() {
            return;
        }
        let Some(key) = self.snapshot_key() else { return };
        let raw = self.storage.get(&key);
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else { return };
        if self.is_local() {
            self.restore_local_playback(&value, cx);
            return;
        }
        let valid = |v: &serde_json::Value| {
            v.get("id").and_then(|x| x.as_str()).is_some()
                && v.get("title").and_then(|x| x.as_str()).is_some()
                && serde_json::from_value::<Song>(v.clone()).is_ok_and(|s| !s.is_local())
        };
        let queue: Vec<Song> = match value.get("queue").and_then(|q| q.as_array()) {
            Some(items) => items.iter().filter(|v| valid(v)).filter_map(|v| serde_json::from_value(v.clone()).ok()).collect(),
            None => {
                // Legacy `{song}` or a bare song.
                let legacy = value.get("song").filter(|s| s.is_object()).unwrap_or(&value);
                if valid(legacy) { serde_json::from_value(legacy.clone()).ok().into_iter().collect() } else { Vec::new() }
            }
        };
        if queue.is_empty() {
            return;
        }
        let number = |key: &str| value.get(key).and_then(|v| v.as_f64()).filter(|v| v.is_finite());
        let index = number("currentIndex").map(|i| i.trunc().clamp(0.0, (queue.len() - 1) as f64) as usize).unwrap_or(0);
        let position = number("position").or_else(|| number("timestamp")).map(|p| p.max(0.0)).unwrap_or(0.0);
        let repeat = match value.get("repeat").and_then(|v| v.as_str()) {
            Some("all") => crate::player::Repeat::All,
            Some("one") => crate::player::Repeat::One,
            _ => crate::player::Repeat::Off,
        };
        let flag = |key: &str| value.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
        let original = value.get("original").and_then(|v| v.as_bool()) != Some(false);
        let (shuffle, was_playing) = (flag("shuffle"), flag("wasPlaying"));
        self.player.update(cx, |p, cx| p.restore_session(queue, index, position, shuffle, repeat, original, was_playing, cx));
    }

    /// Applies `TRANSITION_MODE_KEY` / `CROSSFADE_KEY` to the player (the
    /// App.tsx effect: crossfade length is at least 1s, 6s when unset).
    pub fn sync_transition(&mut self, cx: &mut Context<Self>) {
        let seconds = volta_logic::js::js_number(&self.storage.get(keys::CROSSFADE_KEY));
        let seconds = if seconds.is_finite() { seconds.clamp(0.0, 12.0) } else { 0.0 };
        let mode = match self.storage.get(keys::TRANSITION_MODE_KEY).as_str() {
            "automix" => crate::player::Transition::Automix,
            "crossfade" => crate::player::Transition::Crossfade(if seconds > 0.0 { seconds.max(1.0) } else { 6.0 }),
            "off" => crate::player::Transition::Off,
            _ if seconds > 0.0 => crate::player::Transition::Crossfade(seconds.max(1.0)),
            _ => crate::player::Transition::Off,
        };
        let mix_albums = self.storage.get(keys::AUTOMIX_ALBUMS_KEY) != "false";
        // Fitted planner thresholds (from the measurement lab), if present.
        let planner = volta_services::storage::data_dir()
            .and_then(|dir| std::fs::read_to_string(dir.join("automix-planner.json")).ok())
            .and_then(|text| match volta_logic::mix_planner::PlannerConfig::from_json(&text) {
                Ok(config) => Some(config),
                Err(error) => {
                    log::warn!("automix-planner.json ignored: {error}");
                    None
                }
            })
            .unwrap_or_default();
        self.player.update(cx, |p, _| {
            p.mix_albums = mix_albums;
            p.planner = planner;
            p.set_transition(mode);
        });
    }

}
