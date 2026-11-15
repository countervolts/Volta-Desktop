//! Port of `src/lib/audio-analysis.ts`: tempo (spectral-flux onset envelope +
//! prior-weighted autocorrelation), key (chroma vs Krumhansl–Schmuckler),
//! energy (RMS), and the measured pulse grid of a track's intro/outro.

use serde::{Deserialize, Serialize};

use crate::musical_key::{MAJOR_CAMELOT, MINOR_CAMELOT};

pub const ANALYSIS_SAMPLE_RATE: f64 = 11025.0;
pub const ANALYSIS_FFT_SIZE: usize = 1024;
pub const ANALYSIS_HOP: usize = 256;
const MIN_BPM: f64 = 60.0;
const MAX_BPM: f64 = 200.0;
const PREFERRED_BPM: f64 = 120.0;
const FOLD_LOW: f64 = 70.0;
const FOLD_HIGH: f64 = 180.0;
const CHROMA_MIN_HZ: f64 = 65.4;
const CHROMA_MAX_HZ: f64 = 1975.5;
const MAJOR_PROFILE: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR_PROFILE: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.6, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

/// `TransitionRegion`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionRegion {
    pub bpm: f64,
    pub beat_offset: f64,
    pub confidence: f64,
    pub energy: f64,
}

/// `TrackAnalysis` (stored in `volta-automix-analysis:v2`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackAnalysis {
    pub bpm: f64,
    pub camelot: u8,
    /// "A" (minor) or "B" (major).
    pub mode: String,
    pub energy: f64,
    pub confidence: f64,
    #[serde(default = "analysis_source")]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tempo_confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro: Option<TransitionRegion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outro: Option<TransitionRegion>,
}

fn analysis_source() -> String {
    "analysis".into()
}

/// In-place radix-2 FFT (`fftInPlace`).
pub fn fft_in_place(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * std::f64::consts::PI / len as f64;
        let (w_re, w_im) = (angle.cos() as f32, angle.sin() as f32);
        let half = len >> 1;
        for start in (0..n).step_by(len) {
            let (mut cur_re, mut cur_im) = (1.0f32, 0.0f32);
            for offset in 0..half {
                let a = start + offset;
                let b = a + half;
                let v_re = re[b] * cur_re - im[b] * cur_im;
                let v_im = re[b] * cur_im + im[b] * cur_re;
                re[b] = re[a] - v_re;
                im[b] = im[a] - v_im;
                re[a] += v_re;
                im[a] += v_im;
                let next = cur_re * w_re - cur_im * w_im;
                cur_im = cur_re * w_im + cur_im * w_re;
                cur_re = next;
            }
        }
        len <<= 1;
    }
}

struct OnsetAndChroma {
    onset: Vec<f32>,
    low_onset: Vec<f32>,
    chroma: [f64; 12],
    frame_rate: f64,
    spectral_energy: f64,
}

