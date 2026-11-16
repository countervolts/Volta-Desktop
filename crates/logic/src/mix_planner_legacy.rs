//! The AutoMix planner as it was before the transition model of
//! `mix_planner` (kept so the preview window can A/B the two). Given two
//! [`MixProfile`]s it decides where the outgoing song hands over (a phrase-aligned downbeat before its trailing
//! silence), where the incoming song joins (its first downbeat after any
//! leading silence), how long the overlap lasts (in bars), whether to match
//! tempo with a pitch-preserving stretch, whether to swap basslines, and how
//! much to adjust the incoming level.

use crate::mix_analysis::MixProfile;
use crate::mix_automation::{Envelope, Shape, TransitionAutomation};
use crate::musical_key::{CamelotKey, camelot_distance, harmonic_compatibility};

/// Tempo change the incoming song can take for a full-length blend (±8%).
pub const MAX_TEMPO_SHIFT: f64 = 0.08;
/// Largest tempo change it is ever given (±18%). Beyond ±8% the blend is kept
/// short, so the stretched part is brief and then eases back to its own tempo.
pub const MAX_RAMP_SHIFT: f64 = 0.18;
const MAX_BLEND_SECONDS: f64 = 32.0;
const MAX_GAIN_DB: f64 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyStyle {
    /// Beats and bars aligned, tempo matched, basslines swapped.
    Beatmatch,
    /// A timed equal-power crossfade.
    Blend,
    /// A short cut for songs that clash.
    Cut,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LegacyPlan {
    pub style: LegacyStyle,
    /// Outgoing track time at which the blend starts.
    pub out_start: f64,
    /// Incoming track time it starts playing from.
    pub in_start: f64,
    /// Blend length in seconds of outgoing time.
    pub seconds: f64,
    /// Incoming playback rate during the blend (1 = untouched).
    pub tempo: f64,
    pub bass_swap: bool,
    pub linear: bool,
    /// Level change for the incoming song, eased out by the blend's end.
    pub incoming_gain_db: f64,
    /// Bars blended (beat-matched mixes).
    pub bars: u32,
    pub label: String,
}

fn key(p: &MixProfile) -> Option<CamelotKey> {
    (p.key_confidence >= 0.25).then_some(CamelotKey { camelot: p.camelot, minor: p.minor })
}

/// Tempo ratio folded through half/double time, so 70 and 140 BPM match.
pub fn folded_ratio(out_bpm: f64, in_bpm: f64) -> f64 {
    let mut r = out_bpm / in_bpm.max(1.0);
    while r > 1.5 {
        r /= 2.0;
    }
    while r < 0.75 {
        r *= 2.0;
    }
    r
}

/// The grid time of `beats[index]` from a least-squares line through the
/// surrounding beats: tracked beats are quantised to ~23 ms analysis frames,
/// the fitted grid is not.
pub fn refined_beat(beats: &[f64], index: usize) -> f64 {
    let from = index.saturating_sub(16);
    let to = (index + 17).min(beats.len());
    let points = &beats[from..to];
    if points.len() < 4 {
        return beats[index];
    }
    let n = points.len() as f64;
    let mean_i = points.iter().enumerate().map(|(i, _)| i as f64).sum::<f64>() / n;
    let mean_t = points.iter().sum::<f64>() / n;
    let (mut num, mut den) = (0.0, 0.0);
    for (i, t) in points.iter().enumerate() {
        num += (i as f64 - mean_i) * (t - mean_t);
        den += (i as f64 - mean_i).powi(2);
    }
    let slope = num / den;
    let fitted = mean_t + slope * ((index - from) as f64 - mean_i);
    // Trust the fit only where the local grid is steady.
    if (fitted - beats[index]).abs() < 0.03 { fitted } else { beats[index] }
}

fn mean_db(values: &[f32]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().map(|v| f64::from(*v)).sum::<f64>() / values.len() as f64)
}

/// Everything the planner weighs before choosing a style.
struct Assessment {
    harmonic: f64,
    ratio: f64,
    rhythmic: bool,
    tempo_ok: bool,
    gain: f64,
    energy_gap: f64,
}

