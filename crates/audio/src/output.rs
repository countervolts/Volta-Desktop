//! The cpal output stream and its realtime mixer.
//!
//! The callback never locks and never allocates. It receives new rings and
//! settings through an `rtrb` command queue, returns replaced rings through a
//! second queue so they are freed off the audio thread, and publishes its
//! state through atomics that the engine and UI read.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, OutputCallbackInfo, SampleFormat, SizedSample, Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::decoder::{RING_CHANNELS, RingHandle};
use crate::stretch::{LowCut, Stretcher};

pub const DECKS: usize = 2;
/// Gain changes are smoothed over this long to avoid clicks.
const GAIN_RAMP_SECONDS: f32 = 0.015;

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
    /// Mix `deck` in over `frames` frames, then make it the active deck.
    Blend(BlendSpec),
    CancelBlend,
}

/// A planned transition, executed sample-accurately in the callback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendSpec {
    pub deck: usize,
    pub frames: u64,
    pub linear: bool,
    /// Outgoing deck frame (from its ring start) at which to begin; `None`
    /// starts at once.
    pub start_frame: Option<u64>,
    /// Incoming playback rate during the blend (pitch preserved).
    pub tempo: f64,
    /// Frames over which the incoming rate eases back to 1 afterwards.
    pub release: u64,
    /// Trade the low end at the midpoint instead of stacking basslines.
    pub bass_swap: bool,
    /// Incoming level at the start of the blend, easing to 1 by its end.
    pub gain_start: f32,
}

#[derive(Clone, Copy)]
struct BlendState {
    spec: BlendSpec,
    done: u64,
    started: bool,
}

enum Pull {
    Frame(f32, f32),
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
    frames: u64,
    /// Playback rate (through the stretcher when ≠ 1 or easing).
    rate: f64,
    rate_step: f64,
    /// Low-band cut 0–1 (bass swap).
    cut: f32,
    low: LowCut,
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

struct Mixer {
    commands: Consumer<CallbackCmd>,
    trash: Producer<RingHandle>,
    shared: Arc<Shared>,
    decks: [Option<DeckSlot>; DECKS],
    gains: [f32; DECKS],
    active: usize,
    armed: Option<usize>,
    playing: bool,
    master: f32,
    ramp_step: f32,
    channels: usize,
    blend: Option<BlendState>,
    stretch: [Stretcher; DECKS],
    sample_rate: f32,
}

impl Mixer {
    fn end_blend(&mut self) {
        self.blend = None;
        self.shared.blending.store(false, Ordering::Release);
    }

    /// One raw frame from `deck`: straight from the ring, or through the
    /// stretcher while it is engaged.
    fn pull_raw(&mut self, deck: usize) -> Pull {
        let Some(slot) = self.decks[deck].as_mut() else { return Pull::Drained };
        let stretcher = &mut self.stretch[deck];
        if !stretcher.active {
            if slot.ring.consumer.slots() >= RING_CHANNELS {
                let l = slot.ring.consumer.pop().unwrap_or(0.0);
                let r = slot.ring.consumer.pop().unwrap_or(0.0);
                slot.frames += 1;
                return Pull::Frame(l, r);
            }
            return if slot.ring.eof.load(Ordering::Acquire) { Pull::Drained } else { Pull::Starved };
        }
        loop {
            if let Some([l, r]) = stretcher.pop() {
                slot.frames = stretcher.consumed();
                if slot.rate_step != 0.0 {
                    slot.rate += slot.rate_step;
                    if (slot.rate - 1.0).abs() <= slot.rate_step.abs() {
                        slot.rate = 1.0;
                        slot.rate_step = 0.0;
                    }
                }
                return Pull::Frame(l, r);
            }
            let rate = slot.rate;
            while stretcher.wants_input(rate) && slot.ring.consumer.slots() >= RING_CHANNELS {
                let l = slot.ring.consumer.pop().unwrap_or(0.0);
                let r = slot.ring.consumer.pop().unwrap_or(0.0);
                stretcher.push([l, r]);
            }
            if stretcher.wants_input(rate) {
                if !slot.ring.eof.load(Ordering::Acquire) {
                    return Pull::Starved;
                }
                if !stretcher.pad_tail(rate) {
                    return Pull::Drained;
                }
            }
            stretcher.step(rate);
        }
    }