fn onset_and_chroma(samples: &[f32], sample_rate: f64) -> OnsetAndChroma {
    let (size, hop) = (ANALYSIS_FFT_SIZE, ANALYSIS_HOP);
    let window: Vec<f32> = (0..size).map(|i| (0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / (size - 1) as f64).cos())) as f32).collect();
    let bins = size >> 1;
    let frames = if samples.len() >= size { (samples.len() - size) / hop + 1 } else { 0 };
    let mut onset = vec![0f32; frames];
    let mut low_onset = vec![0f32; frames];
    let mut chroma = [0f64; 12];
    let mut previous = vec![0f32; bins];
    let (mut re, mut im) = (vec![0f32; size], vec![0f32; size]);
    let bin_to_pitch: Vec<i8> = (0..bins)
        .map(|bin| {
            let frequency = bin as f64 * sample_rate / size as f64;
            if bin == 0 || !(CHROMA_MIN_HZ..=CHROMA_MAX_HZ).contains(&frequency) {
                return -1;
            }
            let midi = 69.0 + 12.0 * (frequency / 440.0).log2();
            ((midi.round() as i64).rem_euclid(12)) as i8
        })
        .collect();
    let mut spectral_total = 0f64;
    for frame in 0..frames {
        let start = frame * hop;
        for i in 0..size {
            re[i] = samples.get(start + i).copied().unwrap_or(0.0) * window[i];
            im[i] = 0.0;
        }
        fft_in_place(&mut re, &mut im);
        let (mut flux, mut low_flux) = (0f32, 0f32);
        for bin in 1..bins {
            let magnitude = re[bin].hypot(im[bin]);
            let delta = magnitude - previous[bin];
            if delta > 0.0 {
                flux += delta;
                if (bin as f64 * sample_rate / size as f64) < 250.0 {
                    low_flux += delta;
                }
            }
            previous[bin] = magnitude;
            if bin_to_pitch[bin] >= 0 {
                chroma[bin_to_pitch[bin] as usize] += f64::from(magnitude);
            }
        }
        onset[frame] = flux / bins as f32;
        low_onset[frame] = low_flux / bins as f32;
        spectral_total += f64::from(flux);
    }
    OnsetAndChroma {
        onset,
        low_onset,
        chroma,
        frame_rate: sample_rate / hop as f64,
        spectral_energy: if frames > 0 { spectral_total / frames as f64 } else { 0.0 },
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoEstimate {
    pub bpm: f64,
    pub strength: f64,
}

/// `estimateTempo`.
pub fn estimate_tempo(onset: &[f32], frame_rate: f64) -> Option<TempoEstimate> {
    if onset.len() < 8 {
        return None;
    }
    let mean = onset.iter().map(|v| f64::from(*v)).sum::<f64>() / onset.len() as f64;
    if !mean.is_finite() || mean <= 0.0 {
        return None;
    }
    let mut envelope: Vec<f64> = onset.iter().map(|v| (f64::from(*v) - mean).max(0.0)).collect();
    let variance: f64 = envelope.iter().map(|v| v * v).sum();
    if variance <= 0.0 {
        return None;
    }
    let norm = variance.sqrt();
    envelope.iter_mut().for_each(|v| *v /= norm);
    let min_lag = ((60.0 * frame_rate / MAX_BPM).floor() as usize).max(2);
    let max_lag = (envelope.len() as i64 - 2).min((60.0 * frame_rate / MIN_BPM).ceil() as i64);
    if max_lag <= min_lag as i64 {
        return None;
    }
    let max_lag = max_lag as usize;
    let mut scores = vec![0f64; max_lag + 2];
    let (mut best, mut best_lag) = (f64::NEG_INFINITY, min_lag);
    for lag in min_lag..=max_lag {
        let limit = envelope.len() - lag;
        let sum = (0..limit).map(|i| envelope[i] * envelope[i + lag]).sum::<f64>() / limit.max(1) as f64;
        let bpm = 60.0 * frame_rate / lag as f64;
        let prior = (-0.5 * ((bpm / PREFERRED_BPM).log2() / 0.9).powi(2)).exp();
        let score = sum * prior;
        scores[lag] = score;
        if score > best {
            best = score;
            best_lag = lag;
        }
    }
    if best <= 0.0 {
        return None;
    }
    let mut refined = best_lag as f64;
    if best_lag > min_lag && best_lag < max_lag {
        let (left, right) = (scores[best_lag - 1], scores[best_lag + 1]);
        let denominator = left - 2.0 * best + right;
        if denominator.abs() > 1e-9 {
            let offset = 0.5 * (left - right) / denominator;
            if offset.abs() < 1.0 {
                refined = best_lag as f64 + offset;
            }
        }
    }
    let mut bpm = 60.0 * frame_rate / refined;
    while bpm < FOLD_LOW {
        bpm *= 2.0;
    }
    while bpm > FOLD_HIGH {
        bpm /= 2.0;
    }
    if !bpm.is_finite() || bpm <= 0.0 {
        return None;
    }
    let (mut product, mut left, mut right) = (0f64, 0f64, 0f64);
    for i in 0..envelope.len() - best_lag {
        product += envelope[i] * envelope[i + best_lag];
        left += envelope[i].powi(2);
        right += envelope[i + best_lag].powi(2);
    }
    let strength = (product / (left * right).sqrt().max(1e-12)).clamp(0.0, 1.0);
    Some(TempoEstimate { bpm: crate::js::js_round(bpm * 10.0) / 10.0, strength })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyEstimate {
    pub camelot: u8,
    pub minor: bool,
    pub strength: f64,
}

/// `estimateKey`.
pub fn estimate_key(chroma: &[f64; 12]) -> Option<KeyEstimate> {
    let total: f64 = chroma.iter().sum();
    if !total.is_finite() || total <= 0.0 {
        return None;
    }
    let correlation = |profile: &[f64; 12], rotation: usize| {
        let mean_chroma = chroma.iter().sum::<f64>() / 12.0;
        let mean_profile = profile.iter().sum::<f64>() / 12.0;
        let (mut numerator, mut left, mut right) = (0f64, 0f64, 0f64);
        for (i, value) in chroma.iter().enumerate() {
            let p = profile[(i + 12 - rotation) % 12] - mean_profile;
            let c = value - mean_chroma;
            numerator += c * p;
            left += c * c;
            right += p * p;
        }
        let denominator = (left * right).sqrt();
        if denominator > 1e-9 { numerator / denominator } else { 0.0 }
    };
    let (mut best, mut runner_up, mut best_rotation, mut best_minor) = (f64::NEG_INFINITY, f64::NEG_INFINITY, 0usize, false);
    for rotation in 0..12 {
        for (score, minor) in [(correlation(&MAJOR_PROFILE, rotation), false), (correlation(&MINOR_PROFILE, rotation), true)] {
            if score > best {
                runner_up = best;
                best = score;
                best_rotation = rotation;
                best_minor = minor;
            } else if score > runner_up {
                runner_up = score;
            }
        }
    }
    if !best.is_finite() {
        return None;
    }
    let margin = if runner_up.is_finite() { best - runner_up } else { best };
    Some(KeyEstimate {
        camelot: if best_minor { MINOR_CAMELOT[best_rotation] } else { MAJOR_CAMELOT[best_rotation] },
        minor: best_minor,
        strength: (margin * 4.0).clamp(0.0, 1.0),
    })
}

/// `estimateEnergy`: RMS over −30…0 dBFS.
pub fn estimate_energy(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let rms = (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt();
    if !rms.is_finite() || rms <= 0.0 {
        return 0.0;
    }
    ((20.0 * rms.log10() + 30.0) / 30.0).clamp(0.0, 1.0)
}

/// `resampleMono`: average channels, then box-filter down (or interpolate).
pub fn resample_mono(channels: &[Vec<f32>], input_rate: f64, output_rate: f64) -> Vec<f32> {
    let frames = channels.first().map_or(0, Vec::len);
    if frames == 0 {
        return Vec::new();
    }
    let count = channels.len() as f32;
    let mono: Vec<f32> = (0..frames).map(|i| channels.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() / count).collect();
    if !input_rate.is_finite() || input_rate <= 0.0 || input_rate == output_rate {
        return mono;
    }
    let ratio = input_rate / output_rate;
    let length = ((frames as f64 / ratio).floor() as usize).max(1);
    if ratio >= 2.0 {
        return (0..length)
            .map(|i| {
                let start = (i as f64 * ratio).floor() as usize;
                let end = frames.min((start + 1).max(((i + 1) as f64 * ratio).floor() as usize));
                mono[start..end].iter().sum::<f32>() / (end - start) as f32
            })
            .collect();
    }
    (0..length)
        .map(|i| {
            let position = i as f64 * ratio;
            let index = position.floor() as usize;
            let fraction = (position - index as f64) as f32;
            let a = mono[index.min(frames - 1)];
            let b = mono[(index + 1).min(frames - 1)];
            a + (b - a) * fraction
        })
        .collect()
}

/// `analyzeAudio`: `None` when too short or quiet to say anything useful.
pub fn analyze_audio(samples: &[f32], sample_rate: f64) -> Option<TrackAnalysis> {
    if samples.is_empty() || !sample_rate.is_finite() || sample_rate <= 0.0 {
        return None;
    }
    let features = onset_and_chroma(samples, sample_rate);
    let energy = estimate_energy(samples);
    if energy <= 0.02 || features.spectral_energy <= 1e-6 {
        return None;
    }
    let tempo = estimate_tempo(&features.onset, features.frame_rate);
    let key = estimate_key(&features.chroma);
    if tempo.is_none() && key.is_none() {
        return None;
    }
    let key_margin = key.map_or(0.0, |k| k.strength);
    Some(TrackAnalysis {
        bpm: tempo.map_or(PREFERRED_BPM, |t| t.bpm),
        camelot: key.map_or(8, |k| k.camelot),
        mode: if key.is_some_and(|k| k.minor) { "A".into() } else { "B".into() },
        energy,
        confidence: (0.35 + tempo.map_or(0.0, |t| t.strength) * 0.35 + key_margin * 0.3).clamp(0.0, 1.0),
        source: "analysis".into(),
        tempo_confidence: Some(tempo.map_or(0.0, |t| t.strength)),
        key_confidence: Some(key_margin),
        duration: None,
        intro: None,
        outro: None,
    })
}

/// `analyzeTransitionRegion`: a measured pulse grid, or `None` when weak.
pub fn analyze_transition_region(samples: &[f32], sample_rate: f64, offset: f64) -> Option<TransitionRegion> {
    if !sample_rate.is_finite() || sample_rate <= 0.0 || (samples.len() as f64) / sample_rate < 8.0 {
        return None;
    }
    let features = onset_and_chroma(samples, sample_rate);
    let mut best = TransitionRegion { bpm: 0.0, beat_offset: 0.0, confidence: 0.0, energy: estimate_energy(samples) };
    let seconds = samples.len() as f64 / sample_rate;
    for onset in [&features.onset, &features.low_onset] {
        let Some(tempo) = estimate_tempo(onset, features.frame_rate) else { continue };
        let peak = onset.iter().cloned().fold(0.0f32, f32::max);
        let mean = onset.iter().sum::<f32>() / onset.len() as f32;
        let mut peaks: Vec<(f64, f64)> = Vec::new();
        for i in 1..onset.len().saturating_sub(1) {
            if onset[i] < (peak * 0.15).max(mean * 1.5) || onset[i] < onset[i - 1] || onset[i] <= onset[i + 1] {
                continue;
            }
            peaks.push((((i * ANALYSIS_HOP) as f64 + ANALYSIS_FFT_SIZE as f64 / 2.0) / sample_rate, f64::from(onset[i])));
        }
        if peaks.len() < 8 {
            continue;
        }
        let total: f64 = peaks.iter().map(|p| p.1).sum();
        for seed in [tempo.bpm / 2.0, tempo.bpm, tempo.bpm * 2.0] {
            if !(65.0..=180.0).contains(&seed) {
                continue;
            }
            let mut bpm = seed * 0.98;
            while bpm <= seed * 1.02 {
                let period = 60.0 / bpm;
                for anchor in peaks.iter().take(12) {
                    let (mut matched, mut matched_weight) = (0f64, 0f64);
                    let mut beats = std::collections::HashSet::new();
                    for item in &peaks {
                        let beat = ((item.0 - anchor.0) / period).round();
                        let error = (item.0 - anchor.0 - beat * period).abs();
                        if error < 0.04 {
                            matched += item.1 * (1.0 - error / 0.08);
                            matched_weight += item.1;
                            beats.insert(beat as i64);
                        }
                    }
                    if matched_weight < total * 0.25 {
                        continue;
                    }
                    let coverage = (beats.len() as f64 / (seconds / period)).min(1.0);
                    let confidence = coverage * matched / matched_weight;
                    if confidence > best.confidence {
                        best = TransitionRegion { bpm, beat_offset: offset + anchor.0, confidence, energy: best.energy };
                    }
                }
                bpm += 0.1;
            }
        }
    }
    (best.confidence >= 0.7).then_some(best)
}

/// The worker: analyse a clip, then the 16 s intro and outro.
pub fn analyze_clip(
    clip: &[Vec<f32>],
    intro: &[Vec<f32>],
    outro: &[Vec<f32>],
    sample_rate: f64,
    outro_offset: f64,
    duration: f64,
) -> Option<TrackAnalysis> {
    let mono = resample_mono(clip, sample_rate, ANALYSIS_SAMPLE_RATE);
    let mut analysis = analyze_audio(&mono, ANALYSIS_SAMPLE_RATE)?;
    analysis.duration = Some(duration);
    analysis.intro = analyze_transition_region(&resample_mono(intro, sample_rate, ANALYSIS_SAMPLE_RATE), ANALYSIS_SAMPLE_RATE, 0.0);
    analysis.outro = analyze_transition_region(&resample_mono(outro, sample_rate, ANALYSIS_SAMPLE_RATE), ANALYSIS_SAMPLE_RATE, outro_offset);
    Some(analysis)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A click track: short decaying noise-ish bursts every beat over a tone.
    fn clicks(bpm: f64, seconds: f64, rate: f64) -> Vec<f32> {
        let n = (seconds * rate) as usize;
        let period = (60.0 / bpm * rate) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / rate;
                let since = i % period;
                let click = if since < 400 { ((since as f64 * 1.7).sin() * (1.0 - since as f64 / 400.0)) as f32 } else { 0.0 };
                click * 0.8 + (2.0 * std::f64::consts::PI * 440.0 * t).sin() as f32 * 0.1
            })
            .collect()
    }

    #[test]
    fn measures_tempo_of_a_click_track() {
        let samples = clicks(120.0, 30.0, ANALYSIS_SAMPLE_RATE);
        let analysis = analyze_audio(&samples, ANALYSIS_SAMPLE_RATE).expect("analysis");
        assert!((analysis.bpm - 120.0).abs() < 2.0, "bpm {}", analysis.bpm);
        let region = analyze_transition_region(&clicks(120.0, 16.0, ANALYSIS_SAMPLE_RATE), ANALYSIS_SAMPLE_RATE, 100.0).expect("grid");
        assert!((region.bpm - 120.0).abs() < 2.5, "region bpm {}", region.bpm);
        assert!(region.beat_offset >= 100.0);
    }

    #[test]
    fn silence_and_short_input_are_refused() {
        assert!(analyze_audio(&vec![0.0; 44100], ANALYSIS_SAMPLE_RATE).is_none());
        assert!(estimate_tempo(&[0.0; 4], 43.0).is_none());
        let resampled = resample_mono(&[vec![1.0; 48000]], 48000.0, ANALYSIS_SAMPLE_RATE);
        // `Math.floor(frames / ratio)` lands one short, as in JS.
        assert_eq!(resampled.len(), 11024);
    }

    #[test]
    fn finds_a_major_key_from_its_scale() {
        // C major triad-weighted chroma.
        let mut chroma = [0.1f64; 12];
        for (pc, w) in [(0, 6.0), (4, 4.0), (7, 5.0), (2, 2.0), (5, 2.5), (9, 2.0), (11, 1.5)] {
            chroma[pc] = w;
        }
        let key = estimate_key(&chroma).unwrap();
        assert_eq!((key.camelot, key.minor), (8, false));
    }
}
