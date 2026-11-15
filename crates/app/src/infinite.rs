//! Infinite Play (`addInfinitePlay`, `toggleInfinitePlay`,
//! `removeInfinitePlaySongs` and the refill effect in App.tsx).

use std::collections::HashSet;

use gpui::{AppContext, Context};
use volta_logic::infinite_play::{
    AlbumSongGroup, infinite_play_album_target, interleave_album_songs, load_ranked_album_songs, rank_infinite_play_albums,
};
use volta_logic::model::Song;
use volta_logic::recommendations::{Input, rank_album_recommendations, song_album_candidates};
use volta_logic::storage as keys;

use crate::app_state::AppState;
use crate::components::notifications::NoticeKind;

#[derive(Default)]
pub struct InfinitePlay {
    pub busy: bool,
    /// Songs Infinite Play appended (`infinitePlaySongIds`).
    owned: HashSet<String>,
    /// `${queue.length}:${currentIndex}` of the last refill attempt.
    attempt: String,
}

enum Fetched {
    Random(Vec<Song>),
    Groups(Vec<AlbumSongGroup>),
}

impl AppState {
    pub fn infinite_enabled(&self) -> bool {
        self.storage.get(keys::INFINITE_PLAY_ENABLED_KEY) == "true"
    }

    fn infinite_count(&self) -> usize {
        let value = volta_logic::js::js_number(&self.storage.get(keys::INFINITE_PLAY_COUNT_KEY));
        if value.is_finite() { value.trunc().clamp(1.0, 200.0) as usize } else { 50 }
    }

    /// `toggleInfinitePlay`.
    pub fn toggle_infinite_play(&mut self, cx: &mut Context<Self>) {
        let next = !self.infinite_enabled();
        self.storage.set(keys::INFINITE_PLAY_ENABLED_KEY, if next { "true" } else { "false" });
        if next {
            self.add_infinite_play(cx);
        } else {
            self.remove_infinite_play_songs(cx);
        }
        cx.notify();
    }

    /// `removeInfinitePlaySongs`: drops upcoming songs Infinite Play added.
    fn remove_infinite_play_songs(&mut self, cx: &mut Context<Self>) {
        let owned = std::mem::take(&mut self.infinite.owned);
        self.infinite.attempt.clear();
        if owned.is_empty() {
            return;
        }
        self.player.update(cx, |p, cx| {
            let current = p.index.unwrap_or(0);
            let indices: Vec<usize> = p.queue.iter().enumerate().filter(|(i, s)| *i > current && owned.contains(&s.id)).map(|(i, _)| i).collect();
            for index in indices.into_iter().rev() {
                p.remove_from_queue(index, cx);
            }
        });
    }

    /// The refill effect: when two or fewer songs remain, add more once per
    /// queue position.
    pub fn check_infinite_play(&mut self, cx: &mut Context<Self>) {
        if !self.infinite_enabled() || self.client().is_none() {
            return;
        }
        let player = self.player.read(cx);
        let Some(index) = player.index.filter(|i| *i < player.queue.len()) else { return };
        let remaining = player.queue.len() - index - 1;
        if remaining > 2 {
            self.infinite.attempt.clear();
            return;
        }
        let key = format!("{}:{index}", player.queue.len());
        if self.infinite.attempt == key {
            return;
        }
        self.infinite.attempt = key;
        self.add_infinite_play(cx);
    }

