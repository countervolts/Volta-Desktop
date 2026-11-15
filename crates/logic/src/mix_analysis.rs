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
//! * gated loudness (dB) for level matching, and per-bar energy (dB).

use serde::{Deserialize, Serialize};

use crate::audio_analysis::{ANALYSIS_FFT_SIZE, ANALYSIS_HOP, ANALYSIS_SAMPLE_RATE, estimate_key, estimate_tempo, fft_in_place};

pub const PROFILE_VERSION: u32 = 3;
const SILENCE_DB: f64 = -48.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
}

impl MixProfile {
    /// Downbeat times (every 4th beat from the phase).
    pub fn downbeats(&self) -> impl Iterator<Item = f64> + '_ {
        self.beats.iter().skip(self.downbeat_phase).step_by(4).copied()
    }

    pub fn beat_period(&self) -> f64 {
        60.0 / self.bpm.max(1.0)
    }
}

struct Features {
    onset: Vec<f32>,
    low_onset: Vec<f32>,
    chroma: [f64; 12],
    /// Frame RMS in dBFS.
    level_db: Vec<f32>,
    frame_rate: f64,
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
        for bin in 1..bins {
            // Log magnitude makes the flux follow rhythm, not loudness.
            let magnitude = (1.0 + 100.0 * re[bin].hypot(im[bin])).ln();
            let delta = magnitude - previous[bin];
            if delta > 0.0 {
                flux += delta;
                if bin <= low_bins {
                    low += delta;
                }
            }
            previous[bin] = magnitude;
            if pitch[bin] >= 0 {
                chroma[pitch[bin] as usize] += f64::from(re[bin].hypot(im[bin]));
            }
        }
        onset[frame] = flux;
        low_onset[frame] = low;
    }
    Features { onset, low_onset, chroma, level_db, frame_rate: rate / hop as f64 }
}

/// Ellis' dynamic-programming beat tracker on a normalised onset envelope.
fn track_beats(onset: &[f32], frame_rate: f64, bpm: f64) -> Vec<usize> {
    let n = onset.len();
    if n < 16 {
        return Vec::new();
    }
    let mean = onset.iter().sum::<f32>() / n as f32;
    let deviation = (onset.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n as f32).sqrt().max(1e-6);
    let local: Vec<f32> = onset.iter().map(|v| (v - mean) / deviation).collect();
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
        score[t] = local[t] + if best_p == usize::MAX { 0.0 } else { best.max(0.0) };
        back[t] = if best_p != usize::MAX && best > 0.0 { best_p } else { usize::MAX };
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

/// Analyses mono PCM at `ANALYSIS_SAMPLE_RATE`.
pub fn analyze_track(samples: &[f32]) -> Option<MixProfile> {
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
    let tracked = track_beats(span, f.frame_rate, bpm);
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
    let beats: Vec<f64> = beats_frames.iter().map(|&b| frame_time(b)).collect();
    let (bpm, beat_confidence) = if beats.len() >= 16 {
        let mut gaps: Vec<f64> = beats.windows(2).map(|w| w[1] - w[0]).collect();
        gaps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = gaps[gaps.len() / 2];
        // Regularity: share of gaps within 6% of the median period.
        let regular = gaps.iter().filter(|g| (*g / median - 1.0).abs() < 0.06).count() as f64 / gaps.len() as f64;
        // Onset support: beats should sit on above-average onsets.
        let mean = span.iter().sum::<f32>() / span.len() as f32;
        let support = beats_frames.iter().filter(|&&b| f.onset[b] > mean).count() as f64 / beats_frames.len() as f64;
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
    }
}