fn assess(from: &MixProfile, to: &MixProfile) -> Assessment {
    let harmonic = match (key(from), key(to)) {
        (Some(a), Some(b)) => harmonic_compatibility(camelot_distance(a, b)),
        _ => 0.5,
    };
    let ratio = folded_ratio(from.bpm, to.bpm);
    let rhythmic = from.beat_confidence >= 0.45 && to.beat_confidence >= 0.45 && from.beats.len() >= 32 && to.beats.len() >= 16;
    let tempo_ok = (ratio - 1.0).abs() <= MAX_RAMP_SHIFT;
    let gain = (from.loudness_db - to.loudness_db).clamp(-MAX_GAIN_DB, MAX_GAIN_DB);
    // Energy at the seams: the outgoing last bars vs the incoming first bars.
    let out_energy = mean_db(&from.bar_energy[from.bar_energy.len().saturating_sub(8)..]).unwrap_or(from.loudness_db);
    let in_energy = mean_db(&to.bar_energy[..to.bar_energy.len().min(8)]).unwrap_or(to.loudness_db);
    let energy_gap = (out_energy - (in_energy + gain)).abs();
    Assessment { harmonic, ratio, rhythmic, tempo_ok, gain, energy_gap }
}

/// Why a pair is not beat-matched, in words; `None` when it can be.
pub fn beatmatch_blocker(from: &MixProfile, to: &MixProfile) -> Option<String> {
    let a = assess(from, to);
    if from.beat_confidence < 0.45 || from.beats.len() < 32 {
        return Some(format!("Beats of the first song are not steady enough ({:.0}% sure, needs 45%)", from.beat_confidence * 100.0));
    }
    if to.beat_confidence < 0.45 || to.beats.len() < 16 {
        return Some(format!("Beats of the second song are not steady enough ({:.0}% sure, needs 45%)", to.beat_confidence * 100.0));
    }
    if !a.tempo_ok {
        return Some(format!("Tempos are {:.0}% apart (limit {:.0}%)", (a.ratio - 1.0).abs() * 100.0, MAX_RAMP_SHIFT * 100.0));
    }
    if a.energy_gap >= 12.0 {
        return Some(format!("Energy differs by {:.0} dB at the join (limit 12)", a.energy_gap));
    }
    None
}

