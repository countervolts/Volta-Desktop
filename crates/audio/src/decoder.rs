//! One decoder thread per loaded track.
//!
//! symphonia demux/decode → interleaved f32 → stereo → rubato resample to the
//! device rate → lock-free `rtrb` ring that the output callback drains.
//! A seek builds a fresh ring so stale audio is never played; the new
//! consumer goes to the engine, which hands it to the callback.

use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use rtrb::{Consumer, Producer, RingBuffer};
use rubato::{FftFixedInOut, Resampler};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;

use crate::http_source::HttpSource;

/// Output channels carried through the rings. The callback maps to the device.
pub const RING_CHANNELS: usize = 2;
/// Seconds of decoded audio buffered ahead of the callback.
const RING_SECONDS: usize = 4;
const RESAMPLE_CHUNK: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackSource {
    Url(String),
    File(PathBuf),
}

#[derive(Debug)]
pub enum DecoderCtrl {
    Seek(f64),
    Stop,
}

/// What the callback needs to play one ring.
pub struct RingHandle {
    pub consumer: Consumer<f32>,
    /// Set once the decoder has pushed its final sample into this ring.
    pub eof: Arc<AtomicBool>,
    /// Track time of the first sample in the ring, in seconds.
    pub base_seconds: f64,
}

pub enum DecoderMsg {
    Opened {
        deck: usize,
        generation: u64,
        duration: Option<f64>,
    },
    Ring {
        deck: usize,
        generation: u64,
        ring: RingHandle,
    },
    Failed {
        deck: usize,
        generation: u64,
        message: String,
        /// The source opened but could not be decoded, so a transcoded stream may work.
        decode_error: bool,
    },
}

pub struct DecodeRequest {
    pub deck: usize,
    pub generation: u64,
    pub source: TrackSource,
    pub hint_extension: Option<String>,
    pub start_at: f64,
    pub device_rate: u32,
}

struct Pipeline {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    source_rate: u32,
    resampler: Option<FftFixedInOut<f32>>,
    /// Planar stereo input waiting for a full resampler chunk.
    pending: [Vec<f32>; 2],
    /// Output frames still to drop to cancel the resampler's delay.
    skip_out: usize,
    scratch: Vec<f32>,
    device_rate: u32,
}

fn open_source(request: &DecodeRequest, client: &reqwest::blocking::Client) -> Result<Box<dyn MediaSource>, String> {
    match &request.source {
        TrackSource::Url(url) => HttpSource::open(client.clone(), url.clone())
            .map(|s| Box::new(s) as Box<dyn MediaSource>)
            .map_err(|e| e.to_string()),
        TrackSource::File(path) => File::open(path)
            .map(|f| Box::new(f) as Box<dyn MediaSource>)
            .map_err(|e| format!("could not open file: {e}")),
    }
}

/// A file's length from its container header (no decoding): FLAC
/// STREAMINFO, MP4 `mdhd`, Xing/VBRI or a CBR estimate for MP3, and so on.
pub fn probe_duration(path: &std::path::Path) -> Option<f64> {
    let file = File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let format = symphonia::default::get_probe().probe(&hint, mss, FormatOptions::default(), MetadataOptions::default()).ok()?;
    let track = format.default_track(TrackType::Audio)?;
    let frames = track.num_frames?;
    let seconds = track.time_base?.calc_duration(symphonia::core::units::Duration::new(frames))?.as_secs_f64();
    (seconds > 0.0).then_some(seconds)
}

