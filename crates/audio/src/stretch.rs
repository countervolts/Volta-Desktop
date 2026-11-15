//! Pitch-preserving time-stretch (WSOLA) for the realtime mixer, plus the
//! low-band filter used for bass swaps. Everything is preallocated: nothing
//! here allocates once constructed, so it is safe on the audio thread.
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
        self.active = true;
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
        (self.out_read < self.out_len).then(|| {
            self.out_read += 1;
            self.out[self.out_read - 1]
        })
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
                let (mut best, mut best_score) = (nominal, f32::NEG_INFINITY);
                let mut candidate = low;
                while candidate <= high {
                    let mut score = 0f32;
                    let mut i = 0;
                    while i < WINDOW {
                        let a = self.at(target + i as u64);
                        let b = self.at(candidate + i as u64);
                        score += (a[0] + a[1]) * (b[0] + b[1]);
                        i += 8;
                    }
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

/// A 2nd-order high-pass (RBJ, Q 0.707) per channel; `cut` blends from dry
/// to filtered, removing the low band.
#[derive(Clone, Copy, Default)]
pub struct LowCut {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    state: [[f32; 4]; 2],
}

impl LowCut {
    pub fn new(frequency: f32, sample_rate: f32) -> Self {
        let w = 2.0 * std::f32::consts::PI * frequency / sample_rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 + cos) / 2.0 / a0,
            b1: -(1.0 + cos) / a0,
            b2: (1.0 + cos) / 2.0 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            state: [[0.0; 4]; 2],
        }
    }

    /// `x + cut × (highpass(x) − x)` per channel.
    pub fn process(&mut self, frame: [f32; 2], cut: f32) -> [f32; 2] {
        let mut out = frame;
        for (channel, x) in frame.iter().enumerate() {
            let s = &mut self.state[channel];
            let y = self.b0 * x + self.b1 * s[0] + self.b2 * s[1] - self.a1 * s[2] - self.a2 * s[3];
            s[1] = s[0];
            s[0] = *x;
            s[3] = s[2];
            s[2] = y;
            out[channel] = x + cut * (y - x);
        }
        out
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
    fn low_cut_removes_bass_but_keeps_treble() {
        let rate = 48000.0;
        let mut cut = LowCut::new(180.0, rate);
        let (mut bass, mut treble) = (0f32, 0f32);
        for i in 0..48000 {
            let t = i as f32 / rate;
            let low = (2.0 * std::f32::consts::PI * 50.0 * t).sin();
            let high = (2.0 * std::f32::consts::PI * 4000.0 * t).sin();
            if i > 4800 {
                bass += cut.process([low, low], 1.0)[0].abs();
            } else {
                cut.process([low, low], 1.0);
            }
            let _ = high;
        }
        let mut cut = LowCut::new(180.0, rate);
        for i in 0..48000 {
            let t = i as f32 / rate;
            let high = (2.0 * std::f32::consts::PI * 4000.0 * t).sin();
            let y = cut.process([high, high], 1.0)[0];
            if i > 4800 {
                treble += y.abs();
            }
        }
        assert!(bass / treble < 0.15, "bass {bass} treble {treble}");
    }
}
