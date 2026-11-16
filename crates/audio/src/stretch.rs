//! Pitch-preserving time-stretch (WSOLA) for the realtime mixer. Everything
//! is preallocated: nothing here allocates once constructed, so it is safe
//! on the audio thread.
//!
//! WSOLA: output is built from Hann-windowed input segments overlap-added at
//! a fixed synthesis hop. Each segment's input position advances by
//! `hop × rate`, nudged within ±`SEARCH` frames to the offset whose waveform
//! best continues the previous segment, so beats keep their shape and pitch
//! is unchanged. At rate 1 the search is skipped and the windows tile the
//! input exactly (periodic Hann at 50% overlap sums to one), which is
//! sample-transparent.

pub const WINDOW: usize = 2048;
pub const HOP: usize = WINDOW / 2;
const SEARCH: usize = 384;
/// Correlation given up per (offset / SEARCH)² away from the nominal position.
const DRIFT_PENALTY: f32 = 2.0;
const CAPACITY: usize = WINDOW * 8 + 2 * SEARCH;

pub struct Stretcher {
    window: Vec<f32>,
    /// Input frames (stereo) held from absolute index `input_base`.
    input: Vec<[f32; 2]>,
    input_len: usize,
    input_base: u64,
    /// Absolute index of the next segment's nominal start.
    position: f64,
    /// Absolute start of the previously used segment.
    previous: Option<u64>,
    ola: Vec<[f32; 2]>,
    out: Vec<[f32; 2]>,
    out_read: usize,
    out_len: usize,
    /// Frames of zero padding added after the input ended.
    padded: bool,
    /// Output frames still to drop (the primed history).
    discard: usize,
    pub active: bool,
}

