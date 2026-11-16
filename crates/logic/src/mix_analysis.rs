//! Native AutoMix analysis (designed for the desktop, not a port).
//!
//! The whole track is analysed, not a clip:
//! * onset envelope (spectral flux, ~23 ms hop at 11.025 kHz),
//! * global tempo (prior-weighted autocorrelation) refined by a
//!   dynamic-programming beat tracker (Ellis 2007) that follows the actual
//!   beat positions through the song,
//! * downbeat phase from low-band onset strength on every 4th beat,
//! * key from whole-track chroma,
//! * audible start/end (trailing and leading silence are never mixed over),
//! * gated loudness (dB) for level matching, and per-bar energy (dB),
//! * (v4) a 2 Hz energy envelope (the energy-profile metadata of Apple's
//!   crossfade patent US 8,553,504), fade-in/fade-out lengths, per-bar
//!   low-band share and a mid-band tonality proxy, and section boundaries
//!   from Foote novelty on beat-synchronous features (8-bar checkerboard
//!   kernel, peaks above 0.3 × max in a 4-bar window, snapped to a 4-bar
//!   phrase grid, with a salience check), giving intro/outro lengths in bars
//!   and how much the ending resembles the start (looping endings).

use serde::{Deserialize, Serialize};

use crate::audio_analysis::{ANALYSIS_FFT_SIZE, ANALYSIS_HOP, ANALYSIS_SAMPLE_RATE, estimate_key, estimate_tempo, fft_in_place};

pub const PROFILE_VERSION: u32 = 4;
const SILENCE_DB: f64 = -48.0;
/// Seconds from a frame centre to the attack that peaks its flux (measured:
/// tracked beats of synthetic kicks sat 20–21 ms early before this).
const ONSET_LATENCY: f64 = 0.020;
/// Weight of low-band (< 250 Hz) flux against broadband flux (each relative
/// to its own mean) in the tracker's onset envelope.
const LOW_ONSET_WEIGHT: f32 = 0.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MixProfile {
    pub version: u32,
    pub duration: f64,
    pub bpm: f64,
    /// 0–1: how consistently the tracked beats line up with onsets.
    pub beat_confidence: f64,
    /// Beat times in seconds.
    pub beats: Vec<f64>,
    /// Index into `beats` of the first downbeat (0–3).
    pub downbeat_phase: usize,
    pub downbeat_confidence: f64,
    pub camelot: u8,
    pub minor: bool,
    pub key_confidence: f64,
    /// Gated mean level in dBFS.
    pub loudness_db: f64,
    /// First and last audible moments, in seconds.
    pub start: f64,
    pub end: f64,
    /// Level per bar (dBFS), bar `i` starting at downbeat `i`.
    pub bar_energy: Vec<f32>,
    /// Level (dBFS) every 0.5 s from the track start.
    #[serde(default)]
    pub envelope_db: Vec<f32>,
    /// Seconds of fade at the start and end (0 = hard start/ending).
    #[serde(default)]
    pub fade_in: f64,
    #[serde(default)]
    pub fade_out: f64,
    /// Share of each bar's energy below 200 Hz (0–1).
    #[serde(default)]
    pub bar_low: Vec<f32>,
    /// Tonal mid-band (300–3400 Hz) activity per bar (0–1). A proxy for
    /// vocals or lead lines; NOT a validated vocal detector.
    #[serde(default)]
    pub bar_tonal: Vec<f32>,
    /// Section boundaries (downbeat times, s) and their novelty (0–1).
    #[serde(default)]
    pub boundaries: Vec<(f64, f32)>,
    /// Bars before the first salient boundary (the intro), and after the
    /// last one (the outro); 0 when no clear boundary sits near the edge.
    #[serde(default)]
    pub intro_bars: u32,
    #[serde(default)]
    pub outro_bars: u32,
    /// Bar offset (0–3) of the 4-bar phrase grid from the first downbeat.
    #[serde(default)]
    pub phrase_phase: u32,
    /// Similarity (0–1) of the last four bars to the first four.
    #[serde(default)]
    pub loop_ending: f32,
}

impl MixProfile {
    /// Downbeat times (every 4th beat from the phase).
    pub fn downbeats(&self) -> impl Iterator<Item = f64> + '_ {
        self.beats.iter().skip(self.downbeat_phase).step_by(4).copied()
    }

    pub fn beat_period(&self) -> f64 {
        60.0 / self.bpm.max(1.0)
    }

    pub fn bar_period(&self) -> f64 {
        4.0 * self.beat_period()
    }

    /// Mean level (dB) of the envelope over `[from, to)` seconds.
    pub fn level_between(&self, from: f64, to: f64) -> Option<f64> {
        let (a, b) = ((from.max(0.0) * 2.0) as usize, ((to.max(0.0) * 2.0).ceil() as usize).min(self.envelope_db.len()));
        (b > a).then(|| power_mean_db(&self.envelope_db[a..b]))
    }

    /// Level trend (dB per second, least squares) over `[from, to)`.
    pub fn trend_between(&self, from: f64, to: f64) -> Option<f64> {
        let (a, b) = ((from.max(0.0) * 2.0) as usize, ((to.max(0.0) * 2.0).ceil() as usize).min(self.envelope_db.len()));
        if b < a + 3 {
            return None;
        }
        let points: Vec<(f64, f64)> = (a..b).map(|i| (i as f64 * 0.5, f64::from(self.envelope_db[i]).max(-80.0))).collect();
        let n = points.len() as f64;
        let (mx, my) = (points.iter().map(|p| p.0).sum::<f64>() / n, points.iter().map(|p| p.1).sum::<f64>() / n);
        let (num, den) = points.iter().fold((0.0, 0.0), |(u, v), p| (u + (p.0 - mx) * (p.1 - my), v + (p.0 - mx).powi(2)));
        (den > 0.0).then(|| num / den)
    }

    /// Index of the bar whose downbeat is at or before `t`.
    pub fn bar_at(&self, t: f64) -> Option<usize> {
        let downbeats: Vec<f64> = self.downbeats().collect();
        downbeats.iter().rposition(|d| *d <= t + 1e-6)
    }
}

