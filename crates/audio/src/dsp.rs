//! Realtime filters and the bar looper used by transitions. Nothing here
//! allocates after construction, so it is safe on the audio thread.
//!
//! Filters are Simper/Cytomic state-variable filters (TPT): their state stays
//! well behaved when the cutoff moves every few samples, which a direct-form
//! biquad does not guarantee during fast sweeps.

use volta_logic::mix_automation::{HIGHPASS_OPEN_HZ, LOW_MID_HZ, LOWPASS_OPEN_HZ, MID_HIGH_HZ};

/// One state-variable filter section (one channel).
#[derive(Clone, Copy, Default)]
pub struct Svf {
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
    ic1: f32,
    ic2: f32,
}

/// The SVF's simultaneous outputs.
#[derive(Clone, Copy)]
pub struct SvfOut {
    pub low: f32,
    pub band: f32,
    pub high: f32,
}

impl Svf {
    pub fn new(cutoff: f32, q: f32, sample_rate: f32) -> Self {
        let mut s = Self::default();
        s.set(cutoff, q, sample_rate);
        s
    }

    /// Changes the cutoff without touching the state.
    pub fn set(&mut self, cutoff: f32, q: f32, sample_rate: f32) {
        let cutoff = cutoff.clamp(10.0, sample_rate * 0.49);
        let g = (std::f32::consts::PI * cutoff / sample_rate).tan();
        self.k = 1.0 / q.max(0.05);
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    pub fn tick(&mut self, x: f32) -> SvfOut {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        SvfOut { low: v2, band: v1, high: x - self.k * v1 - v2 }
    }

    /// Second-order allpass (same pole pair as the low/high outputs).
    pub fn allpass(&mut self, x: f32) -> f32 {
        let k = self.k;
        let out = self.tick(x);
        x - 2.0 * k * out.band
    }

    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }
}

const BUTTERWORTH_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// A Linkwitz–Riley 4th-order split: two cascaded Butterworth sections per
/// side. Low + high is a 2nd-order allpass, so the bands sum flat.
#[derive(Clone, Copy, Default)]
struct Lr4 {
    lp: [Svf; 2],
    hp: [Svf; 2],
}

impl Lr4 {
    fn new(cutoff: f32, rate: f32) -> Self {
        let s = Svf::new(cutoff, BUTTERWORTH_Q, rate);
        Self { lp: [s; 2], hp: [s; 2] }
    }

    fn split(&mut self, x: f32) -> (f32, f32) {
        let low = self.lp[0].tick(x).low;
        let low = self.lp[1].tick(low).low;
        let high = self.hp[0].tick(x).high;
        let high = self.hp[1].tick(high).high;
        (low, high)
    }
}

/// Three bands (low < 200 Hz < mid < 2.5 kHz < high) for one channel. The
/// low band runs through the 2.5 kHz allpass so all three sum to an allpass
/// of the input: unity band gains change only phase, not level.
#[derive(Clone, Copy, Default)]
struct Crossover3 {
    first: Lr4,
    second: Lr4,
    align: Svf,
}

impl Crossover3 {
    fn new(rate: f32) -> Self {
        Self { first: Lr4::new(LOW_MID_HZ, rate), second: Lr4::new(MID_HIGH_HZ, rate), align: Svf::new(MID_HIGH_HZ, BUTTERWORTH_Q, rate) }
    }

    fn process(&mut self, x: f32, gains: [f32; 3]) -> f32 {
        let (low, rest) = self.first.split(x);
        let (mid, high) = self.second.split(rest);
        let low = self.align.allpass(low);
        low * gains[0] + mid * gains[1] + high * gains[2]
    }
}

/// Per-deck stereo effects: 3-band EQ and swept 24 dB/oct low-/high-pass.
/// `wet` fades between the dry signal and the processed one so engaging or
/// releasing the chain never clicks.
pub struct DeckFx {
    rate: f32,
    bands: [Crossover3; 2],
    lowpass: [[Svf; 2]; 2],
    highpass: [[Svf; 2]; 2],
    lp_hz: f32,
    hp_hz: f32,
    counter: u32,
    pub wet: f32,
    /// Where `wet` is heading (0 or 1).
    pub wet_target: f32,
    wet_step: f32,
    /// Frames the filters still run unheard before `wet` may rise, so their
    /// state has settled (a cold low band would add a thump).
    warmup: u32,
}

/// Coefficients of the swept filters are refreshed every this many frames.
const SWEEP_UPDATE: u32 = 16;
/// Seconds for `wet` to go from 0 to 1.
const WET_SECONDS: f32 = 0.02;
/// Seconds the filters run unheard before first fading in.
const WARMUP_SECONDS: f32 = 0.05;