impl Stretcher {
    pub fn new() -> Self {
        let window = (0..WINDOW).map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / WINDOW as f64).cos()) as f32).collect();
        Self {
            window,
            input: vec![[0.0; 2]; CAPACITY],
            input_len: 0,
            input_base: 0,
            position: 0.0,
            previous: None,
            ola: vec![[0.0; 2]; WINDOW],
            out: vec![[0.0; 2]; HOP],
            out_read: 0,
            out_len: 0,
            padded: false,
            discard: 0,
            active: false,
        }
    }

    /// Starts stretching from absolute input frame `at` (frames already
    /// consumed by the deck).
    pub fn engage(&mut self, at: u64) {
        self.input_len = 0;
        self.input_base = at;
        self.position = at as f64;
        self.previous = None;
        self.ola.iter_mut().for_each(|f| *f = [0.0; 2]);
        self.out_read = 0;
        self.out_len = 0;
        self.padded = false;
        self.discard = 0;
        self.active = true;
    }

    /// Starts stretching at `at` mid-stream without a dip: `history` holds
    /// the frames just before `at` (oldest first, ideally [`HOP`] of them).
    /// They seed the overlap-add, and the output they produce is dropped, so
    /// at rate 1 the output continues the input sample for sample.
    pub fn engage_primed(&mut self, at: u64, history: &[[f32; 2]]) {
        let h = history.len().min(HOP).min(at as usize);
        self.engage(at - h as u64);
        for frame in &history[history.len() - h..] {
            self.push(*frame);
        }
        self.discard = h;
    }

    pub fn reset(&mut self) {
        self.active = false;
        self.input_len = 0;
        self.out_len = 0;
        self.out_read = 0;
    }

    /// Input frames the stretcher has used, as an absolute position.
    pub fn consumed(&self) -> u64 {
        self.position.max(0.0) as u64
    }

    pub fn pop(&mut self) -> Option<[f32; 2]> {
        while self.out_read < self.out_len {
            self.out_read += 1;
            if self.discard > 0 {
                self.discard -= 1;
                continue;
            }
            return Some(self.out[self.out_read - 1]);
        }
        None
    }

    /// Input frames pushed but not yet played (nominally).
    pub fn buffered(&self) -> u64 {
        self.input_end().saturating_sub(self.position.max(0.0) as u64)
    }

    fn input_end(&self) -> u64 {
        self.input_base + self.input_len as u64
    }

    /// Absolute input index needed before the next step can run.
    pub fn needed(&self, rate: f64) -> u64 {
        let nominal = self.position.round() as u64;
        let mut need = nominal + (if rate == 1.0 { 0 } else { SEARCH as u64 }) + WINDOW as u64;
        if let Some(previous) = self.previous {
            need = need.max(previous + HOP as u64 + WINDOW as u64);
        }
        need
    }

    pub fn wants_input(&self, rate: f64) -> bool {
        self.input_end() < self.needed(rate) && self.input_len < CAPACITY
    }

    pub fn push(&mut self, frame: [f32; 2]) {
        if self.input_len < CAPACITY {
            self.input[self.input_len] = frame;
            self.input_len += 1;
        }
    }

    /// After the source ended: pad with silence so the tail is flushed.
    /// Returns false once nothing more can be produced.
    pub fn pad_tail(&mut self, rate: f64) -> bool {
        if self.padded {
            return self.position < (self.input_end() as f64) - WINDOW as f64;
        }
        self.padded = true;
        while self.input_len < CAPACITY && self.input_end() < self.needed(rate) + WINDOW as u64 {
            self.push([0.0; 2]);
        }
        true
    }

    fn at(&self, absolute: u64) -> [f32; 2] {
        absolute.checked_sub(self.input_base).and_then(|i| self.input.get(i as usize).filter(|_| (i as usize) < self.input_len)).copied().unwrap_or([0.0; 2])
    }

    /// Produces `HOP` output frames. Call when `wants_input` is false.
    pub fn step(&mut self, rate: f64) {
        let nominal = self.position.round() as u64;
        let chosen = match self.previous {
            Some(previous) if rate == 1.0 => previous + HOP as u64,
            Some(previous) => {
                // Best continuation of the previous segment, by decimated
                // mono cross-correlation over the search range.
                let target = previous + HOP as u64;
                let low = nominal.saturating_sub(SEARCH as u64).max(self.input_base);
                let high = nominal + SEARCH as u64;
                // Normalised so loud and quiet candidates compare fairly, and
                // pulled toward the nominal position: without the pull the
                // search keeps choosing the natural continuation and the
                // output drifts to the edge of the search range (a steady
                // timing error of up to SEARCH frames at rates ≠ 1).
                let (mut best, mut best_score) = (nominal, f32::NEG_INFINITY);
                let mut target_energy = 0f32;
                let mut i = 0;
                while i < WINDOW {
                    let a = self.at(target + i as u64);
                    target_energy += (a[0] + a[1]) * (a[0] + a[1]);
                    i += 8;
                }
                let mut candidate = low;
                while candidate <= high {
                    let (mut dot, mut energy) = (0f32, 0f32);
                    let mut i = 0;
                    while i < WINDOW {
                        let a = self.at(target + i as u64);
                        let b = self.at(candidate + i as u64);
                        let (ma, mb) = (a[0] + a[1], b[0] + b[1]);
                        dot += ma * mb;
                        energy += mb * mb;
                        i += 8;
                    }
                    let corr = dot / (target_energy * energy).sqrt().max(1e-9);
                    let distance = (candidate as f32 - nominal as f32) / SEARCH as f32;
                    let score = corr - DRIFT_PENALTY * distance * distance;
                    if score > best_score {
                        best_score = score;
                        best = candidate;
                    }
                    candidate += 2;
                }
                best
            }
            None => nominal,
        };
        for i in 0..WINDOW {
            let frame = self.at(chosen + i as u64);
            let w = self.window[i];
            self.ola[i][0] += frame[0] * w;
            self.ola[i][1] += frame[1] * w;
        }
        self.out[..HOP].copy_from_slice(&self.ola[..HOP]);
        self.ola.copy_within(HOP.., 0);
        self.ola[WINDOW - HOP..].iter_mut().for_each(|f| *f = [0.0; 2]);
        self.out_read = 0;
        self.out_len = HOP;
        self.previous = Some(chosen);
        self.position = if rate == 1.0 { (chosen + HOP as u64) as f64 } else { self.position + HOP as f64 * rate };
        // Drop input that no future segment can reach.
        let keep_from = (self.position as u64).saturating_sub(SEARCH as u64).min(chosen + HOP as u64);
        if keep_from > self.input_base + WINDOW as u64 {
            let drop = (keep_from - self.input_base) as usize;
            let drop = drop.min(self.input_len);
            self.input.copy_within(drop..self.input_len, 0);
            self.input_len -= drop;
            self.input_base += drop as u64;
        }
    }
}

