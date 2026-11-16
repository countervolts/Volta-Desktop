//! Transition automation: what each deck does over the course of a mix.
//!
//! The planner (`mix_planner`) produces a [`TransitionAutomation`]; the audio
//! engine evaluates it sample by sample, both live and in the offline
//! renderer. Every type here is `Copy` and fixed-size so the realtime
//! callback can receive it without allocating.
//!
//! Envelopes run over the transition's progress `p` in `[0, 1]` (output time
//! from the scheduled start to the end of the overlap). Before the first
//! point an envelope holds its first value; after the last, its last value.

use serde::{Deserialize, Serialize};

/// Most breakpoints an envelope can hold.
pub const MAX_POINTS: usize = 6;

/// Frequency at or above which the swept low-pass counts as open.
pub const LOWPASS_OPEN_HZ: f32 = 20_000.0;
/// Frequency at or below which the swept high-pass counts as open.
pub const HIGHPASS_OPEN_HZ: f32 = 20.0;
/// Crossover points of the 3-band EQ (Linkwitz–Riley 4th order).
pub const LOW_MID_HZ: f32 = 200.0;
pub const MID_HIGH_HZ: f32 = 2500.0;

/// Shape of one envelope segment, as a map from local time `t` in `[0, 1]`
/// to the fraction of the way from the segment's start value to its end.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", content = "k", rename_all = "camelCase")]
pub enum Shape {
    #[default]
    Linear,
    /// `sin(t·π/2)`: fast at first. A 0→1 fade-in with this shape and a
    /// 1→0 fade-out with [`Shape::SineOut`] sum to constant power.
    SineIn,
    /// `1 − cos(t·π/2)`: slow at first.
    SineOut,
    /// `t^k`. `k < 1` changes fast at first, `k > 1` slowly at first.
    Power(f32),
    /// Smoothstep `3t² − 2t³`.
    SCurve,
}

impl Shape {
    pub fn map(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Shape::Linear => t,
            Shape::SineIn => (t * std::f32::consts::FRAC_PI_2).sin(),
            Shape::SineOut => 1.0 - (t * std::f32::consts::FRAC_PI_2).cos(),
            Shape::Power(k) => t.powf(k.max(0.01)),
            Shape::SCurve => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// A piecewise envelope with up to [`MAX_POINTS`] breakpoints.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    len: u8,
    t: [f32; MAX_POINTS],
    v: [f32; MAX_POINTS],
    /// `shape[i]` shapes the segment from point `i` to point `i + 1`.
    shape: [Shape; MAX_POINTS],
}

impl Default for Envelope {
    fn default() -> Self {
        Self::constant(1.0)
    }
}

impl Envelope {
    pub const fn constant(value: f32) -> Self {
        Self { len: 1, t: [0.0; MAX_POINTS], v: [value; MAX_POINTS], shape: [Shape::Linear; MAX_POINTS] }
    }

    /// `from` until `t0`, shaped change to `to` by `t1`, then `to`.
    pub fn ramp(from: f32, to: f32, t0: f32, t1: f32, shape: Shape) -> Self {
        Self::points(&[(t0, from, shape), (t1.max(t0), to, Shape::Linear)])
    }

    /// Breakpoints `(time, value, shape of the following segment)`, sorted
    /// by time. Extra points beyond [`MAX_POINTS`] are dropped.
    pub fn points(points: &[(f32, f32, Shape)]) -> Self {
        let mut e = Self::constant(points.first().map_or(1.0, |p| p.1));
        let n = points.len().min(MAX_POINTS);
        for (i, &(t, v, shape)) in points.iter().take(n).enumerate() {
            let floor = if i > 0 { e.t[i - 1] } else { 0.0 };
            e.t[i] = t.clamp(0.0, 1.0).max(floor);
            e.v[i] = v;
            e.shape[i] = shape;
        }
        e.len = n.max(1) as u8;
        e
    }