/// Mean of dB values taken in the power domain.
pub fn power_mean_db(values: &[f32]) -> f64 {
    if values.is_empty() {
        return -120.0;
    }
    10.0 * (values.iter().map(|d| 10f64.powf(f64::from(*d) / 10.0)).sum::<f64>() / values.len() as f64).max(1e-12).log10()
}

struct Features {
    onset: Vec<f32>,
    low_onset: Vec<f32>,
    chroma: [f64; 12],
    /// Frame RMS in dBFS.
    level_db: Vec<f32>,
    frame_rate: f64,
    /// Per frame: chroma (12), band powers (low < 200 Hz < mid < 2.5 kHz <
    /// high), and mid-band tonality (1 − spectral flatness, 300–3400 Hz).
    frame_chroma: Vec<[f32; 12]>,
    bands: Vec<[f32; 3]>,
    tonal: Vec<f32>,
}

fn features(samples: &[f32]) -> Features {
    let (size, hop) = (ANALYSIS_FFT_SIZE, ANALYSIS_HOP);
    let rate = ANALYSIS_SAMPLE_RATE;
    let window: Vec<f32> = (0..size).map(|i| (0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / (size - 1) as f64).cos())) as f32).collect();
    let bins = size / 2;
    let frames = if samples.len() >= size { (samples.len() - size) / hop + 1 } else { 0 };
    let pitch: Vec<i8> = (0..bins)
        .map(|bin| {
            let f = bin as f64 * rate / size as f64;
            if bin == 0 || !(65.4..=1975.5).contains(&f) {
                -1
            } else {
                ((69.0 + 12.0 * (f / 440.0).log2()).round() as i64).rem_euclid(12) as i8
            }
        })
        .collect();
    let low_bins = (250.0 * size as f64 / rate) as usize;
    let band_of = |bin: usize| {
        let f = bin as f64 * rate / size as f64;
        if f < 200.0 { 0 } else if f < 2500.0 { 1 } else { 2 }
    };
    let band: Vec<usize> = (0..bins).map(band_of).collect();
    let (tonal_lo, tonal_hi) = ((300.0 * size as f64 / rate) as usize, (3400.0 * size as f64 / rate) as usize);
    let mut frame_chroma = vec![[0f32; 12]; frames];
    let mut bands = vec![[0f32; 3]; frames];
    let mut tonal = vec![0f32; frames];
    let mut onset = vec![0f32; frames];
    let mut low_onset = vec![0f32; frames];
    let mut level_db = vec![-120f32; frames];
    let mut chroma = [0f64; 12];
    let mut previous = vec![0f32; bins];
    let (mut re, mut im) = (vec![0f32; size], vec![0f32; size]);
    for frame in 0..frames {
        let start = frame * hop;
        let mut energy = 0f64;
        for i in 0..size {
            let s = samples[start + i];
            energy += f64::from(s * s);
            re[i] = s * window[i];
            im[i] = 0.0;
        }
        level_db[frame] = (10.0 * (energy / size as f64).max(1e-12).log10()) as f32;
        fft_in_place(&mut re, &mut im);
        let (mut flux, mut low) = (0f32, 0f32);
        let (mut log_sum, mut lin_sum) = (0f64, 0f64);
        for bin in 1..bins {
            let raw = re[bin].hypot(im[bin]);
            // Log magnitude makes the flux follow rhythm, not loudness.
            let magnitude = (1.0 + 100.0 * raw).ln();
            let delta = magnitude - previous[bin];
            if delta > 0.0 {
                flux += delta;
                if bin <= low_bins {
                    low += delta;
                }
            }
            previous[bin] = magnitude;
            if pitch[bin] >= 0 {
                chroma[pitch[bin] as usize] += f64::from(raw);
                frame_chroma[frame][pitch[bin] as usize] += raw;
            }
            bands[frame][band[bin]] += raw * raw;
            if (tonal_lo..tonal_hi).contains(&bin) {
                let power = f64::from(raw * raw) + 1e-12;
                log_sum += power.ln();
                lin_sum += power;
            }
        }
        let count = (tonal_hi - tonal_lo).max(1) as f64;
        let flatness = (log_sum / count).exp() / (lin_sum / count).max(1e-12);
        tonal[frame] = (1.0 - flatness).clamp(0.0, 1.0) as f32;
        onset[frame] = flux;
        low_onset[frame] = low;
    }
    Features { onset, low_onset, chroma, level_db, frame_rate: rate / hop as f64, frame_chroma, bands, tonal }
}