impl DeckFx {
    pub fn new(rate: f32) -> Self {
        let lp = Svf::new(LOWPASS_OPEN_HZ.min(rate * 0.45), BUTTERWORTH_Q, rate);
        let hp = Svf::new(HIGHPASS_OPEN_HZ, BUTTERWORTH_Q, rate);
        Self {
            rate,
            bands: [Crossover3::new(rate); 2],
            lowpass: [[lp; 2]; 2],
            highpass: [[hp; 2]; 2],
            lp_hz: LOWPASS_OPEN_HZ,
            hp_hz: HIGHPASS_OPEN_HZ,
            counter: 0,
            wet: 0.0,
            wet_target: 0.0,
            wet_step: 1.0 / (WET_SECONDS * rate).max(1.0),
            warmup: (WARMUP_SECONDS * rate) as u32,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.rate);
    }

    /// Something to do (processing, or fading in or out of it).
    pub fn busy(&self) -> bool {
        self.wet > 0.0 || self.wet_target > 0.0
    }

    /// Runs the chain. `bands` are linear gains; cutoffs in Hz.
    pub fn process(&mut self, frame: [f32; 2], bands: [f32; 3], lp_hz: f32, hp_hz: f32) -> [f32; 2] {
        if self.warmup > 0 && self.wet_target > 0.0 {
            self.warmup -= 1;
        } else if self.wet != self.wet_target {
            let delta = self.wet_target - self.wet;
            self.wet = if delta.abs() <= self.wet_step { self.wet_target } else { self.wet + self.wet_step * delta.signum() };
        }
        if self.counter.is_multiple_of(SWEEP_UPDATE) && ((lp_hz - self.lp_hz).abs() > 0.5 || (hp_hz - self.hp_hz).abs() > 0.5) {
            self.update_cutoffs(lp_hz, hp_hz);
        }
        self.counter = self.counter.wrapping_add(1);
        let lp_open = self.lp_hz >= LOWPASS_OPEN_HZ;
        let hp_open = self.hp_hz <= HIGHPASS_OPEN_HZ;
        let mut out = frame;
        for c in 0..2 {
            let mut y = self.bands[c].process(frame[c], bands);
            if !lp_open {
                y = self.lowpass[c][0].tick(y).low;
                y = self.lowpass[c][1].tick(y).low;
            }
            if !hp_open {
                y = self.highpass[c][0].tick(y).high;
                y = self.highpass[c][1].tick(y).high;
            }
            out[c] = frame[c] + self.wet * (y - frame[c]);
        }
        out
    }

    fn update_cutoffs(&mut self, lp_hz: f32, hp_hz: f32) {
        let (rate, q) = (self.rate, BUTTERWORTH_Q);
        if lp_hz < LOWPASS_OPEN_HZ && self.lp_hz >= LOWPASS_OPEN_HZ {
            // Closing from open: start from a clean state.
            self.lowpass.iter_mut().flatten().for_each(Svf::reset);
        }
        if hp_hz > HIGHPASS_OPEN_HZ && self.hp_hz <= HIGHPASS_OPEN_HZ {
            self.highpass.iter_mut().flatten().for_each(Svf::reset);
        }
        self.lp_hz = lp_hz;
        self.hp_hz = hp_hz;
        for section in self.lowpass.iter_mut().flatten() {
            section.set(lp_hz.min(rate * 0.45), q, rate);
        }
        for section in self.highpass.iter_mut().flatten() {
            section.set(hp_hz, q, rate);
        }
    }
}

/// Seam crossfade at loop boundaries (seconds).
pub const LOOP_SEAM_SECONDS: f64 = 0.01;
/// Longest region (plus seam) a looper can hold.
pub const LOOP_MAX_SECONDS: f64 = 12.0;

/// Sample-accurate repeat of a region of a deck's source frames.
///
/// The region `[start, start + len)` is captured (with `seam` frames of
/// pre-roll) as it plays the first time. At its end the deck jumps back to
/// `start`, `repeats` times; each jump is an equal-power crossfade over the
/// last `seam` frames into the pre-roll, so it lands exactly on `start`.
/// After the last pass the deck continues from the ring at `start + len`,
/// which is the region's natural continuation.
pub struct Looper {
    buf: Vec<[f32; 2]>,
    pub armed: bool,
    start: u64,
    len: u64,
    seam: u64,
    pub repeats_left: u8,
    replaying: bool,
    j: u64,
}