    pub fn len(&self) -> usize {
        usize::from(self.len)
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn first(&self) -> f32 {
        self.v[0]
    }

    pub fn last(&self) -> f32 {
        self.v[self.len() - 1]
    }

    /// The same value everywhere.
    pub fn is_constant(&self) -> bool {
        self.v[..self.len()].iter().all(|v| *v == self.v[0])
    }

    /// Value at progress `p`.
    pub fn at(&self, p: f32) -> f32 {
        let n = self.len();
        if n == 1 || p <= self.t[0] {
            return self.v[0];
        }
        for i in 0..n - 1 {
            let (t0, t1) = (self.t[i], self.t[i + 1]);
            if p < t1 {
                let local = if t1 > t0 { (p - t0) / (t1 - t0) } else { 1.0 };
                return self.v[i] + (self.v[i + 1] - self.v[i]) * self.shape[i].map(local);
            }
        }
        self.v[n - 1]
    }

    /// Breakpoints as `(time, value)`, for display.
    pub fn breakpoints(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        (0..self.len()).map(|i| (self.t[i], self.v[i]))
    }

    /// The same envelope with every value scaled.
    pub fn scaled(mut self, factor: f32) -> Self {
        for v in self.v.iter_mut() {
            *v *= factor;
        }
        self
    }
}

/// Everything automated on one deck during a transition.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeckAutomation {
    /// Broadband amplitude (linear).
    pub gain: Envelope,
    /// Band amplitudes (linear) of the 3-band crossover.
    pub low: Envelope,
    pub mid: Envelope,
    pub high: Envelope,
    /// Swept low-pass cutoff in Hz ([`LOWPASS_OPEN_HZ`] = open).
    pub lowpass_hz: Envelope,
    /// Swept high-pass cutoff in Hz ([`HIGHPASS_OPEN_HZ`] = open).
    pub highpass_hz: Envelope,
    /// Playback rate, pitch preserved (1 = untouched).
    pub rate: Envelope,
}

impl Default for DeckAutomation {
    fn default() -> Self {
        Self {
            gain: Envelope::constant(1.0),
            low: Envelope::constant(1.0),
            mid: Envelope::constant(1.0),
            high: Envelope::constant(1.0),
            lowpass_hz: Envelope::constant(LOWPASS_OPEN_HZ),
            highpass_hz: Envelope::constant(HIGHPASS_OPEN_HZ),
            rate: Envelope::constant(1.0),
        }
    }
}

impl DeckAutomation {
    /// The 3-band EQ does something at some point.
    pub fn uses_bands(&self) -> bool {
        [self.low, self.mid, self.high].iter().any(|e| !(e.is_constant() && e.first() == 1.0))
    }

    pub fn uses_filters(&self) -> bool {
        !(self.lowpass_hz.is_constant() && self.lowpass_hz.first() >= LOWPASS_OPEN_HZ)
            || !(self.highpass_hz.is_constant() && self.highpass_hz.first() <= HIGHPASS_OPEN_HZ)
    }

    pub fn uses_rate(&self) -> bool {
        !(self.rate.is_constant() && self.rate.first() == 1.0)
    }
}

/// A region of a track repeated during a transition. Times are track
/// seconds; `length` should be a whole number of beats.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopRegion {
    pub start: f64,
    pub length: f64,
    /// Extra passes after the first (1 = the region is heard twice).
    pub repeats: u8,
}

/// The full transition: both decks' automation plus optional loops.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TransitionAutomation {
    pub outgoing: DeckAutomation,
    pub incoming: DeckAutomation,
    pub out_loop: Option<LoopRegion>,
    pub in_loop: Option<LoopRegion>,
    /// Seconds over which the incoming rate eases back to 1 afterwards.
    pub release_seconds: f64,
}

impl TransitionAutomation {
    /// A plain fade: outgoing `1 → 0`, incoming `0 → 1`.
    pub fn crossfade(out_shape: Shape, in_shape: Shape) -> Self {
        Self {
            outgoing: DeckAutomation { gain: Envelope::ramp(1.0, 0.0, 0.0, 1.0, out_shape), ..Default::default() },
            incoming: DeckAutomation { gain: Envelope::ramp(0.0, 1.0, 0.0, 1.0, in_shape), ..Default::default() },
            out_loop: None,
            in_loop: None,
            release_seconds: 0.0,
        }
    }

    /// Equal-power crossfade (`cos`/`sin`).
    pub fn equal_power() -> Self {
        Self::crossfade(Shape::SineOut, Shape::SineIn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_hold_interpolate_and_shape() {
        let e = Envelope::ramp(1.0, 0.0, 0.25, 0.75, Shape::Linear);
        assert_eq!(e.at(0.0), 1.0);
        assert_eq!(e.at(0.25), 1.0);
        assert!((e.at(0.5) - 0.5).abs() < 1e-6);
        assert_eq!(e.at(0.9), 0.0);
        let c = Envelope::constant(0.3);
        assert!(c.is_constant() && c.at(0.7) == 0.3);
        // Equal power: out² + in² = 1 everywhere.
        let fade = TransitionAutomation::equal_power();
        for i in 0..=20 {
            let p = i as f32 / 20.0;
            let (o, n) = (fade.outgoing.gain.at(p), fade.incoming.gain.at(p));
            assert!((o * o + n * n - 1.0).abs() < 1e-5, "p {p}: {o} {n}");
        }
        // Power shapes: k > 1 is slower than linear at first.
        assert!(Shape::Power(2.0).map(0.5) < 0.5 && Shape::Power(0.5).map(0.5) > 0.5);
        let multi = Envelope::points(&[(0.0, 0.0, Shape::Linear), (0.5, 1.0, Shape::Linear), (1.0, 0.0, Shape::Linear)]);
        assert!((multi.at(0.25) - 0.5).abs() < 1e-6 && (multi.at(0.75) - 0.5).abs() < 1e-6);
        assert!(!DeckAutomation::default().uses_bands() && !DeckAutomation::default().uses_filters());
    }
}