    /// One frame from `deck` with its gain ramp and bass cut applied.
    fn pull(&mut self, deck: usize) -> Pull {
        let ramp = self.ramp_step;
        match self.pull_raw(deck) {
            Pull::Frame(l, r) => {
                let Some(slot) = self.decks[deck].as_mut() else { return Pull::Drained };
                if slot.gain != slot.target_gain {
                    let delta = slot.target_gain - slot.gain;
                    slot.gain = if delta.abs() <= ramp { slot.target_gain } else { slot.gain + ramp * delta.signum() };
                }
                let [l, r] = slot.low.process([l, r], slot.cut);
                Pull::Frame(l * slot.gain, r * slot.gain)
            }
            other => other,
        }
    }

    /// Begins a scheduled blend: engage the stretcher for a tempo change and
    /// cut the incoming bass for a swap.
    fn start_blend(&mut self, blend: &mut BlendState) {
        blend.started = true;
        let spec = blend.spec;
        if let Some(slot) = self.decks[spec.deck].as_mut() {
            slot.rate = spec.tempo;
            slot.rate_step = 0.0;
            slot.cut = if spec.bass_swap { 1.0 } else { 0.0 };
            if (spec.tempo - 1.0).abs() > 1e-6 {
                self.stretch[spec.deck].engage(slot.frames);
            }
        }
    }

