//! Offline (deterministic, no audio device) rendering of one transition.
//!
//! This drives the very same [`Mixer`] the cpal callback runs — WSOLA,
//! crossover, swept filters, loops, envelopes, declicker — fed from
//! in-memory rings instead of decoder threads. A test of the renderer is a
//! test of the live product's DSP.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rtrb::RingBuffer;
use volta_logic::mix_automation::TransitionAutomation;

use crate::decoder::{RING_CHANNELS, RingHandle};
use crate::output::{CallbackCmd, Mixer, Shared, TransitionSpec};

/// What to render. Times are track seconds.
#[derive(Debug, Clone, Copy)]
pub struct RenderRequest {
    /// Outgoing time at which the transition starts.
    pub out_start: f64,
    /// Incoming time it starts playing from.
    pub in_start: f64,
    /// Overlap length (output seconds).
    pub seconds: f64,
    pub automation: TransitionAutomation,
    /// Seconds of the outgoing song rendered before the transition.
    pub lead: f64,
    /// Seconds of the incoming song rendered after it.
    pub tail: f64,
    /// Linear ReplayGain per song.
    pub out_gain: f32,
    pub in_gain: f32,
}

/// A rendered transition.
#[derive(Debug, Clone, Default)]
pub struct Rendered {
    pub rate: u32,
    /// Interleaved stereo.
    pub mix: Vec<f32>,
    /// Output frame at which the transition began, and where it switched.
    pub start_frame: usize,
    pub end_frame: usize,
    /// Track time of the outgoing song at output frame 0.
    pub out_base: f64,
    /// Each deck alone through the same chain (`[outgoing, incoming]`).
    pub stems: Option<[Vec<f32>; 2]>,
}

fn ring_from(audio: &[f32], from_frame: usize, max_frames: usize, base_seconds: f64) -> RingHandle {
    let from = (from_frame * RING_CHANNELS).min(audio.len());
    let to = (from + max_frames * RING_CHANNELS).min(audio.len());
    let (mut producer, consumer) = RingBuffer::new((to - from).max(RING_CHANNELS));
    for &sample in &audio[from..to] {
        let _ = producer.push(sample);
    }
    RingHandle { consumer, eof: Arc::new(AtomicBool::new(true)), base_seconds }
}

fn render_once(a: &[f32], b: &[f32], rate: u32, request: &RenderRequest, solo: Option<usize>) -> Rendered {
    let r = f64::from(rate);
    let out_base = (request.out_start - request.lead).max(0.0);
    let total_frames = ((request.lead + request.seconds * 2.5 + request.tail + 2.0) * r) as usize;
    let shared = Arc::new(Shared::new());
    let (mut commands, command_rx) = RingBuffer::new(8);
    let (trash_tx, mut trash) = RingBuffer::new(8);
    let (retired_tx, _retired) = RingBuffer::new(8);
    let mut mixer = Mixer::new(command_rx, trash_tx, retired_tx, shared, rate as f32, 2);
    mixer.solo = solo;
    mixer.set_playing(true);
    let a_ring = ring_from(a, (out_base * r).round() as usize, total_frames + (request.seconds * r) as usize, out_base);
    let b_ring = ring_from(b, (request.in_start.max(0.0) * r).round() as usize, total_frames, request.in_start);
    let _ = commands.push(CallbackCmd::DeckGain { deck: 0, gain: request.out_gain });
    let _ = commands.push(CallbackCmd::DeckGain { deck: 1, gain: request.in_gain });
    let _ = commands.push(CallbackCmd::Install { deck: 0, token: 1, ring: a_ring });
    let _ = commands.push(CallbackCmd::Install { deck: 1, token: 2, ring: b_ring });
    let spec = TransitionSpec::new(1, request.seconds, Some(request.out_start), request.automation, out_base, request.in_start, r);
    let _ = commands.push(CallbackCmd::Blend(Box::new(spec)));
    mixer.apply_commands();
    let mut mix = Vec::with_capacity(total_frames * 2);
    let (mut start_frame, mut end_frame) = (None, None);
    let tail_frames = (request.tail * r) as usize;
    for i in 0..total_frames {
        let [l, rr] = mixer.next_frame();
        mix.push(l);
        mix.push(rr);
        if start_frame.is_none() && mixer.blend_started() {
            start_frame = Some(i);
        }
        if start_frame.is_some() && end_frame.is_none() && !mixer.is_blending() {
            end_frame = Some(i + 1);
        }
        if end_frame.is_some_and(|e| i >= e + tail_frames) {
            break;
        }
    }
    while trash.pop().is_ok() {}
    Rendered { rate, mix, start_frame: start_frame.unwrap_or(0), end_frame: end_frame.unwrap_or(total_frames), out_base, stems: None }
}

/// Renders the transition of `a` (outgoing) into `b` (incoming), whole
/// tracks as interleaved stereo at `rate`. With `stems`, each deck is also
/// rendered alone through the identical chain.
pub fn render_transition(a: &[f32], b: &[f32], rate: u32, request: &RenderRequest, stems: bool) -> Rendered {
    let mut rendered = render_once(a, b, rate, request, None);
    if stems {
        let out = render_once(a, b, rate, request, Some(0)).mix;
        let inc = render_once(a, b, rate, request, Some(1)).mix;
        rendered.stems = Some([out, inc]);
    }
    rendered
}

/// 16-bit PCM WAV bytes of interleaved stereo (for export and the lab).
pub fn wav_bytes(stereo: &[f32], rate: u32) -> Vec<u8> {
    let data_len = (stereo.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in stereo {
        wav.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    wav
}

#[cfg(test)]
mod tests;
