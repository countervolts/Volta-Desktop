//! The audio service thread. Owns the cpal stream and both decks, turns UI
//! commands into decoder and callback commands, and reports events back.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, select, tick, unbounded};

use crate::decoder::{self, DecodeRequest, DecoderCtrl, DecoderMsg, TrackSource};
use crate::output::{self, CallbackCmd, DECKS, Output, Shared};
use crate::{Command, Event, LoadRequest};

#[derive(Default)]
struct Deck {
    token: u64,
    generation: u64,
    ctrl: Option<Sender<DecoderCtrl>>,
    request: Option<LoadRequest>,
    /// The compatible fallback has been tried for this load.
    fell_back: bool,
    duration: Option<f64>,
    /// Ring installed at least once (track is playable).
    ready: bool,
    /// This deck holds the preloaded next track rather than the current one.
    preload: bool,
}

pub(crate) struct Engine {
    shared: Arc<Shared>,
    output: Option<Output>,
    decks: [Deck; DECKS],
    active: usize,
    playing: bool,
    generation: u64,
    seen_advance: u64,
    seen_ended: u64,
    /// A blend command is in flight or mixing.
    blending: bool,
    decoder_tx: Sender<DecoderMsg>,
    decoder_rx: Receiver<DecoderMsg>,
    events: Sender<Event>,
    client: reqwest::blocking::Client,
}

impl Engine {
    pub(crate) fn new(shared: Arc<Shared>, events: Sender<Event>) -> Self {
        let (decoder_tx, decoder_rx) = unbounded();
        Self {
            shared,
            output: None,
            decks: Default::default(),
            active: 0,
            playing: false,
            generation: 0,
            seen_advance: 0,
            seen_ended: 0,
            blending: false,
            decoder_tx,
            decoder_rx,
            events,
            client: crate::http_source::http_client(),
        }
    }

    fn ensure_output(&mut self) -> bool {
        if self.output.is_some() {
            return true;
        }
        match output::open(self.shared.clone()) {
            Ok(out) => {
                log::info!("audio output open at {} Hz", out.sample_rate);
                self.output = Some(out);
                self.seen_advance = self.shared.advance_count.load(Ordering::Acquire);
                true
            }
            Err(error) => {
                let _ = self.events.send(Event::Error {
                    token: 0,
                    message: format!("Audio output unavailable: {error}"),
                });
                false
            }
        }
    }

    fn send(&mut self, command: CallbackCmd) {
        if let Some(output) = self.output.as_mut()
            && output.commands.push(command).is_err() {
                log::warn!("audio command queue full");
            }
    }

    fn stop_deck(&mut self, deck: usize) {
        if deck == self.active {
            self.shared.pending_seek.store(f64::NAN.to_bits(), Ordering::Release);
        }
        if let Some(ctrl) = self.decks[deck].ctrl.take() {
            let _ = ctrl.send(DecoderCtrl::Stop);
        }
        self.decks[deck] = Deck::default();
        self.send(CallbackCmd::Clear { deck });
    }

    fn start_decoder(&mut self, deck: usize, start_at: f64, source: TrackSource) {
        let Some(output) = self.output.as_ref() else { return };
        let device_rate = output.sample_rate;
        self.generation += 1;
        let generation = self.generation;
        let (ctrl_tx, ctrl_rx) = unbounded();
        let slot = &mut self.decks[deck];
        slot.generation = generation;
        slot.ctrl = Some(ctrl_tx);
        slot.ready = false;
        let hint = slot.request.as_ref().and_then(|r| r.extension.clone());
        decoder::spawn(
            DecodeRequest {
                deck,
                generation,
                source,
                hint_extension: hint,
                start_at,
                device_rate,
            },
            ctrl_rx,
            self.decoder_tx.clone(),
            self.client.clone(),
        );
    }

    fn load(&mut self, request: LoadRequest, preload: bool) {
        if !self.ensure_output() {
            return;
        }
        let deck = if preload { 1 - self.active } else { self.active };
        if !preload {
            // A new current track replaces everything, including a preloaded next.
            for index in 0..DECKS {
                self.stop_deck(index);
            }
            self.send(CallbackCmd::Disarm);
            self.send(CallbackCmd::Activate { deck });
            self.shared.deck_base[deck].store(request.start_at.to_bits(), Ordering::Release);
            self.shared.deck_frames[deck].store(0, Ordering::Release);
            self.shared.active_deck.store(deck, Ordering::Release);
            self.playing = request.autoplay;
            self.send(CallbackCmd::Playing(false));
        } else {
            self.stop_deck(deck);
        }
        self.send(CallbackCmd::DeckGain {
            deck,
            gain: request.gain,
        });
        let start_at = request.start_at;
        let source = request.source.clone();
        self.decks[deck].token = request.token;
        self.decks[deck].preload = preload;
        self.decks[deck].request = Some(request);
        self.start_decoder(deck, start_at, source);
    }