impl Default for Stretcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(stretcher: &mut Stretcher, input: &[f32], rate: f64, frames: usize) -> Vec<f32> {
        let mut out = Vec::new();
        let mut next = 0;
        while out.len() < frames {
            if let Some(f) = stretcher.pop() {
                out.push(f[0]);
                continue;
            }
            while stretcher.wants_input(rate) && next < input.len() {
                stretcher.push([input[next]; 2]);
                next += 1;
            }
            if stretcher.wants_input(rate) {
                break;
            }
            stretcher.step(rate);
        }
        out
    }

    #[test]
    fn rate_one_is_transparent() {
        let input: Vec<f32> = (0..20000).map(|i| (i as f32 * 0.05).sin()).collect();
        let mut s = Stretcher::new();
        s.engage(0);
        let out = run(&mut s, &input, 1.0, 12000);
        // After the first half window, output equals the input exactly.
        for i in HOP..out.len() {
            assert!((out[i] - input[i]).abs() < 1e-4, "frame {i}: {} vs {}", out[i], input[i]);
        }
    }

    #[test]
    fn stretching_keeps_pitch_and_changes_speed() {
        let rate = 44100.0;
        let freq = 440.0;
        let input: Vec<f32> = (0..(rate as usize * 4)).map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate).sin()).collect();
        let mut s = Stretcher::new();
        s.engage(0);
        let speed = 1.06;
        let out = run(&mut s, &input, speed, rate as usize * 2);
        // Pitch: zero crossings per second stay at 2 × 440.
        let body = &out[WINDOW..out.len() - WINDOW];
        let crossings = body.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count() as f32;
        let measured = crossings / (body.len() as f32 / rate);
        assert!((measured - freq).abs() < 6.0, "pitch {measured}");
        // Speed: input consumed ≈ output × rate.
        let consumed = s.consumed() as f64;
        assert!((consumed / out.len() as f64 - speed).abs() < 0.03, "speed {}", consumed / out.len() as f64);
    }

    #[test]
    fn primed_engage_is_seamless_mid_stream() {
        let input: Vec<f32> = (0..30_000).map(|i| (i as f32 * 0.031).sin() * 0.5).collect();
        let at = 10_000usize;
        let history: Vec<[f32; 2]> = input[at - HOP..at].iter().map(|v| [*v; 2]).collect();
        let mut s = Stretcher::new();
        s.engage_primed(at as u64, &history);
        let out = run(&mut s, &input[at..], 1.0, 8000);
        // run() feeds from index 0 of the slice; the primed history already
        // sits before it, so output frame i is input frame at + i.
        for i in 0..out.len() {
            assert!((out[i] - input[at + i]).abs() < 1e-4, "frame {i}: {} vs {}", out[i], input[at + i]);
        }
    }

    /// Envelope fluctuation of a stretched sustained chord: the std of 10 ms
    /// RMS over its mean. Plain overlap-add of unaligned segments makes this
    /// swing (phasiness); WSOLA's search keeps it small.
    fn chord_wobble(rate: f64) -> f64 {
        let fs = 44_100.0;
        let input: Vec<f32> = (0..(fs as usize * 4))
            .map(|i| {
                let t = i as f32 / fs as f32;
                [220.0f32, 277.2, 329.6, 110.0].iter().map(|f| (2.0 * std::f32::consts::PI * f * t).sin()).sum::<f32>() * 0.2
            })
            .collect();
        let mut s = Stretcher::new();
        s.engage(0);
        let out = run(&mut s, &input, rate, fs as usize * 3);
        let body = &out[WINDOW * 2..out.len() - WINDOW * 2];
        let rms: Vec<f64> = body.chunks(441).map(|c| (c.iter().map(|v| f64::from(v * v)).sum::<f64>() / c.len() as f64).sqrt()).collect();
        let mean = rms.iter().sum::<f64>() / rms.len() as f64;
        (rms.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / rms.len() as f64).sqrt() / mean
    }

    #[test]
    fn stretched_chords_stay_steady() {
        for rate in [0.85, 0.92, 1.08, 1.18] {
            let wobble = chord_wobble(rate);
            if std::env::var("BENCH_DEBUG").is_ok() {
                eprintln!("rate {rate}: wobble {wobble:.4}");
            }
            assert!(wobble < 0.2, "rate {rate}: envelope wobble {wobble:.3}");
        }
    }
}