/// Analysis switches, for measuring tracker changes one by one against the
/// previous behaviour (the lab's `tracker-variants`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnalysisOptions {
    /// Everything as in v3 (overrides the switches below).
    pub legacy_tracker: bool,
    /// Every frame links to its best predecessor (v3 restarted the chain
    /// where the cumulative score dipped below zero).
    pub unbroken_chain: bool,
    /// Onsets normalised over a sliding ±3 s window (v3: whole track).
    pub local_onsets: bool,
    /// Weight of low-band flux in the tracker's envelope (v3: 0).
    pub low_weight: f32,
    /// Beat support judged against the local onset mean (v3: global).
    pub local_support: bool,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        Self { legacy_tracker: false, unbroken_chain: true, local_onsets: true, low_weight: LOW_ONSET_WEIGHT, local_support: true }
    }
}

impl AnalysisOptions {
    pub fn legacy() -> Self {
        Self { legacy_tracker: true, unbroken_chain: false, local_onsets: false, low_weight: 0.0, local_support: false }
    }
}

/// Ellis' dynamic-programming beat tracker on a normalised onset envelope.
/// Every frame links to its best predecessor, so the beat chain runs through
/// quiet sections on tempo continuity instead of restarting after them.
fn track_beats(onset: &[f32], frame_rate: f64, bpm: f64, unbroken: bool, local_onsets: bool) -> Vec<usize> {
    let n = onset.len();
    if n < 16 {
        return Vec::new();
    }
    let mean = onset.iter().sum::<f32>() / n as f32;
    let deviation = (onset.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n as f32).sqrt().max(1e-6);
    let local: Vec<f32> = if !local_onsets {
        onset.iter().map(|v| (v - mean) / deviation).collect()
    } else {
        // Normalised over a sliding ±3 s window: a quiet intro's onsets are
        // as salient to the tracker as a loud chorus's.
        let radius = (3.0 * frame_rate) as usize;
        let (mut sum, mut squares) = (vec![0f64; n + 1], vec![0f64; n + 1]);
        for (i, v) in onset.iter().enumerate() {
            sum[i + 1] = sum[i] + f64::from(*v);
            squares[i + 1] = squares[i] + f64::from(*v) * f64::from(*v);
        }
        (0..n)
            .map(|i| {
                let (lo, hi) = (i.saturating_sub(radius), (i + radius + 1).min(n));
                let count = (hi - lo) as f64;
                let m = (sum[hi] - sum[lo]) / count;
                let sd = ((squares[hi] - squares[lo]) / count - m * m).max(0.0).sqrt();
                // Floor the deviation at a fraction of the global one so near
                // silence is not blown up into beats.
                ((f64::from(onset[i]) - m) / sd.max(0.05 * f64::from(deviation))) as f32
            })
            .collect()
    };
    let period = 60.0 * frame_rate / bpm;
    let tightness = 100.0f32;
    let mut score = vec![0f32; n];
    let mut back = vec![usize::MAX; n];
    let (lo, hi) = ((period / 2.0).round() as usize, (period * 2.0).round() as usize);
    for t in 0..n {
        let mut best = f32::NEG_INFINITY;
        let mut best_p = usize::MAX;
        if t >= lo {
            for p in t.saturating_sub(hi)..=(t - lo) {
                let ratio = (t - p) as f64 / period;
                let penalty = -tightness * (ratio.ln().powi(2) as f32);
                let candidate = score[p] + penalty;
                if candidate > best {
                    best = candidate;
                    best_p = p;
                }
            }
        }
        if !unbroken {
            score[t] = local[t] + if best_p == usize::MAX { 0.0 } else { best.max(0.0) };
            back[t] = if best_p != usize::MAX && best > 0.0 { best_p } else { usize::MAX };
        } else {
            score[t] = local[t] + if best_p == usize::MAX { 0.0 } else { best };
            back[t] = best_p;
        }
    }
    // End on the strongest beat within the last period.
    let tail = n.saturating_sub(period.ceil() as usize + 1);
    let mut t = (tail..n).max_by(|a, b| score[*a].partial_cmp(&score[*b]).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(n - 1);
    let mut beats = vec![t];
    while back[t] != usize::MAX {
        t = back[t];
        beats.push(t);
    }
    beats.reverse();
    beats
}

/// Sections, phrase grid and edge lengths (see the module docs).
struct Structure {
    boundaries: Vec<(f64, f32)>,
    intro_bars: u32,
    outro_bars: u32,
    phrase_phase: u32,
    loop_ending: f32,
}

/// Beat-synchronous feature vectors: unit chroma plus per-track z-scored
/// band levels (half weight) and onset density.
fn beat_vectors(f: &Features, beats: &[usize]) -> Vec<Vec<f32>> {
    let n = beats.len().saturating_sub(1);
    let mut raw: Vec<([f32; 12], [f32; 3], f32)> = Vec::with_capacity(n);
    for w in beats.windows(2) {
        let (a, b) = (w[0].min(f.bands.len()), w[1].min(f.bands.len()).max(w[0] + 1).min(f.bands.len()));
        let mut chroma = [0f32; 12];
        let mut bands = [0f32; 3];
        let mut onset = 0f32;
        for i in a..b {
            for k in 0..12 {
                chroma[k] += f.frame_chroma[i][k];
            }
            for k in 0..3 {
                bands[k] += f.bands[i][k];
            }
            onset += f.onset[i];
        }
        let norm = chroma.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
        chroma.iter_mut().for_each(|v| *v /= norm);
        let frames = (b - a).max(1) as f32;
        let levels = bands.map(|e| 10.0 * (e / frames + 1e-12).log10());
        raw.push((chroma, levels, onset / frames));
    }
    // z-score band levels and onset across the track.
    let stats = |get: &dyn Fn(&([f32; 12], [f32; 3], f32)) -> f32| {
        let mean = raw.iter().map(get).sum::<f32>() / raw.len().max(1) as f32;
        let sd = (raw.iter().map(|r| (get(r) - mean).powi(2)).sum::<f32>() / raw.len().max(1) as f32).sqrt().max(1e-3);
        (mean, sd)
    };
    let band_stats: Vec<(f32, f32)> = (0..3).map(|k| stats(&|r| r.1[k])).collect();
    let onset_stats = stats(&|r| r.2);
    raw.iter()
        .map(|(chroma, levels, onset)| {
            let mut v: Vec<f32> = chroma.to_vec();
            for k in 0..3 {
                v.push(0.5 * (levels[k] - band_stats[k].0) / band_stats[k].1);
            }
            v.push(0.5 * (onset - onset_stats.0) / onset_stats.1);
            v
        })
        .collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut ea, mut eb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        ea += x * x;
        eb += y * y;
    }
    dot / (ea * eb).sqrt().max(1e-9)
}

