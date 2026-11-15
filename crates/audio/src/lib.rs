//! Volta audio service.
//!
//! A dedicated engine thread owns the cpal output stream and two decks (A/B).
//! The UI talks to it only through [`AudioHandle`]: commands go in over a
//! channel, events come back over another, and the playback clock is read from
//! atomics the realtime callback publishes, so the progress bar and the lyric
//! fill follow the samples actually handed to the device.
//!
//! Deck B already pre-decodes the next track. When the current track drains,
//! the callback switches to it at the next sample, which is gapless. A blend
//! (Crossfade, AutoMix) mixes both decks in the callback, then switches.

mod decoder;
mod engine;
mod http_source;
mod output;
pub mod stretch;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crossbeam_channel::{Receiver, Sender, unbounded};

pub use decoder::{AnalysisClip, TrackSource, decode_for_analysis, decode_mono_11k, probe_duration};
pub use http_source::http_client;

/// A track to open on a deck.
#[derive(Debug, Clone)]
pub struct LoadRequest {
    /// Caller-chosen identifier, unique per load (reloading a song uses a new one).
    pub token: u64,
    pub source: TrackSource,
    /// Transcoded stream tried once if `source` cannot be decoded.
    pub fallback: Option<TrackSource>,
    /// File extension hint for the demuxer (`flac`, `m4a`, ...).
    pub extension: Option<String>,
    pub start_at: f64,
    pub autoplay: bool,
    /// Linear ReplayGain factor for this track.
    pub gain: f32,
}

#[derive(Debug)]
pub enum Command {
    /// Replace the current track.
    Load(LoadRequest),
    /// Pre-decode the next track on the idle deck for a gapless switch.
    Preload(LoadRequest),
    ClearPreload,
    Play,
    Pause,
    Seek(f64),
    /// Effective master volume, 0..1 (0 when muted).
    SetVolume(f32),
    SetDeckGain { token: u64, gain: f32 },
    /// Crossfade into the preloaded track `token` over `seconds`: both decks
    /// play, mixed sample by sample with `curve`, then the callback switches
    /// to the incoming deck and reports [`Event::Advanced`]. Ignored unless
    /// that track is preloaded and decoded.
    Blend(BlendRequest),
    /// Abandon a running blend; the incoming track is preloaded again.
    CancelBlend,
    Stop,
    Shutdown,
}

/// A transition into the preloaded track `token`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlendRequest {
    pub token: u64,
    /// Blend length in seconds of outgoing time.
    pub seconds: f64,
    pub curve: BlendCurve,
    /// Outgoing track time at which to start (sample-accurate); `None` = now.
    pub start_at: Option<f64>,
    /// Incoming playback rate during the blend (pitch preserved).
    pub tempo: f64,
    /// Seconds for the incoming rate to ease back to 1 afterwards.
    pub release_seconds: f64,
    pub bass_swap: bool,
    /// Incoming level change at the blend start, eased out by its end.
    pub gain_db: f64,
}

/// Blend gain curve: equal-power (cos/sin) or linear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendCurve {
    EqualPower,
    Linear,
}

#[derive(Debug, Clone)]
pub enum Event {
    /// Container opened; duration from the stream when known.
    Opened { token: u64, duration: Option<f64> },
    /// The first audio for this load is queued for output.
    Started { token: u64 },
    /// The callback moved on to the preloaded track without a gap.
    Advanced { token: u64, duration: Option<f64> },
    /// The current track finished with nothing preloaded after it.
    Ended { token: u64 },
    /// The original stream failed to decode; the compatible stream is in use.
    FellBack { token: u64 },
    Error { token: u64, message: String },
}

pub struct AudioHandle {
    commands: Sender<Command>,
    events: Receiver<Event>,
    shared: Arc<output::Shared>,
}

impl AudioHandle {
    /// Starts the engine thread. The output device opens on the first load.
    pub fn spawn() -> Self {
        let (commands, command_rx) = unbounded();
        let (event_tx, events) = unbounded();
        let shared = Arc::new(output::Shared::new());
        let engine = engine::Engine::new(shared.clone(), event_tx);
        std::thread::Builder::new()
            .name("volta-audio".into())
            .spawn(move || engine.run(command_rx))
            .expect("spawn audio engine");
        Self {
            commands,
            events,
            shared,
        }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn try_event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    /// Current position of the audible track, in seconds.
    pub fn position(&self) -> f64 {
        self.shared.position()
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Acquire)
    }

    /// Token of the track currently audible (0 when none).
    pub fn active_token(&self) -> u64 {
        let deck = self.shared.active_deck.load(Ordering::Acquire);
        self.shared.deck_token[deck.min(1)].load(Ordering::Acquire)
    }

    /// A crossfade is mixing both decks.
    pub fn is_blending(&self) -> bool {
        self.shared.blending.load(Ordering::Acquire)
    }

    pub fn underruns(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed)
    }
}

impl Drop for AudioHandle {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}

/// Replaces Subsonic credentials in a URL (`t`, `s`, `p`, `u`) for logs and errors.
pub fn redact_url(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let query = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((key, _)) if matches!(key, "t" | "s" | "p" | "u") => format!("{key}=REDACTED"),
            _ => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}?{query}")
}

/// A reqwest error message with its URL removed (it may carry credentials).
pub fn redact_error(error: &reqwest::Error) -> String {
    let mut text = error.to_string();
    if let Some(url) = error.url() {
        text = text.replace(url.as_str(), &redact_url(url.as_str()));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_credentials() {
        assert_eq!(
            redact_url("https://m.test/rest/stream.view?u=me&t=abc&s=salt&v=1.16.1&id=7"),
            "https://m.test/rest/stream.view?u=REDACTED&t=REDACTED&s=REDACTED&v=1.16.1&id=7"
        );
        assert_eq!(redact_url("https://m.test/x"), "https://m.test/x");
    }
}