impl Pipeline {
    fn open(source: Box<dyn MediaSource>, extension: Option<&str>, device_rate: u32) -> Result<(Self, Option<f64>), String> {
        let mss = MediaSourceStream::new(source, Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = extension {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
            .map_err(|e| format!("unsupported format: {e}"))?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or("no audio track")?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or("no audio codec parameters")?;
        let source_rate = params.sample_rate.ok_or("unknown sample rate")?;
        let duration = match (track.time_base, track.num_frames) {
            (Some(tb), Some(frames)) => tb
                .calc_duration(symphonia::core::units::Duration::new(frames))
                .map(|t| t.as_secs_f64()),
            _ => None,
        };
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(|e| format!("unsupported codec: {e}"))?;
        let track_id = track.id;
        let mut pipeline = Self {
            format,
            decoder,
            track_id,
            source_rate,
            resampler: None,
            pending: [Vec::new(), Vec::new()],
            skip_out: 0,
            scratch: Vec::new(),
            device_rate,
        };
        pipeline.reset_resampler()?;
        Ok((pipeline, duration))
    }

    fn reset_resampler(&mut self) -> Result<(), String> {
        self.pending = [Vec::new(), Vec::new()];
        if self.source_rate == self.device_rate {
            self.resampler = None;
            self.skip_out = 0;
            return Ok(());
        }
        let resampler = FftFixedInOut::<f32>::new(
            self.source_rate as usize,
            self.device_rate as usize,
            RESAMPLE_CHUNK,
            RING_CHANNELS,
        )
        .map_err(|e| format!("resampler: {e}"))?;
        self.skip_out = resampler.output_delay();
        self.resampler = Some(resampler);
        Ok(())
    }

    /// Seeks and returns the actual position reached, in seconds.
    fn seek(&mut self, seconds: f64) -> Result<f64, String> {
        let time = Time::try_from_secs_f64(seconds.max(0.0)).ok_or("bad seek time")?;
        let seeked = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| format!("seek failed: {e}"))?;
        self.decoder.reset();
        self.reset_resampler()?;
        let actual = self
            .format
            .tracks()
            .iter()
            .find(|t| t.id == self.track_id)
            .and_then(|t| t.time_base)
            .and_then(|tb| tb.calc_time(seeked.actual_ts))
            .map(|t| t.as_secs_f64())
            .unwrap_or(seconds);
        Ok(actual)
    }

    /// Decodes the next packet into device-rate interleaved stereo in `out`.
    /// Returns `Ok(false)` at end of stream (after flushing).
    fn next(&mut self, out: &mut Vec<f32>) -> Result<bool, String> {
        out.clear();
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    self.flush(out);
                    return Ok(false);
                }
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.flush(out);
                    return Ok(false);
                }
                Err(SymError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(e) => return Err(format!("read failed: {e}")),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(buffer) => buffer,
                // A corrupt packet is skipped, as browsers do.
                Err(SymError::DecodeError(_)) => continue,
                Err(e) => return Err(format!("decode failed: {e}")),
            };
            let channels = decoded.spec().channels().count().max(1);
            let frames = decoded.frames();
            if frames == 0 {
                continue;
            }
            self.scratch.resize(frames * channels, 0.0);
            decoded.copy_to_slice_interleaved(&mut self.scratch[..]);
            self.push_stereo(channels, frames, out);
            if !out.is_empty() {
                return Ok(true);
            }
        }
    }

    fn push_stereo(&mut self, channels: usize, frames: usize, out: &mut Vec<f32>) {
        let src = &self.scratch;
        let stereo = |frame: usize| -> (f32, f32) {
            let base = frame * channels;
            if channels == 1 {
                (src[base], src[base])
            } else {
                (src[base], src[base + 1])
            }
        };
        match self.resampler.as_mut() {
            None => {
                out.reserve(frames * 2);
                for frame in 0..frames {
                    let (l, r) = stereo(frame);
                    out.push(l);
                    out.push(r);
                }
            }
            Some(_) => {
                for frame in 0..frames {
                    let (l, r) = stereo(frame);
                    self.pending[0].push(l);
                    self.pending[1].push(r);
                }
                self.drain_resampler(out, false);
            }
        }
    }

    fn drain_resampler(&mut self, out: &mut Vec<f32>, flush: bool) {
        let Some(resampler) = self.resampler.as_mut() else { return };
        loop {
            let need = resampler.input_frames_next();
            let have = self.pending[0].len();
            let result = if have >= need {
                let input = [&self.pending[0][..need], &self.pending[1][..need]];
                let result = resampler.process(&input, None);
                self.pending[0].drain(..need);
                self.pending[1].drain(..need);
                result
            } else if flush && have > 0 {
                let input = [&self.pending[0][..], &self.pending[1][..]];
                let result = resampler.process_partial(Some(&input), None);
                self.pending[0].clear();
                self.pending[1].clear();
                result
            } else {
                break;
            };
            let Ok(planes) = result else { break };
            let produced = planes[0].len();
            let start = self.skip_out.min(produced);
            self.skip_out -= start;
            for i in start..produced {
                out.push(planes[0][i]);
                out.push(planes[1][i]);
            }
        }
    }

    fn flush(&mut self, out: &mut Vec<f32>) {
        if self.resampler.is_some() {
            self.drain_resampler(out, true);
            // Push the resampler tail so the end of the track is not cut.
            if let Some(resampler) = self.resampler.as_mut() {
                let tail = resampler.output_delay();
                if let Ok(planes) = resampler.process_partial::<&[f32]>(None, None) {
                    for i in 0..planes[0].len().min(tail) {
                        out.push(planes[0][i]);
                        out.push(planes[1][i]);
                    }
                }
            }
        }
    }
}