    /// A frame while blending: outgoing × fade-out + incoming × fade-in. The
    /// switch happens when the fade completes or the outgoing track drains.
    fn blend_frame(&mut self, mut blend: BlendState) -> (f32, f32) {
        let outgoing = self.active;
        let spec = blend.spec;
        let p = (blend.done as f32 / spec.frames.max(1) as f32).min(1.0);
        if spec.bass_swap {
            // Basslines trade around the midpoint over ~20% of the blend.
            let s = smoothstep(0.4, 0.6, p);
            if let Some(slot) = self.decks[outgoing].as_mut() {
                slot.cut = s;
            }
            if let Some(slot) = self.decks[spec.deck].as_mut() {
                slot.cut = 1.0 - s;
            }
        }
        let out = self.pull(outgoing);
        let incoming = self.pull(spec.deck);
        let (go, gi) = if spec.linear {
            (1.0 - p, p)
        } else {
            let angle = p * std::f32::consts::FRAC_PI_2;
            (angle.cos(), angle.sin())
        };
        let gi = gi * (spec.gain_start + (1.0 - spec.gain_start) * p);
        blend.done += 1;
        let (ol, or) = if let Pull::Frame(l, r) = out { (l, r) } else { (0.0, 0.0) };
        let (il, ir) = if let Pull::Frame(l, r) = incoming { (l, r) } else { (0.0, 0.0) };
        if matches!(out, Pull::Starved) || matches!(incoming, Pull::Starved) {
            self.shared.underruns.fetch_add(1, Ordering::Relaxed);
        }
        if blend.done >= spec.frames || matches!(out, Pull::Drained) {
            if let Some(slot) = self.decks[outgoing].as_mut() {
                slot.cut = 0.0;
                self.shared.deck_frames[outgoing].store(slot.frames, Ordering::Release);
            }
            if let Some(slot) = self.decks[spec.deck].as_mut() {
                slot.cut = 0.0;
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
        (ol * go + il * gi, or * go + ir * gi)
    }

    /// Undoes a blend's effects on both decks.
    fn cancel_blend(&mut self) {
        if let Some(blend) = self.blend {
            for deck in [self.active, blend.spec.deck] {
                if let Some(slot) = self.decks[deck].as_mut() {
                    slot.cut = 0.0;
                }
            }
            if blend.started {
                self.stretch[blend.spec.deck].reset();
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

    fn apply_commands(&mut self) {
        while let Ok(command) = self.commands.pop() {
            match command {
                CallbackCmd::Install { deck, token, ring } => {
                    if deck == self.active {
                        self.shared.pending_seek.store(f64::NAN.to_bits(), Ordering::Release);
                    }
                    self.shared.deck_base[deck].store(ring.base_seconds.to_bits(), Ordering::Release);
                    self.shared.deck_frames[deck].store(0, Ordering::Release);
                    self.shared.deck_token[deck].store(token, Ordering::Release);
                    let gain = self.gains[deck];
                    self.stretch[deck].reset();
                    let previous = self.decks[deck].replace(DeckSlot {
                        token,
                        ring,
                        gain,
                        target_gain: gain,
                        frames: 0,
                        rate: 1.0,
                        rate_step: 0.0,
                        cut: 0.0,
                        low: LowCut::new(180.0, self.sample_rate),
                    });
                    if let Some(slot) = previous {
                        self.drop_slot(slot);
                    }
                }
                CallbackCmd::Clear { deck } => {
                    if self.blend.is_some_and(|b| b.spec.deck == deck) || (self.blend.is_some() && deck == self.active) {
                        self.cancel_blend();
                    }
                    self.stretch[deck].reset();
                    if let Some(slot) = self.decks[deck].take() {
                        self.drop_slot(slot);
                    }
                    self.shared.deck_token[deck].store(0, Ordering::Release);
                    if self.armed == Some(deck) {
                        self.armed = None;
                    }
                }
                CallbackCmd::Activate { deck } => {
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
                    if spec.deck != self.active && self.decks[spec.deck].is_some() && self.decks[self.active].is_some() {
                        let mut state = BlendState { spec: BlendSpec { frames: spec.frames.max(1), ..spec }, done: 0, started: false };
                        if spec.start_frame.is_none() {
                            self.start_blend(&mut state);
                        }
                        self.blend = Some(state);
                        self.shared.blending.store(true, Ordering::Release);
                    }
                }
                CallbackCmd::CancelBlend => self.cancel_blend(),
            }
        }
    }

    /// Next stereo frame from the active deck, switching gaplessly if armed.
    fn next_frame(&mut self) -> (f32, f32) {
        if let Some(mut blend) = self.blend {
            if !blend.started {
                // A scheduled blend begins on its exact outgoing frame (or
                // at once if the outgoing track runs out first).
                let at = self.decks[self.active].as_ref().map(|s| (s.frames, s.ring.eof.load(Ordering::Acquire) && s.ring.consumer.slots() < RING_CHANNELS));
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
                return (0.0, 0.0);
            }
            match self.pull(active) {
                Pull::Frame(l, r) => return (l, r),
                Pull::Starved => {
                    self.shared.underruns.fetch_add(1, Ordering::Relaxed);
                    return (0.0, 0.0);
                }
                Pull::Drained => {}
            }
            // The active track has fully drained.
            let Some(slot) = self.decks[active].as_ref() else { return (0.0, 0.0) };
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
                    return (0.0, 0.0);
                }
            }
        }
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
            let (l, r) = self.next_frame();
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
        for deck in 0..DECKS {
            if let Some(slot) = self.decks[deck].as_ref()
                && (deck == self.active || self.blend.is_some_and(|b| b.spec.deck == deck)) {
                    self.shared.deck_frames[deck].store(slot.frames, Ordering::Release);
                }
        }
    }
}

pub struct Output {
    /// Held so the stream keeps running; dropping it stops output.
    pub _stream: Stream,
    pub commands: Producer<CallbackCmd>,
    pub trash: Consumer<RingHandle>,
    pub sample_rate: u32,
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
    let mixer = Mixer {
        commands: command_rx,
        trash: trash_tx,
        shared: shared.clone(),
        decks: [None, None],
        gains: [1.0; DECKS],
        active: 0,
        armed: None,
        playing: false,
        master: f32::from_bits(shared.master.load(Ordering::Relaxed)),
        ramp_step: 1.0 / (GAIN_RAMP_SECONDS * sample_rate as f32),
        channels: usize::from(config.channels).max(1),
        blend: None,
        stretch: [Stretcher::new(), Stretcher::new()],
        sample_rate: sample_rate as f32,
    };
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
mod tests {
    use super::*;

    fn ring(value: f32, frames: usize, eof: bool) -> RingHandle {
        let (mut producer, consumer) = RingBuffer::new(frames * RING_CHANNELS);
        for _ in 0..frames * RING_CHANNELS {
            producer.push(value).unwrap();
        }
        RingHandle { consumer, eof: Arc::new(AtomicBool::new(eof)), base_seconds: 0.0 }
    }

    fn spec(frames: u64, linear: bool) -> BlendSpec {
        BlendSpec { deck: 1, frames, linear, start_frame: None, tempo: 1.0, release: 1, bass_swap: false, gain_start: 1.0 }
    }

    fn mixer() -> (Mixer, Producer<CallbackCmd>, Arc<Shared>) {
        let shared = Arc::new(Shared::new());
        let (commands, command_rx) = RingBuffer::new(16);
        let (trash_tx, _trash) = RingBuffer::new(16);
        let mixer = Mixer {
            commands: command_rx,
            trash: trash_tx,
            shared: shared.clone(),
            decks: [None, None],
            gains: [1.0; DECKS],
            active: 0,
            armed: None,
            playing: true,
            master: 1.0,
            ramp_step: 1.0,
            channels: 2,
            blend: None,
            stretch: [Stretcher::new(), Stretcher::new()],
            sample_rate: 48000.0,
        };
        (mixer, commands, shared)
    }

    #[test]
    fn equal_power_blend_switches_decks_at_the_end() {
        let (mut m, mut commands, shared) = mixer();
        commands.push(CallbackCmd::Install { deck: 0, token: 1, ring: ring(1.0, 200, true) }).ok().unwrap();
        commands.push(CallbackCmd::Install { deck: 1, token: 2, ring: ring(0.5, 200, false) }).ok().unwrap();
        commands.push(CallbackCmd::Blend(spec(100, false))).ok().unwrap();
        m.apply_commands();
        assert!(shared.blending.load(Ordering::Acquire));
        let first = m.next_frame();
        assert!((first.0 - 1.0).abs() < 1e-6, "starts on the outgoing deck");
        for _ in 1..50 {
            m.next_frame();
        }
        // Halfway: cos(π/4) * 1.0 + sin(π/4) * 0.5.
        let mid = m.next_frame();
        let expected = std::f32::consts::FRAC_1_SQRT_2 * 1.5;
        assert!((mid.0 - expected).abs() < 0.02, "{} vs {expected}", mid.0);
        for _ in 51..100 {
            m.next_frame();
        }
        assert!(!shared.blending.load(Ordering::Acquire));
        assert_eq!(m.active, 1);
        assert_eq!(shared.advance_count.load(Ordering::Acquire), 1);
        // After the switch only the incoming deck plays, from frame 101.
        let after = m.next_frame();
        assert!((after.0 - 0.5).abs() < 1e-6);
        assert_eq!(m.decks[1].as_ref().unwrap().frames, 101);
    }

    #[test]
    fn a_draining_outgoing_track_ends_the_blend_early() {
        let (mut m, mut commands, _) = mixer();
        commands.push(CallbackCmd::Install { deck: 0, token: 1, ring: ring(1.0, 10, true) }).ok().unwrap();
        commands.push(CallbackCmd::Install { deck: 1, token: 2, ring: ring(0.5, 200, false) }).ok().unwrap();
        commands.push(CallbackCmd::Blend(spec(100, true))).ok().unwrap();
        m.apply_commands();
        for _ in 0..11 {
            m.next_frame();
        }
        assert_eq!(m.active, 1);
        assert!(m.blend.is_none());
    }

    #[test]
    fn scheduled_tempo_matched_blend_with_bass_swap() {
        let (mut m, mut commands, shared) = mixer();
        commands.push(CallbackCmd::Install { deck: 0, token: 1, ring: ring(1.0, 20_000, true) }).ok().unwrap();
        commands.push(CallbackCmd::Install { deck: 1, token: 2, ring: ring(0.5, 30_000, false) }).ok().unwrap();
        let spec = BlendSpec {
            deck: 1,
            frames: 6000,
            linear: false,
            start_frame: Some(1000),
            tempo: 1.05,
            release: 2000,
            bass_swap: true,
            gain_start: 0.5,
        };
        commands.push(CallbackCmd::Blend(spec)).ok().unwrap();
        m.apply_commands();
        // Before the scheduled frame only the outgoing deck plays.
        for _ in 0..1000 {
            assert!((m.next_frame().0 - 1.0).abs() < 1e-6);
        }
        assert!(m.blend.is_some_and(|b| !b.started));
        m.next_frame();
        assert!(m.blend.is_some_and(|b| b.started));
        assert!(m.stretch[1].active, "tempo change engages the stretcher");
        for _ in 0..6000 {
            m.next_frame();
        }
        assert_eq!(m.active, 1);
        assert_eq!(shared.advance_count.load(Ordering::Acquire), 1);
        let slot = m.decks[1].as_ref().unwrap();
        assert_eq!(slot.cut, 0.0);
        assert!(slot.rate_step < 0.0, "rate eases back to 1");
        // The incoming deck consumed ~5% more input than frames it produced.
        assert!(slot.frames as f64 > 6000.0 * 1.02, "consumed {}", slot.frames);
        for _ in 0..2500 {
            m.next_frame();
        }
        assert_eq!(m.decks[1].as_ref().unwrap().rate, 1.0);
    }
}
