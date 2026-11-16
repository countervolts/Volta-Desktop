//! Objective measurements of rendered or recorded transitions, shared by
//! the offline renderer tests (`volta-audio`) and the measurement lab.
//!
//! * loudness: ITU-R BS.1770 K-weighting, momentary (0.4 s) and short-term
//!   (3 s) windows, reported in LUFS;
//! * peaks and clipping;
//! * clicks: spikes of the second difference against its local level;
//! * onsets: energy-rise peaks, for beat alignment between per-deck stems;
//! * spectral flux, for spikes at a seam.
//!
//! Buffers are interleaved stereo `f32` unless a function says mono.

use crate::audio_analysis::fft_in_place;

/// A biquad in direct form I (f64 state for long filters at low cutoffs).
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Biquad {
    pub fn new(b: [f64; 3], a: [f64; 2]) -> Self {
        Self { b, a, x: [0.0; 2], y: [0.0; 2] }
    }

    pub fn process(&mut self, input: f64) -> f64 {
        let y = self.b[0] * input + self.b[1] * self.x[0] + self.b[2] * self.x[1] - self.a[0] * self.y[0] - self.a[1] * self.y[1];
        self.x = [input, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

/// The two K-weighting stages of BS.1770 for any sample rate (bilinear
/// transform of the analogue prototypes the standard's 48 kHz
/// coefficients come from).
fn k_weighting(rate: f64) -> [Biquad; 2] {
    // Stage 1: high-shelf, +4 dB above ~1.7 kHz.
    let (f0, gain_db, q) = (1681.974_450_955_533, 3.999_843_853_973_347, 0.707_175_236_955_419_6);
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let vh = 10f64.powf(gain_db / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad::new(
        [(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0],
        [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    );
    // Stage 2: RLB high-pass at ~38 Hz.
    let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let highpass = Biquad::new([1.0, -2.0, 1.0], [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0]);
    [shelf, highpass]
}

/// K-weighted mean-square power per block of `block` frames (summed over channels).
fn k_power_blocks(stereo: &[f32], rate: f64, block: usize) -> Vec<f64> {
    let mut filters = [k_weighting(rate), k_weighting(rate)];
    let frames = stereo.len() / 2;
    let mut out = Vec::with_capacity(frames / block.max(1) + 1);
    let mut acc = 0.0;
    let mut count = 0usize;
    for i in 0..frames {
        for (c, filter) in filters.iter_mut().enumerate() {
            let shelved = filter[0].process(f64::from(stereo[2 * i + c]));
            let y = filter[1].process(shelved);
            acc += y * y;
        }
        count += 1;
        if count == block {
            out.push(acc / block as f64);
            acc = 0.0;
            count = 0;
        }
    }
    out
}

fn lufs(power: f64) -> f64 {
    -0.691 + 10.0 * power.max(1e-12).log10()
}

/// Loudness over sliding windows of `window` seconds, every `hop` seconds:
/// `(time of the window centre, LUFS)`. 0.4 s = momentary, 3 s = short-term.
pub fn loudness_series(stereo: &[f32], rate: f64, window: f64, hop: f64) -> Vec<(f64, f64)> {
    let block = ((hop * rate).round() as usize).max(1);
    let blocks = k_power_blocks(stereo, rate, block);
    let span = ((window / hop).round() as usize).max(1);
    if blocks.len() < span {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(blocks.len() - span + 1);
    let mut sum: f64 = blocks[..span].iter().sum();
    for i in 0..=blocks.len() - span {
        if i > 0 {
            sum += blocks[i + span - 1] - blocks[i - 1];
        }
        let centre = (i as f64 + span as f64 / 2.0) * hop;
        out.push((centre, lufs(sum.max(0.0) / span as f64)));
    }
    out
}

/// Integrated loudness (BS.1770 gating: −70 LUFS absolute, −10 LU relative).
pub fn integrated_loudness(stereo: &[f32], rate: f64) -> f64 {
    let blocks = k_power_blocks(stereo, rate, ((0.1 * rate).round() as usize).max(1));
    // 400 ms blocks with 75% overlap.
    let gating: Vec<f64> = blocks.windows(4).map(|w| w.iter().sum::<f64>() / 4.0).collect();
    let absolute: Vec<f64> = gating.iter().copied().filter(|p| lufs(*p) > -70.0).collect();
    if absolute.is_empty() {
        return -70.0;
    }
    let mean = absolute.iter().sum::<f64>() / absolute.len() as f64;
    let threshold = lufs(mean) - 10.0;
    let relative: Vec<f64> = absolute.into_iter().filter(|p| lufs(*p) > threshold).collect();
    lufs(relative.iter().sum::<f64>() / relative.len().max(1) as f64)
}

/// Largest change of a loudness series between points `gap` seconds apart,
/// within `[from, to]` seconds: `(time, change in LU)`.
pub fn max_loudness_jump(series: &[(f64, f64)], gap: f64, from: f64, to: f64) -> (f64, f64) {
    let mut best: (f64, f64) = (from, 0.0);
    for (i, &(t, l)) in series.iter().enumerate() {
        if t < from || t > to {
            continue;
        }
        if let Some(&(_, later)) = series[i..].iter().find(|(u, _)| *u >= t + gap)
            && (later - l).abs() > best.1.abs() {
                best = (t, later - l);
            }
    }
    best
}

/// Largest absolute sample value.
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

/// Mono mix of interleaved stereo.
pub fn to_mono(stereo: &[f32]) -> Vec<f32> {
    stereo.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect()
}

/// Clicks: frames where the second difference jumps far above its local
/// level. Returns `(frame, ratio)` for every spike above `threshold`
/// (ratio = spike / local RMS of the second difference, local = ±`window` s).
pub fn detect_clicks(mono: &[f32], rate: f64, window: f64, threshold: f64) -> Vec<(usize, f64)> {
    if mono.len() < 3 {
        return Vec::new();
    }
    let d2: Vec<f64> = (0..mono.len()).map(|i| if i == 0 || i + 1 >= mono.len() { 0.0 } else { f64::from(mono[i + 1] - 2.0 * mono[i] + mono[i - 1]) }).collect();
    let half = ((window * rate) as usize).max(8);
    // Running sum of squares for the local RMS.
    let mut prefix = vec![0.0; d2.len() + 1];
    for (i, v) in d2.iter().enumerate() {
        prefix[i + 1] = prefix[i] + v * v;
    }
    let mut clicks: Vec<(usize, f64)> = Vec::new();
    for (i, v) in d2.iter().enumerate() {
        let (lo, hi) = (i.saturating_sub(half), (i + half + 1).min(d2.len()));
        let local = ((prefix[hi] - prefix[lo] - v * v) / (hi - lo - 1).max(1) as f64).sqrt();
        let ratio = v.abs() / local.max(1e-7);
        if ratio > threshold && v.abs() > 1e-4 {
            match clicks.last_mut() {
                Some(last) if i - last.0 < half => {
                    if ratio > last.1 {
                        *last = (i, ratio);
                    }
                }
                _ => clicks.push((i, ratio)),
            }
        }
    }
    clicks
}

/// Energy of the second difference (a high-frequency emphasis that a
/// discontinuity excites) over `[center − before, center + after)` frames.
pub fn d2_energy(mono: &[f32], center: usize, before: usize, after: usize) -> f64 {
    let (lo, hi) = (center.saturating_sub(before).max(1), (center + after).min(mono.len().saturating_sub(1)));
    (lo..hi).map(|i| f64::from(mono[i + 1] - 2.0 * mono[i] + mono[i - 1]).powi(2)).sum()
}

/// A seam against the same musical position `period` frames earlier (one
/// bar, say): the d2 energy ratio. ≈ 1 when the seam adds nothing; a click
/// makes it large.
pub fn seam_ratio(mono: &[f32], seam: usize, period: usize, before: usize, after: usize) -> f64 {
    d2_energy(mono, seam, before, after) / d2_energy(mono, seam.saturating_sub(period), before, after).max(1e-12)
}

/// Worst click ratio within `±radius` frames of `frame`.
pub fn click_ratio_near(mono: &[f32], rate: f64, frame: usize, radius: usize) -> f64 {
    let lo = frame.saturating_sub(radius + (0.02 * rate) as usize);
    let hi = (frame + radius + (0.02 * rate) as usize).min(mono.len());
    if hi <= lo + 4 {
        return 0.0;
    }
    detect_clicks(&mono[lo..hi], rate, 0.005, 0.0)
        .into_iter()
        .filter(|(i, _)| (lo + i).abs_diff(frame) <= radius)
        .map(|(_, r)| r)
        .fold(0.0, f64::max)
}

/// A mono signal through a 4th-order low-pass (cascaded one-poles, run
/// forwards and backwards so it adds no delay): the kick band, for beat
/// alignment.
pub fn lowpass_mono(mono: &[f32], rate: f64, cutoff: f64) -> Vec<f32> {
    let a = (-2.0 * std::f64::consts::PI * cutoff / rate).exp();
    let mut x: Vec<f64> = mono.iter().map(|v| f64::from(*v)).collect();
    for _ in 0..2 {
        let mut y = 0.0;
        for v in x.iter_mut() {
            y = (1.0 - a) * *v + a * y;
            *v = y;
        }
        let mut y = 0.0;
        for v in x.iter_mut().rev() {
            y = (1.0 - a) * *v + a * y;
            *v = y;
        }
    }
    x.into_iter().map(|v| v as f32).collect()
}

/// Onset times (seconds) of a mono signal: peaks of the positive change of a
/// 5 ms energy envelope, at least `min_gap` s apart, above `rel` × the
/// largest rise.
pub fn onsets(mono: &[f32], rate: f64, min_gap: f64, rel: f64) -> Vec<f64> {
    let hop = ((0.001 * rate) as usize).max(1);
    let win = ((0.005 * rate) as usize).max(hop);
    let env: Vec<f64> = (0..mono.len() / hop)
        .map(|i| {
            let s = i * hop;
            let e = (s + win).min(mono.len());
            (mono[s..e].iter().map(|v| f64::from(v * v)).sum::<f64>() / (e - s).max(1) as f64).sqrt()
        })
        .collect();
    let rise: Vec<f64> = (0..env.len()).map(|i| if i == 0 { 0.0 } else { (env[i] - env[i - 1]).max(0.0) }).collect();
    let top = rise.iter().copied().fold(0.0, f64::max);
    if top <= 0.0 {
        return Vec::new();
    }
    let gap = ((min_gap * rate) as usize / hop).max(1);
    let mut out: Vec<(usize, f64)> = Vec::new();
    for i in 1..rise.len().saturating_sub(1) {
        if rise[i] >= rel * top && rise[i] >= rise[i - 1] && rise[i] > rise[i + 1] {
            match out.last_mut() {
                Some(last) if i - last.0 < gap => {
                    if rise[i] > last.1 {
                        *last = (i, rise[i]);
                    }
                }
                _ => out.push((i, rise[i])),
            }
        }
    }
    // A rise at envelope frame `i` is the newest hop entering the window's leading edge.
    out.into_iter().map(|(i, _)| ((i * hop + win) as f64 - hop as f64 / 2.0) / rate).collect()
}

/// Beat alignment between two onset lists within `[from, to]` s: for each
/// onset of `b`, the distance to the nearest onset of `a`. Returns
/// `(median, 95th percentile, count)` in seconds.
pub fn alignment_error(a: &[f64], b: &[f64], from: f64, to: f64) -> (f64, f64, usize) {
    let mut errors: Vec<f64> = b
        .iter()
        .filter(|t| **t >= from && **t <= to)
        .filter_map(|t| a.iter().map(|u| (u - t).abs()).min_by(|x, y| x.total_cmp(y)))
        .collect();
    if errors.is_empty() {
        return (f64::NAN, f64::NAN, 0);
    }
    errors.sort_by(|x, y| x.total_cmp(y));
    let median = errors[errors.len() / 2];
    let p95 = errors[((errors.len() as f64 * 0.95) as usize).min(errors.len() - 1)];
    (median, p95, errors.len())
}

/// Best match of `reference[from..from + len]` inside `signal` at offsets
/// `guess ± search` (normalised cross-correlation): `(offset, correlation)`.
pub fn locate(reference: &[f32], from: usize, len: usize, signal: &[f32], guess: i64, search: usize) -> Option<(usize, f64)> {
    let snippet = reference.get(from..from + len)?;
    let energy: f64 = snippet.iter().map(|v| f64::from(v * v)).sum();
    if energy <= 1e-12 {
        return None;
    }
    let (lo, hi) = ((guess - search as i64).max(0), (guess + search as i64).min(signal.len() as i64 - len as i64));
    let mut best: Option<(usize, f64)> = None;
    for offset in lo..=hi {
        let window = &signal[offset as usize..offset as usize + len];
        let (mut dot, mut e) = (0f64, 0f64);
        for (a, b) in snippet.iter().zip(window) {
            dot += f64::from(a * b);
            e += f64::from(b * b);
        }
        let corr = dot / (energy * e).sqrt().max(1e-12);
        if best.is_none_or(|(_, c)| corr > c) {
            best = Some((offset as usize, corr));
        }
    }
    best
}

/// Where each of `beats` (source seconds) is heard in `stem` (mono, output
/// time), found by matching a short window around each beat of `source`.
/// `first_output` is a guess for the first beat and `scale` the initial
/// output seconds per source second (1 / playback rate); later guesses
/// follow the measured interval. Returns `(source beat, output s, corr)`.
pub fn follow_beats(stem: &[f32], source: &[f32], rate: f64, beats: &[f64], first_output: f64, scale: f64, search: f64) -> Vec<(f64, f64, f64)> {
    // 10 ms before to 40 ms after: long enough to be unambiguous on a kick's
    // low cycles, short enough that stretching barely shifts the match.
    let (pre, len) = ((0.01 * rate) as usize, (0.05 * rate) as usize);
    let search = (search * rate) as usize;
    let mut found: Vec<(f64, f64, f64)> = Vec::new();
    for (k, &beat) in beats.iter().enumerate() {
        let guess = match found.len() {
            0 => first_output,
            1 => found[0].1 + (beat - beats[k - 1]) * scale,
            n => found[n - 1].1 + (found[n - 1].1 - found[n - 2].1) * (beat - beats[k - 1]) / (found[n - 1].0 - found[n - 2].0).max(1e-9),
        };
        let from = ((beat * rate) as usize).saturating_sub(pre);
        let Some((offset, corr)) = locate(source, from, len, stem, (guess * rate) as i64 - pre as i64, search) else { continue };
        found.push((beat, (offset + pre) as f64 / rate, corr));
    }
    found
}

/// Spectral flux (positive log-magnitude change summed over bins) per hop of
/// a mono signal: `(time, flux)`.
pub fn spectral_flux(mono: &[f32], rate: f64, size: usize, hop: usize) -> Vec<(f64, f64)> {
    let window: Vec<f32> = (0..size).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / size as f32).cos()).collect();
    let mut previous = vec![0f32; size / 2];
    let (mut re, mut im) = (vec![0f32; size], vec![0f32; size]);
    let mut out = Vec::new();
    let mut start = 0;
    while start + size <= mono.len() {
        for i in 0..size {
            re[i] = mono[start + i] * window[i];
            im[i] = 0.0;
        }
        fft_in_place(&mut re, &mut im);
        let mut flux = 0f64;
        for bin in 1..size / 2 {
            let m = (1.0 + 1000.0 * re[bin].hypot(im[bin])).ln();
            if start > 0 {
                flux += f64::from((m - previous[bin]).max(0.0));
            }
            previous[bin] = m;
        }
        out.push(((start + size / 2) as f64 / rate, flux));
        start += hop;
    }
    out
}

/// Largest flux in `[from, to]` over the median flux of the whole series.
pub fn flux_spike_ratio(flux: &[(f64, f64)], from: f64, to: f64) -> f64 {
    let mut all: Vec<f64> = flux.iter().map(|f| f.1).collect();
    if all.is_empty() {
        return 0.0;
    }
    all.sort_by(|a, b| a.total_cmp(b));
    let median = all[all.len() / 2].max(1e-9);
    flux.iter().filter(|(t, _)| *t >= from && *t <= to).map(|f| f.1).fold(0.0, f64::max) / median
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, amp: f32, seconds: f64, rate: f64) -> Vec<f32> {
        (0..(seconds * rate) as usize)
            .flat_map(|i| {
                let v = amp * (2.0 * std::f64::consts::PI * freq * i as f64 / rate).sin() as f32;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn k_weighted_loudness_matches_the_standard_reference() {
        // BS.1770: a 1 kHz sine at −18 dBFS on both channels of a stereo pair
        // reads about −18 + 3 − 0.691 + 0.69 ≈ −15 LUFS... computed exactly:
        // each channel's K-weighted power ≈ the sine power (shelf ≈ +0.7 dB at 1 kHz).
        let rate = 48_000.0;
        let amp = 10f32.powf(-18.0 / 20.0);
        let x = sine(997.0, amp, 6.0, rate);
        let l = integrated_loudness(&x, rate);
        // Sine power = amp²/2 per channel; two channels sum.
        let expected = -0.691 + 10.0 * (2.0 * f64::from(amp * amp) / 2.0).log10();
        assert!((l - expected).abs() < 0.8, "{l} vs ~{expected}");
        // Halving the amplitude is −6 LU, at another sample rate too.
        let y = sine(997.0, amp / 2.0, 6.0, 44_100.0);
        assert!((integrated_loudness(&y, 44_100.0) - (l - 6.02)).abs() < 0.2);
        let series = loudness_series(&x, rate, 3.0, 0.1);
        assert!(series.iter().all(|(_, v)| (v - l).abs() < 0.3));
        assert!(max_loudness_jump(&series, 1.0, 0.0, 6.0).1.abs() < 0.1);
    }

    #[test]
    fn finds_clicks_and_onsets() {
        let rate = 44_100.0;
        let mut mono: Vec<f32> = (0..44_100).map(|i| 0.3 * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / 44_100.0).sin()).collect();
        // A step discontinuity at 0.5 s.
        for v in mono.iter_mut().skip(22_050) {
            *v += 0.2;
        }
        let clicks = detect_clicks(&mono, rate, 0.005, 8.0);
        assert!(clicks.iter().any(|(i, _)| i.abs_diff(22_050) < 3), "{clicks:?}");
        assert!(click_ratio_near(&mono, rate, 22_050, 4) > 8.0);
        assert!(click_ratio_near(&mono, rate, 10_000, 4) < 4.0);
        // Onsets of clicks every 0.5 s.
        let mut pulses = vec![0f32; 88_200];
        for k in 0..4 {
            for j in 0..200 {
                pulses[k * 22_050 + 1000 + j] = 0.8 * (1.0 - j as f32 / 200.0);
            }
        }
        let found = onsets(&pulses, rate, 0.1, 0.3);
        assert_eq!(found.len(), 4, "{found:?}");
        let truth: Vec<f64> = (0..4).map(|k| (k * 22_050 + 1000) as f64 / rate).collect();
        let (median, p95, n) = alignment_error(&truth, &found, 0.0, 2.0);
        assert!(n == 4 && median < 0.003 && p95 < 0.003, "{median} {p95}");
    }
}