/// Foote novelty over beats with a Gaussian-tapered checkerboard kernel of
/// `2 × half` beats, normalised to a maximum of 1.
fn foote_novelty(vectors: &[Vec<f32>], half: usize) -> Vec<f32> {
    let n = vectors.len();
    let mut novelty = vec![0f32; n];
    if n < 2 * half + 1 {
        return novelty;
    }
    let sigma = half as f32 / 2.0;
    let kernel: Vec<Vec<f32>> = (0..2 * half)
        .map(|a| {
            (0..2 * half)
                .map(|b| {
                    let (x, y) = (a as f32 - half as f32 + 0.5, b as f32 - half as f32 + 0.5);
                    let sign = if (x < 0.0) == (y < 0.0) { 1.0 } else { -1.0 };
                    sign * (-(x * x + y * y) / (2.0 * sigma * sigma)).exp()
                })
                .collect()
        })
        .collect();
    for (i, slot) in novelty.iter_mut().enumerate().take(n - half).skip(half) {
        let mut total = 0f32;
        for a in 0..2 * half {
            for b in a..2 * half {
                let k = kernel[a][b];
                let sim = cosine(&vectors[i + a - half], &vectors[i + b - half]);
                total += if a == b { k * sim } else { 2.0 * k * sim };
            }
        }
        *slot = total.max(0.0);
    }
    let top = novelty.iter().copied().fold(0f32, f32::max);
    if top > 0.0 {
        novelty.iter_mut().for_each(|v| *v /= top);
    }
    novelty
}

fn structure(f: &Features, beats: &[usize], beat_times: &[f64], phase: usize, bar_energy: &[f32], start: f64, end: f64) -> Structure {
    let empty = Structure { boundaries: Vec::new(), intro_bars: 0, outro_bars: 0, phrase_phase: 0, loop_ending: 0.0 };
    if beats.len() < 64 {
        return empty;
    }
    let vectors = beat_vectors(f, beats);
    // 8-bar kernel: 4 bars (16 beats) either side.
    let novelty = foote_novelty(&vectors, 16);
    // Bars are downbeats `phase + 4k`. Novelty per bar, then the 4-bar phrase
    // phase whose bars carry the most novelty.
    let bars = (beats.len().saturating_sub(phase)).div_ceil(4);
    let bar_novelty: Vec<f32> = (0..bars).map(|b| novelty.get(phase + 4 * b).copied().unwrap_or(0.0)).collect();
    let phrase_phase = (0..4).max_by(|&x, &y| {
        let sum = |q: usize| bar_novelty.iter().skip(q).step_by(4).map(|v| v * v).sum::<f32>().sqrt();
        sum(x).total_cmp(&sum(y))
    }).unwrap_or(0);
    // Peaks: local maxima within ±2 bars (a 4-bar window), above 0.3 × max.
    let mut peaks: Vec<usize> = Vec::new();
    for i in 0..novelty.len() {
        let (lo, hi) = (i.saturating_sub(8), (i + 9).min(novelty.len()));
        if novelty[i] >= 0.3 && novelty[lo..hi].iter().all(|v| *v <= novelty[i]) {
            peaks.push(i);
        }
    }
    // Snap each peak to a downbeat: on the phrase grid when within a bar.
    let mut snapped: Vec<(usize, f32)> = Vec::new();
    for &peak in &peaks {
        let bar = ((peak as f64 - phase as f64) / 4.0).round().max(0.0) as usize;
        let grid = (bar as i64 - phrase_phase as i64).rem_euclid(4) as usize;
        let on_grid = if grid <= 1 { bar - grid.min(bar) } else { bar + (4 - grid) };
        let chosen = if on_grid.abs_diff(bar) <= 1 { on_grid } else { bar };
        if chosen < bars && snapped.last().is_none_or(|(b, _)| *b != chosen) {
            snapped.push((chosen, novelty[peak]));
        }
    }
    let bar_time = |b: usize| beat_times.get(phase + 4 * b).copied();
    // Salience: the 4 bars after a boundary reach 0.4 × the loudest 4 bars.
    let four = |b: usize| (b + 4 <= bar_energy.len()).then(|| power_mean_db(&bar_energy[b..b + 4]));
    let loudest = (0..bar_energy.len().saturating_sub(3)).filter_map(four).fold(-120.0, f64::max);
    let salient = |b: usize| four(b).is_some_and(|e| e >= loudest + 10.0 * 0.4f64.log10());
    let first_bar = (0..bars).find(|&b| bar_time(b).is_some_and(|t| t >= start - 0.05)).unwrap_or(0);
    let last_bar = (0..bars).rev().find(|&b| bar_time(b).is_some_and(|t| t <= end)).unwrap_or(bars.saturating_sub(1));
    // Lengths by time from the audible edges (the first tracked downbeat can
    // sit a bar in).
    let bar_seconds = 4.0 * (beat_times[beat_times.len() - 1] - beat_times[0]) / (beat_times.len() - 1) as f64;
    let intro_bars = snapped
        .iter()
        .map(|(b, _)| *b)
        .find(|&b| b > first_bar && b <= first_bar + 32 && salient(b))
        .and_then(bar_time)
        .map_or(0, |t| ((t - start) / bar_seconds).round().max(0.0) as u32);
    let outro_bars = snapped
        .iter()
        .rev()
        .map(|(b, _)| *b)
        .find(|&b| b < last_bar && b + 32 >= last_bar)
        .and_then(bar_time)
        .map_or(0, |t| ((end - t) / bar_seconds).round().max(0.0) as u32);
    // Looping ending: the last four bars against the first four.
    let span = |from: usize, to: usize| {
        let mut mean = vec![0f32; vectors.first().map_or(0, Vec::len)];
        let count = to.saturating_sub(from).max(1) as f32;
        for v in &vectors[from.min(vectors.len())..to.min(vectors.len())] {
            for (m, x) in mean.iter_mut().zip(v) {
                *m += x / count;
            }
        }
        mean
    };
    let head = phase + 4 * first_bar;
    let tail = (phase + 4 * last_bar).saturating_sub(16);
    let loop_ending = if tail > head + 32 { cosine(&span(head, head + 16), &span(tail, tail + 16)).max(0.0) } else { 0.0 };
    Structure {
        boundaries: snapped.iter().filter_map(|&(b, n)| bar_time(b).map(|t| (t, n))).collect(),
        intro_bars,
        outro_bars,
        phrase_phase: phrase_phase as u32,
        loop_ending,
    }
}