impl Looper {
    pub fn new(rate: f32) -> Self {
        let frames = (f64::from(rate) * (LOOP_MAX_SECONDS + LOOP_SEAM_SECONDS)) as usize + 4;
        Self { buf: vec![[0.0; 2]; frames], armed: false, start: 0, len: 0, seam: 0, repeats_left: 0, replaying: false, j: 0 }
    }

    /// Arms a loop in deck frames. Refused (returns false) if it does not fit.
    pub fn arm(&mut self, start: u64, len: u64, seam: u64, repeats: u8) -> bool {
        self.disarm();
        let seam = seam.min(len / 4).max(1);
        if repeats == 0 || len < 4 * seam || (len + seam) as usize > self.buf.len() {
            return false;
        }
        // Pre-roll that will never be captured (region at the track start) stays silent.
        self.buf[..seam as usize].iter_mut().for_each(|f| *f = [0.0; 2]);
        self.start = start;
        self.len = len;
        self.seam = seam;
        self.repeats_left = repeats;
        self.armed = true;
        true
    }

    pub fn disarm(&mut self) {
        self.armed = false;
        self.replaying = false;
        self.repeats_left = 0;
    }

    /// No more jumps: the current pass plays out, then the ring continues.
    pub fn release(&mut self) {
        if self.armed {
            if self.replaying {
                self.repeats_left = self.repeats_left.min(1);
            } else {
                self.armed = false;
            }
        }
    }

    pub fn replaying(&self) -> bool {
        self.replaying
    }

    fn seam_mix(a: [f32; 2], b: [f32; 2], k: u64, seam: u64) -> [f32; 2] {
        let w = (k as f32 + 0.5) / seam as f32 * std::f32::consts::FRAC_PI_2;
        let (fo, fi) = (w.cos(), w.sin());
        [a[0] * fo + b[0] * fi, a[1] * fo + b[1] * fi]
    }

    /// A replayed frame; `position` is set to the frame's deck position + 1.
    pub fn replay(&mut self, position: &mut u64) -> [f32; 2] {
        let (seam, len, j) = (self.seam, self.len, self.j);
        let mut frame = self.buf[(seam + j) as usize];
        if self.repeats_left > 1 && j >= len - seam {
            let k = j - (len - seam);
            frame = Self::seam_mix(frame, self.buf[k as usize], k, seam);
        }
        self.j += 1;
        *position = self.start + self.j;
        if self.j >= len {
            self.repeats_left = self.repeats_left.saturating_sub(1);
            if self.repeats_left > 0 {
                self.j = 0;
                *position = self.start;
            } else {
                self.replaying = false;
                self.armed = false;
                *position = self.start + len;
            }
        }
        frame
    }

    /// Sees a ring frame at deck position `position` (before it is counted):
    /// captures it and, on the first pass's last `seam` frames, crossfades it
    /// into the pre-roll. Returns the frame to play.
    pub fn through(&mut self, position: u64, frame: [f32; 2]) -> [f32; 2] {
        if !self.armed {
            return frame;
        }
        let (start, len, seam) = (self.start, self.len, self.seam);
        // Buffer index of this frame: pre-roll occupies [0, seam).
        if position + seam >= start && position < start + len {
            self.buf[(position + seam - start) as usize] = frame;
        }
        if self.repeats_left > 0 && position + seam >= start + len && position < start + len {
            let k = position + seam - (start + len);
            return Self::seam_mix(frame, self.buf[k as usize], k, seam);
        }
        frame
    }

    /// Called after a ring frame was counted (deck now at `position`):
    /// starts the replay at the region's end.
    pub fn after(&mut self, position: u64) {
        if self.armed && self.repeats_left > 0 && !self.replaying && position == self.start + self.len {
            self.replaying = true;
            self.j = 0;
        }
    }
}

/// Smooths away a step in the output (a hard cut, a cancelled blend): the
/// jump is spread over a few milliseconds.
pub struct Declicker {
    last: [f32; 2],
    offset: [f32; 2],
    remaining: u32,
    length: u32,
    pending: bool,
}

impl Declicker {
    pub fn new(rate: f32) -> Self {
        Self { last: [0.0; 2], offset: [0.0; 2], remaining: 0, length: (0.005 * rate) as u32, pending: false }
    }

    /// The next frame may jump: smooth from the last one.
    pub fn arm(&mut self) {
        self.pending = true;
    }

