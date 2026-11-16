//! The cpal output stream and its realtime mixer.
//!
//! The callback never locks and never allocates. It receives new rings and
//! settings through an `rtrb` command queue, returns replaced rings through a
//! second queue so they are freed off the audio thread, and publishes its
//! state through atomics that the engine and UI read.
//!
//! The same [`Mixer`] runs offline in [`crate::offline`], fed from in-memory
//! rings, so the renderer's tests exercise the product's own DSP.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, OutputCallbackInfo, SampleFormat, SizedSample, Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};
use volta_logic::mix_automation::{DeckAutomation, HIGHPASS_OPEN_HZ, LOWPASS_OPEN_HZ, TransitionAutomation};

use crate::decoder::{RING_CHANNELS, RingHandle};
use crate::dsp::{Declicker, DeckFx, LOOP_SEAM_SECONDS, Looper};
use crate::stretch::{HOP, Stretcher};

pub const DECKS: usize = 2;
/// Gain changes are smoothed over this long to avoid clicks.
const GAIN_RAMP_SECONDS: f32 = 0.015;
/// A deck's transition gain returns to unity this fast after a cancel.
const RESTORE_SECONDS: f32 = 0.3;

pub enum CallbackCmd {
    /// Installs a ring on a deck. `token` identifies the track.
    Install {
        deck: usize,
        token: u64,
        ring: RingHandle,
    },
    /// Removes and frees a deck's ring.
    Clear { deck: usize },
    /// Makes `deck` the audible deck immediately.
    Activate { deck: usize },
    /// When the active deck drains, continue with `deck` at the next sample.
    Arm { deck: usize },
    Disarm,
    Playing(bool),
    /// Per-deck gain (ReplayGain).
    DeckGain { deck: usize, gain: f32 },
    /// Mix `deck` in following the spec's automation, then make it active.
    Blend(Box<TransitionSpec>),
    CancelBlend,
    /// Compress what is left of a running blend into `frames` frames (a skip
    /// during a mix). A blend that has not started is cancelled.
    FinishBlend { frames: u64 },
}

/// A loop in deck frames (from the ring's start).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopFrames {
    pub start: u64,
    pub len: u64,
    pub repeats: u8,
}

/// A planned transition, executed sample-accurately in the callback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransitionSpec {
    /// The incoming deck.
    pub deck: usize,
    /// Overlap length in output frames.
    pub frames: u64,
    /// Outgoing deck frame (from its ring start) at which to begin; `None`
    /// starts at once.
    pub start_frame: Option<u64>,
    pub automation: TransitionAutomation,
    pub out_loop: Option<LoopFrames>,
    pub in_loop: Option<LoopFrames>,
    /// Frames over which the incoming rate eases back to 1 afterwards.
    pub release: u64,
}

impl TransitionSpec {
    /// Converts track-time parameters to frames. `out_base`/`in_base` are the
    /// track seconds at which each deck's ring starts.
    pub fn new(deck: usize, seconds: f64, start_at: Option<f64>, automation: TransitionAutomation, out_base: f64, in_base: f64, rate: f64) -> Self {
        let frames_of = |t: f64| (t.max(0.0) * rate).round() as u64;
        let looped = |region: Option<volta_logic::mix_automation::LoopRegion>, base: f64| {
            region.filter(|r| r.length > 0.0 && r.repeats > 0 && r.start >= base).map(|r| LoopFrames { start: frames_of(r.start - base), len: frames_of(r.length), repeats: r.repeats })
        };
        Self {
            deck,
            frames: frames_of(seconds).max(1),
            start_frame: start_at.map(|t| frames_of(t - out_base)),
            automation,
            out_loop: looped(automation.out_loop, out_base),
            in_loop: looped(automation.in_loop, in_base),
            release: frames_of(automation.release_seconds),
        }
    }
}

#[derive(Clone, Copy)]
struct BlendState {
    spec: TransitionSpec,
    /// Output frames into the overlap (fractional while finishing early).
    done: f64,
    /// Progress per output frame (1, or more when hurried by a skip).
    speed: f64,
    started: bool,
}

enum Pull {
    Frame([f32; 2]),
    Starved,
    Drained,
}