    /// `addInfinitePlay`.
    pub fn add_infinite_play(&mut self, cx: &mut Context<Self>) {
        if self.infinite.busy || self.is_local() {
            return;
        }
        let Some(client) = self.client() else { return };
        self.infinite.busy = true;
        cx.notify();
        let count = self.infinite_count();
        let random_mode = self.storage.get(keys::INFINITE_PLAY_MODE_KEY) == "random";
        let player = self.player.read(cx);
        let existing: HashSet<String> = player.queue.iter().map(|s| s.id.clone()).collect();
        let queued_albums: HashSet<String> =
            player.queue.iter().filter_map(|s| s.album_id.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)).collect();
        let shelf: Vec<String> = self.learning.slate.iter().map(|r| r.album.id.clone()).collect();
        let pool: Vec<String> = self.home.recommendation_pool.iter().map(|a| a.id.clone()).collect();
        let fetch_client = client.clone();
        let fetch_existing = existing.clone();
        let task = cx.background_spawn(async move {
            if random_mode {
                return Ok::<_, volta_services::NavidromeError>(Fetched::Random(random_fill(&fetch_client, count, &fetch_existing)));
            }
            let ranked = rank_infinite_play_albums(&shelf, &pool, &queued_albums, None);
            let groups = load_ranked_album_songs(&ranked, infinite_play_album_target(count), count.max(20) * 2, &queued_albums, |offset, size| {
                fetch_client.songs(offset as u32, size as u32)
            })?;
            Ok(Fetched::Groups(groups))
        });
        cx.spawn(async move |this, cx| {
            let fetched = task.await;
            let plan = this.update(cx, |state, cx| match fetched {
                Err(_) => None,
                Ok(Fetched::Random(songs)) => {
                    Some(songs.into_iter().filter(|s| !state.is_song_artist_muted(s)).collect::<Vec<_>>())
                }
                Ok(Fetched::Groups(groups)) => Some(state.rank_infinite_groups(groups, count, &existing, cx)),
            });
            let Ok(Some(mut candidates)) = plan else {
                let _ = this.update(cx, |state, cx| {
                    state.infinite.busy = false;
                    state.notify("Infinite Play could not load more songs.", NoticeKind::Error, cx);
                });
                return;
            };
            // Last resort: random songs, so a fill never silently adds nothing.
            if !random_mode && candidates.len() < count {
                let mut picked: HashSet<String> = existing.clone();
                picked.extend(candidates.iter().map(|s| s.id.clone()));
                let wanted = count - candidates.len();
                let filler_client = client.clone();
                let filler = cx.background_spawn(async move { random_fill(&filler_client, wanted, &picked) }).await;
                candidates.extend(filler);
            }
            let _ = this.update(cx, |state, cx| {
                state.infinite.busy = false;
                let mut seen = existing.clone();
                let additions: Vec<Song> = candidates
                    .into_iter()
                    .filter(|s| !state.is_song_artist_muted(s) && seen.insert(s.id.clone()))
                    .take(count)
                    .collect();
                let added = additions.len();
                state.infinite.owned.extend(additions.iter().map(|s| s.id.clone()));
                state.player.update(cx, |p, cx| {
                    for song in additions {
                        p.append(song, false, cx);
                    }
                });
                if added > 0 {
                    state.notify(format!("{added} song{} added to queue", if added == 1 { "" } else { "s" }), NoticeKind::Success, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Ranks the retrieved album groups with the recommender and interleaves
    /// one track per album per round.
    fn rank_infinite_groups(&mut self, groups: Vec<AlbumSongGroup>, count: usize, existing: &HashSet<String>, cx: &mut Context<Self>) -> Vec<Song> {
        let allowed: Vec<AlbumSongGroup> = groups
            .into_iter()
            .map(|g| AlbumSongGroup { songs: g.songs.into_iter().filter(|s| !self.is_song_artist_muted(s)).collect(), ..g })
            .filter(|g| !g.songs.is_empty())
            .collect();
        let candidates: Vec<_> = allowed
            .iter()
            .filter_map(|g| {
                self.home
                    .recommendation_pool
                    .iter()
                    .find(|a| a.id == g.album_id)
                    .cloned()
                    .or_else(|| song_album_candidates(&g.songs).into_iter().next())
            })
            .collect();
        let player = self.player.read(cx);
        let start = player.index.unwrap_or(0);
        let context: Vec<Song> = player.queue.iter().skip(start).take(4).cloned().collect();
        let ranked = rank_album_recommendations(&Input {
            candidates: &candidates,
            context_songs: &context,
            listening_profile: Some(&self.learning.listening_profile),
            engagement_profile: Some(&self.learning.engagement_profile),
            ranker_model: Some(&self.learning.ranker),
            tuning: Some(self.learning.tuning),
            limit: Some(allowed.len()),
            ..Default::default()
        });
        let ordered: Vec<AlbumSongGroup> =
            ranked.iter().filter_map(|r| allowed.iter().find(|g| g.album_id == r.album.id).cloned()).collect();
        interleave_album_songs(&ordered, count, existing)
    }
}

/// `fillWithRandom`: up to three random batches, skipping known songs.
fn random_fill(client: &volta_services::Navidrome, wanted: usize, known: &HashSet<String>) -> Vec<Song> {
    let mut picked = known.clone();
    let mut collected = Vec::new();
    for _ in 0..3 {
        if collected.len() >= wanted {
            break;
        }
        let batch = client.random_songs((wanted * 2).max(40) as u32).unwrap_or_default();
        if batch.is_empty() {
            break;
        }
        for song in batch {
            if collected.len() >= wanted {
                break;
            }
            if song.id.is_empty() || !picked.insert(song.id.clone()) {
                continue;
            }
            collected.push(song);
        }
    }
    collected
}