    fn deck_for(&self, deck: usize, generation: u64) -> bool {
        self.decks[deck].generation == generation && self.decks[deck].ctrl.is_some()
    }

    fn on_decoder(&mut self, message: DecoderMsg) {
        match message {
            DecoderMsg::Opened {
                deck,
                generation,
                duration,
            } => {
                if !self.deck_for(deck, generation) {
                    return;
                }
                self.decks[deck].duration = duration;
                let _ = self.events.send(Event::Opened {
                    token: self.decks[deck].token,
                    duration,
                });
            }
            DecoderMsg::Ring { deck, generation, ring } => {
                if !self.deck_for(deck, generation) {
                    return;
                }
                let token = self.decks[deck].token;
                let first = !self.decks[deck].ready;
                self.decks[deck].ready = true;
                self.send(CallbackCmd::Install { deck, token, ring });
                if self.decks[deck].preload {
                    if deck != self.active {
                        self.send(CallbackCmd::Arm { deck });
                    }
                } else if first {
                    self.send(CallbackCmd::Playing(self.playing));
                    let _ = self.events.send(Event::Started { token });
                }
            }
            DecoderMsg::Failed {
                deck,
                generation,
                message,
                decode_error,
            } => {
                if !self.deck_for(deck, generation) {
                    return;
                }
                let slot = &self.decks[deck];
                let token = slot.token;
                let fallback = slot.request.as_ref().and_then(|r| r.fallback.clone());
                if decode_error && !slot.fell_back
                    && let Some(fallback) = fallback {
                        // Same behaviour as the web: retry the transcoded stream
                        // at the current position.
                        let position = if deck == self.active && !slot.preload {
                            self.shared.position()
                        } else {
                            0.0
                        };
                        let preload = slot.preload;
                        self.decks[deck].fell_back = true;
                        if let Some(old) = self.decks[deck].ctrl.take() {
                            let _ = old.send(DecoderCtrl::Stop);
                        }
                        if !preload {
                            self.send(CallbackCmd::Playing(false));
                        }
                        self.start_decoder(deck, position, fallback);
                        let _ = self.events.send(Event::FellBack { token });
                        return;
                    }
                log::warn!("track failed: {message}");
                let preload = slot.preload;
                if preload {
                    self.stop_deck(deck);
                    self.send(CallbackCmd::Disarm);
                }
                let _ = self.events.send(Event::Error { token, message });
            }
        }
    }

    fn poll_callback(&mut self) {
        // Free rings the callback has retired.
        if let Some(output) = self.output.as_mut() {
            while output.trash.pop().is_ok() {}
        }
        let advance = self.shared.advance_count.load(Ordering::Acquire);
        if advance != self.seen_advance {
            self.seen_advance = advance;
            let next = self.shared.active_deck.load(Ordering::Acquire);
            let previous = self.active;
            if next != previous {
                self.blending = false;
                self.active = next;
                self.decks[next].preload = false;
                // The old deck is now free for the following preload.
                self.stop_deck(previous);
                let _ = self.events.send(Event::Advanced {
                    token: self.decks[next].token,
                    duration: self.decks[next].duration,
                });
            }
        }
        let ended = self.shared.ended_token.load(Ordering::Acquire);
        if ended != 0 && ended != self.seen_ended {
            self.seen_ended = ended;
            if ended == self.decks[self.active].token {
                self.playing = false;
                self.send(CallbackCmd::Playing(false));
                let _ = self.events.send(Event::Ended { token: ended });
            }
        }
        if self.shared.stream_failed.swap(false, Ordering::AcqRel) {
            self.rebuild_output();
        }
    }

    /// Output device changed or vanished: reopen and resume where we were.
    fn rebuild_output(&mut self) {
        let position = self.shared.position();
        let current = self.decks[self.active].request.clone();
        let next = self.decks[1 - self.active]
            .request
            .clone()
            .filter(|_| self.decks[1 - self.active].preload);
        let playing = self.playing;
        for index in 0..DECKS {
            self.stop_deck(index);
        }
        self.output = None;
        self.active = 0;
        if let Some(mut request) = current {
            request.start_at = position;
            request.autoplay = playing;
            self.load(request, false);
            if let Some(next) = next {
                self.load(next, true);
            }
        }
    }