/// State the callback publishes.
pub struct Shared {
    pub sample_rate: AtomicU32,
    pub active_deck: AtomicUsize,
    /// Token of the track on each deck as installed in the callback.
    pub deck_token: [AtomicU64; DECKS],
    /// Frames consumed from each deck's current ring.
    pub deck_frames: [AtomicU64; DECKS],
    /// f64 bits: track seconds at the start of each deck's ring.
    pub deck_base: [AtomicU64; DECKS],
    /// Increments every time the callback switches decks on its own (gapless).
    pub advance_count: AtomicU64,
    /// Token of a track that drained with nothing armed after it.
    pub ended_token: AtomicU64,
    /// Master volume as f32 bits (already 0 when muted).
    pub master: AtomicU32,
    pub playing: AtomicBool,
    pub underruns: AtomicU64,
    /// Output latency (callback → speaker) in nanoseconds.
    pub latency_ns: AtomicU64,
    pub stream_failed: AtomicBool,
    /// f64 bits of a requested seek target until its ring is installed (NaN = none).
    pub pending_seek: AtomicU64,
    /// A blend is mixing both decks.
    pub blending: AtomicBool,
    /// f32 bits: progress 0–1 of the running blend (for the cover morph).
    pub blend_progress: AtomicU32,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            sample_rate: AtomicU32::new(48_000),
            active_deck: AtomicUsize::new(0),
            deck_token: [AtomicU64::new(0), AtomicU64::new(0)],
            deck_frames: [AtomicU64::new(0), AtomicU64::new(0)],
            deck_base: [AtomicU64::new(0), AtomicU64::new(0)],
            advance_count: AtomicU64::new(0),
            ended_token: AtomicU64::new(0),
            master: AtomicU32::new(1.0f32.to_bits()),
            playing: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
            latency_ns: AtomicU64::new(0),
            stream_failed: AtomicBool::new(false),
            pending_seek: AtomicU64::new(f64::NAN.to_bits()),
            blending: AtomicBool::new(false),
            blend_progress: AtomicU32::new(0),
        }
    }

    /// Playback position of the active deck in seconds, compensated for output latency.
    pub fn position(&self) -> f64 {
        let pending = f64::from_bits(self.pending_seek.load(Ordering::Acquire));
        if !pending.is_nan() {
            return pending;
        }
        let deck = self.active_deck.load(Ordering::Acquire).min(DECKS - 1);
        let base = f64::from_bits(self.deck_base[deck].load(Ordering::Acquire));
        let frames = self.deck_frames[deck].load(Ordering::Acquire) as f64;
        let rate = f64::from(self.sample_rate.load(Ordering::Relaxed).max(1));
        let latency = if self.playing.load(Ordering::Relaxed) {
            self.latency_ns.load(Ordering::Relaxed) as f64 / 1e9
        } else {
            0.0
        };
        (base + frames / rate - latency).max(base)
    }
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

struct DeckSlot {
    token: u64,
    ring: RingHandle,
    gain: f32,
    target_gain: f32,
    /// Playback position in deck frames (what has been heard).
    frames: u64,
    /// Source position of the next frame read (ring or loop).
    read: u64,
    /// Playback rate (through the stretcher when ≠ 1 or easing).
    rate: f64,
    rate_step: f64,
    /// Transition gain held after a blend, easing back to 1.
    trans_gain: f32,
    /// The last `HOP` source frames, to prime the stretcher mid-stream.
    history: [[f32; 2]; HOP],
    history_at: usize,
    fx: DeckFx,
}

impl DeckSlot {
    fn new(token: u64, ring: RingHandle, gain: f32, sample_rate: f32) -> Self {
        Self {
            token,
            ring,
            gain,
            target_gain: gain,
            frames: 0,
            read: 0,
            rate: 1.0,
            rate_step: 0.0,
            trans_gain: 1.0,
            history: [[0.0; 2]; HOP],
            history_at: 0,
            fx: DeckFx::new(sample_rate),
        }
    }

    fn remember(&mut self, frame: [f32; 2]) {
        self.history[self.history_at] = frame;
        self.history_at = (self.history_at + 1) % HOP;
    }

    /// The remembered frames, oldest first, copied into `out`.
    fn history_into(&self, out: &mut [[f32; 2]; HOP]) {
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = self.history[(self.history_at + i) % HOP];
        }
    }
}

/// One source frame of a deck: replayed from its looper, or read from its
/// ring (through the looper, which captures and seams it).
fn source(slot: &mut DeckSlot, looper: &mut Looper) -> Pull {
    if looper.replaying() {
        let frame = looper.replay(&mut slot.read);
        return Pull::Frame(frame);
    }
    if slot.ring.consumer.slots() >= RING_CHANNELS {
        let l = slot.ring.consumer.pop().unwrap_or(0.0);
        let r = slot.ring.consumer.pop().unwrap_or(0.0);
        let frame = looper.through(slot.read, [l, r]);
        slot.read += 1;
        looper.after(slot.read);
        return Pull::Frame(frame);
    }
    if slot.ring.eof.load(Ordering::Acquire) { Pull::Drained } else { Pull::Starved }
}

pub(crate) struct Mixer {
    commands: Consumer<CallbackCmd>,
    trash: Producer<RingHandle>,
    retired: Producer<Box<TransitionSpec>>,
    shared: Arc<Shared>,
    decks: [Option<DeckSlot>; DECKS],
    gains: [f32; DECKS],
    active: usize,
    armed: Option<usize>,
    playing: bool,
    master: f32,
    ramp_step: f32,
    restore_step: f32,
    channels: usize,
    blend: Option<BlendState>,
    stretch: [Stretcher; DECKS],
    loops: [Looper; DECKS],
    declick: Declicker,
    sample_rate: f32,
    /// Offline stems: only this deck is heard.
    pub(crate) solo: Option<usize>,
    scratch: [[f32; 2]; HOP],
}