/// Seconds of fade at one edge of the envelope (`reverse` = the ending):
/// from the edge back to the last point within 3 dB of `reference`, if the
/// level falls by ≥ 10 dB across it without a sudden step.
fn fade_length(envelope: &[f32], reference: f64, edge: f64, reverse: bool) -> f64 {
    let at = |t: f64| envelope.get((t * 2.0) as usize).map(|v| f64::from(*v));
    let step = if reverse { -0.5 } else { 0.5 };
    let mut t = edge;
    let mut lowest: f64 = 0.0;
    let mut steps = 0;
    let edge_level = at(if reverse { (edge - 0.5).max(0.0) } else { edge }).unwrap_or(-120.0);
    while let Some(level) = at(t) {
        if level >= reference - 3.0 {
            break;
        }
        lowest = lowest.min(level);
        t += step;
        steps += 1;
        if steps > 120 || t < 0.0 {
            return 0.0;
        }
    }
    let length = steps as f64 * 0.5;
    if length < 3.0 || reference - edge_level < 10.0 {
        return 0.0;
    }
    // A fade has a consistent slope, not just a low level (a quiet intro is
    // not a fade-in): the level must fit a line falling toward the edge by
    // ≥ 8 dB with r² ≥ 0.6.
    let (lo, hi) = if reverse { (edge - length, edge - 0.5) } else { (edge + 0.5, edge + length) };
    let points: Vec<(f64, f64)> = (0..((hi - lo) * 2.0) as usize).filter_map(|k| at(lo + k as f64 * 0.5).map(|v| (lo + k as f64 * 0.5, v.max(-90.0)))).collect();
    if points.len() < 4 {
        return 0.0;
    }
    let n = points.len() as f64;
    let (mx, my) = (points.iter().map(|p| p.0).sum::<f64>() / n, points.iter().map(|p| p.1).sum::<f64>() / n);
    let (sxy, sxx, syy) = points.iter().fold((0.0, 0.0, 0.0), |(a, b, c), p| (a + (p.0 - mx) * (p.1 - my), b + (p.0 - mx).powi(2), c + (p.1 - my).powi(2)));
    let slope = sxy / sxx.max(1e-9);
    let r2 = sxy * sxy / (sxx * syy).max(1e-9);
    let toward_edge = if reverse { -slope } else { slope };
    if toward_edge * (hi - lo) < 8.0 || r2 < 0.6 {
        return 0.0;
    }
    // Gradual: no single 0.5 s step drops by more than 6 dB inside the fade
    // (the very edge, where audio stops, excepted).
    let (from, to) = if reverse { (edge - length, edge - 1.0) } else { (edge + 1.0, edge + length) };
    let mut x = from;
    while x + 0.5 < to {
        if let (Some(a), Some(b)) = (at(x), at(x + 0.5))
            && (a - b).abs() > 6.0 {
                return 0.0;
            }
        x += 0.5;
    }
    length
}

