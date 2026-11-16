//! The developer AutoMix preview. It picks two songs from the library (by
//! outcome category, or at random), analyzes them, and plays just the join
//! between them (a few seconds of the first song, the transition, a few
//! seconds of the second) on an audio engine of its own, so the queue and
//! the main player are left alone. The plan of the current planner and of
//! the previous one can be A/B'd, and either rendered offline to a WAV.

use std::time::{Duration, Instant};

use rand::seq::SliceRandom;
use gpui::{AnyElement, AsyncApp, Context, Entity, FontWeight, IntoElement, ParentElement, Render, Styled, Task, WeakEntity, Window, deferred, div, prelude::*, relative};
use volta_audio::{AudioHandle, BlendRequest, Command, Event, LoadRequest, TrackSource};
use volta_logic::mix_analysis::{MixProfile, analyze_track};
use volta_logic::mix_planner::{MixPlan, PairContext, PlannerConfig, TransitionKind, beatmatch_blocker, plan_transition};
use volta_logic::model::Song;
use volta_services::Navidrome;

use crate::app_state::AppState;
use crate::components::ui::icon;
use crate::theme::{self, Tokens, sz};
use crate::views::library::secondary_button;

/// Seconds of the first song heard before the blend, and of the second after it.
const LEAD: f64 = 3.0;
const TAIL: f64 = 3.0;
/// The clip is faded out over this long at its end.
const FADE_OUT: f64 = 0.6;
/// Longest wait for both songs to open and decode before giving up.
const LOAD_TIMEOUT: Duration = Duration::from_secs(25);
const MAX_ANALYSIS_BYTES: f64 = 256.0 * 1024.0 * 1024.0;

struct Chosen {
    from: Song,
    to: Song,
    a: MixProfile,
    b: MixProfile,
    /// The current planner's plan and the previous planner's.
    plan: MixPlan,
    legacy: MixPlan,
}

impl Chosen {
    fn plan(&self, old: bool) -> &MixPlan {
        if old { &self.legacy } else { &self.plan }
    }
}

/// Both planners on a pair, the current one with the player's settings and
/// the same context the player gives it.
fn plans(from: &Song, to: &Song, a: &MixProfile, b: &MixProfile, config: &PlannerConfig) -> (MixPlan, MixPlan) {
    let context = PairContext {
        album_in_order: volta_logic::automix::is_album_sequence(&[from, to]),
        from_genre: from.genre.clone(),
        to_genre: to.genre.clone(),
        same_song: from.id == to.id,
    };
    (plan_transition(a, b, &context, config), volta_logic::mix_planner_legacy::plan_mix(a, b).to_mix_plan())
}

enum Stage {
    Picking,
    /// Working on it: the text says what.
    Analyzing(String),
    Ready,
    Loading,
    Playing,
    Failed(String),
}

/// One run of the clip.
struct Run {
    tokens: (u64, u64),
    since: Instant,
    started: bool,
    playing: bool,
    advanced_at: Option<Instant>,
    volume: f32,
}

pub struct AutomixPreview {
    state: Entity<AppState>,
    stage: Stage,
    chosen: Option<Chosen>,
    audio: Option<AudioHandle>,
    run: Option<Run>,
    next_token: u64,
    mode: Mode,
    mode_open: bool,
    /// Play/export the previous planner's plan instead of the current one.
    old: bool,
    /// The planner settings the player uses.
    config: PlannerConfig,
    /// Result of the last WAV export.
    export: Option<String>,
    /// Bumped by every pick, so a slow earlier pick cannot overwrite a newer one.
    generation: u64,
    _poll: Task<()>,
}

fn clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    format!("{}:{:04.1}", (seconds / 60.0) as u64, seconds % 60.0)
}

fn camelot(profile: &MixProfile) -> String {
    format!("{}{}", profile.camelot, if profile.minor { "A" } else { "B" })
}


/// The same sources the player uses for a song. `None` when the song cannot be played from here.
fn sources(client: Option<&Navidrome>, song: &Song, original: bool) -> Option<(TrackSource, Option<TrackSource>, Option<String>)> {
    if song.is_local() {
        let path = song.local_file_path.clone().or(song.local_path.clone())?;
        return Some((TrackSource::File(path.into()), None, song.suffix.clone()));
    }
    let client = client?;
    let raw = TrackSource::Url(client.stream_url(&song.id, true));
    let mp3 = TrackSource::Url(client.stream_url(&song.id, false));
    Some(if original { (raw, Some(mp3), song.suffix.clone()) } else { (mp3, None, Some("mp3".into())) })
}