pub fn plan_mix(from: &MixProfile, to: &MixProfile) -> LegacyPlan {
    let Assessment { harmonic, ratio, rhythmic, tempo_ok, gain, energy_gap } = assess(from, to);
    let in_start = to.start.max(0.0);

    // Keys that do not fit no longer rule out a beat-matched mix: the bass
    // swap keeps the lows from fighting, and the blend is simply shorter.
    if rhythmic && tempo_ok && energy_gap < 12.0 {
        let mut bars: u32 = if harmonic >= 0.66 { 8 } else if harmonic >= 0.5 { 4 } else { 2 };
        // A bigger tempo change is only held for a short stretch.
        let shift = (ratio - 1.0).abs();
        if shift > MAX_TEMPO_SHIFT + 0.04 {
            bars = bars.min(2);
        } else if shift > MAX_TEMPO_SHIFT {
            bars = bars.min(4);
        }
        let period = from.beat_period();
        while bars > 1 && (bars as f64 * 4.0 * period > MAX_BLEND_SECONDS || bars as f64 * 4.0 * period > (from.end - from.start) / 3.0) {
            bars /= 2;
        }
        let downbeats: Vec<f64> = from.downbeats().collect();
        let seconds = bars as f64 * 4.0 * period;
        // Latest downbeat whose blend ends before the outgoing audio does;
        // prefer one on a 4-bar phrase boundary within the last 16 bars.
        let fits: Vec<(usize, f64)> = downbeats.iter().copied().enumerate().filter(|(_, d)| d + seconds <= from.end + period * 0.25).collect();
        let entry_index = (0..to.beats.len()).skip(to.downbeat_phase).step_by(4).find(|&i| to.beats[i] >= in_start - 0.05);
        let entry_in = entry_index.map(|i| refined_beat(&to.beats, i));
        if let (Some(&(last_index, last)), Some(entry)) = (fits.last(), entry_in) {
            let phrase = fits.iter().rev().take_while(|(i, _)| last_index - i < 16).find(|(i, _)| i % 4 == 0).map(|(_, d)| *d);
            let chosen = phrase.unwrap_or(last);
            let out_index = from.beats.iter().position(|b| *b == chosen).unwrap_or(0);
            let out_start = refined_beat(&from.beats, out_index);
            return LegacyPlan {
                style: LegacyStyle::Beatmatch,
                out_start,
                in_start: entry,
                seconds,
                tempo: ratio,
                bass_swap: true,
                linear: false,
                incoming_gain_db: gain,
                bars,
                label: format!("Beat-matched · {bars} bars at {} BPM", from.bpm.round()),
            };
        }
    }

    // Only a big loudness jump earns a hard cut; keys that differ get a short blend.
    let clash = energy_gap >= 12.0;
    let seconds: f64 = if clash { 0.8 } else if harmonic >= 0.5 { 6.0 } else if harmonic >= 0.34 { 4.0 } else { 3.0 };
    let seconds = seconds.min((from.end - from.start) / 4.0).max(0.2);
    let mut out_start = (from.end - seconds).max(from.start);
    // Land the blend on a beat when the outgoing song has one.
    if from.beat_confidence >= 0.3 {
        if let Some(beat) = from.beats.iter().rev().find(|b| **b <= out_start + 0.01) {
            if out_start - beat < from.beat_period() {
                out_start = *beat;
            }
        }
    }
    let seconds = (from.end - out_start).max(0.2);
    LegacyPlan {
        style: if clash { LegacyStyle::Cut } else { LegacyStyle::Blend },
        out_start,
        in_start,
        seconds,
        tempo: 1.0,
        bass_swap: false,
        linear: clash,
        incoming_gain_db: gain,
        bars: 0,
        label: if clash { "Quick cut · Loudness jumps".into() } else { "Crossfade · Smooth blend".into() },
    }
}

impl LegacyPlan {
    /// As a [`crate::mix_planner::MixPlan`], so the same engine and preview
    /// can play it (A/B against the current planner).
    pub fn to_mix_plan(&self) -> crate::mix_planner::MixPlan {
        use crate::mix_planner::TransitionKind;
        crate::mix_planner::MixPlan {
            kind: match self.style {
                LegacyStyle::Beatmatch => TransitionKind::BeatMatched,
                LegacyStyle::Blend | LegacyStyle::Cut => TransitionKind::EnergyCrossfade,
            },
            out_start: self.out_start,
            in_start: self.in_start,
            seconds: self.seconds,
            automation: self.automation(),
            bars: self.bars,
            tempo: self.tempo,
            score: 1.0,
            label: self.label.clone(),
            reason: format!("previous planner: {}", self.label),
            candidates: Vec::new(),
        }
    }