    /// Stops a running blend. The incoming deck has been partly consumed, so
    /// its track is decoded again from the start as the preload.
    fn cancel_blend(&mut self) {
        if !self.blending {
            return;
        }
        self.blending = false;
        self.send(CallbackCmd::CancelBlend);
        let other = 1 - self.active;
        if self.decks[other].preload
            && let Some(request) = self.decks[other].request.clone() {
                self.load(request, true);
            }
    }

    fn command(&mut self, command: Command) -> bool {
        if matches!(command, Command::Load(_) | Command::ClearPreload | Command::Pause | Command::Seek(_) | Command::Stop | Command::CancelBlend) {
            self.cancel_blend();
        }
        match command {
            Command::Blend(request) => {
                let other = 1 - self.active;
                let deck = &self.decks[other];
                if !self.blending && deck.preload && deck.ready && deck.token == request.token && self.decks[self.active].ready {
                    let rate = f64::from(self.shared.sample_rate.load(Ordering::Relaxed).max(1));
                    let base = f64::from_bits(self.shared.deck_base[self.active].load(Ordering::Acquire));
                    self.blending = true;
                    self.send(CallbackCmd::Blend(crate::output::BlendSpec {
                        deck: other,
                        frames: (request.seconds.max(0.01) * rate) as u64,
                        linear: request.curve == crate::BlendCurve::Linear,
                        start_frame: request.start_at.map(|t| ((t - base).max(0.0) * rate) as u64),
                        tempo: request.tempo.clamp(0.5, 2.0),
                        release: (request.release_seconds.max(0.0) * rate) as u64,
                        bass_swap: request.bass_swap,
                        gain_start: 10f64.powf(request.gain_db / 20.0) as f32,
                    }));
                }
            }
            Command::CancelBlend => {}
            Command::Load(request) => self.load(request, false),
            Command::Preload(request) => self.load(request, true),
            Command::ClearPreload => {
                let other = 1 - self.active;
                if self.decks[other].preload {
                    self.stop_deck(other);
                }
                self.send(CallbackCmd::Disarm);
            }
            Command::Play => {
                self.playing = true;
                if self.decks[self.active].ready {
                    self.send(CallbackCmd::Playing(true));
                }
                // Replay after the end: seek back to the start.
                if self.seen_ended == self.decks[self.active].token && self.seen_ended != 0 {
                    self.seen_ended = 0;
                    self.shared.ended_token.store(0, Ordering::Release);
                    if let Some(ctrl) = self.decks[self.active].ctrl.as_ref() {
                        let _ = ctrl.send(DecoderCtrl::Seek(0.0));
                    }
                }
            }
            Command::Pause => {
                self.playing = false;
                self.send(CallbackCmd::Playing(false));
            }
            Command::Seek(seconds) => {
                let deck = self.active;
                if self.seen_ended == self.decks[deck].token {
                    self.seen_ended = 0;
                    self.shared.ended_token.store(0, Ordering::Release);
                }
                // Show the target right away; the callback clears it on install.
                if let Some(ctrl) = self.decks[deck].ctrl.as_ref() {
                    self.shared.pending_seek.store(seconds.to_bits(), Ordering::Release);
                    let _ = ctrl.send(DecoderCtrl::Seek(seconds));
                }
            }
            Command::SetVolume(volume) => {
                self.shared
                    .master
                    .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Release);
            }
            Command::SetDeckGain { token, gain } => {
                for deck in 0..DECKS {
                    if self.decks[deck].token == token {
                        self.send(CallbackCmd::DeckGain { deck, gain });
                    }
                }
            }
            Command::Stop => {
                self.playing = false;
                for index in 0..DECKS {
                    self.stop_deck(index);
                }
                self.send(CallbackCmd::Playing(false));
            }
            Command::Shutdown => return false,
        }
        true
    }

    pub(crate) fn run(mut self, commands: Receiver<Command>) {
        let ticker = tick(Duration::from_millis(5));
        let decoder_rx = self.decoder_rx.clone();
        loop {
            select! {
                recv(commands) -> command => match command {
                    Ok(command) => {
                        if !self.command(command) {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                recv(decoder_rx) -> message => {
                    if let Ok(message) = message {
                        self.on_decoder(message);
                    }
                }
                recv(ticker) -> _ => self.poll_callback(),
            }
        }
        for index in 0..DECKS {
            self.stop_deck(index);
        }
    }
}