/// Long enough to mix, small enough to analyze, and playable from here.
fn usable(s: &Song) -> bool {
    s.duration.is_none_or(|d| (60.0..=900.0).contains(&d)) && s.size.unwrap_or(0.0) <= MAX_ANALYSIS_BYTES && (!s.is_local() || s.local_file_path.is_some() || s.local_path.is_some())
}

/// Two different songs, from different albums when possible, that are long
/// enough to mix. `songs` should already be in random order.
fn choose_pair(songs: Vec<Song>) -> Option<(Song, Song)> {
    if let Some((a, b)) = pick_across_albums(&songs) {
        return Some((a, b));
    }
    // A one-album library: any two different songs.
    let mut songs = songs.into_iter();
    let first = songs.next()?;
    let second = songs.find(|s| s.id != first.id)?;
    Some((first, second))
}

fn pick_across_albums(songs: &[Song]) -> Option<(Song, Song)> {
    let mut songs = songs.iter().cloned();
    let first = songs.next()?;
    let second = songs.find(|s| s.id != first.id && (s.album_id.is_none() || s.album_id != first.album_id))?;
    Some((first, second))
}

/// What kind of pair to look for: any, one outcome of the current planner,
/// or a pair the two planners treat differently.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Random,
    Kind(TransitionKind),
    Disagree,
}

const MODES: [Mode; 7] = [
    Mode::Random,
    Mode::Kind(TransitionKind::BeatMatched),
    Mode::Kind(TransitionKind::Extend),
    Mode::Kind(TransitionKind::EnergyCrossfade),
    Mode::Kind(TransitionKind::TrimOnly),
    Mode::Kind(TransitionKind::NoMix),
    Mode::Disagree,
];
/// Songs analyzed per search round, and rounds before giving up.
const POOL: usize = 8;
const ROUNDS: usize = 3;

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Mode::Random => "Any two songs",
            Mode::Kind(TransitionKind::BeatMatched) => "Beat-matched blend",
            Mode::Kind(TransitionKind::Extend) => "Extended (looped outro)",
            Mode::Kind(TransitionKind::EnergyCrossfade) => "Energy crossfade",
            Mode::Kind(TransitionKind::TrimOnly) => "Silence trimmed",
            Mode::Kind(TransitionKind::NoMix) => "Not mixed (gapless)",
            Mode::Kind(TransitionKind::Filter) => "Filter transition",
            Mode::Disagree => "Old and new planners disagree",
        }
    }

    /// How well a pair fits this mode; `None` when it does not.
    fn score(self, plan: &MixPlan, legacy: &MixPlan) -> Option<f64> {
        match self {
            Mode::Random => Some(0.0),
            Mode::Kind(kind) => (plan.kind == kind).then(|| plan.score + 0.1 * rand::random::<f64>()),
            Mode::Disagree => (plan.kind != legacy.kind).then(rand::random::<f64>),
        }
    }
}

struct Entry {
    song: Song,
    profile: MixProfile,
}

/// The best-fitting ordered pair in `pool` for `mode`, from different albums.
fn best_pair(pool: &[Entry], mode: Mode, config: &PlannerConfig) -> Option<(usize, usize)> {
    let mut best: Option<(f64, usize, usize)> = None;
    for (i, a) in pool.iter().enumerate() {
        for (j, b) in pool.iter().enumerate() {
            if i == j || a.song.id == b.song.id || (a.song.album_id.is_some() && a.song.album_id == b.song.album_id) {
                continue;
            }
            let (plan, legacy) = plans(&a.song, &b.song, &a.profile, &b.profile, config);
            if let Some(score) = mode.score(&plan, &legacy)
                && best.is_none_or(|(top, ..)| score > top)
            {
                best = Some((score, i, j));
            }
        }
    }
    best.map(|(_, i, j)| (i, j))
}

/// A profile that is either known or being measured.
enum Slot {
    Ready(Option<MixProfile>),
    Pending(Task<Option<MixProfile>>),
}