impl Mixer {
    pub(crate) fn new(commands: Consumer<CallbackCmd>, trash: Producer<RingHandle>, retired: Producer<Box<TransitionSpec>>, shared: Arc<Shared>, sample_rate: f32, channels: usize) -> Self {
        Self {
            commands,
            trash,
            retired,
            master: f32::from_bits(shared.master.load(Ordering::Relaxed)),
            shared,
            decks: [None, None],
            gains: [1.0; DECKS],
            active: 0,
            armed: None,
            playing: false,
            ramp_step: 1.0 / (GAIN_RAMP_SECONDS * sample_rate),
            restore_step: 1.0 / (RESTORE_SECONDS * sample_rate),
            channels: channels.max(1),
            blend: None,
            stretch: [Stretcher::new(), Stretcher::new()],
            loops: [Looper::new(sample_rate), Looper::new(sample_rate)],
            declick: Declicker::new(sample_rate),
            sample_rate,
            solo: None,
            scratch: [[0.0; 2]; HOP],
        }
    }

    pub(crate) fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
    }

    fn end_blend(&mut self) {
        self.blend = None;
        self.shared.blending.store(false, Ordering::Release);
        self.shared.blend_progress.store(0f32.to_bits(), Ordering::Release);
    }

    /// One raw frame from `deck`: straight from the source, or through the
    /// stretcher while it is engaged.
    fn pull_raw(&mut self, deck: usize) -> Pull {
        let Some(slot) = self.decks[deck].as_mut() else { return Pull::Drained };
        let stretcher = &mut self.stretch[deck];
        let looper = &mut self.loops[deck];
        if !stretcher.active {
            let pulled = source(slot, looper);
            if let Pull::Frame(frame) = pulled {
                slot.remember(frame);
                slot.frames = slot.read;
            }
            return pulled;
        }
        loop {
            if let Some(frame) = stretcher.pop() {
                slot.frames = slot.read.saturating_sub(stretcher.buffered());
                if slot.rate_step != 0.0 {
                    slot.rate += slot.rate_step;
                    if (slot.rate - 1.0).abs() <= slot.rate_step.abs() {
                        slot.rate = 1.0;
                        slot.rate_step = 0.0;
                    }
                }
                return Pull::Frame(frame);
            }
            let rate = slot.rate;
            let mut drained = false;
            while stretcher.wants_input(rate) {
                match source(slot, looper) {
                    Pull::Frame(frame) => stretcher.push(frame),
                    Pull::Starved => return Pull::Starved,
                    Pull::Drained => {
                        drained = true;
                        break;
                    }
                }
            }
            if drained && !stretcher.pad_tail(rate) {
                return Pull::Drained;
            }
            stretcher.step(rate);
        }
    }

    /// One frame from `deck` with ReplayGain, effects and transition gain.
    /// `automation` is the deck's transition automation at progress `p`.
    fn pull(&mut self, deck: usize, automation: Option<(&DeckAutomation, f32)>) -> Pull {
        if let (Some((auto, p)), Some(slot)) = (automation, self.decks[deck].as_mut())
            && (self.stretch[deck].active || auto.uses_rate()) {
                slot.rate = f64::from(auto.rate.at(p));
            }
        let ramp = self.ramp_step;
        let restore = self.restore_step;
        match self.pull_raw(deck) {
            Pull::Frame(frame) => {
                let Some(slot) = self.decks[deck].as_mut() else { return Pull::Drained };
                if slot.gain != slot.target_gain {
                    let delta = slot.target_gain - slot.gain;
                    slot.gain = if delta.abs() <= ramp { slot.target_gain } else { slot.gain + ramp * delta.signum() };
                }
                let mut frame = frame;
                let trans = match automation {
                    Some((auto, p)) => {
                        if slot.fx.busy() {
                            frame = slot.fx.process(frame, [auto.low.at(p), auto.mid.at(p), auto.high.at(p)], auto.lowpass_hz.at(p), auto.highpass_hz.at(p));
                        }
                        auto.gain.at(p)
                    }
                    None => {
                        if slot.fx.busy() {
                            frame = slot.fx.process(frame, [1.0; 3], LOWPASS_OPEN_HZ, HIGHPASS_OPEN_HZ);
                        }
                        if slot.trans_gain != 1.0 {
                            let delta = 1.0 - slot.trans_gain;
                            slot.trans_gain = if delta.abs() <= restore { 1.0 } else { slot.trans_gain + restore * delta.signum() };
                        }
                        slot.trans_gain
                    }
                };
                let g = slot.gain * trans;
                Pull::Frame([frame[0] * g, frame[1] * g])
            }
            other => other,
        }
    }

    /// Begins a scheduled blend: engage stretchers for tempo automation and
    /// the effects chain for band or filter automation.
    fn start_blend(&mut self, blend: &mut BlendState) {
        blend.started = true;
        let spec = blend.spec;
        for (deck, auto) in [(self.active, &spec.automation.outgoing), (spec.deck, &spec.automation.incoming)] {
            let Some(slot) = self.decks[deck].as_mut() else { continue };
            slot.rate_step = 0.0;
            if auto.uses_rate() {
                slot.rate = f64::from(auto.rate.at(0.0));
                if !self.stretch[deck].active {
                    slot.history_into(&mut self.scratch);
                    self.stretch[deck].engage_primed(slot.read, &self.scratch);
                }
            }
            if auto.uses_bands() || auto.uses_filters() {
                slot.fx.wet_target = 1.0;
            }
        }
    }

    /// Arms a deck's loop; the region is captured as it first plays.
    fn arm_loop(&mut self, deck: usize, region: Option<LoopFrames>) {
        let seam = (f64::from(self.sample_rate) * LOOP_SEAM_SECONDS) as u64;
        match region {
            Some(r) => {
                if !self.loops[deck].arm(r.start, r.len, seam, r.repeats) {
                    self.loops[deck].disarm();
                }
            }
            None => self.loops[deck].disarm(),
        }
    }

    /// A frame while blending. The switch happens when the overlap completes
    /// or the outgoing track drains.
    fn blend_frame(&mut self, mut blend: BlendState) -> [f32; 2] {
        let outgoing = self.active;
        let spec = blend.spec;
        let p = (blend.done / spec.frames.max(1) as f64).min(1.0) as f32;
        let out = self.pull(outgoing, Some((&spec.automation.outgoing, p)));
        let incoming = self.pull(spec.deck, Some((&spec.automation.incoming, p)));
        blend.done += blend.speed;
        self.shared.blend_progress.store(p.to_bits(), Ordering::Relaxed);
        if matches!(out, Pull::Starved) || matches!(incoming, Pull::Starved) {
            self.shared.underruns.fetch_add(1, Ordering::Relaxed);
        }
        let o = if let Pull::Frame(f) = out { f } else { [0.0; 2] };
        let i = if let Pull::Frame(f) = incoming { f } else { [0.0; 2] };
        let (o, i) = match self.solo {
            Some(d) if d == outgoing => (o, [0.0; 2]),
            Some(_) => ([0.0; 2], i),
            None => (o, i),
        };
        if blend.done >= spec.frames as f64 || matches!(out, Pull::Drained) {
            if let Some(slot) = self.decks[outgoing].as_mut() {
                self.shared.deck_frames[outgoing].store(slot.frames, Ordering::Release);
            }
            self.loops[outgoing].disarm();
            self.loops[spec.deck].release();
            if let Some(slot) = self.decks[spec.deck].as_mut() {
                // Normally the envelope's end; if the outgoing song ran out
                // early, the incoming continues from where its fade was and
                // eases up from there instead of jumping.
                let completed = blend.done >= spec.frames as f64;
                slot.trans_gain = if completed { spec.automation.incoming.gain.last() } else { spec.automation.incoming.gain.at(p) };
                slot.fx.wet_target = 0.0;
                if (slot.rate - 1.0).abs() > 1e-9 {
                    slot.rate_step = (1.0 - slot.rate) / spec.release.max(1) as f64;
                }
            }
            self.active = spec.deck;
            self.armed = None;
            self.shared.active_deck.store(spec.deck, Ordering::Release);
            self.shared.advance_count.fetch_add(1, Ordering::AcqRel);
            self.end_blend();
        } else {
            self.blend = Some(blend);
        }
        [o[0] + i[0], o[1] + i[1]]
    }

    /// Undoes a blend's effects on both decks. The outgoing deck returns to
    /// full level over a short ramp; the step that remains is declicked.
    fn cancel_blend(&mut self) {
        if let Some(blend) = self.blend {
            let p = (blend.done / blend.spec.frames.max(1) as f64).min(1.0) as f32;
            if let Some(slot) = self.decks[self.active].as_mut() {
                slot.fx.wet_target = 0.0;
                if blend.started {
                    slot.trans_gain = blend.spec.automation.outgoing.gain.at(p);
                    if (slot.rate - 1.0).abs() > 1e-9 {
                        slot.rate_step = (1.0 - slot.rate) / (0.5 * f64::from(self.sample_rate));
                    }
                }
            }
            if let Some(slot) = self.decks[blend.spec.deck].as_mut() {
                slot.fx.wet_target = 0.0;
            }
            self.loops[self.active].disarm();
            self.loops[blend.spec.deck].disarm();
            if blend.started {
                self.stretch[blend.spec.deck].reset();
                self.declick.arm();
            }
        }
        self.end_blend();
    }

    fn drop_slot(&mut self, slot: DeckSlot) {
        // Freeing the ring here would deallocate on the audio thread.
        if let Err(rtrb::PushError::Full(ring)) = self.trash.push(slot.ring) {
            // The engine drains the trash every few ms; leaking beats blocking.
            std::mem::forget(ring);
        }
    }

    fn retire(&mut self, spec: Box<TransitionSpec>) {
        if let Err(rtrb::PushError::Full(spec)) = self.retired.push(spec) {
            std::mem::forget(spec);
        }
    }

    pub(crate) fn apply_commands(&mut self) {
        while let Ok(command) = self.commands.pop() {
            match command {
                CallbackCmd::Install { deck, token, ring } => {
                    if deck == self.active {
                        self.shared.pending_seek.store(f64::NAN.to_bits(), Ordering::Release);
                        if self.playing {
                            self.declick.arm();
                        }
                    }
                    self.shared.deck_base[deck].store(ring.base_seconds.to_bits(), Ordering::Release);
                    self.shared.deck_frames[deck].store(0, Ordering::Release);
                    self.shared.deck_token[deck].store(token, Ordering::Release);
                    let gain = self.gains[deck];
                    self.stretch[deck].reset();
                    self.loops[deck].disarm();
                    let previous = self.decks[deck].replace(DeckSlot::new(token, ring, gain, self.sample_rate));
                    if let Some(slot) = previous {
                        self.drop_slot(slot);
                    }
                }
                CallbackCmd::Clear { deck } => {
                    if self.blend.is_some_and(|b| b.spec.deck == deck) || (self.blend.is_some() && deck == self.active) {
                        self.cancel_blend();
                    }
                    if deck == self.active && self.playing {
                        self.declick.arm();
                    }
                    self.stretch[deck].reset();
                    self.loops[deck].disarm();
                    if let Some(slot) = self.decks[deck].take() {
                        self.drop_slot(slot);
                    }
                    self.shared.deck_token[deck].store(0, Ordering::Release);
                    if self.armed == Some(deck) {
                        self.armed = None;
                    }
                }
                CallbackCmd::Activate { deck } => {
                    if deck != self.active && self.playing {
                        self.declick.arm();
                    }
                    self.active = deck;
                    if self.armed == Some(deck) {
                        self.armed = None;
                    }
                    self.shared.active_deck.store(deck, Ordering::Release);
                }
                CallbackCmd::Arm { deck } => self.armed = Some(deck),
                CallbackCmd::Disarm => self.armed = None,
                CallbackCmd::Playing(playing) => {
                    self.playing = playing;
                    self.shared.playing.store(playing, Ordering::Release);
                }
                CallbackCmd::DeckGain { deck, gain } => {
                    self.gains[deck] = gain;
                    if let Some(slot) = self.decks[deck].as_mut() {
                        slot.target_gain = gain;
                    }
                }
                CallbackCmd::Blend(spec) => {
                    if self.blend.is_none() && spec.deck != self.active && self.decks[spec.deck].is_some() && self.decks[self.active].is_some() {
                        let mut state = BlendState { spec: TransitionSpec { frames: spec.frames.max(1), ..*spec }, done: 0.0, speed: 1.0, started: false };
                        self.arm_loop(self.active, spec.out_loop);
                        self.arm_loop(spec.deck, spec.in_loop);
                        // Warm the outgoing deck's filters before they are heard.
                        let out = spec.automation.outgoing;
                        if (out.uses_bands() || out.uses_filters())
                            && let Some(slot) = self.decks[self.active].as_mut() {
                                slot.fx.wet_target = 1.0;
                            }
                        if spec.start_frame.is_none() {
                            self.start_blend(&mut state);
                        }
                        self.blend = Some(state);
                        self.shared.blending.store(true, Ordering::Release);
                    }
                    self.retire(spec);
                }
                CallbackCmd::CancelBlend => self.cancel_blend(),
                CallbackCmd::FinishBlend { frames } => {
                    if let Some(mut blend) = self.blend {
                        if blend.started {
                            let remaining = (blend.spec.frames as f64 - blend.done).max(0.0);
                            blend.speed = (remaining / frames.max(1) as f64).max(1.0);
                            self.loops[self.active].release();
                            self.loops[blend.spec.deck].release();
                            self.blend = Some(blend);
                        } else {
                            self.cancel_blend();
                        }
                    }
                }
            }
        }
    }

    /// Next stereo frame from the active deck, switching gaplessly if armed.
    pub(crate) fn next_frame(&mut self) -> [f32; 2] {
        let frame = self.next_frame_raw();
        self.declick.process(frame)
    }

    fn next_frame_raw(&mut self) -> [f32; 2] {
        if let Some(mut blend) = self.blend {
            if !blend.started {
                // A scheduled blend begins on its exact outgoing frame (or
                // at once if the outgoing track runs out first).
                let at = self.decks[self.active].as_ref().map(|s| (s.frames, s.ring.eof.load(Ordering::Acquire) && s.ring.consumer.slots() < RING_CHANNELS && !self.loops[self.active].replaying()));
                let due = match (at, blend.spec.start_frame) {
                    (Some((frames, drained)), Some(start)) => frames >= start || drained,
                    _ => true,
                };
                if due {
                    self.start_blend(&mut blend);
                    self.blend = Some(blend);
                }
            }
            if blend.started {
                return self.blend_frame(blend);
            }
        }
        loop {
            let active = self.active;
            if self.decks[active].is_none() {
                return [0.0; 2];
            }
            match self.pull(active, None) {
                Pull::Frame(f) => return if self.solo.is_some_and(|d| d != active) { [0.0; 2] } else { f },
                Pull::Starved => {
                    self.shared.underruns.fetch_add(1, Ordering::Relaxed);
                    return [0.0; 2];
                }
                Pull::Drained => {}
            }
            // The active track has fully drained.
            let Some(slot) = self.decks[active].as_ref() else { return [0.0; 2] };
            let (token, frames) = (slot.token, slot.frames);
            let next = self.armed.take().filter(|&next| self.decks[next].is_some());
            match next {
                Some(next) => {
                    self.shared.deck_frames[active].store(frames, Ordering::Release);
                    self.active = next;
                    self.shared.active_deck.store(next, Ordering::Release);
                    self.shared.advance_count.fetch_add(1, Ordering::AcqRel);
                    // Loop to read the first frame of the next track in this same slot.
                }
                None => {
                    if self.shared.ended_token.load(Ordering::Relaxed) != token {
                        self.shared.ended_token.store(token, Ordering::Release);
                    }
                    return [0.0; 2];
                }
            }
        }
    }

    /// Publishes per-deck positions (after a buffer).
    pub(crate) fn publish(&self) {
        for deck in 0..DECKS {
            if let Some(slot) = self.decks[deck].as_ref()
                && (deck == self.active || self.blend.is_some_and(|b| b.spec.deck == deck)) {
                    self.shared.deck_frames[deck].store(slot.frames, Ordering::Release);
                }
        }
    }

    pub(crate) fn blend_started(&self) -> bool {
        self.blend.is_some_and(|b| b.started)
    }

    pub(crate) fn is_blending(&self) -> bool {
        self.blend.is_some()
    }

    fn render<T: SizedSample + FromSample<f32>>(&mut self, output: &mut [T], info: &OutputCallbackInfo) {
        self.apply_commands();
        let timestamp = info.timestamp();
        let latency = timestamp.playback.duration_since(timestamp.callback);
        self.shared.latency_ns.store(latency.as_nanos() as u64, Ordering::Relaxed);

        let target_master = f32::from_bits(self.shared.master.load(Ordering::Relaxed));
        let channels = self.channels;
        let frames = output.len() / channels.max(1);
        if !self.playing {
            for sample in output.iter_mut() {
                *sample = T::from_sample(0.0);
            }
            // Ease the master toward target while paused so resume does not click.
            self.master = target_master;
            return;
        }
        let master_step = (target_master - self.master) / frames.max(1) as f32;
        for frame in output.chunks_mut(channels) {
            let [l, r] = self.next_frame();
            self.master += master_step;
            let (l, r) = (l * self.master, r * self.master);
            match frame.len() {
                1 => frame[0] = T::from_sample((l + r) * 0.5),
                _ => {
                    frame[0] = T::from_sample(l);
                    frame[1] = T::from_sample(r);
                    for extra in frame.iter_mut().skip(2) {
                        *extra = T::from_sample(0.0);
                    }
                }
            }
        }
        self.master = target_master;
        self.publish();
    }
}

