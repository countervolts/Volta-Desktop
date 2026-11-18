//! Playback controller. The gpui-side counterpart of `PlaybackController`
//! in `src/lib/use-player.ts`: queue, transport, volume, repeat/shuffle,
//! stream fallback and scrobbling. Audio itself runs in `volta-audio`.

use std::sync::Arc;
use std::time::Duration;

use gpui::{AppContext, Context, EventEmitter, Task};
use volta_audio::{AudioHandle, Command, Event, LoadRequest, TrackSource};
use volta_logic::format::{Normalization, replay_gain_factor};
use volta_logic::listening_history::{EventInput, EventKind, ListeningEvent, create_listening_event, now_ms};
use volta_logic::model::Song;
use volta_logic::storage::VolumePreference;
use volta_services::Navidrome;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
    Off,
    All,
    One,
}

impl Repeat {
    pub fn label(self) -> &'static str {
        match self {
            Repeat::Off => "off",
            Repeat::All => "all",
            Repeat::One => "one",
        }
    }
}

/// Seconds of real listening that count toward a scrobble.
struct Listen {
    token: u64,
    seconds: f64,
    last_position: f64,
    announced: bool,
    submitted: bool,
    /// Listening-history bookkeeping (`listen` in use-player.ts).
    song: Song,
    session_id: String,
    session_index: u32,
    reported_seconds: f64,
    finished: bool,
}

/// `LISTENING_SESSION_GAP_MS`: a new session after 20 minutes of quiet.
const SESSION_GAP_MS: f64 = 20.0 * 60.0 * 1000.0;

/// `TransitionMode` with the crossfade length.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Transition {
    Off,
    Crossfade(f64),
    Automix,
}

pub struct Player {
    audio: AudioHandle,
    client: Option<Arc<Navidrome>>,
    pub queue: Vec<Song>,
    /// Queue order before shuffling, for un-shuffling.
    ordered: Vec<Song>,
    pub index: Option<usize>,
    pub playing: bool,
    pub loading: bool,
    pub position: f64,
    pub duration: f64,
    pub volume: f64,
    pub muted: bool,
    pub previous_volume: f64,
    pub shuffle: bool,
    pub repeat: Repeat,
    /// "compatible" stream in use (MP3 320) after the original failed.
    pub compatible: bool,
    /// User preference: original quality first.
    pub original: bool,
    pub normalization: Normalization,
    pub error: Option<String>,
    /// When the current load was requested, for the time-to-first-audio metric.
    load_started: Option<std::time::Instant>,
    next_token: u64,
    current_token: u64,
    preloaded: Option<(u64, usize)>,
    /// Song transitions (`TRANSITION_MODE_KEY`, `CROSSFADE_KEY`).
    pub transition: Transition,
    /// AutoMix also mixes albums played in order (`AUTOMIX_ALBUMS_KEY`).
    pub mix_albums: bool,
    /// The Up Next AutoMix toggle: AutoMix for this queue (reset to on when a
    /// new queue starts, like Apple Music's per-queue button).
    pub automix_queue: bool,
    /// Planner thresholds (defaults, or `automix-planner.json` in the data
    /// folder, which the measurement lab can write).
    pub planner: volta_logic::mix_planner::PlannerConfig,
    /// Why the coming (or running) transition is what it is, for the
    /// status line: `(incoming index, text)`.
    pub mix_status: Option<(usize, String)>,
    /// The planned transition being scheduled or mixed (kind and label).
    pub mix_kind: Option<volta_logic::mix_planner::TransitionKind>,
    /// A blend into this preload token was requested, and when.
    blend: Option<(u64, std::time::Instant)>,
    /// A blend into this token failed; that pair falls back to gapless.
    failed_blend: u64,
    /// `automixLabel`: non-empty while an AutoMix blend is mixing.
    pub automix_label: Option<String>,
    /// AutoMix track profiles by `analysis_key`.
    pub profiles: std::collections::HashMap<String, volta_logic::mix_analysis::MixProfile>,
    /// Track time the preloaded song starts from (its mix-in point).
    preload_start: f64,
    /// Outgoing time the scheduled blend starts, and its label.
    mix_out: f64,
    mix_label: Option<String>,
    listen: Option<Listen>,
    session_id: String,
    session_index: i64,
    last_activity: f64,
    _poll: Task<()>,
}