    pub fn process(&mut self, frame: [f32; 2]) -> [f32; 2] {
        // Starting from silence is not a step worth smoothing.
        if self.pending && self.last != [0.0; 2] {
            self.pending = false;
            self.offset = [self.last[0] - frame[0], self.last[1] - frame[1]];
            self.remaining = self.length;
        }
        self.pending = false;
        let mut out = frame;
        if self.remaining > 0 {
            let w = self.remaining as f32 / self.length.max(1) as f32;
            out[0] += self.offset[0] * w;
            out[1] += self.offset[1] * w;
            self.remaining -= 1;
        }
        self.last = out;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, n: usize, rate: f32) -> Vec<f32> {
        (0..n).map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate).sin()).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn crossover_sums_flat_and_isolates_bands() {
        let rate = 48_000.0;
        for freq in [60.0, 200.0, 800.0, 2500.0, 8000.0] {
            let x = tone(freq, 48_000, rate);
            let mut flat = Crossover3::new(rate);
            let y: Vec<f32> = x.iter().map(|v| flat.process(*v, [1.0, 1.0, 1.0])).collect();
            let level = rms(&y[24_000..]) / rms(&x[24_000..]);
            assert!((level - 1.0).abs() < 0.01, "{freq} Hz: unity bands give {level}");
        }
        // Killing the low band removes a 50 Hz tone but keeps 5 kHz.
        let mut kill = Crossover3::new(rate);
        let bass: Vec<f32> = tone(50.0, 48_000, rate).iter().map(|v| kill.process(*v, [0.0, 1.0, 1.0])).collect();
        let mut kill = Crossover3::new(rate);
        let treble: Vec<f32> = tone(5000.0, 48_000, rate).iter().map(|v| kill.process(*v, [0.0, 1.0, 1.0])).collect();
        assert!(rms(&bass[24_000..]) < 0.05, "bass {}", rms(&bass[24_000..]));
        assert!(rms(&treble[24_000..]) > 0.69);
    }

    #[test]
    fn swept_lowpass_darkens_and_open_is_transparent() {
        let rate = 48_000.0;
        let x = tone(6000.0, 24_000, rate);
        let mut fx = DeckFx::new(rate);
        fx.wet_target = 1.0;
        let open: Vec<f32> = x.iter().map(|v| fx.process([*v, *v], [1.0; 3], LOWPASS_OPEN_HZ, HIGHPASS_OPEN_HZ)[0]).collect();
        assert!((rms(&open[12_000..]) / rms(&x[12_000..]) - 1.0).abs() < 0.01);
        let mut fx = DeckFx::new(rate);
        fx.wet_target = 1.0;
        let closed: Vec<f32> = x.iter().map(|v| fx.process([*v, *v], [1.0; 3], 500.0, HIGHPASS_OPEN_HZ)[0]).collect();
        assert!(rms(&closed[12_000..]) < 0.01, "{}", rms(&closed[12_000..]));
    }

    #[test]
    fn looper_repeats_exactly_and_continues() {
        let mut looper = Looper::new(1000.0);
        // Source frames are their own index; region [100, 140) twice more.
        assert!(looper.arm(100, 40, 4, 2));
        let mut position = 0u64;
        let mut out = Vec::new();
        let mut next_ring = 0u64;
        while out.len() < 260 {
            if looper.replaying() {
                out.push(looper.replay(&mut position)[0]);
            } else {
                let f = looper.through(position, [next_ring as f32; 2]);
                next_ring += 1;
                position += 1;
                looper.after(position);
                out.push(f[0]);
            }
        }
        // First pass plays 0..140 (seam into pre-roll on 136..140), then
        // 100..140 twice (the second seam only on the middle pass), then 140...
        assert_eq!(out[99], 99.0);
        assert_eq!(out[140], 100.0, "jumps back exactly");
        assert_eq!(out[180], 100.0);
        assert_eq!(out[219], 139.0, "last pass ends clean");
        assert_eq!(out[220], 140.0, "then the ring continues");
        // Seams blend the region's end with the pre-roll (96..100): equal power, so
        // frame 138 is cos·138 + sin·98, neither raw value.
        let w = 2.5f32 / 4.0 * std::f32::consts::FRAC_PI_2;
        assert!((out[138] - (138.0 * w.cos() + 98.0 * w.sin())).abs() < 1e-3);
    }

    #[test]
    fn declicker_spreads_a_step() {
        let mut d = Declicker::new(1000.0);
        d.process([1.0, 1.0]);
        d.arm();
        let first = d.process([0.0, 0.0]);
        assert!((first[0] - 1.0).abs() < 1e-6);
        let later: Vec<f32> = (0..5).map(|_| d.process([0.0, 0.0])[0]).collect();
        assert!(later.windows(2).all(|w| w[1] <= w[0]) && later[4] == 0.0);
    }
}