pub struct Output {
    /// Held so the stream keeps running; dropping it stops output.
    pub _stream: Stream,
    pub commands: Producer<CallbackCmd>,
    pub trash: Consumer<RingHandle>,
    /// Transition specs the callback is done with (freed off the audio thread).
    pub retired: Consumer<Box<TransitionSpec>>,
    pub sample_rate: u32,
}

impl Output {
    /// Suspends the device stream so the OS stops reporting the app as playing
    /// audio. Returns false if the backend cannot pause (stream keeps running).
    pub fn suspend(&self) -> bool {
        self._stream.pause().is_ok()
    }

    pub fn resume(&self) {
        if let Err(error) = self._stream.play() {
            log::warn!("audio stream resume failed: {error}");
        }
    }
}

pub fn open(shared: Arc<Shared>) -> anyhow::Result<Output> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no audio output device"))?;
    let supported = device.default_output_config()?;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let sample_rate = config.sample_rate;
    shared.sample_rate.store(sample_rate, Ordering::Release);
    let (commands, command_rx) = RingBuffer::new(256);
    let (trash_tx, trash) = RingBuffer::new(64);
    let (retired_tx, retired) = RingBuffer::new(64);
    let mixer = Mixer::new(command_rx, trash_tx, retired_tx, shared.clone(), sample_rate as f32, usize::from(config.channels));
    let stream = match sample_format {
        SampleFormat::F32 => build::<f32>(&device, &config, mixer, shared)?,
        SampleFormat::I16 => build::<i16>(&device, &config, mixer, shared)?,
        SampleFormat::U16 => build::<u16>(&device, &config, mixer, shared)?,
        SampleFormat::I32 => build::<i32>(&device, &config, mixer, shared)?,
        SampleFormat::F64 => build::<f64>(&device, &config, mixer, shared)?,
        other => anyhow::bail!("unsupported output sample format {other}"),
    };
    stream.play()?;
    Ok(Output {
        _stream: stream,
        commands,
        trash,
        retired,
        sample_rate,
    })
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut mixer: Mixer,
    shared: Arc<Shared>,
) -> anyhow::Result<Stream> {
    let stream = device.build_output_stream(
        *config,
        move |data: &mut [T], info: &OutputCallbackInfo| mixer.render(data, info),
        move |error: cpal::Error| {
            log::warn!("audio output error: {error}");
            if matches!(
                error.kind(),
                cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::DeviceNotAvailable
            ) {
                shared.stream_failed.store(true, Ordering::Release);
            }
        },
        None,
    )?;
    Ok(stream)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use volta_logic::mix_automation::{Envelope, Shape};

    pub(crate) fn ring(value: f32, frames: usize, eof: bool) -> RingHandle {
        let (mut producer, consumer) = RingBuffer::new(frames * RING_CHANNELS);
        for _ in 0..frames * RING_CHANNELS {
            producer.push(value).unwrap();
        }
        RingHandle { consumer, eof: Arc::new(AtomicBool::new(eof)), base_seconds: 0.0 }
    }

    fn spec(frames: u64, automation: TransitionAutomation) -> Box<TransitionSpec> {
        Box::new(TransitionSpec { deck: 1, frames, start_frame: None, automation, out_loop: None, in_loop: None, release: 1 })
    }

    fn mixer() -> (Mixer, Producer<CallbackCmd>, Arc<Shared>) {
        let shared = Arc::new(Shared::new());
        let (commands, command_rx) = RingBuffer::new(16);
        let (trash_tx, _trash) = RingBuffer::new(16);
        let (retired_tx, _retired) = RingBuffer::new(16);
        let mut mixer = Mixer::new(command_rx, trash_tx, retired_tx, shared.clone(), 48_000.0, 2);
        mixer.ramp_step = 1.0;
        mixer.playing = true;
        (mixer, commands, shared)
    }

    #[test]
    fn equal_power_blend_switches_decks_at_the_end() {
        let (mut m, mut commands, shared) = mixer();
        commands.push(CallbackCmd::Install { deck: 0, token: 1, ring: ring(1.0, 200, true) }).ok().unwrap();
        commands.push(CallbackCmd::Install { deck: 1, token: 2, ring: ring(0.5, 200, false) }).ok().unwrap();
        commands.push(CallbackCmd::Blend(spec(100, TransitionAutomation::equal_power()))).ok().unwrap();
        m.apply_commands();
        assert!(shared.blending.load(Ordering::Acquire));
        let first = m.next_frame();
        assert!((first[0] - 1.0).abs() < 1e-6, "starts on the outgoing deck");
        for _ in 1..50 {
            m.next_frame();
        }
        // Halfway: cos(π/4) * 1.0 + sin(π/4) * 0.5.
        let mid = m.next_frame();
        let expected = std::f32::consts::FRAC_1_SQRT_2 * 1.5;
        assert!((mid[0] - expected).abs() < 0.02, "{} vs {expected}", mid[0]);
        for _ in 51..100 {
            m.next_frame();
        }
        assert!(!shared.blending.load(Ordering::Acquire));
        assert_eq!(m.active, 1);
        assert_eq!(shared.advance_count.load(Ordering::Acquire), 1);
        // After the switch only the incoming deck plays, from frame 101.
        let after = m.next_frame();
        assert!((after[0] - 0.5).abs() < 1e-6);
        assert_eq!(m.decks[1].as_ref().unwrap().frames, 101);
    }

    #[test]
    fn a_draining_outgoing_track_ends_the_blend_early() {
        let (mut m, mut commands, _) = mixer();
        commands.push(CallbackCmd::Install { deck: 0, token: 1, ring: ring(1.0, 10, true) }).ok().unwrap();
        commands.push(CallbackCmd::Install { deck: 1, token: 2, ring: ring(0.5, 200, false) }).ok().unwrap();
        commands.push(CallbackCmd::Blend(spec(100, TransitionAutomation::crossfade(Shape::Linear, Shape::Linear)))).ok().unwrap();
        m.apply_commands();
        for _ in 0..11 {
            m.next_frame();
        }
        assert_eq!(m.active, 1);
        assert!(m.blend.is_none());
    }

    #[test]
    fn scheduled_tempo_matched_blend_with_band_automation() {
        let (mut m, mut commands, shared) = mixer();
        commands.push(CallbackCmd::Install { deck: 0, token: 1, ring: ring(1.0, 20_000, true) }).ok().unwrap();
        commands.push(CallbackCmd::Install { deck: 1, token: 2, ring: ring(0.5, 30_000, false) }).ok().unwrap();
        let mut automation = TransitionAutomation::equal_power();
        automation.incoming.rate = Envelope::constant(1.05);
        automation.incoming.low = Envelope::ramp(0.0, 1.0, 0.4, 0.6, Shape::SCurve);
        automation.outgoing.low = Envelope::ramp(1.0, 0.0, 0.4, 0.6, Shape::SCurve);
        let spec = Box::new(TransitionSpec { deck: 1, frames: 6000, start_frame: Some(1000), automation, out_loop: None, in_loop: None, release: 2000 });
        commands.push(CallbackCmd::Blend(spec)).ok().unwrap();
        m.apply_commands();
        // Before the scheduled frame only the outgoing deck plays.
        for _ in 0..1000 {
            assert!((m.next_frame()[0] - 1.0).abs() < 1e-3);
        }
        assert!(m.blend.is_some_and(|b| !b.started));
        m.next_frame();
        assert!(m.blend.is_some_and(|b| b.started));
        assert!(m.stretch[1].active, "tempo automation engages the stretcher");
        assert!(!m.stretch[0].active, "the outgoing tempo is untouched");
        for _ in 0..6000 {
            m.next_frame();
        }
        assert_eq!(m.active, 1);
        assert_eq!(shared.advance_count.load(Ordering::Acquire), 1);
        let slot = m.decks[1].as_ref().unwrap();
        assert!(slot.rate_step < 0.0, "rate eases back to 1");
        // The incoming deck consumed ~5% more input than frames it produced.
        assert!(slot.frames as f64 > 6000.0 * 1.02, "consumed {}", slot.frames);
        for _ in 0..2500 {
            m.next_frame();
        }
        assert_eq!(m.decks[1].as_ref().unwrap().rate, 1.0);
    }

    #[test]
    fn finishing_early_hurries_the_mix_and_cancel_restores_the_outgoing() {
        let (mut m, mut commands, shared) = mixer();
        commands.push(CallbackCmd::Install { deck: 0, token: 1, ring: ring(1.0, 50_000, true) }).ok().unwrap();
        commands.push(CallbackCmd::Install { deck: 1, token: 2, ring: ring(0.5, 50_000, false) }).ok().unwrap();
        commands.push(CallbackCmd::Blend(spec(20_000, TransitionAutomation::equal_power()))).ok().unwrap();
        m.apply_commands();
        for _ in 0..1000 {
            m.next_frame();
        }
        commands.push(CallbackCmd::FinishBlend { frames: 500 }).ok().unwrap();
        m.apply_commands();
        let mut previous = m.next_frame()[0];
        let mut worst = 0f32;
        for _ in 0..600 {
            let v = m.next_frame()[0];
            worst = worst.max((v - previous).abs());
            previous = v;
        }
        assert_eq!(m.active, 1, "switched within the hurried span");
        assert!(worst < 0.01, "no jump: {worst}");
        assert_eq!(shared.advance_count.load(Ordering::Acquire), 1);

        // Cancel mid-blend: the outgoing returns to full level without a step.
        let (mut m, mut commands, _) = mixer();
        commands.push(CallbackCmd::Install { deck: 0, token: 1, ring: ring(1.0, 50_000, true) }).ok().unwrap();
        commands.push(CallbackCmd::Install { deck: 1, token: 2, ring: ring(0.5, 50_000, false) }).ok().unwrap();
        commands.push(CallbackCmd::Blend(spec(20_000, TransitionAutomation::equal_power()))).ok().unwrap();
        m.apply_commands();
        for _ in 0..10_000 {
            m.next_frame();
        }
        commands.push(CallbackCmd::CancelBlend).ok().unwrap();
        m.apply_commands();
        let mut previous = m.next_frame()[0];
        let mut worst = 0f32;
        for _ in 0..20_000 {
            let v = m.next_frame()[0];
            worst = worst.max((v - previous).abs());
            previous = v;
        }
        assert!(worst < 0.01, "no jump after cancel: {worst}");
        assert!((previous - 1.0).abs() < 1e-3, "outgoing back to full: {previous}");
    }
}