/// Listening events go to the app state, which keeps the history.
impl EventEmitter<ListeningEvent> for Player {}

impl Player {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let playing = this.update(cx, |player, cx| player.tick(cx)).unwrap_or(false);
                // Frame-rate updates while audible keep the seek bar smooth.
                let interval = if playing { 16 } else { 120 };
                cx.background_executor().timer(Duration::from_millis(interval)).await;
            }
        });
        Self {
            audio: AudioHandle::spawn(),
            client: None,
            queue: Vec::new(),
            ordered: Vec::new(),
            index: None,
            playing: false,
            loading: false,
            position: 0.0,
            duration: 0.0,
            volume: 0.8,
            muted: false,
            previous_volume: 0.8,
            shuffle: false,
            repeat: Repeat::Off,
            compatible: false,
            original: true,
            normalization: Normalization::Off,
            error: None,
            load_started: None,
            next_token: 1,
            current_token: 0,
            preloaded: None,
            transition: Transition::Off,
            mix_albums: true,
            automix_queue: true,
            planner: Default::default(),
            mix_status: None,
            mix_kind: None,
            blend: None,
            failed_blend: 0,
            automix_label: None,
            profiles: Default::default(),
            preload_start: 0.0,
            mix_out: 0.0,
            mix_label: None,
            listen: None,
            session_id: String::new(),
            session_index: -1,
            last_activity: 0.0,
            _poll: poll,
        }
    }

    /// `updateSongMetadata`: refresh queued copies after a local edit.
    pub fn update_songs(&mut self, songs: &std::collections::HashMap<String, Song>, cx: &mut Context<Self>) {
        let mut changed = false;
        for queue in [&mut self.queue, &mut self.ordered] {
            for song in queue.iter_mut() {
                if let Some(updated) = songs.get(&song.id) {
                    *song = updated.clone();
                    changed = true;
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    pub fn set_transition(&mut self, transition: Transition) {
        if transition != self.transition {
            self.transition = transition;
            if transition == Transition::Off {
                self.cancel_blend();
            }
        }
    }

    /// `cancelCrossfade`.
    fn cancel_blend(&mut self) {
        if self.blend.take().is_some() {
            self.audio.send(Command::CancelBlend);
        }
        self.automix_label = None;
        self.mix_kind = None;
    }

    /// Plans the coming transition and, ~10 s ahead, schedules it in the
    /// engine, which starts it on the exact outgoing sample. AutoMix waits
    /// for both songs' analysis; if it is not ready in time the handoff is
    /// natural (gapless) rather than a blind crossfade.
    fn check_handoff(&mut self) {
        use volta_logic::mix_planner::{MixPlan, PairContext, TransitionKind, plan_transition};
        if self.transition == Transition::Off || !self.playing || self.loading || self.blend.is_some() || self.duration <= 0.0 {
            return;
        }
        let (Some(index), Some((token, next))) = (self.index, self.preloaded) else { return };
        if token == self.failed_blend || next == index {
            return;
        }
        let (Some(from), Some(to)) = (self.queue.get(index).cloned(), self.queue.get(next).cloned()) else { return };
        let crossfade = |seconds: f64, duration: f64| {
            let seconds = seconds.min(to.duration.filter(|d| *d > 0.0).map_or(seconds, |d| d / 2.0)).min(duration / 2.0);
            MixPlan {
                kind: TransitionKind::EnergyCrossfade,
                out_start: (duration - seconds).max(0.0),
                in_start: 0.0,
                seconds,
                automation: volta_audio::TransitionAutomation::equal_power(),
                bars: 0,
                tempo: 1.0,
                score: 1.0,
                label: "Crossfade · Smooth blend".into(),
                reason: format!("Crossfade of {seconds:.0} s"),
                candidates: Vec::new(),
            }
        };
        let plan = match self.transition {
            Transition::Automix => {
                if !self.automix_queue {
                    self.set_status(next, "AutoMix is off for this queue");
                    return;
                }
                let (server, username) = self.client.as_ref().map(|c| (c.server.clone(), c.username.clone())).unwrap_or_default();
                let key = |song: &Song| crate::automix_analysis::analysis_key(&server, &username, song, self.original);
                let context = PairContext {
                    album_in_order: volta_logic::automix::is_album_sequence(&[&from, &to]),
                    from_genre: from.genre.clone(),
                    to_genre: to.genre.clone(),
                    same_song: from.id == to.id,
                };
                let config = volta_logic::mix_planner::PlannerConfig { mix_albums: self.mix_albums, ..self.planner.clone() };
                match (self.profiles.get(&key(&from)), self.profiles.get(&key(&to))) {
                    (Some(a), Some(b)) => plan_transition(a, b, &context, &config),
                    _ => {
                        // Wait for the analysis; past the last useful moment
                        // the songs simply play on gaplessly.
                        let text = if self.position > self.duration - 2.0 { "Not analyzed in time: playing on without a mix" } else { "Analyzing the next song…" };
                        self.set_status(next, text);
                        return;
                    }
                }
            }
            Transition::Crossfade(seconds) => crossfade(seconds, self.duration),
            Transition::Off => return,
        };
        self.set_status(next, &format!("{} — {}", plan.kind.name(), plan.reason));
        if !plan.mixes() {
            // Natural handoff: the preloaded deck takes over on its own.
            self.failed_blend = token;
            return;
        }
        if self.position < plan.out_start - 10.0 {
            return;
        }
        if self.position > plan.out_start + 0.25 {
            // Too late to start on the planned frame: hand off naturally.
            self.failed_blend = token;
            return;
        }
        // The incoming song must be decoded from its mix-in point.
        if (plan.in_start - self.preload_start).abs() > 0.05 {
            self.preload_at(next, plan.in_start);
            return;
        }
        self.audio.send(Command::Blend(volta_audio::BlendRequest {
            token,
            seconds: plan.seconds,
            start_at: Some(plan.out_start),
            automation: plan.automation,
        }));
        self.blend = Some((token, std::time::Instant::now()));
        self.mix_out = plan.out_start;
        self.mix_kind = Some(plan.kind);
        self.mix_label = (self.transition == Transition::Automix).then_some(plan.label);
    }

    fn set_status(&mut self, next: usize, text: &str) {
        if self.mix_status.as_ref().is_none_or(|(i, t)| *i != next || t != text) {
            self.mix_status = Some((next, text.to_string()));
        }
    }

    /// Progress 0–1 of the audible mix (for the cover morph), `None` when no
    /// mix is playing.
    pub fn mix_progress(&self) -> Option<f32> {
        (self.automix_label.is_some() && self.audio.is_blending()).then(|| self.audio.blend_progress())
    }

    /// The song mixing in, while a mix is audible.
    pub fn mixing_into(&self) -> Option<&Song> {
        self.automix_label.as_ref()?;
        self.preloaded.and_then(|(_, index)| self.queue.get(index))
    }

    /// The Up Next AutoMix button.
    pub fn set_automix_queue(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.automix_queue != on {
            self.automix_queue = on;
            if !on {
                self.cancel_blend();
            }
            self.failed_blend = 0;
            self.mix_status = None;
            cx.notify();
        }
    }

    pub fn set_client(&mut self, client: Option<Arc<Navidrome>>) {
        self.client = client;
    }

    /// Output underruns since launch (diagnostics).
    pub fn underruns(&self) -> u64 {
        self.audio.underruns()
    }

    pub fn current_song(&self) -> Option<&Song> {
        self.index.and_then(|i| self.queue.get(i))
    }

    fn token(&mut self) -> u64 {
        let token = self.next_token;
        self.next_token += 1;
        token
    }

    fn request_for(&mut self, index: usize, start_at: f64, autoplay: bool) -> Option<LoadRequest> {
        let song = self.queue.get(index)?.clone();
        let token = self.token();
        let (source, fallback) = if let Some(path) = song.local_file_path.clone().or(song.local_path.clone()).filter(|_| song.is_local()) {
            (TrackSource::File(path.into()), None)
        } else {
            let client = self.client.as_ref()?;
            let original = TrackSource::Url(client.stream_url(&song.id, true));
            let compatible = TrackSource::Url(client.stream_url(&song.id, false));
            if self.original {
                (original, Some(compatible))
            } else {
                (compatible, None)
            }
        };
        let extension = if self.original {
            song.suffix.clone()
        } else {
            Some("mp3".into())
        };
        Some(LoadRequest {
            token,
            source,
            fallback,
            extension,
            start_at,
            autoplay,
            gain: replay_gain_factor(Some(&song), self.normalization) as f32,
        })
    }

    fn effective_volume(&self) -> f32 {
        if self.muted { 0.0 } else { self.volume as f32 }
    }

    fn load_index(&mut self, index: usize, start_at: f64, autoplay: bool, cx: &mut Context<Self>) {
        let Some(request) = self.request_for(index, start_at, autoplay) else { return };
        self.cancel_blend();
        self.index = Some(index);
        self.current_token = request.token;
        self.compatible = !self.original;
        self.position = start_at;
        self.duration = self.queue[index].duration.unwrap_or(0.0);
        self.playing = autoplay;
        self.loading = true;
        self.error = None;
        self.preloaded = None;
        let song = self.queue[index].clone();
        log::info!(
            "play: \"{}\" by {} [{} {} {}] from {}, start {:.1}s, autoplay {autoplay}",
            song.title,
            song.artist.as_deref().unwrap_or("unknown"),
            song.suffix.as_deref().or(song.codec.as_deref()).unwrap_or("?"),
            song.bit_rate.map(|b| format!("{b:.0} kbps")).unwrap_or_default(),
            song.sampling_rate.map(|r| format!("{:.1} kHz", r / 1000.0)).unwrap_or_default(),
            if song.is_local() { "this device" } else { "server" },
            start_at,
        );
        self.load_started = Some(std::time::Instant::now());
        let (session_id, session_index) = self.ensure_session();
        self.listen = Some(Listen {
            token: request.token,
            seconds: 0.0,
            last_position: start_at,
            announced: false,
            submitted: false,
            song,
            session_id,
            session_index,
            reported_seconds: 0.0,
            finished: false,
        });
        self.audio.send(Command::SetVolume(self.effective_volume()));
        self.audio.send(Command::Load(request));
        self.preload_next();
        cx.notify();
    }

    /// The index that plays after the current track ends on its own.
    fn automatic_next_index(&self) -> Option<usize> {
        let index = self.index?;
        match self.repeat {
            Repeat::One => Some(index),
            _ if index + 1 < self.queue.len() => Some(index + 1),
            Repeat::All if !self.queue.is_empty() => Some(0),
            _ => None,
        }
    }

    /// Keeps deck B warm with the following track so the change is gapless.
    fn preload_next(&mut self) {
        self.cancel_blend();
        self.audio.send(Command::ClearPreload);
        self.preloaded = None;
        self.preload_start = 0.0;
        let Some(next) = self.automatic_next_index() else { return };
        self.preload_at(next, 0.0);
    }

    /// Preloads `index` decoding from `start` seconds.
    fn preload_at(&mut self, index: usize, start: f64) {
        if let Some(request) = self.request_for(index, start, true) {
            self.preloaded = Some((request.token, index));
            self.preload_start = start;
            self.audio.send(Command::Preload(request));
        }
    }

    /// Replaces the queue and starts `start` (the web's `playQueue`).
    pub fn play_queue(&mut self, songs: Vec<Song>, start: usize, cx: &mut Context<Self>) {
        if songs.is_empty() {
            return;
        }
        self.finish_listen(EventKind::Skip, "queue-replaced", cx);
        self.ordered = songs.clone();
        self.queue = songs;
        self.shuffle = false;
        self.automix_queue = true;
        let start = start.min(self.queue.len() - 1);
        self.load_index(start, 0.0, true, cx);
    }

    /// `restoreSession`: the saved queue at its position. Reopening Volta never
    /// starts audio by itself: `was_playing` is ignored, the song is loaded paused.
    #[allow(clippy::too_many_arguments)]
    pub fn restore_session(
        &mut self,
        queue: Vec<Song>,
        index: usize,
        position: f64,
        shuffle: bool,
        repeat: Repeat,
        original: bool,
        was_playing: bool,
        cx: &mut Context<Self>,
    ) {
        if queue.is_empty() || (self.client.is_none() && !queue.iter().all(Song::is_local)) {
            return;
        }
        self.finish_listen(EventKind::Skip, "session-restored", cx);
        let index = index.min(queue.len() - 1);
        self.ordered = queue.clone();
        self.queue = queue;
        self.shuffle = shuffle;
        self.repeat = repeat;
        self.original = original;
        let _ = was_playing;
        self.load_index(index, position.max(0.0), false, cx);
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.playing { self.pause(cx) } else { self.play(cx) }
    }

    pub fn play(&mut self, cx: &mut Context<Self>) {
        if self.current_song().is_none() {
            return;
        }
        let resuming = !self.playing && self.listen.as_ref().is_some_and(|l| l.announced);
        self.playing = true;
        self.audio.send(Command::Play);
        if resuming {
            self.emit_listen(EventKind::Resume, "resume", None, true, cx);
        }
        cx.notify();
    }

    pub fn pause(&mut self, cx: &mut Context<Self>) {
        // Pausing abandons a scheduled or running blend; it is planned again
        // on resume when there is still time, otherwise the handoff is natural.
        self.cancel_blend();
        if self.playing {
            self.finish_listen(EventKind::Pause, "pause", cx);
        }
        self.playing = false;
        self.audio.send(Command::Pause);
        cx.notify();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.cancel_blend();
        self.finish_listen(EventKind::Stop, "stop", cx);
        self.audio.send(Command::Stop);
        self.queue.clear();
        self.ordered.clear();
        self.index = None;
        self.playing = false;
        self.loading = false;
        self.position = 0.0;
        self.duration = 0.0;
        self.listen = None;
        cx.notify();
    }

    pub fn next(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.index else { return };
        self.finish_listen(EventKind::Skip, "manual-next", cx);
        // Skipping during an audible mix: the incoming song is already
        // playing, so finish the mix within a third of a second instead of
        // restarting it from its beginning (the engine then reports Advanced).
        // (The engine reports a scheduled blend as blending before it is
        // audible; only an audible one is finished, otherwise skip as usual.)
        let audible = self.blend.is_some() && self.audio.is_blending() && self.position >= self.mix_out - 0.05;
        if audible && self.preloaded.is_some_and(|(_, next)| next == index + 1 || (index + 1 == self.queue.len() && next == 0)) {
            self.audio.send(Command::FinishBlend { seconds: 0.35 });
            cx.notify();
            return;
        }
        if index + 1 < self.queue.len() {
            self.load_index(index + 1, 0.0, true, cx);
        } else if self.repeat == Repeat::All {
            self.load_index(0, 0.0, true, cx);
        } else {
            self.pause(cx);
        }
    }

    pub fn previous(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.index else { return };
        if self.position > 3.0 {
            self.seek(0.0, cx);
            self.play(cx);
        } else if index > 0 {
            self.finish_listen(EventKind::Skip, "manual-next", cx);
            self.load_index(index - 1, 0.0, true, cx);
        } else if self.repeat == Repeat::All {
            let last = self.queue.len() - 1;
            self.load_index(last, 0.0, true, cx);
        } else {
            self.seek(0.0, cx);
            self.play(cx);
        }
    }

    pub fn seek(&mut self, seconds: f64, cx: &mut Context<Self>) {
        if self.current_song().is_none() || !seconds.is_finite() {
            return;
        }
        let upper = if self.duration > 0.0 { self.duration } else { seconds.max(0.0) };
        let position = seconds.clamp(0.0, upper);
        let previous = self.position;
        if (position - previous).abs() >= 3.0 {
            let reason = if position > previous { "seek-forward" } else { "seek-backward" };
            self.emit_listen(EventKind::Seek, reason, Some(position - previous), false, cx);
        }
        self.position = position;
        if let Some(listen) = self.listen.as_mut() {
            // Seeked time does not count toward a listen.
            listen.last_position = position;
        }
        self.cancel_blend();
        self.audio.send(Command::Seek(position));
        cx.notify();
    }

    pub fn set_volume(&mut self, value: f64, cx: &mut Context<Self>) {
        let volume = if value.is_finite() { value.clamp(0.0, 1.0) } else { self.volume };
        self.previous_volume = if volume > 0.0 {
            volume
        } else if self.volume > 0.0 {
            self.volume
        } else {
            self.previous_volume
        };
        self.volume = volume;
        self.muted = volume == 0.0;
        self.audio.send(Command::SetVolume(self.effective_volume()));
        cx.notify();
    }

    pub fn restore_volume(&mut self, preference: VolumePreference, cx: &mut Context<Self>) {
        let volume = preference.volume.clamp(0.0, 1.0);
        let muted = preference.muted || volume == 0.0;
        self.volume = if muted { 0.0 } else { volume };
        self.muted = muted;
        self.previous_volume = if volume > 0.0 {
            volume
        } else {
            preference.previous_volume.clamp(f64::EPSILON, 1.0)
        };
        self.audio.send(Command::SetVolume(self.effective_volume()));
        cx.notify();
    }

    pub fn volume_preference(&self) -> VolumePreference {
        VolumePreference {
            volume: self.volume,
            muted: self.muted,
            previous_volume: self.previous_volume,
        }
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        if self.muted || self.volume == 0.0 {
            let previous = self.previous_volume;
            self.set_volume(previous, cx);
        } else {
            self.set_volume(0.0, cx);
        }
    }

    pub fn toggle_shuffle(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.index else { return };
        self.shuffle = !self.shuffle;
        let split = index + 1;
        let mut upcoming: Vec<Song> = self.queue.split_off(split);
        if self.shuffle {
            use rand::seq::SliceRandom;
            upcoming.shuffle(&mut rand::rng());
        } else {
            let order = |song: &Song| self.ordered.iter().position(|s| s.id == song.id).unwrap_or(usize::MAX);
            upcoming.sort_by_key(order);
        }
        self.queue.extend(upcoming);
        self.preload_next();
        cx.notify();
    }

    /// `ensureListeningSession`.
    fn ensure_session(&mut self) -> (String, u32) {
        let now = now_ms();
        if self.session_id.is_empty() || now - self.last_activity > SESSION_GAP_MS {
            use rand::Rng;
            let bytes: [u8; 16] = rand::rng().random();
            self.session_id = bytes.iter().map(|b| format!("{b:02x}")).collect();
            self.session_index = -1;
        }
        self.session_index += 1;
        (self.session_id.clone(), self.session_index as u32)
    }

    /// `emitListeningEvent`.
    fn emit_listen(&mut self, kind: EventKind, reason: &str, seek_delta: Option<f64>, count_delta: bool, cx: &mut Context<Self>) {
        let (index, shuffle, repeat, position, duration) = (self.index.unwrap_or(0), self.shuffle, self.repeat, self.position, self.duration);
        let Some(listen) = self.listen.as_mut() else { return };
        if listen.finished {
            return;
        }
        let total = listen.seconds.max(0.0);
        let delta = (total - listen.reported_seconds).max(0.0);
        if count_delta {
            listen.reported_seconds = total;
        }
        let source = if listen.song.is_local() { "local" } else { "navidrome" };
        let event = create_listening_event(
            &listen.song,
            &EventInput {
                kind: Some(kind),
                at: None,
                session_id: listen.session_id.clone(),
                session_index: listen.session_index,
                duration_seconds: Some(if duration > 0.0 { duration } else { listen.song.duration.unwrap_or(0.0) }),
                delta_seconds: Some(if count_delta { delta } else { 0.0 }),
                total_seconds: Some(total),
                position_seconds: Some(position),
                source: source.into(),
                queue_index: index as u32,
                shuffle,
                repeat: repeat.label().into(),
                reason: Some(reason.into()),
                seek_delta_seconds: seek_delta,
            },
        );
        if matches!(kind, EventKind::Skip | EventKind::Complete | EventKind::Stop) {
            listen.finished = true;
        }
        self.last_activity = event.at;
        cx.emit(event);
    }

    /// `finishListening`.
    fn finish_listen(&mut self, kind: EventKind, reason: &str, cx: &mut Context<Self>) {
        if kind != EventKind::Pause && self.last_activity > 0.0 && now_ms() - self.last_activity > SESSION_GAP_MS {
            self.session_id.clear();
            self.session_index = -1;
        }
        self.sample_listen(cx);
        self.emit_listen(kind, reason, None, true, cx);
    }

    /// `setOriginal`: reload the current song in the other stream format at
    /// the same position, keeping play/pause.
    pub fn set_original(&mut self, original: bool, cx: &mut Context<Self>) {
        if original == self.original {
            return;
        }
        self.original = original;
        if let Some(index) = self.index {
            let (position, playing) = (self.position, self.playing);
            self.load_index(index, position, playing, cx);
        }
        cx.notify();
    }

    /// `append(song, next)`: Play Next inserts after the current song, Play
    /// Last appends. With nothing current it only queues (as the web does).
    pub fn append(&mut self, song: Song, next: bool, cx: &mut Context<Self>) {
        let Some(current) = self.index else {
            self.queue.insert(if next { 0 } else { self.queue.len() }, song.clone());
            self.ordered.push(song);
            cx.notify();
            return;
        };
        let index = if next { current + 1 } else { self.queue.len() };
        let ordered_index = if next {
            let current_id = self.queue[current].id.clone();
            self.ordered.iter().position(|s| s.id == current_id).map_or(self.ordered.len(), |i| i + 1)
        } else {
            self.ordered.len()
        };
        self.queue.insert(index, song.clone());
        self.ordered.insert(ordered_index.min(self.ordered.len()), song);
        self.preload_next();
        cx.notify();
    }

    /// `jumpTo`.
    pub fn jump_to(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.queue.len() {
            self.finish_listen(EventKind::Skip, "manual-next", cx);
            self.load_index(index, 0.0, true, cx);
        }
    }

    /// `reorderQueue`: only upcoming entries move; the new order also becomes
    /// the unshuffled order.
    pub fn reorder_queue(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let Some(current) = self.index else { return };
        if from <= current || to <= current || from >= self.queue.len() || to >= self.queue.len() || from == to {
            return;
        }
        let song = self.queue.remove(from);
        self.queue.insert(to, song);
        self.ordered = self.queue.clone();
        self.preload_next();
        cx.notify();
    }

    /// `removeFromQueue`.
    pub fn remove_from_queue(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(current) = self.index else { return };
        if index >= self.queue.len() {
            return;
        }
        let was_current = index == current;
        let autoplay = self.playing;
        if was_current {
            self.finish_listen(EventKind::Skip, "removed-from-queue", cx);
        }
        let removed = self.queue.remove(index);
        if let Some(position) = self.ordered.iter().position(|s| s.id == removed.id) {
            self.ordered.remove(position);
        }
        if self.queue.is_empty() {
            self.stop(cx);
            return;
        }
        if was_current {
            let next = index.min(self.queue.len() - 1);
            self.load_index(next, 0.0, autoplay, cx);
        } else {
            if index < current {
                self.index = Some(current - 1);
            }
            self.preload_next();
            cx.notify();
        }
    }

    /// `clearUpcoming`.
    pub fn clear_upcoming(&mut self, cx: &mut Context<Self>) {
        let Some(current) = self.index else { return };
        self.queue.truncate(current + 1);
        let retained: std::collections::HashSet<&str> = self.queue.iter().map(|s| s.id.as_str()).collect();
        self.ordered.retain(|s| retained.contains(s.id.as_str()));
        self.preload_next();
        cx.notify();
    }

    pub fn cycle_repeat(&mut self, cx: &mut Context<Self>) {
        self.repeat = match self.repeat {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        };
        self.preload_next();
        cx.notify();
    }

    fn scrobble(&self, song_id: String, submission: bool, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else { return };
        cx.background_spawn(async move {
            // Playback continues if history is unavailable.
            let _ = client.scrobble(&song_id, submission);
        })
        .detach();
    }

    fn sample_listen(&mut self, cx: &mut Context<Self>) {
        let duration = self.duration;
        let song = self.current_song().filter(|s| !s.is_local()).map(|s| s.id.clone());
        let Some(listen) = self.listen.as_mut() else { return };
        if listen.token != self.current_token {
            return;
        }
        let delta = self.position - listen.last_position;
        // Small forward steps are heard audio; jumps are seeks.
        if self.playing && delta > 0.0 && delta <= 1.5 {
            listen.seconds += delta;
        }
        listen.last_position = self.position;
        if !listen.submitted && duration > 0.0 && listen.seconds > (duration / 2.0).min(240.0) {
            listen.submitted = true;
            if let Some(id) = song {
                self.scrobble(id, true, cx);
            }
        }
    }

    /// Drains audio events and refreshes the clock. Returns whether audible.
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        cx.set_global(crate::app_state::GlobalPlaying(self.playing));
        let mut changed = false;
        while let Some(event) = self.audio.try_event() {
            changed = true;
            match event {
                Event::Opened { token, duration } if token == self.current_token => {
                    if let Some(duration) = duration.filter(|d| *d > 0.0) {
                        self.duration = duration;
                    }
                }
                Event::Started { token } if token == self.current_token => {
                    self.loading = false;
                    if let Some(started) = self.load_started.take() {
                        let what = format!(
                            "time to first audio ({})",
                            self.current_song().map(|s| if s.is_local() { "local file" } else { "streamed" }).unwrap_or("?")
                        );
                        crate::logs::timed(&what, started);
                    }
                    let song_id = self.current_song().filter(|s| !s.is_local()).map(|s| s.id.clone());
                    if let Some(listen) = self.listen.as_mut().filter(|l| !l.announced) {
                        listen.announced = true;
                        self.emit_listen(EventKind::Start, "play", None, true, cx);
                        if let Some(id) = song_id {
                            self.scrobble(id, false, cx);
                        }
                    }
                }
                Event::Advanced { token, duration } => {
                    if let Some((preload_token, index)) = self.preloaded.take()
                        && preload_token == token {
                            self.index = Some(index);
                            self.current_token = token;
                            self.compatible = !self.original;
                            self.duration = duration
                                .or(self.queue.get(index).and_then(|s| s.duration))
                                .unwrap_or(0.0);
                            self.position = 0.0;
                            self.loading = false;
                            let blended = self.blend.take().is_some();
                            self.automix_label = None;
                            self.mix_kind = None;
                            self.mix_status = None;
                            self.finish_listen(EventKind::Complete, if blended { "crossfade" } else { "automatic-complete" }, cx);
                            let song = self.queue[index].clone();
                            let (session_id, session_index) = self.ensure_session();
                            self.listen = Some(Listen {
                                token,
                                seconds: 0.0,
                                last_position: 0.0,
                                announced: true,
                                submitted: false,
                                song,
                                session_id,
                                session_index,
                                reported_seconds: 0.0,
                                finished: false,
                            });
                            self.emit_listen(EventKind::Start, "play", None, true, cx);
                            if let Some(id) = self.current_song().filter(|s| !s.is_local()).map(|s| s.id.clone()) {
                                self.scrobble(id, false, cx);
                            }
                            self.preload_next();
                        }
                }
                Event::Ended { token } if token == self.current_token => {
                    log::debug!("playback ended (end of queue)");
                    self.playing = false;
                    self.position = self.duration;
                    self.finish_listen(EventKind::Complete, "ended", cx);
                }
                Event::FellBack { token } if token == self.current_token => {
                    log::warn!("the original stream failed to decode; falling back to the compatible stream");
                    self.compatible = true;
                }
                Event::Error { token, message }
                    if (token == self.current_token || token == 0) => {
                        log::error!("playback error: {message} (song: {})", self.current_song().map(|s| s.title.as_str()).unwrap_or("none"));
                        self.finish_listen(EventKind::Stop, "playback-error", cx);
                        self.loading = false;
                        self.playing = false;
                        self.error = Some(message);
                    }
                _ => {}
            }
        }
        if self.current_song().is_some() && self.audio.active_token() == self.current_token {
            let position = self.audio.position();
            if (position - self.position).abs() > 1e-4 {
                self.position = position;
                changed = true;
            }
        }
        if let Some((token, at)) = self.blend {
            if self.audio.is_blending() {
                // The Mixing badge shows once the blend is audible.
                if self.automix_label.is_none() && self.position >= self.mix_out - 0.05 {
                    self.automix_label = self.mix_label.clone();
                    changed |= self.automix_label.is_some();
                }
            } else if at.elapsed() > Duration::from_millis(1500) {
                // Not accepted (incoming not decoded yet): plan again, or give
                // up once the start has passed.
                self.blend = None;
                if self.position > self.mix_out + 0.25 {
                    self.failed_blend = token;
                }
                changed = true;
            }
        }
        if self.blend.is_none() {
            let had_label = self.automix_label.is_some();
            self.check_handoff();
            changed |= self.blend.is_some() || had_label != self.automix_label.is_some();
        }
        self.sample_listen(cx);
        if changed {
            cx.notify();
        }
        self.playing && !self.loading
    }
}