    /// The same transition in the engine's automation terms: equal-power or
    /// linear fades, the bass swap over the middle 20%, the incoming level
    /// offset eased out, the incoming tempo held then released over 8 s.
    pub fn automation(&self) -> TransitionAutomation {
        let mut a = if self.linear { TransitionAutomation::crossfade(Shape::Linear, Shape::Linear) } else { TransitionAutomation::equal_power() };
        let g0 = 10f32.powf(self.incoming_gain_db as f32 / 20.0);
        a.incoming.gain = Envelope::points(&(0..6).map(|i| {
            let p = i as f32 / 5.0;
            let fade = if self.linear { p } else { (p * std::f32::consts::FRAC_PI_2).sin() };
            (p, fade * (g0 + (1.0 - g0) * p), Shape::Linear)
        }).collect::<Vec<_>>());
        if self.bass_swap {
            a.outgoing.low = Envelope::ramp(1.0, 0.0, 0.4, 0.6, Shape::SCurve);
            a.incoming.low = Envelope::ramp(0.0, 1.0, 0.4, 0.6, Shape::SCurve);
        }
        if (self.tempo - 1.0).abs() > 1e-9 {
            a.incoming.rate = Envelope::constant(self.tempo as f32);
        }
        a.release_seconds = 8.0;
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(bpm: f64, seconds: f64, camelot: u8, lead: f64, tail: f64) -> MixProfile {
        let period = 60.0 / bpm;
        let beats: Vec<f64> = (0..).map(|i| lead + i as f64 * period).take_while(|t| *t < lead + seconds).collect();
        let bars = beats.len() / 4;
        MixProfile {
            version: 3,
            duration: lead + seconds + tail,
            bpm,
            beat_confidence: 0.9,
            beats,
            downbeat_phase: 0,
            downbeat_confidence: 0.8,
            camelot,
            minor: true,
            key_confidence: 0.8,
            loudness_db: -12.0,
            start: lead,
            end: lead + seconds,
            bar_energy: vec![-12.0; bars],
            ..Default::default()
        }
    }

    #[test]
    fn beatmatches_compatible_songs_on_a_phrase() {
        let from = profile(124.0, 180.0, 8, 0.5, 4.0);
        let to = profile(126.0, 200.0, 9, 1.2, 0.0);
        let plan = plan_mix(&from, &to);
        assert_eq!(plan.style, LegacyStyle::Beatmatch);
        assert_eq!(plan.bars, 8);
        assert!(plan.out_start + plan.seconds <= from.end + 0.2, "ends in audio");
        assert!((plan.in_start - 1.2).abs() < 1e-6, "skips the incoming silence");
        assert!((plan.tempo - 124.0 / 126.0).abs() < 1e-9);
        // On a downbeat, a whole number of bars from the first.
        let bars = (plan.out_start - 0.5) / (4.0 * 60.0 / 124.0);
        assert!((bars - bars.round()).abs() < 1e-6 && (bars.round() as i64) % 4 == 0);
        // Jittered beats still give an exact grid point.
        let jittered: Vec<f64> = (0..64).map(|i| i as f64 * 0.5 + if i % 2 == 0 { 0.011 } else { -0.011 }).collect();
        assert!((refined_beat(&jittered, 32) - 16.0).abs() < 0.003);
    }

    #[test]
    fn falls_back_to_blends_and_cuts() {
        let from = profile(124.0, 180.0, 8, 0.0, 3.0);
        let slow = profile(90.0, 180.0, 8, 0.0, 0.0);
        let plan = plan_mix(&from, &slow);
        assert_eq!(plan.style, LegacyStyle::Blend);
        assert!(plan.out_start + plan.seconds <= from.end + 1e-6);
        // Far-off keys no longer stop a beat-matched mix; they shorten it.
        let other_key = profile(124.0, 180.0, 2, 0.0, 0.0);
        let keyed = plan_mix(&from, &other_key);
        assert_eq!((keyed.style, keyed.bars), (LegacyStyle::Beatmatch, 2));
        // A loudness jump at the join is cut.
        let mut quiet = profile(124.0, 180.0, 8, 0.0, 0.0);
        quiet.bar_energy = vec![-40.0; quiet.bar_energy.len()];
        quiet.loudness_db = -12.0;
        assert_eq!(plan_mix(&from, &quiet).style, LegacyStyle::Cut);
        // A 14% tempo gap is ridden for a couple of bars, not blended in full.
        let faster = profile(124.0 * 1.14, 180.0, 8, 0.0, 0.0);
        let ridden = plan_mix(&from, &faster);
        assert_eq!((ridden.style, ridden.bars), (LegacyStyle::Beatmatch, 2));
        assert!((folded_ratio(70.0, 140.0) - 1.0).abs() < 1e-9);
        // The preview explains why a pair was not beat-matched.
        assert!(beatmatch_blocker(&from, &slow).unwrap().contains("apart"));
        assert!(beatmatch_blocker(&from, &profile(126.0, 180.0, 8, 0.0, 0.0)).is_none());
    }
}