/// Analyses mono PCM at `ANALYSIS_SAMPLE_RATE`.
pub fn analyze_track(samples: &[f32]) -> Option<MixProfile> {
    analyze_track_with(samples, AnalysisOptions::default())
}

pub fn analyze_track_with(samples: &[f32], options: AnalysisOptions) -> Option<MixProfile> {
    let rate = ANALYSIS_SAMPLE_RATE;
    let duration = samples.len() as f64 / rate;
    if duration < 8.0 {
        return None;
    }
    let f = features(samples);
    let frame_time = |i: usize| (i * ANALYSIS_HOP + ANALYSIS_FFT_SIZE / 2) as f64 / rate;
    let audible: Vec<usize> = (0..f.level_db.len()).filter(|&i| f64::from(f.level_db[i]) > SILENCE_DB).collect();
    let (&first, &last) = (audible.first()?, audible.last()?);
    let start = (frame_time(first) - ANALYSIS_FFT_SIZE as f64 / 2.0 / rate).max(0.0);
    let end = (frame_time(last) + ANALYSIS_FFT_SIZE as f64 / 2.0 / rate).min(duration);

    // Gated loudness: absolute −70 dB gate, then relative −10 dB gate.
    let gated: Vec<f64> = f.level_db.iter().map(|d| f64::from(*d)).filter(|d| *d > -70.0).collect();
    let power = |values: &[f64]| 10.0 * (values.iter().map(|d| 10f64.powf(d / 10.0)).sum::<f64>() / values.len().max(1) as f64).max(1e-12).log10();
    let first_pass = power(&gated);
    let relative: Vec<f64> = gated.into_iter().filter(|d| *d > first_pass - 10.0).collect();
    let loudness_db = power(&relative);

    // Tempo from the audible span; the tracker then follows the real beats.
    let span = &f.onset[first..=last];
    let tempo = estimate_tempo(span, f.frame_rate);
    let bpm = tempo.map_or(120.0, |t| t.bpm);
    // The tracker follows broadband flux plus extra weight on the low band:
    // kicks carry the beat, while broadband flux alone tends to lock onto
    // off-beat hats (then a global half-beat fix misplaces sections that
    // have no hats).
    let options = if options.legacy_tracker { AnalysisOptions::legacy() } else { options };
    let weighted: Vec<f32> = if options.low_weight == 0.0 {
        span.to_vec()
    } else {
        // Each envelope relative to its own mean, so the low band (a few
        // dozen bins) is not outvoted by broadband hat noise (hundreds).
        let low = &f.low_onset[first..=last];
        let mean_of = |v: &[f32]| (v.iter().sum::<f32>() / v.len().max(1) as f32).max(1e-9);
        let (mo, ml) = (mean_of(span), mean_of(low));
        span.iter().zip(low).map(|(o, l)| o / mo + options.low_weight * l / ml).collect()
    };
    let tracked = track_beats(&weighted, f.frame_rate, bpm, options.unbroken_chain, options.local_onsets);
    let mut beats_frames: Vec<usize> = tracked.iter().map(|b| b + first).collect();
    // Broadband flux can lock onto off-beat hats; the beat is where the bass
    // attacks land, so take the half-beat-shifted grid when it fits better.
    if beats_frames.len() >= 8 {
        let half = (30.0 * f.frame_rate / bpm).round() as usize;
        let bass_at = |frames: &mut dyn Iterator<Item = usize>| -> f64 {
            frames
                .map(|b| (b.saturating_sub(1)..(b + 2).min(f.low_onset.len())).map(|j| f64::from(f.low_onset[j])).fold(0.0, f64::max))
                .sum()
        };
        let on = bass_at(&mut beats_frames.iter().copied());
        let shifted = bass_at(&mut beats_frames.iter().map(|b| b + half).filter(|b| *b < f.low_onset.len()));
        if shifted > on * 1.25 {
            beats_frames = beats_frames.iter().map(|b| b + half).filter(|b| *b < f.onset.len()).collect();
        }
    }
    // Beat times: frame centres corrected by the onset latency.
    let latency = if options.legacy_tracker { 0.0 } else { ONSET_LATENCY };
    // Sub-frame timing (frames are 23 ms): where the tracked frame is itself
    // an onset peak, a parabola through it and its neighbours places the
    // peak between frames. (Never moved to another frame: that would trade
    // the beat for a nearby hat.)
    let refine = |b: usize| -> f64 {
        if options.legacy_tracker || b == 0 || b + 1 >= f.onset.len() {
            return frame_time(b);
        }
        let (l, c, r) = (f64::from(f.onset[b - 1]), f64::from(f.onset[b]), f64::from(f.onset[b + 1]));
        let denom = l - 2.0 * c + r;
        let delta = if c >= l && c >= r && denom < -1e-9 { (0.5 * (l - r) / denom).clamp(-0.5, 0.5) } else { 0.0 };
        frame_time(b) + delta * ANALYSIS_HOP as f64 / rate
    };
    let beats: Vec<f64> = beats_frames.iter().map(|&b| refine(b) + latency).collect();
    let (bpm, beat_confidence) = if beats.len() >= 16 {
        // Regularity on the frame grid the tracker chose (the sub-frame
        // refinement follows onset peaks and would add their jitter here).
        let mut gaps: Vec<f64> = beats_frames.windows(2).map(|w| frame_time(w[1]) - frame_time(w[0])).collect();
        gaps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = gaps[gaps.len() / 2];
        // Regularity: share of gaps within 6% of the median period.
        let regular = gaps.iter().filter(|g| (*g / median - 1.0).abs() < 0.06).count() as f64 / gaps.len() as f64;
        // Onset support: beats should sit on above-average onsets (the local
        // average over ±2 s, so quiet sections are judged on their own terms).
        let mean = span.iter().sum::<f32>() / span.len() as f32;
        let mut prefix = vec![0f64; f.onset.len() + 1];
        for (i, v) in f.onset.iter().enumerate() {
            prefix[i + 1] = prefix[i] + f64::from(*v);
        }
        let radius = (2.0 * f.frame_rate) as usize;
        let local_mean = |b: usize| {
            let (lo, hi) = (b.saturating_sub(radius), (b + radius + 1).min(f.onset.len()));
            ((prefix[hi] - prefix[lo]) / (hi - lo).max(1) as f64) as f32
        };
        let support = beats_frames.iter().filter(|&&b| f.onset[b] > if options.local_support { local_mean(b) } else { mean }).count() as f64 / beats_frames.len() as f64;
        // Period from a least-squares fit of beat time against beat index
        // over the whole song: sub-frame precise, unlike any single gap.
        let n = beats.len() as f64;
        let mean_i = (n - 1.0) / 2.0;
        let mean_t = beats.iter().sum::<f64>() / n;
        let (mut num, mut den) = (0.0, 0.0);
        for (i, t) in beats.iter().enumerate() {
            num += (i as f64 - mean_i) * (t - mean_t);
            den += (i as f64 - mean_i).powi(2);
        }
        let fitted = if den > 0.0 { num / den } else { median };
        let period = if (fitted / median - 1.0).abs() < 0.1 { fitted } else { median };
        (60.0 / period, (regular * support * tempo.map_or(0.5, |t| 0.5 + t.strength / 2.0)).clamp(0.0, 1.0))
    } else {
        (bpm, 0.0)
    };

    // Downbeat: the phase whose beats carry the most bass attack.
    let (mut phase, mut phase_scores) = (0usize, [0f64; 4]);
    for (i, &frame) in beats_frames.iter().enumerate() {
        let window = frame.saturating_sub(1)..(frame + 2).min(f.low_onset.len());
        phase_scores[i % 4] += window.map(|j| f64::from(f.low_onset[j])).fold(0.0, f64::max);
    }
    for k in 1..4 {
        if phase_scores[k] > phase_scores[phase] {
            phase = k;
        }
    }
    let total: f64 = phase_scores.iter().sum();
    let downbeat_confidence = if total > 0.0 { ((phase_scores[phase] / total) - 0.25) / 0.75 * 4.0 } else { 0.0 }.clamp(0.0, 1.0);

    let key = estimate_key(&f.chroma);
    let downbeat_frames: Vec<usize> = beats_frames.iter().skip(phase).step_by(4).copied().collect();
    let bar_energy: Vec<f32> = downbeat_frames
        .iter()
        .enumerate()
        .map(|(i, &from)| {
            let to = downbeat_frames.get(i + 1).copied().unwrap_or(f.level_db.len()).max(from + 1).min(f.level_db.len());
            let slice = &f.level_db[from.min(to - 1)..to];
            (10.0 * (slice.iter().map(|d| 10f64.powf(f64::from(*d) / 10.0)).sum::<f64>() / slice.len() as f64).max(1e-12).log10()) as f32
        })
        .collect();
    // 2 Hz envelope (power mean of the frames in each half second).
    let per_block = f.frame_rate / 2.0;
    let blocks = (f.level_db.len() as f64 / per_block).ceil() as usize;
    let envelope_db: Vec<f32> = (0..blocks)
        .map(|k| {
            let (a, b) = ((k as f64 * per_block) as usize, (((k + 1) as f64 * per_block) as usize).min(f.level_db.len()));
            power_mean_db(&f.level_db[a..b.max(a + 1).min(f.level_db.len())]) as f32
        })
        .collect();
    let fade_out = fade_length(&envelope_db, loudness_db, end, true);
    let fade_in = fade_length(&envelope_db, loudness_db, start, false);
    // Per-bar low share and tonal mid activity.
    let per_bar = |i: usize| {
        let from = downbeat_frames[i].min(f.bands.len());
        let to = downbeat_frames.get(i + 1).copied().unwrap_or(f.bands.len()).min(f.bands.len()).max(from + 1).min(f.bands.len());
        let (mut low, mut total, mut tonal) = (0f64, 0f64, 0f64);
        for j in from..to {
            let [l, m, h] = f.bands[j];
            let all = f64::from(l + m + h) + 1e-12;
            low += f64::from(l);
            total += all;
            tonal += f64::from(f.tonal[j]) * f64::from(m) / all;
        }
        ((low / total.max(1e-12)) as f32, (tonal / (to - from).max(1) as f64) as f32)
    };
    let (bar_low, bar_tonal): (Vec<f32>, Vec<f32>) = (0..downbeat_frames.len()).map(per_bar).unzip();
    let sections = structure(&f, &beats_frames, &beats, phase, &bar_energy, start, end);
    Some(MixProfile {
        version: PROFILE_VERSION,
        duration,
        bpm,
        beat_confidence,
        beats,
        downbeat_phase: phase,
        downbeat_confidence,
        camelot: key.map_or(8, |k| k.camelot),
        minor: key.is_some_and(|k| k.minor),
        key_confidence: key.map_or(0.0, |k| k.strength),
        loudness_db,
        start,
        end,
        bar_energy,
        envelope_db,
        fade_in,
        fade_out,
        bar_low,
        bar_tonal,
        boundaries: sections.boundaries,
        intro_bars: sections.intro_bars,
        outro_bars: sections.outro_bars,
        phrase_phase: sections.phrase_phase,
        loop_ending: sections.loop_ending,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Kicks on every beat (accent on the bar), hats between, a pad tone,
    /// with `lead` seconds of silence before and `tail` after.
    pub fn synth(bpm: f64, seconds: f64, lead: f64, tail: f64) -> Vec<f32> {
        let rate = ANALYSIS_SAMPLE_RATE;
        let total = ((lead + seconds + tail) * rate) as usize;
        let beat = 60.0 / bpm;
        (0..total)
            .map(|i| {
                let t = i as f64 / rate;
                if t < lead || t > lead + seconds {
                    return 0.0;
                }
                let local = t - lead;
                let in_beat = local % beat;
                let bar_accent = if ((local / beat).floor() as i64) % 4 == 0 { 1.0 } else { 0.55 };
                let kick = if in_beat < 0.12 { (2.0 * std::f64::consts::PI * (60.0 - in_beat * 200.0) * in_beat).sin() * (1.0 - in_beat / 0.12) * bar_accent } else { 0.0 };
                let off = (local + beat / 2.0) % beat;
                let hat = if off < 0.02 { ((i * 7919) % 200) as f64 / 200.0 - 0.5 } else { 0.0 } * 0.3;
                let pad = (2.0 * std::f64::consts::PI * 261.63 * t).sin() * 0.05 + (2.0 * std::f64::consts::PI * 329.63 * t).sin() * 0.04;
                (kick * 0.8 + hat + pad) as f32
            })
            .collect()
    }

    #[test]
    fn tracks_beats_downbeats_and_silence() {
        let profile = analyze_track(&synth(124.0, 40.0, 2.0, 3.0)).expect("profile");
        assert!((profile.bpm - 124.0).abs() < 1.5, "bpm {}", profile.bpm);
        assert!(profile.beat_confidence > 0.5, "confidence {}", profile.beat_confidence);
        assert!((profile.start - 2.0).abs() < 0.15, "start {}", profile.start);
        assert!((profile.end - 42.0).abs() < 0.2, "end {}", profile.end);
        // The first downbeat sits on an accented kick: a whole number of bars after 2 s.
        let first = profile.downbeats().next().unwrap();
        let bars = (first - 2.0) / (4.0 * 60.0 / 124.0);
        assert!((bars - bars.round()).abs() < 0.08, "downbeat {first} ({bars} bars)");
        assert!(profile.loudness_db < 0.0 && profile.loudness_db > -40.0);
        assert_eq!(profile.envelope_db.len(), ((profile.duration * 2.0).ceil()) as usize);
        assert_eq!(profile.fade_out, 0.0, "hard ending");
    }

    #[test]
    fn finds_sections_fades_and_edges() {
        use crate::mix_synth::{SynthOptions, synth_song, to_analysis_rate};
        // 16 bars of kick-only intro, then a full section, then a 20 s fade.
        let rate = 44_100;
        // The intro is kick and bass only, 20 dB down: quiet enough that the
        // v3 tracker's chain restarted after it.
        let full = synth_song(rate, SynthOptions { bpm: 124.0, seconds: 120.0, fade_out: 20.0, intro_bars: 16, intro_gain: 0.1, ..Default::default() });
        let intro_end = full.beats[64];
        let song = full.clone();
        let profile = analyze_track(&to_analysis_rate(&song)).expect("profile");
        // The beat chain runs through the quiet intro.
        assert!(profile.beats[0] < 1.0, "first beat {}", profile.beats[0]);
        // Known limitation, measured: without low-band weighting the chain can
        // sit off-phase in a kick-and-bass-only intro this quiet; the weight
        // fixed that here but cost consistency on the owner's library
        // (private/lab/results/TRACKER_VARIANTS.md), so it is off. The main
        // section is on the beat.
        let hit = |truth: &[f64], found: &[f64]| truth.iter().filter(|t| found.iter().any(|f| (f - *t).abs() < 0.07)).count() as f64 / truth.len() as f64;
        let main_truth: Vec<f64> = full.beats.iter().copied().filter(|t| *t > intro_end + 1.0 && *t < full.end - 20.0).collect();
        let main_recall = hit(&main_truth, &profile.beats);
        assert!(main_recall > 0.95, "main section beat recall {main_recall}");
        // A linear-amplitude 20 s fade is 3 dB down ~6 s in; the measured fade
        // starts there.
        assert!(profile.fade_out > 8.0 && profile.fade_out < 20.5, "fade-out {}", profile.fade_out);
        assert_eq!(profile.fade_in, 0.0);
        assert!(profile.boundaries.iter().any(|(t, _)| (t - intro_end).abs() < 0.6), "boundary at {intro_end}: {:?}", profile.boundaries);
        assert_eq!(profile.intro_bars, 16, "intro {:?}", profile.boundaries);
        assert!(profile.bar_low[2] > profile.bar_low[40], "the intro is all low end");
        assert!(profile.level_between(108.0, 118.0).unwrap() < profile.loudness_db - 3.0);
        assert!(profile.trend_between(105.0, 118.0).unwrap() < -0.5);
    }
}