impl AutomixPreview {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let busy = this.update(cx, |view, cx| view.tick(cx));
                match busy {
                    Ok(true) => cx.background_executor().timer(Duration::from_millis(40)).await,
                    Ok(false) => cx.background_executor().timer(Duration::from_millis(200)).await,
                    Err(_) => break,
                }
            }
        });
        let config = {
            let player = state.read(cx).player.read(cx);
            PlannerConfig { mix_albums: player.mix_albums, ..player.planner.clone() }
        };
        let mut view = Self { state, stage: Stage::Picking, chosen: None, audio: None, run: None, next_token: 1, mode: Mode::Random, mode_open: false, old: false, config, export: None, generation: 0, _poll: poll };
        view.pick(cx);
        view
    }

    fn stop_audio(&mut self) {
        if let Some(audio) = self.audio.as_ref() {
            audio.send(Command::Stop);
        }
        self.run = None;
    }

    /// Looks for a new pair of the chosen kind and analyzes it.
    fn pick(&mut self, cx: &mut Context<Self>) {
        self.stop_audio();
        self.chosen = None;
        self.export = None;
        self.generation += 1;
        let generation = self.generation;
        let mode = self.mode;
        let config = self.config.clone();
        // Local library: pick from the scanned tracks. Otherwise ask the server.
        let state = self.state.read(cx);
        let local = state.local.active.then(|| state.local.library.clone()).flatten();
        let client = state.client_handle();
        if local.is_none() && client.is_none() {
            self.stage = Stage::Failed("Connect to a server or open a local library to pick songs.".into());
            cx.notify();
            return;
        }
        self.stage = Stage::Picking;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let songs = cx
                .background_executor()
                .spawn(async move {
                    let mut songs = match (local, client) {
                        (Some(library), _) => library.tracks.clone(),
                        (None, Some(client)) => client.random_songs(90).unwrap_or_default(),
                        (None, None) => Vec::new(),
                    };
                    songs.shuffle(&mut rand::rng());
                    songs
                })
                .await;
            let result = Self::search(&this, cx, songs, mode, &config, generation).await;
            let _ = this.update(cx, |view, cx| {
                if view.generation != generation {
                    return;
                }
                match result {
                    Ok(chosen) => {
                        view.chosen = Some(chosen);
                        view.stage = Stage::Ready;
                    }
                    Err(message) => view.stage = Stage::Failed(message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows progress; false when a newer pick has replaced this one.
    fn progress(this: &WeakEntity<Self>, cx: &mut AsyncApp, generation: u64, text: String) -> bool {
        this.update(cx, |view, cx| {
            let current = view.generation == generation;
            if current {
                view.stage = Stage::Analyzing(text);
                cx.notify();
            }
            current
        })
        .unwrap_or(false)
    }

    async fn search(this: &WeakEntity<Self>, cx: &mut AsyncApp, songs: Vec<Song>, mode: Mode, config: &PlannerConfig, generation: u64) -> Result<Chosen, String> {
        const CLOSED: &str = "The window was closed.";
        let songs: Vec<Song> = songs.into_iter().filter(usable).collect();
        let chosen = |from: Song, to: Song, a: MixProfile, b: MixProfile| {
            let (plan, legacy) = plans(&from, &to, &a, &b, config);
            Chosen { from, to, a, b, plan, legacy }
        };
        if mode == Mode::Random {
            let (from, to) = choose_pair(songs).ok_or("Could not find two songs of 1 to 15 minutes in the library.")?;
            if !Self::progress(this, cx, generation, format!("Analyzing {} and {}…", from.title, to.title)) {
                return Err(CLOSED.into());
            }
            let profiles = Self::profiles(this, cx, &[from.clone(), to.clone()]).await.ok_or(CLOSED)?;
            return match <[Option<MixProfile>; 2]>::try_from(profiles) {
                Ok([Some(a), Some(b)]) => Ok(chosen(from, to, a, b)),
                _ => Err("One of the songs could not be analyzed. Pick again.".into()),
            };
        }
        // Analyze a batch at a time and look for the wanted kind among everything analyzed so far.
        let mut pool: Vec<Entry> = Vec::new();
        let mut remaining = songs.into_iter();
        for round in 1..=ROUNDS {
            let batch: Vec<Song> = remaining.by_ref().take(POOL).collect();
            if batch.is_empty() {
                break;
            }
            let text = format!("Looking for “{}”: analyzing {} songs (round {round} of {ROUNDS})…", mode.label(), batch.len());
            if !Self::progress(this, cx, generation, text) {
                return Err(CLOSED.into());
            }
            let profiles = Self::profiles(this, cx, &batch).await.ok_or(CLOSED)?;
            for (song, profile) in batch.into_iter().zip(profiles) {
                if let Some(profile) = profile {
                    pool.push(Entry { song, profile });
                }
            }
            if let Some((i, j)) = best_pair(&pool, mode, config) {
                let (from, to) = (pool[i].song.clone(), pool[j].song.clone());
                return Ok(chosen(from, to, pool[i].profile.clone(), pool[j].profile.clone()));
            }
        }
        Err(format!("No “{}” pair turned up among {} songs. Try again, or pick another kind.", mode.label(), pool.len()))
    }

    /// Cached profiles are used as they are; the rest start measuring.
    fn start_profiles(&mut self, songs: &[Song], cx: &mut Context<Self>) -> Vec<(String, Slot)> {
        let state = self.state.read(cx);
        let client = state.client_handle();
        let (server, username) = client.as_ref().map(|c| (c.server.clone(), c.username.clone())).unwrap_or_default();
        let original = state.player.read(cx).original;
        let mut slots = Vec::new();
        for song in songs {
            let key = crate::automix_analysis::analysis_key(&server, &username, song, original);
            let state = self.state.read(cx);
            let known = state.player.read(cx).profiles.get(&key).cloned().or_else(|| state.read_profile(&key));
            let slot = match known {
                Some(profile) => Slot::Ready(Some(profile)),
                None => match sources(client.as_deref(), song, original) {
                    Some((source, _, extension)) => Slot::Pending(cx.background_executor().spawn(async move {
                        let mono = volta_audio::decode_mono_11k(&source, extension.as_deref(), &volta_audio::http_client())?;
                        analyze_track(&mono)
                    })),
                    None => Slot::Ready(None),
                },
            };
            slots.push((key, slot));
        }
        slots
    }

    /// Profiles for `songs`, measured together and cached; `None` where analysis failed.
    async fn profiles(this: &WeakEntity<Self>, cx: &mut AsyncApp, songs: &[Song]) -> Option<Vec<Option<MixProfile>>> {
        let slots = this.update(cx, |view, cx| view.start_profiles(songs, cx)).ok()?;
        let (mut profiles, mut fresh) = (Vec::new(), Vec::new());
        for (key, slot) in slots {
            match slot {
                Slot::Ready(profile) => profiles.push(profile),
                Slot::Pending(task) => {
                    let profile = task.await;
                    if let Some(profile) = profile.clone() {
                        fresh.push((key, profile));
                    }
                    profiles.push(profile);
                }
            }
        }
        this.update(cx, |view, cx| {
            for (key, profile) in fresh {
                view.state.update(cx, |state, cx| state.store_profile(key, profile, cx));
            }
        })
        .ok()?;
        Some(profiles)
    }

    /// Plays the clip: lead-in, the blend, tail. Also used to listen again.
    fn listen(&mut self, cx: &mut Context<Self>) {
        let Some(chosen) = self.chosen.as_ref() else { return };
        let client = self.state.read(cx).client_handle();
        // The main player would talk over the clip.
        let player = self.state.read(cx).player.clone();
        let (original, volume) = {
            let p = player.read(cx);
            (p.original, p.volume as f32)
        };
        if player.read(cx).playing {
            player.update(cx, |p, cx| p.pause(cx));
        }
        let plan = chosen.plan(self.old);
        // A natural handoff has no overlap: play the first song's last
        // seconds and let the second follow gaplessly.
        let natural = !plan.mixes();
        let lead_from = if natural { (chosen.a.duration - LEAD).max(0.0) } else { (plan.out_start - LEAD).max(0.0) };
        let in_from = if natural { 0.0 } else { plan.in_start };
        let (tokens, request) = {
            let tokens = (self.next_token, self.next_token + 1);
            self.next_token += 2;
            let (Some((from_source, from_fallback, from_extension)), Some((to_source, to_fallback, to_extension))) =
                (sources(client.as_deref(), &chosen.from, original), sources(client.as_deref(), &chosen.to, original))
            else {
                self.stage = Stage::Failed("One of the songs has no playable file. Pick again.".into());
                cx.notify();
                return;
            };
            (
                tokens,
                (
                    LoadRequest {
                        token: tokens.0,
                        source: from_source,
                        fallback: from_fallback,
                        extension: from_extension,
                        start_at: lead_from,
                        autoplay: false,
                        gain: 1.0,
                    },
                    LoadRequest {
                        token: tokens.1,
                        source: to_source,
                        fallback: to_fallback,
                        extension: to_extension,
                        start_at: in_from,
                        autoplay: true,
                        gain: 1.0,
                    },
                ),
            )
        };
        let audio = self.audio.get_or_insert_with(AudioHandle::spawn);
        // Drain events left from an earlier run.
        while audio.try_event().is_some() {}
        audio.send(Command::Stop);
        audio.send(Command::SetVolume(volume));
        audio.send(Command::Load(request.0));
        audio.send(Command::Preload(request.1));
        self.run = Some(Run { tokens, since: Instant::now(), started: false, playing: false, advanced_at: None, volume });
        self.stage = Stage::Loading;
        cx.notify();
    }

    /// Renders the selected plan offline (the product's own mixer, no audio
    /// device) and writes it as a WAV in the data folder.
    fn export(&mut self, cx: &mut Context<Self>) {
        let Some(chosen) = self.chosen.as_ref() else { return };
        let state = self.state.read(cx);
        let client = state.client_handle();
        let original = state.player.read(cx).original;
        let (Some((a_source, _, a_ext)), Some((b_source, _, b_ext))) = (sources(client.as_deref(), &chosen.from, original), sources(client.as_deref(), &chosen.to, original)) else {
            self.export = Some("One of the songs has no playable file.".into());
            cx.notify();
            return;
        };
        let plan = chosen.plan(self.old).clone();
        let (duration_a, natural) = (chosen.a.duration, !plan.mixes());
        let name = format!(
            "{} -> {} ({}).wav",
            chosen.from.title.replace(['/', '\\'], "-"),
            chosen.to.title.replace(['/', '\\'], "-"),
            if self.old { "old" } else { "new" }
        );
        let Some(dir) = volta_services::storage::data_dir().map(|d| d.join("automix-exports")) else {
            self.export = Some("No data folder to write to.".into());
            cx.notify();
            return;
        };
        self.export = Some("Rendering…".into());
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            const RATE: u32 = 44_100;
            let http = volta_audio::http_client();
            let a = volta_audio::decode_stereo(&a_source, a_ext.as_deref(), RATE, &http).ok_or("could not decode the first song")?;
            let b = volta_audio::decode_stereo(&b_source, b_ext.as_deref(), RATE, &http).ok_or("could not decode the second song")?;
            let request = if natural {
                // Gapless: a 1 ms join at the end of the file.
                volta_audio::offline::RenderRequest { out_start: duration_a - 0.001, in_start: 0.0, seconds: 0.001, automation: volta_audio::TransitionAutomation::equal_power(), lead: 6.0, tail: 6.0, out_gain: 1.0, in_gain: 1.0 }
            } else {
                volta_audio::offline::RenderRequest { out_start: plan.out_start, in_start: plan.in_start, seconds: plan.seconds, automation: plan.automation, lead: 6.0, tail: 6.0, out_gain: 1.0, in_gain: 1.0 }
            };
            let rendered = volta_audio::offline::render_transition(&a, &b, RATE, &request, false);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join(name);
            std::fs::write(&path, volta_audio::offline::wav_bytes(&rendered.mix, RATE)).map_err(|e| e.to_string())?;
            Ok::<_, String>(path)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |view, cx| {
                view.export = Some(match result {
                    Ok(path) => format!("Exported to {}", path.display()),
                    Err(error) => format!("Export failed: {error}"),
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        self.stop_audio();
        self.stage = Stage::Failed(message);
        cx.notify();
    }

    /// Advances a running clip; true while there is something to follow.
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        let (Some(audio), Some(run), Some(chosen)) = (self.audio.as_ref(), self.run.as_mut(), self.chosen.as_ref()) else { return false };
        let mut error = None;
        let mut ended = false;
        while let Some(event) = audio.try_event() {
            match event {
                Event::Started { token } if token == run.tokens.0 => run.started = true,
                Event::Advanced { token, .. } if token == run.tokens.1 => run.advanced_at = Some(Instant::now()),
                Event::Ended { token } if token == run.tokens.0 || token == run.tokens.1 => ended = true,
                Event::Error { token, message } if token == run.tokens.0 || token == run.tokens.1 => error = Some(message),
                _ => {}
            }
        }
        if let Some(message) = error {
            self.fail(format!("Playback failed: {message}"), cx);
            return false;
        }
        let plan = chosen.plan(self.old);
        if !run.playing {
            if run.since.elapsed() > LOAD_TIMEOUT {
                self.fail("The songs took too long to load. Pick again.".into(), cx);
                return false;
            }
            if run.started && !plan.mixes() {
                // Gapless: the second song is preloaded and follows on its own.
                audio.send(Command::Play);
                run.playing = true;
                run.since = Instant::now();
                self.stage = Stage::Playing;
                cx.notify();
            } else if run.started {
                // The engine takes the blend once the second song has decoded;
                // keep offering it, then start the clip.
                audio.send(Command::Blend(BlendRequest {
                    token: run.tokens.1,
                    seconds: plan.seconds,
                    start_at: Some(plan.out_start),
                    automation: plan.automation,
                }));
                if audio.is_blending() {
                    audio.send(Command::Play);
                    run.playing = true;
                    run.since = Instant::now();
                    self.stage = Stage::Playing;
                    cx.notify();
                }
            }
            return true;
        }
        let finished = match run.advanced_at {
            Some(at) => {
                let remaining = TAIL - at.elapsed().as_secs_f64();
                if remaining < FADE_OUT {
                    audio.send(Command::SetVolume(run.volume * (remaining / FADE_OUT).clamp(0.0, 1.0) as f32));
                }
                remaining <= 0.0
            }
            // Never run longer than the clip can possibly last.
            None => run.since.elapsed().as_secs_f64() > LEAD + plan.seconds + 20.0,
        };
        if finished || ended {
            self.stop_audio();
            self.stage = Stage::Ready;
            if let Some(audio) = self.audio.as_ref() {
                audio.send(Command::SetVolume(self.state.read(cx).player.read(cx).volume as f32));
            }
        }
        cx.notify();
        !(finished || ended)
    }
}

fn line(tokens: &Tokens, label: &str, value: impl Into<String>) -> AnyElement {
    div()
        .flex()
        .justify_between()
        .gap(sz(12.0))
        .py(sz(3.0))
        .text_size(sz(12.0))
        .child(div().text_color(tokens.muted).child(label.to_string()))
        .child(div().min_w_0().text_right().font_weight(FontWeight::MEDIUM).child(value.into()))
        .into_any_element()
}

fn card(tokens: &Tokens, title: &str, rows: Vec<AnyElement>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(sz(2.0))
        .p(sz(12.0))
        .rounded(sz(8.0))
        .bg(tokens.surface)
        .child(div().pb(sz(4.0)).text_size(sz(11.0)).font_weight(FontWeight::BOLD).text_color(tokens.secondary).child(title.to_string()))
        .children(rows)
}

fn song_rows(tokens: &Tokens, song: &Song, profile: &MixProfile) -> Vec<AnyElement> {
    vec![
        div()
            .pb(sz(4.0))
            .text_size(sz(13.0))
            .font_weight(FontWeight::MEDIUM)
            .child(format!("{} · {}", song.title, song.artist.as_deref().unwrap_or("Unknown artist")))
            .into_any_element(),
        line(tokens, "Tempo", format!("{:.1} BPM · beats {:.0}%", profile.bpm, profile.beat_confidence * 100.0)),
        line(tokens, "Key", format!("{} · confidence {:.0}%", camelot(profile), profile.key_confidence * 100.0)),
        line(tokens, "Loudness", format!("{:.1} dB", profile.loudness_db)),
        line(tokens, "Audible", format!("{} – {} of {}", clock(profile.start), clock(profile.end), clock(profile.duration))),
    ]
}

/// A song's audible span with the part used by the blend highlighted.
fn track_bar(tokens: &Tokens, id: &'static str, duration: f64, audible: (f64, f64), blend: (f64, f64), playhead: Option<f64>) -> AnyElement {
    let at = |t: f64| (t / duration).clamp(0.0, 1.0) as f32;
    let (audible_from, audible_to) = (at(audible.0), at(audible.1));
    let (blend_from, blend_to) = (at(blend.0), at(blend.1));
    div()
        .id(id)
        .relative()
        .h(sz(22.0))
        .rounded(sz(4.0))
        .bg(tokens.hover)
        .overflow_hidden()
        .child(div().absolute().top_0().bottom_0().left(relative(audible_from)).w(relative((audible_to - audible_from).max(0.0))).bg(tokens.line))
        .child(div().absolute().top_0().bottom_0().left(relative(blend_from)).w(relative((blend_to - blend_from).max(0.004))).bg(tokens.accent))
        .when_some(playhead, |d, position| d.child(div().absolute().top_0().bottom_0().left(relative(at(position))).w(sz(2.0)).bg(tokens.text)))
        .into_any_element()
}

fn plan_rows(tokens: &Tokens, plan: &MixPlan, from: &MixProfile, to: &MixProfile) -> Vec<AnyElement> {
    let mut rows = vec![line(tokens, "Outcome", plan.kind.name()), line(tokens, "Label", plan.label.clone()), line(tokens, "Why", plan.reason.clone())];
    if !matches!(plan.kind, TransitionKind::BeatMatched | TransitionKind::Extend) {
        rows.push(line(tokens, "Why not beat-matched", beatmatch_blocker(from, to).unwrap_or_else(|| "Another outcome scored higher".into())));
    }
    if !plan.mixes() {
        return rows;
    }
    let a = &plan.automation;
    rows.extend([
        line(tokens, "Starts at", format!("{} in the first song ({:.1} s before its end)", clock(plan.out_start), from.end - plan.out_start)),
        line(tokens, "Second song joins at", format!("{} ({:.1} s skipped)", clock(plan.in_start), (plan.in_start - to.start).max(0.0))),
        line(tokens, "Overlap", format!("{:.1} s{}", plan.seconds, if plan.bars > 0 { format!(" · {} bars", plan.bars) } else { String::new() })),
        line(tokens, "Second song tempo", format!("×{:.3} ({:+.1}%)", a.incoming.rate.first(), (f64::from(a.incoming.rate.first()) - 1.0) * 100.0)),
        line(tokens, "Tempo ratio", format!("{:.3} ({:.1} → {:.1} BPM)", volta_logic::mix_planner::folded_ratio(from.bpm, to.bpm), from.bpm, to.bpm)),
        line(tokens, "EQ handover", if a.outgoing.uses_bands() || a.incoming.uses_bands() { "3-band (bass swap)" } else { "none" }),
        line(tokens, "Filters", if a.outgoing.uses_filters() || a.incoming.uses_filters() { "low-pass out / high-pass in" } else { "none" }),
        line(tokens, "Second song level", format!("{:+.1} dB at the start", 20.0 * f64::from(a.incoming.gain.first().max(1e-6)).log10().max(-60.0))),
    ]);
    if let Some(region) = a.out_loop {
        rows.push(line(tokens, "Loop", format!("{:.2} s from {} × {} more", region.length, clock(region.start), region.repeats)));
    }
    if !plan.candidates.is_empty() {
        let list: Vec<String> = plan.candidates.iter().map(|c| format!("{} {:.2}", c.kind.name(), c.score)).collect();
        rows.push(line(tokens, "Candidates", list.join(" · ")));
    }
    rows
}

impl Render for AutomixPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx).clone();
        let busy = matches!(self.stage, Stage::Loading | Stage::Playing);
        let (position, blending) = self.audio.as_ref().map_or((0.0, false), |a| (a.position(), a.is_blending()));
        let advanced = self.run.as_ref().is_some_and(|r| r.advanced_at.is_some());

        let status: String = match &self.stage {
            Stage::Picking => "Picking two songs from your library…".into(),
            Stage::Analyzing(text) => text.clone(),
            Stage::Ready => format!("Plays {LEAD:.0} s before the blend and {TAIL:.0} s after it."),
            Stage::Loading => "Loading the clip…".into(),
            Stage::Playing if advanced => "Playing the end of the clip: the second song.".into(),
            Stage::Playing if blending => "Blending…".into(),
            Stage::Playing => "Playing the first song, the blend is coming.".into(),
            Stage::Failed(message) => message.clone(),
        };
        let failed = matches!(self.stage, Stage::Failed(_));

        let mut body = div().flex().flex_col().gap(sz(12.0));
        if let Some(chosen) = self.chosen.as_ref() {
            let duration = chosen.a.duration.max(1.0);
            let incoming = chosen.b.duration.max(1.0);
            let plan = chosen.plan(self.old);
            body = body
                .child(card(&tokens, "FIRST SONG", song_rows(&tokens, &chosen.from, &chosen.a)))
                .child(card(&tokens, "SECOND SONG", song_rows(&tokens, &chosen.to, &chosen.b)))
                .child(card(&tokens, if self.old { "TRANSITION · OLD PLANNER" } else { "TRANSITION · NEW PLANNER" }, plan_rows(&tokens, plan, &chosen.a, &chosen.b)))
                .child(card(
                    &tokens,
                    "TIMELINE",
                    vec![
                        track_bar(
                            &tokens,
                            "mix-out-bar",
                            duration,
                            (chosen.a.start, chosen.a.end),
                            (plan.out_start, plan.out_start + plan.seconds),
                            (self.run.is_some() && !advanced).then_some(position),
                        ),
                        track_bar(
                            &tokens,
                            "mix-in-bar",
                            incoming,
                            (chosen.b.start, chosen.b.end),
                            (plan.in_start, plan.in_start + plan.seconds * plan.tempo),
                            advanced.then_some(position),
                        ),
                        div()
                            .pt(sz(4.0))
                            .text_size(sz(11.0))
                            .text_color(tokens.muted)
                            .child("Top: first song. Bottom: second song. Pale = audible, red = blended.")
                            .into_any_element(),
                    ],
                ));
        }

        // Which kind of pair to look for.
        let mode = self.mode;
        let menu = self.mode_open.then(|| {
            let items = MODES.map(|option| {
                div()
                    .id(gpui::SharedString::from(format!("mix-mode-{}", option.label())))
                    .flex()
                    .items_center()
                    .gap(sz(8.0))
                    .min_h(sz(30.0))
                    .px(sz(9.0))
                    .py(sz(6.0))
                    .rounded(sz(5.0))
                    .text_size(sz(12.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(tokens.accent).text_color(gpui::white()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.mode = option;
                        this.mode_open = false;
                        this.pick(cx);
                    }))
                    .child(div().w(sz(14.0)).flex_none().flex().justify_center().when(option == mode, |d| d.child(icon("check", 13.0))))
                    .child(option.label())
            });
            deferred(
                div()
                    .id("mix-mode-menu")
                    .absolute()
                    .top(relative(1.0))
                    .mt(sz(5.0))
                    .left_0()
                    .min_w(relative(1.0))
                    .p(sz(5.0))
                    .border(sz(1.0))
                    .border_color(tokens.line)
                    .rounded(sz(9.0))
                    .bg(tokens.sidebar)
                    .text_color(tokens.text)
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        if this.mode_open {
                            this.mode_open = false;
                            cx.notify();
                        }
                    }))
                    .children(items),
            )
            .with_priority(1)
        });
        let dropdown = div()
            .relative()
            .child(
                div()
                    .id("mix-mode")
                    .flex()
                    .items_center()
                    .gap(sz(8.0))
                    .min_h(sz(30.0))
                    .px(sz(9.0))
                    .py(sz(5.0))
                    .border(sz(1.0))
                    .border_color(tokens.line)
                    .rounded(sz(7.0))
                    .bg(tokens.sidebar)
                    .text_color(tokens.text)
                    .text_size(sz(11.0))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.mode_open = !this.mode_open;
                        cx.notify();
                    }))
                    .child(div().child(format!("Find: {}", mode.label())))
                    .child(div().flex_none().text_color(tokens.muted).child(icon("chevron-down", 14.0))),
            )
            .children(menu);

        let controls = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(sz(8.0))
            .when(self.chosen.is_some() && !busy, |d| {
                d.child(secondary_button("mix-listen", if self.run.is_none() { "Play transition" } else { "Listen again" }, "", cx).on_click(cx.listener(|this, _, _, cx| this.listen(cx))))
            })
            .when(busy, |d| {
                d.child(secondary_button("mix-stop", "Stop", "", cx).on_click(cx.listener(|this, _, _, cx| {
                    this.stop_audio();
                    this.stage = Stage::Ready;
                    cx.notify();
                })))
            })
            .when(self.chosen.is_some() && !busy, |d| {
                let old = self.old;
                d.child(secondary_button("mix-ab-new", if old { "B: new planner" } else { "B: new planner ✓" }, "", cx).on_click(cx.listener(|this, _, _, cx| {
                    this.old = false;
                    this.export = None;
                    cx.notify();
                })))
                .child(secondary_button("mix-ab-old", if old { "A: old planner ✓" } else { "A: old planner" }, "", cx).on_click(cx.listener(|this, _, _, cx| {
                    this.old = true;
                    this.export = None;
                    cx.notify();
                })))
                .child(secondary_button("mix-export", "Export WAV", "", cx).on_click(cx.listener(|this, _, _, cx| this.export(cx))))
            })
            .when(!matches!(self.stage, Stage::Picking | Stage::Analyzing(_)), |d| {
                d.child(secondary_button("mix-pick", "Pick two new songs", "", cx).on_click(cx.listener(|this, _, _, cx| this.pick(cx))))
            });

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(tokens.canvas)
            .text_color(tokens.text)
            .child(
                div()
                    .px(sz(14.0))
                    .py(sz(10.0))
                    .border_b(sz(1.0))
                    .border_color(tokens.line)
                    .flex()
                    .flex_col()
                    .gap(sz(8.0))
                    .child(div().text_size(sz(14.0)).font_weight(FontWeight::BOLD).child("AutoMix preview"))
                    .child(div().flex().child(dropdown))
                    .child(div().text_size(sz(12.0)).text_color(if failed { theme::hex("#ff7b72") } else { tokens.muted }).child(status))
                    .child(controls)
                    .when_some(self.export.clone(), |d, text| d.child(div().text_size(sz(11.0)).text_color(tokens.muted).child(text))),
            )
            .child(div().id("mix-preview-body").flex_1().min_h_0().overflow_y_scroll().p(sz(14.0)).child(body))
    }
}