fn new_ring(device_rate: u32, base_seconds: f64) -> (Producer<f32>, RingHandle) {
    let capacity = device_rate as usize * RING_CHANNELS * RING_SECONDS;
    let (producer, consumer) = RingBuffer::new(capacity);
    (
        producer,
        RingHandle {
            consumer,
            eof: Arc::new(AtomicBool::new(false)),
            base_seconds,
        },
    )
}

enum Push {
    Done,
    Ctrl(DecoderCtrl),
}

/// Blocks until `samples` fit in the ring, checking for control messages.
fn push_all(producer: &mut Producer<f32>, samples: &[f32], ctrl: &Receiver<DecoderCtrl>) -> Push {
    let mut offset = 0;
    while offset < samples.len() {
        match ctrl.try_recv() {
            Ok(message) => return Push::Ctrl(message),
            Err(TryRecvError::Disconnected) => return Push::Ctrl(DecoderCtrl::Stop),
            Err(TryRecvError::Empty) => {}
        }
        let free = producer.slots();
        if free == 0 {
            std::thread::sleep(Duration::from_millis(8));
            continue;
        }
        let count = free.min(samples.len() - offset);
        // Keep frames whole so the callback never sees half a frame.
        let count = count - count % RING_CHANNELS;
        if count == 0 {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        if let Ok(chunk) = producer.write_chunk_uninit(count) {
            chunk.fill_from_iter(samples[offset..offset + count].iter().copied());
            offset += count;
        }
    }
    Push::Done
}

pub fn spawn(
    request: DecodeRequest,
    ctrl: Receiver<DecoderCtrl>,
    events: Sender<DecoderMsg>,
    client: reqwest::blocking::Client,
) {
    let name = format!("volta-decoder-{}", request.deck);
    std::thread::Builder::new()
        .name(name)
        .spawn(move || run(request, ctrl, events, client))
        .expect("spawn decoder thread");
}

fn run(request: DecodeRequest, ctrl: Receiver<DecoderCtrl>, events: Sender<DecoderMsg>, client: reqwest::blocking::Client) {
    let deck = request.deck;
    let generation = request.generation;
    let fail = |message: String, decode_error: bool| {
        let _ = events.send(DecoderMsg::Failed {
            deck,
            generation,
            message,
            decode_error,
        });
    };
    let source = match open_source(&request, &client) {
        Ok(source) => source,
        Err(message) => return fail(message, false),
    };
    let (mut pipeline, duration) =
        match Pipeline::open(source, request.hint_extension.as_deref(), request.device_rate) {
            Ok(opened) => opened,
            Err(message) => return fail(message, true),
        };
    let _ = events.send(DecoderMsg::Opened {
        deck,
        generation,
        duration,
    });
    let mut base = 0.0;
    if request.start_at > 0.0 {
        base = pipeline.seek(request.start_at).unwrap_or(0.0);
    }
    let (mut producer, ring) = new_ring(request.device_rate, base);
    let mut eof = ring.eof.clone();
    let _ = events.send(DecoderMsg::Ring { deck, generation, ring });
    let mut buffer = Vec::with_capacity(8192);
    let mut ended = false;

    let handle_ctrl = |message: DecoderCtrl,
                           pipeline: &mut Pipeline,
                           producer: &mut Producer<f32>,
                           eof: &mut Arc<AtomicBool>,
                           ended: &mut bool|
     -> bool {
        match message {
            DecoderCtrl::Stop => false,
            DecoderCtrl::Seek(seconds) => {
                let actual = pipeline.seek(seconds).unwrap_or(seconds);
                let (next_producer, ring) = new_ring(request.device_rate, actual);
                *producer = next_producer;
                *eof = ring.eof.clone();
                *ended = false;
                let _ = events.send(DecoderMsg::Ring { deck, generation, ring });
                true
            }
        }
    };

    loop {
        if ended {
            // Stay alive so a seek after the end can replay.
            match ctrl.recv() {
                Ok(message) => {
                    if !handle_ctrl(message, &mut pipeline, &mut producer, &mut eof, &mut ended) {
                        return;
                    }
                }
                Err(_) => return,
            }
            continue;
        }
        match ctrl.try_recv() {
            Ok(message) => {
                if !handle_ctrl(message, &mut pipeline, &mut producer, &mut eof, &mut ended) {
                    return;
                }
                continue;
            }
            Err(TryRecvError::Disconnected) => return,
            Err(TryRecvError::Empty) => {}
        }
        let more = match pipeline.next(&mut buffer) {
            Ok(more) => more,
            Err(message) => {
                // Mid-stream failure: play what we have, then report.
                eof.store(true, Ordering::Release);
                return fail(message, true);
            }
        };
        match push_all(&mut producer, &buffer, &ctrl) {
            Push::Done => {}
            Push::Ctrl(message) => {
                if !handle_ctrl(message, &mut pipeline, &mut producer, &mut eof, &mut ended) {
                    return;
                }
                continue;
            }
        }
        if !more {
            eof.store(true, Ordering::Release);
            ended = true;
        }
    }
}

/// Audio cut for AutoMix analysis (`decodeClip` in automix-analyzer.ts).
pub struct AnalysisClip {
    pub sample_rate: f64,
    pub duration: f64,
    /// 45 s from 15 s in (or what the track allows), per channel.
    pub clip: Vec<Vec<f32>>,
    pub intro: Vec<Vec<f32>>,
    pub outro: Vec<Vec<f32>>,
    pub outro_offset: f64,
}

const ANALYSIS_DECODE_RATE: u32 = 22050;
const ANALYSIS_MAX_SECONDS: f64 = 900.0;

/// Decodes a whole track at 22.05 kHz and cuts the analysis windows.
pub fn decode_for_analysis(source: &TrackSource, extension: Option<&str>, client: &reqwest::blocking::Client) -> Option<AnalysisClip> {
    let request = DecodeRequest {
        deck: 0,
        generation: 0,
        source: source.clone(),
        hint_extension: extension.map(str::to_string),
        start_at: 0.0,
        device_rate: ANALYSIS_DECODE_RATE,
    };
    let media = open_source(&request, client).ok()?;
    let (mut pipeline, _) = Pipeline::open(media, extension, ANALYSIS_DECODE_RATE).ok()?;
    let rate = f64::from(ANALYSIS_DECODE_RATE);
    let limit = (ANALYSIS_MAX_SECONDS * rate) as usize;
    let (mut left, mut right) = (Vec::new(), Vec::new());
    let mut chunk = Vec::new();
    loop {
        let more = pipeline.next(&mut chunk).ok()?;
        for frame in chunk.chunks_exact(2) {
            left.push(frame[0]);
            right.push(frame[1]);
        }
        if left.len() > limit {
            return None;
        }
        if !more {
            break;
        }
    }
    let frames = left.len();
    if frames == 0 {
        return None;
    }
    let duration = frames as f64 / rate;
    let start = 15f64.min((duration - 5.0).max(0.0));
    let length = 45f64.min((duration - start).max(1.0));
    let (s, n) = ((start * rate) as usize, (length * rate) as usize);
    let cut = |a: usize, b: usize| vec![left[a.min(frames)..b.min(frames)].to_vec(), right[a.min(frames)..b.min(frames)].to_vec()];
    let edge = frames.min((16.0 * rate) as usize);
    let outro_start = frames - edge;
    Some(AnalysisClip {
        sample_rate: rate,
        duration,
        clip: cut(s, s + n),
        intro: cut(0, edge),
        outro: cut(outro_start, frames),
        outro_offset: outro_start as f64 / rate,
    })
}

/// The whole track as mono PCM at 11.025 kHz (22.05 kHz decode, averaged
/// pairs), for the native AutoMix analysis. `None` past 15 minutes.
pub fn decode_mono_11k(source: &TrackSource, extension: Option<&str>, client: &reqwest::blocking::Client) -> Option<Vec<f32>> {
    let request = DecodeRequest {
        deck: 0,
        generation: 0,
        source: source.clone(),
        hint_extension: extension.map(str::to_string),
        start_at: 0.0,
        device_rate: ANALYSIS_DECODE_RATE,
    };
    let media = open_source(&request, client).ok()?;
    let (mut pipeline, _) = Pipeline::open(media, extension, ANALYSIS_DECODE_RATE).ok()?;
    let limit = (ANALYSIS_MAX_SECONDS * 11025.0) as usize;
    let mut mono = Vec::new();
    let mut chunk = Vec::new();
    let mut carry: Option<f32> = None;
    loop {
        let more = pipeline.next(&mut chunk).ok()?;
        for frame in chunk.chunks_exact(2) {
            let sample = (frame[0] + frame[1]) * 0.5;
            match carry.take() {
                Some(previous) => mono.push((previous + sample) * 0.5),
                None => carry = Some(sample),
            }
        }
        if mono.len() > limit {
            return None;
        }
        if !more {
            break;
        }
    }
    (!mono.is_empty()).then_some(mono)
}
