//! AutoMix transition model.
//!
//! For a pair of [`MixProfile`]s the planner builds every candidate
//! transition it can, scores each, and plays the best one, falling back to
//! a natural gapless handoff ([`TransitionKind::NoMix`]) below a confidence
//! floor: a missing mix is forgiven, a broken one is not.
//!
//! Outcome families (Apple documents the first three as AutoMix outcomes;
//! the rest are the DJ techniques the research points to):
//!
//! * `NoMix` — gapless, untouched.
//! * `TrimOnly` — leading/trailing silence removed, joined gaplessly.
//! * `EnergyCrossfade` — Apple's crossfade patent (US 8,553,504) as a
//!   continuous function: fade start, length and curve follow the loudness
//!   level and trend of the outgoing ending and the incoming start.
//! * `BeatMatched` — downbeat- and phrase-aligned blend with the incoming
//!   tempo matched (pitch preserved) and eased back afterwards, and a 3-band
//!   EQ handover (highs, then mids, then a bass swap).
//! * `Extend` — like `BeatMatched`, but the outgoing song's last bars are
//!   repeated to make room for the blend instead of starting it early.
//! * `Filter` — low-pass wash out, high-pass opening in. Off by default:
//!   nothing measured yet says Apple does it.
//!
//! **Every threshold lives in [`PlannerConfig`]. The defaults are Volta's
//! own choices, not measurements of Apple Music**; the measurement lab fits
//! replacements from recordings and writes them as JSON.

use serde::{Deserialize, Serialize};

use crate::mix_analysis::MixProfile;
use crate::mix_automation::{DeckAutomation, Envelope, LoopRegion, Shape, TransitionAutomation};
use crate::musical_key::{CamelotKey, camelot_distance, harmonic_compatibility};

pub use crate::mix_planner_legacy::{folded_ratio, refined_beat};

/// The grid time of `beats[index]`: a least-squares line through the 16
/// beats either side. Tracked beats are quantised to ~23 ms frames (worse
/// at a file's very start), the fitted grid is not. The fit is trusted
/// whenever the surrounding grid is steady (its other beats sit within
/// 15 ms RMS of the line), however far the beat itself strays; otherwise
/// only when it moves the beat by less than 30 ms.
pub fn grid_beat(beats: &[f64], index: usize) -> f64 {
    let from = index.saturating_sub(16);
    let to = (index + 17).min(beats.len());
    let points = &beats[from..to];
    if points.len() < 6 {
        return beats[index];
    }
    let others: Vec<(f64, f64)> = points.iter().enumerate().filter(|(i, _)| from + i != index).map(|(i, t)| (i as f64, *t)).collect();
    let n = others.len() as f64;
    let (mi, mt) = (others.iter().map(|p| p.0).sum::<f64>() / n, others.iter().map(|p| p.1).sum::<f64>() / n);
    let (num, den) = others.iter().fold((0.0, 0.0), |(a, b), p| (a + (p.0 - mi) * (p.1 - mt), b + (p.0 - mi).powi(2)));
    let slope = num / den.max(1e-12);
    let line = |i: f64| mt + slope * (i - mi);
    let rms = (others.iter().map(|p| (p.1 - line(p.0)).powi(2)).sum::<f64>() / n).sqrt();
    let fitted = line((index - from) as f64);
    if rms < 0.015 || (fitted - beats[index]).abs() < 0.03 { fitted } else { beats[index] }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum TransitionKind {
    NoMix,
    TrimOnly,
    EnergyCrossfade,
    BeatMatched,
    Extend,
    Filter,
}

impl TransitionKind {
    pub const ALL: [TransitionKind; 6] = [Self::NoMix, Self::TrimOnly, Self::EnergyCrossfade, Self::BeatMatched, Self::Extend, Self::Filter];

    pub fn name(self) -> &'static str {
        match self {
            Self::NoMix => "No mix",
            Self::TrimOnly => "Trim only",
            Self::EnergyCrossfade => "Crossfade",
            Self::BeatMatched => "Beat-matched",
            Self::Extend => "Extended",
            Self::Filter => "Filter",
        }
    }

    /// Stable identifier (lab tables, JSON).
    pub fn id(self) -> &'static str {
        match self {
            Self::NoMix => "no-mix",
            Self::TrimOnly => "trim-only",
            Self::EnergyCrossfade => "crossfade",
            Self::BeatMatched => "beat-matched",
            Self::Extend => "extend",
            Self::Filter => "filter",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.id() == id)
    }
}

/// Planner thresholds. Defaults are Volta's, not Apple's (see module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PlannerConfig {
    /// Below this best score the pair is not mixed.
    pub floor: f64,
    pub min_song_seconds: f64,
    /// Beat tracking confidence both songs need for a beat-matched mix.
    pub min_beat_confidence: f64,
    pub min_key_confidence: f64,
    /// Tempo tiers (fraction): up to `tier1` the stretch is held through any
    /// blend; up to `ceiling` only short blends; beyond, no beat-matching.
    /// (0.18 is the ceiling Volta used before; Apple's is unmeasured.)
    pub tempo_tier1: f64,
    pub tempo_ceiling: f64,
    /// Largest energy gap (dB) at the seam for a beat-matched mix.
    pub max_energy_gap_db: f64,
    /// Blend lengths in bars by key fit (compatible / neutral / clashing).
    pub bars_compatible: u32,
    pub bars_neutral: u32,
    pub bars_clash: u32,
    pub max_blend_seconds: f64,
    /// Bars over which the incoming tempo eases back after the blend.
    pub release_bars: f64,
    /// Most of the outgoing song left unheard after the switch, as seconds
    /// and as a fraction of its audible length; same for the incoming intro
    /// skipped.
    pub max_unheard_seconds: f64,
    pub max_unheard_fraction: f64,
    pub max_skip_seconds: f64,
    /// Score lost per second of outro left unheard beyond two bars.
    pub unheard_penalty: f64,
    /// Energy crossfade (patent model): base length, limits, and how strongly
    /// quiet/loud edges change length, timing and curve.
    pub crossfade_seconds: f64,
    pub min_crossfade_seconds: f64,
    pub max_crossfade_seconds: f64,
    /// Edge level (dB under the song's own loudness) where "quiet" starts,
    /// and the range over which it becomes fully quiet.
    pub quiet_offset_db: f64,
    pub quiet_range_db: f64,
    /// Edge level (dB over the song's loudness) where "loud" starts / range.
    pub loud_offset_db: f64,
    pub loud_range_db: f64,
    /// A falling (or rising, for intros) level of this many dB/s counts as
    /// fully decreasing (increasing).
    pub trend_db_per_s: f64,
    /// Fade length multiplier for a fully quiet ending (1 = up to 2×).
    pub quiet_stretch: f64,
    /// Curve exponent strength: quiet ending → fade-out `t^(2^s)` (slower than
    /// linear); quiet start → fade-in faster than linear; loud → faster out.
    pub curve_strength: f64,
    /// A quiet start begins this fraction of the base length before the
    /// outgoing fade does.
    pub early_in: f64,
    /// Least silence (s) worth trimming.
    pub min_trim_seconds: f64,
    /// Largest incoming level adjustment (dB) during a mix.
    pub max_gain_db: f64,
    /// Tonal-activity level counted as a busy lead/vocal at the seam.
    pub tonal_clash: f64,
    /// Ending-like-start similarity treated as a looping ending.
    pub loop_ending_threshold: f64,
    /// Downbeat confidence an Extend loop needs. 0 = off: the measure is not
    /// calibrated (a strongly accented synthetic track reads 0.14), and a
    /// whole-bar loop on beat boundaries stays in time even off-phase.
    pub min_downbeat_confidence_loop: f64,
    /// Base scores per family before pair-specific adjustment.
    pub score_beatmatch: f64,
    pub score_extend: f64,
    pub score_crossfade: f64,
    pub score_trim: f64,
    pub score_filter: f64,
    pub enable_extend: bool,
    pub enable_filter: bool,
    /// Mix albums played in track order (Apple: never).
    pub mix_albums: bool,
    /// Genre words that mean "do not mix" (lower-case substrings).
    pub no_mix_genres: Vec<String>,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            floor: 0.35,
            min_song_seconds: 30.0,
            min_beat_confidence: 0.45,
            min_key_confidence: 0.25,
            tempo_tier1: 0.06,
            tempo_ceiling: 0.18,
            max_energy_gap_db: 12.0,
            bars_compatible: 8,
            bars_neutral: 4,
            bars_clash: 2,
            max_blend_seconds: 24.0,
            release_bars: 8.0,
            max_unheard_seconds: 12.0,
            max_unheard_fraction: 0.15,
            max_skip_seconds: 8.0,
            unheard_penalty: 0.03,
            crossfade_seconds: 5.0,
            min_crossfade_seconds: 1.5,
            max_crossfade_seconds: 12.0,
            quiet_offset_db: 4.0,
            quiet_range_db: 12.0,
            loud_offset_db: 1.0,
            loud_range_db: 4.0,
            trend_db_per_s: 1.5,
            quiet_stretch: 1.0,
            curve_strength: 1.0,
            early_in: 0.5,
            min_trim_seconds: 0.4,
            max_gain_db: 6.0,
            tonal_clash: 0.55,
            loop_ending_threshold: 0.95,
            min_downbeat_confidence_loop: 0.0,
            score_beatmatch: 0.9,
            score_extend: 0.8,
            score_crossfade: 0.55,
            score_trim: 0.45,
            score_filter: 0.6,
            enable_extend: true,
            enable_filter: false,
            mix_albums: false,
            no_mix_genres: ["classical", "opera", "spoken", "audiobook", "podcast", "comedy", "speech", "meditation"].map(String::from).to_vec(),
        }
    }
}

impl PlannerConfig {
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn unmixable_genre(&self, genre: &str) -> bool {
        let genre = genre.to_lowercase();
        self.no_mix_genres.iter().any(|g| genre.contains(g.as_str()))
    }
}

/// Facts about the pair beyond the audio.
#[derive(Debug, Clone, Default)]
pub struct PairContext {
    /// One album playing in track order.
    pub album_in_order: bool,
    pub from_genre: Option<String>,
    pub to_genre: Option<String>,
    /// The same recording twice.
    pub same_song: bool,
}

/// One scored candidate (for the preview and the lab).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub kind: TransitionKind,
    pub score: f64,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MixPlan {
    pub kind: TransitionKind,
    /// Outgoing track time at which the transition starts.
    pub out_start: f64,
    /// Incoming track time it starts playing from.
    pub in_start: f64,
    /// Overlap length (output seconds); 0 for `NoMix`.
    pub seconds: f64,
    pub automation: TransitionAutomation,
    /// Bars blended (beat-matched kinds).
    pub bars: u32,
    /// Incoming playback rate during the overlap.
    pub tempo: f64,
    pub score: f64,
    /// Short label for the player ("Beat-matched · 8 bars at 124 BPM").
    pub label: String,
    /// Why this outcome, in words (status line).
    pub reason: String,
    pub candidates: Vec<Candidate>,
}

impl MixPlan {
    pub fn no_mix(reason: impl Into<String>) -> Self {
        Self {
            kind: TransitionKind::NoMix,
            out_start: 0.0,
            in_start: 0.0,
            seconds: 0.0,
            automation: TransitionAutomation::default(),
            bars: 0,
            tempo: 1.0,
            score: 0.0,
            label: "Gapless".into(),
            reason: reason.into(),
            candidates: Vec::new(),
        }
    }

    /// The engine has something to do (anything but a natural handoff).
    pub fn mixes(&self) -> bool {
        self.kind != TransitionKind::NoMix && self.seconds > 0.0
    }
}

/// Everything about a pair the planner (and the lab's decision table) uses.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairFeatures {
    pub out_bpm: f64,
    pub in_bpm: f64,
    /// Folded tempo ratio out/in and its distance from 1.
    pub ratio: f64,
    pub tempo_gap: f64,
    /// Camelot steps between keys (None when either key is unsure).
    pub key_distance: Option<u8>,
    pub harmonic: f64,
    pub out_confidence: f64,
    pub in_confidence: f64,
    pub out_downbeat_confidence: f64,
    /// Level offset applied to the incoming song (dB).
    pub gain_db: f64,
    /// Seam energy gap: outgoing last 8 bars vs incoming first 8 (+ gain).
    pub energy_gap_db: f64,
    /// Outgoing last 10 s and incoming first 10 s: level relative to the
    /// song's own loudness (dB) and trend (dB/s).
    pub out_tail_db: f64,
    pub out_tail_trend: f64,
    pub in_head_db: f64,
    pub in_head_trend: f64,
    pub out_fade: f64,
    pub in_fade: f64,
    pub outro_bars: u32,
    pub intro_bars: u32,
    /// Tonal mid activity of the last / first 4 bars.
    pub out_tonal: f64,
    pub in_tonal: f64,
    pub loop_ending: f64,
    /// Silence (s) after the outgoing audio and before the incoming audio.
    pub out_silence: f64,
    pub in_silence: f64,
}

fn key(p: &MixProfile, config: &PlannerConfig) -> Option<CamelotKey> {
    (p.key_confidence >= config.min_key_confidence).then_some(CamelotKey { camelot: p.camelot, minor: p.minor })
}

fn mean(values: &[f32]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().map(|v| f64::from(*v)).sum::<f64>() / values.len() as f64)
}

pub fn pair_features(from: &MixProfile, to: &MixProfile, config: &PlannerConfig) -> PairFeatures {
    let (ka, kb) = (key(from, config), key(to, config));
    let key_distance = match (ka, kb) {
        (Some(a), Some(b)) => Some(camelot_distance(a, b)),
        _ => None,
    };
    let harmonic = key_distance.map_or(0.5, harmonic_compatibility);
    let ratio = folded_ratio(from.bpm, to.bpm);
    let gain_db = (from.loudness_db - to.loudness_db).clamp(-config.max_gain_db, config.max_gain_db);
    let out_energy = mean(&from.bar_energy[from.bar_energy.len().saturating_sub(8)..]).unwrap_or(from.loudness_db);
    let in_energy = mean(&to.bar_energy[..to.bar_energy.len().min(8)]).unwrap_or(to.loudness_db);
    let tail_from = (from.end - 10.0).max(from.start);
    let head_to = (to.start + 10.0).min(to.end);
    let tonal = |p: &MixProfile, tail: bool| {
        let n = p.bar_tonal.len();
        let slice = if tail { &p.bar_tonal[n.saturating_sub(4)..] } else { &p.bar_tonal[..n.min(4)] };
        mean(slice).unwrap_or(0.0)
    };
    PairFeatures {
        out_bpm: from.bpm,
        in_bpm: to.bpm,
        ratio,
        tempo_gap: (ratio - 1.0).abs(),
        key_distance,
        harmonic,
        out_confidence: from.beat_confidence,
        in_confidence: to.beat_confidence,
        out_downbeat_confidence: from.downbeat_confidence,
        gain_db,
        energy_gap_db: (out_energy - (in_energy + gain_db)).abs(),
        out_tail_db: from.level_between(tail_from, from.end).map_or(0.0, |l| l - from.loudness_db),
        out_tail_trend: from.trend_between(tail_from, from.end).unwrap_or(0.0),
        in_head_db: to.level_between(to.start, head_to).map_or(0.0, |l| l - to.loudness_db),
        in_head_trend: to.trend_between(to.start, head_to).unwrap_or(0.0),
        out_fade: from.fade_out,
        in_fade: to.fade_in,
        outro_bars: from.outro_bars,
        intro_bars: to.intro_bars,
        out_tonal: tonal(from, true),
        in_tonal: tonal(to, false),
        loop_ending: f64::from(from.loop_ending),
        out_silence: (from.duration - from.end).max(0.0),
        in_silence: to.start.max(0.0),
    }
}

/// `x` mapped to 0–1 between `lo` and `hi`.
fn ramp01(x: f64, lo: f64, hi: f64) -> f64 {
    if hi <= lo {
        return if x >= hi { 1.0 } else { 0.0 };
    }
    ((x - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// An envelope sampled from a function of progress at `n` points.
fn sampled(n: usize, f: impl Fn(f32) -> f32) -> Envelope {
    let n = n.clamp(2, crate::mix_automation::MAX_POINTS);
    let points: Vec<(f32, f32, Shape)> = (0..n).map(|i| {
        let p = i as f32 / (n - 1) as f32;
        (p, f(p), Shape::Linear)
    }).collect();
    Envelope::points(&points)
}

/// Incoming gain: a fade `fade(p)` times a level offset that eases from
/// `gain_db` to 0 dB by the end of the overlap.
fn incoming_gain(gain_db: f64, fade: impl Fn(f32) -> f32) -> Envelope {
    let g0 = 10f32.powf(gain_db as f32 / 20.0);
    sampled(6, |p| fade(p) * (g0 + (1.0 - g0) * p))
}

/// Why a pair is not beat-matched, in words; `None` when it can be.
pub fn beatmatch_blocker(from: &MixProfile, to: &MixProfile) -> Option<String> {
    beatmatch_blocker_with(from, to, &PlannerConfig::default())
}

pub fn beatmatch_blocker_with(from: &MixProfile, to: &MixProfile, config: &PlannerConfig) -> Option<String> {
    let f = pair_features(from, to, config);
    let need = config.min_beat_confidence * 100.0;
    if from.beat_confidence < config.min_beat_confidence || from.beats.len() < 32 {
        return Some(format!("Beats of the first song are not steady enough ({:.0}% sure, needs {need:.0}%)", from.beat_confidence * 100.0));
    }
    if to.beat_confidence < config.min_beat_confidence || to.beats.len() < 16 {
        return Some(format!("Beats of the second song are not steady enough ({:.0}% sure, needs {need:.0}%)", to.beat_confidence * 100.0));
    }
    if f.tempo_gap > config.tempo_ceiling {
        return Some(format!("Tempos are {:.0}% apart (limit {:.0}%)", f.tempo_gap * 100.0, config.tempo_ceiling * 100.0));
    }
    if f.energy_gap_db >= config.max_energy_gap_db {
        return Some(format!("Energy differs by {:.0} dB at the join (limit {:.0})", f.energy_gap_db, config.max_energy_gap_db));
    }
    None
}

/// Plans with the default configuration and no context.
pub fn plan_mix(from: &MixProfile, to: &MixProfile) -> MixPlan {
    plan_transition(from, to, &PairContext::default(), &PlannerConfig::default())
}

pub fn plan_transition(from: &MixProfile, to: &MixProfile, context: &PairContext, config: &PlannerConfig) -> MixPlan {
    if context.same_song {
        return MixPlan::no_mix("Same song twice");
    }
    if context.album_in_order && !config.mix_albums {
        return MixPlan::no_mix("Album playing in order");
    }
    for genre in [&context.from_genre, &context.to_genre].into_iter().flatten() {
        if config.unmixable_genre(genre) {
            return MixPlan::no_mix(format!("{genre} plays without transitions"));
        }
    }
    if from.end - from.start < config.min_song_seconds || to.end - to.start < config.min_song_seconds {
        return MixPlan::no_mix("Song too short to mix");
    }
    let f = pair_features(from, to, config);
    let mut plans: Vec<MixPlan> = Vec::new();
    if let Some(plan) = trim_only(from, to, &f, config) {
        plans.push(plan);
    }
    plans.push(energy_crossfade(from, to, &f, config));
    let beat = beat_matched(from, to, &f, config);
    let beat_unfit = beat.is_err();
    match beat {
        Ok(plan) => plans.push(plan),
        Err(Unfit::Blocked) => {}
        Err(Unfit::NoRoom) => {}
    }
    if config.enable_extend && (beat_unfit || from.fade_out == 0.0)
        && let Some(plan) = extend(from, to, &f, config) {
            plans.push(plan);
        }
    if config.enable_filter
        && let Some(plan) = filter(from, to, &f, config) {
            plans.push(plan);
        }
    // A looping ending is not a place to cut into: mixes lose their appeal.
    if f.loop_ending >= config.loop_ending_threshold && from.fade_out == 0.0 {
        for plan in plans.iter_mut().filter(|p| matches!(p.kind, TransitionKind::BeatMatched | TransitionKind::Extend | TransitionKind::EnergyCrossfade)) {
            plan.score -= 0.25;
            plan.reason.push_str("; ending loops back to its start");
        }
    }
    let candidates: Vec<Candidate> = plans.iter().map(|p| Candidate { kind: p.kind, score: p.score, note: p.reason.clone() }).collect();
    let best = plans.into_iter().max_by(|a, b| a.score.total_cmp(&b.score));
    let mut plan = match best {
        Some(plan) if plan.score >= config.floor => plan,
        Some(plan) => MixPlan::no_mix(format!("No transition is convincing enough ({}: {:.2} < {:.2})", plan.kind.name(), plan.score, config.floor)),
        None => MixPlan::no_mix("Nothing to plan"),
    };
    plan.candidates = candidates;
    if !plan.mixes() && plan.kind == TransitionKind::NoMix && plan.reason.is_empty() {
        plan.reason = "Natural transition".into();
    }
    plan
}

/// Leading/trailing silence removed: a 30 ms join at the audible edges.
fn trim_only(from: &MixProfile, to: &MixProfile, f: &PairFeatures, config: &PlannerConfig) -> Option<MixPlan> {
    let silence = f.out_silence + f.in_silence;
    if silence < config.min_trim_seconds {
        return None;
    }
    // A hard ending into a hard start is where trimming beats blending.
    let hard = from.fade_out == 0.0 && to.fade_in == 0.0;
    let score = config.score_trim + if hard { 0.1 } else { -0.1 };
    let seconds = 0.03;
    Some(MixPlan {
        kind: TransitionKind::TrimOnly,
        out_start: (from.end - seconds).max(from.start),
        in_start: to.start,
        seconds,
        automation: TransitionAutomation::crossfade(Shape::Linear, Shape::Linear),
        bars: 0,
        tempo: 1.0,
        score,
        label: "Silence trimmed".into(),
        reason: format!("{silence:.1} s of silence removed{}", if hard { ", hard ending into a hard start" } else { "" }),
        candidates: Vec::new(),
    })
}

/// Apple's crossfade patent (US 8,553,504) as a continuous function of the
/// edge levels and trends: a quiet or falling ending fades out earlier,
/// longer and slower than linear; a loud ending fades out faster; a quiet or
/// rising start begins earlier and rises faster than linear.
fn energy_crossfade(from: &MixProfile, to: &MixProfile, f: &PairFeatures, config: &PlannerConfig) -> MixPlan {
    let c = config;
    let quiet_out = (ramp01(-f.out_tail_db, c.quiet_offset_db, c.quiet_offset_db + c.quiet_range_db) + 0.5 * ramp01(-f.out_tail_trend, 0.0, c.trend_db_per_s)).min(1.0);
    let loud_out = ramp01(f.out_tail_db, c.loud_offset_db, c.loud_offset_db + c.loud_range_db) * (1.0 - quiet_out);
    let quiet_in = (ramp01(-f.in_head_db, c.quiet_offset_db, c.quiet_offset_db + c.quiet_range_db) + 0.5 * ramp01(f.in_head_trend, 0.0, c.trend_db_per_s)).min(1.0);
    let base = c.crossfade_seconds;
    let out_len = base * (1.0 + c.quiet_stretch * quiet_out) * (1.0 - 0.4 * loud_out);
    let in_len = base * (1.0 + 0.5 * quiet_in);
    let lead = c.early_in * base * quiet_in;
    let limit = c.max_crossfade_seconds.min((from.end - from.start) / 4.0).min((to.end - to.start) / 4.0);
    let mut seconds = (out_len + lead).clamp(c.min_crossfade_seconds, limit.max(c.min_crossfade_seconds));
    let mut out_start = (from.end - seconds).max(from.start);
    // Land on a beat when the outgoing song has one.
    if from.beat_confidence >= 0.3
        && let Some(beat) = from.beats.iter().rev().find(|b| **b <= out_start + 0.01)
        && out_start - beat < from.beat_period() {
            out_start = *beat;
        }
    seconds = (from.end - out_start).max(0.2);
    let out_from = (lead / seconds).min(0.9) as f32;
    let in_to = (in_len / seconds).clamp(0.1, 1.0) as f32;
    let k_out = 2f64.powf(c.curve_strength * (quiet_out - loud_out)) as f32;
    let k_in = 2f64.powf(-c.curve_strength * quiet_in) as f32;
    // Fade-out t^k on the way down: 1 − (1 − value) shaped.
    let outgoing = DeckAutomation { gain: Envelope::ramp(1.0, 0.0, out_from, 1.0, Shape::Power(k_out)), ..Default::default() };
    let in_shape = Shape::Power(k_in);
    let incoming = DeckAutomation { gain: incoming_gain(f.gain_db, |p| in_shape.map((p / in_to).min(1.0))), ..Default::default() };
    let mut score = c.score_crossfade + 0.1 * quiet_out;
    if from.fade_out == 0.0 && to.fade_in == 0.0 && quiet_out < 0.2 {
        score -= 0.1;
    }
    if f.out_tonal > c.tonal_clash && f.in_tonal > c.tonal_clash {
        score -= 0.05;
    }
    let ending = if quiet_out > 0.5 { "quiet ending" } else if loud_out > 0.5 { "loud ending" } else { "steady ending" };
    let start = if quiet_in > 0.5 { "quiet start" } else { "full start" };
    MixPlan {
        kind: TransitionKind::EnergyCrossfade,
        out_start,
        in_start: to.start,
        seconds,
        automation: TransitionAutomation { outgoing, incoming, ..Default::default() },
        bars: 0,
        tempo: 1.0,
        score,
        label: format!("Crossfade · {seconds:.1} s"),
        reason: format!("{ending}, {start}: fade-out curve t^{k_out:.2}, fade-in t^{k_in:.2}"),
        candidates: Vec::new(),
    }
}

enum Unfit {
    /// Not beat-matchable at all (beats, tempo, energy).
    Blocked,
    /// Beat-matchable, but no room for the blend at the end.
    NoRoom,
}

/// The DJ-style EQ handover: per band an equal-power crossfade, staggered —
/// highs first, then mids, with a short bass swap in the middle so the two
/// basslines never stack. Each band's two fades share a window, so the
/// summed power stays level (uncorrelated material) through the blend.
fn eq_blend(f: &PairFeatures, ratio: f64, seconds: f64, release: f64) -> TransitionAutomation {
    let swap = if seconds < 6.0 { (0.48, 0.52) } else { (0.46, 0.54) };
    let fade_in = |t0: f32, t1: f32| Envelope::ramp(0.0, 1.0, t0, t1, Shape::SineIn);
    let fade_out = |t0: f32, t1: f32| Envelope::ramp(1.0, 0.0, t0, t1, Shape::SineOut);
    let outgoing = DeckAutomation {
        high: fade_out(0.0, 0.6),
        mid: fade_out(0.2, 0.8),
        low: Envelope::ramp(1.0, 0.0, swap.0, swap.1, Shape::SCurve),
        // Whatever is left (the swap's tail) is gone by the end.
        gain: fade_out(0.8, 1.0),
        ..Default::default()
    };
    let mut incoming = DeckAutomation {
        high: fade_in(0.0, 0.6),
        mid: fade_in(0.2, 0.8),
        low: Envelope::ramp(0.0, 1.0, swap.0, swap.1, Shape::SCurve),
        gain: incoming_gain(f.gain_db, |_| 1.0),
        ..Default::default()
    };
    // Hold the matched tempo through the overlap unless the drift without it
    // would stay under 4 ms.
    if (ratio - 1.0).abs() * seconds > 0.004 {
        incoming.rate = Envelope::constant(ratio as f32);
    }
    TransitionAutomation { outgoing, incoming, out_loop: None, in_loop: None, release_seconds: release }
}

/// Bars a beat-matched blend of this pair should last.
fn desired_bars(from: &MixProfile, to: &MixProfile, f: &PairFeatures, c: &PlannerConfig) -> u32 {
    let mut bars = if f.harmonic >= 0.66 { c.bars_compatible } else if f.harmonic >= 0.5 { c.bars_neutral } else { c.bars_clash };
    if f.tempo_gap > c.tempo_tier1 {
        bars = bars.min(if f.tempo_gap > (c.tempo_tier1 + c.tempo_ceiling) / 2.0 { 2 } else { 4 });
    }
    // Lead over lead: keep it short.
    if f.out_tonal > c.tonal_clash && f.in_tonal > c.tonal_clash {
        bars = bars.min(4);
    }
    // No intro to blend under: short entrance.
    if to.intro_bars == 0 && f.in_head_db > -c.quiet_offset_db {
        bars = bars.min(4);
    }
    let bar = from.bar_period();
    while bars > 1 && (bars as f64 * bar > c.max_blend_seconds || bars as f64 * bar > (from.end - from.start) / 3.0) {
        bars /= 2;
    }
    bars.max(1)
}

fn beat_preconditions(from: &MixProfile, to: &MixProfile, f: &PairFeatures, c: &PlannerConfig) -> bool {
    from.beat_confidence >= c.min_beat_confidence && to.beat_confidence >= c.min_beat_confidence && from.beats.len() >= 32 && to.beats.len() >= 16 && f.tempo_gap <= c.tempo_ceiling && f.energy_gap_db < c.max_energy_gap_db
}

/// A feasible beat-matched mix normally outranks a crossfade or a trim
/// (≈ 0.65–0.9); marginal confidence, a big tempo change, a key clash or
/// lead over lead pull it down toward them.
fn beat_score(f: &PairFeatures, c: &PlannerConfig, base: f64) -> f64 {
    let confidence = ramp01(f.out_confidence.min(f.in_confidence), c.min_beat_confidence, 0.8);
    let tempo = 1.0 - 0.3 * (f.tempo_gap / c.tempo_ceiling.max(1e-6));
    let clash = if f.out_tonal > c.tonal_clash && f.in_tonal > c.tonal_clash { 0.08 } else { 0.0 };
    base * (0.85 + 0.15 * confidence) * tempo * (0.85 + 0.15 * f.harmonic) - clash
}

/// The incoming entry: its first downbeat at or after the audible start.
/// When tracking began a bar or more into the song (its first beats were
/// missed), the grid is extended back by whole bars to the start.
fn entry(to: &MixProfile) -> Option<f64> {
    let index = (0..to.beats.len()).skip(to.downbeat_phase).step_by(4).find(|&i| to.beats[i] >= to.start - 0.05)?;
    let mut t = grid_beat(&to.beats, index);
    let bar = to.bar_period();
    while t - bar >= to.start - 0.05 {
        t -= bar;
    }
    Some(t)
}

fn beat_matched(from: &MixProfile, to: &MixProfile, f: &PairFeatures, c: &PlannerConfig) -> Result<MixPlan, Unfit> {
    if !beat_preconditions(from, to, f, c) {
        return Err(Unfit::Blocked);
    }
    let bars = desired_bars(from, to, f, c);
    let bar = from.bar_period();
    let seconds = bars as f64 * bar;
    let in_start = entry(to).ok_or(Unfit::Blocked)?;
    if in_start - to.start > c.max_skip_seconds {
        return Err(Unfit::Blocked);
    }
    let downbeat_index: Vec<usize> = (0..from.beats.len()).skip(from.downbeat_phase).step_by(4).collect();
    let audible = from.end - from.start;
    let max_unheard = c.max_unheard_seconds.min(c.max_unheard_fraction * audible);
    // Latest downbeat whose blend ends before the outgoing audio does,
    // preferring a phrase start (4-bar grid) and an outro boundary.
    let fits: Vec<(usize, usize, f64)> = downbeat_index
        .iter()
        .enumerate()
        .map(|(bar_no, &i)| (bar_no, i, grid_beat(&from.beats, i)))
        .filter(|(_, _, t)| t + seconds <= from.end + from.beat_period() * 0.25 && t + seconds >= from.end - max_unheard)
        .collect();
    let Some(&(last_bar, _, _)) = fits.last() else { return Err(Unfit::NoRoom) };
    let boundary = |t: f64| from.boundaries.iter().any(|(b, _)| (b - t).abs() < from.beat_period() * 0.5);
    let on_phrase = |bar_no: usize| (bar_no as i64 - i64::from(from.phrase_phase)).rem_euclid(4) == 0;
    let chosen = fits
        .iter()
        .rev()
        .take_while(|(b, _, _)| last_bar - b < 8)
        .find(|(b, _, t)| on_phrase(*b) && boundary(*t))
        .or_else(|| fits.iter().rev().take_while(|(b, _, _)| last_bar - b < 4).find(|(b, _, _)| on_phrase(*b)))
        .or(fits.last())
        .copied();
    let (bar_no, _, out_start) = chosen.ok_or(Unfit::NoRoom)?;
    let release = c.release_bars * to.bar_period() * if f.tempo_gap > c.tempo_tier1 { 0.5 } else { 1.0 };
    let automation = eq_blend(f, f.ratio, seconds, release);
    // Loud outro the switch would leave unheard (within 10 dB of the song's
    // loudness; a fading tail does not count), beyond two bars, costs score:
    // over-trimming is a top complaint, and an extension that blends at the
    // very end then wins.
    let switch = out_start + seconds;
    let loud_after = if from.envelope_db.is_empty() {
        (from.end - switch).max(0.0)
    } else {
        from.envelope_db.iter().enumerate().filter(|(i, l)| *i as f64 * 0.5 >= switch && (*i as f64 * 0.5) < from.end && f64::from(**l) >= from.loudness_db - 10.0).count() as f64 * 0.5
    };
    let unheard = (loud_after - 2.0 * bar).max(0.0);
    let trim_penalty = c.unheard_penalty * unheard;
    let phrase = if boundary(out_start) { "on the outro boundary" } else if on_phrase(bar_no) { "on a phrase start" } else { "on a downbeat" };
    let tier = if f.tempo_gap <= 0.004 { "same tempo" } else if f.tempo_gap <= c.tempo_tier1 { "tempo held" } else { "short ride on a bigger tempo change" };
    Ok(MixPlan {
        kind: TransitionKind::BeatMatched,
        out_start,
        in_start,
        seconds,
        automation,
        bars,
        tempo: f.ratio,
        score: beat_score(f, c, c.score_beatmatch) - trim_penalty,
        label: format!("Beat-matched · {bars} bars at {} BPM", from.bpm.round()),
        reason: format!("{bars} bars {phrase}, {tier} ({:+.1}%), key fit {:.0}%", (f.ratio - 1.0) * 100.0, f.harmonic * 100.0),
        candidates: Vec::new(),
    })
}

/// Repeats the outgoing song's last bars so a beat-matched blend fits at
/// the very end instead of starting early (no outro is eaten).
fn extend(from: &MixProfile, to: &MixProfile, f: &PairFeatures, c: &PlannerConfig) -> Option<MixPlan> {
    if !beat_preconditions(from, to, f, c) || from.downbeat_confidence < c.min_downbeat_confidence_loop {
        return None;
    }
    let bars = desired_bars(from, to, f, c).max(2);
    let bar = from.bar_period();
    let in_start = entry(to)?;
    if in_start - to.start > c.max_skip_seconds {
        return None;
    }
    let loop_bars: u32 = if bars >= 8 { 2 } else { 1 };
    // The loop covers the last whole bars: it ends where the bar after the
    // last tracked downbeat ends, if the audio lasts that long, otherwise on
    // that downbeat.
    let downbeats: Vec<usize> = (0..from.beats.len()).skip(from.downbeat_phase).step_by(4).collect();
    let last = downbeats.iter().rposition(|&i| from.beats[i] <= from.end - 0.05)?;
    let last_time = grid_beat(&from.beats, downbeats[last]);
    // (The loop must end inside the file: the deck cannot jump back from a
    // point it never reaches.)
    let (l1, end_bar) = if last_time + bar <= from.end + 0.25 * from.beat_period() && last_time + bar <= from.duration - 0.01 { (last_time + bar, last + 1) } else { (last_time, last) };
    if end_bar < loop_bars as usize + 1 {
        return None;
    }
    let l0 = l1 - f64::from(loop_bars) * bar;
    let length = l1 - l0;
    // The loop must sound like a steady bar: similar energy across it.
    let bar_from = end_bar - loop_bars as usize;
    if let Some(slice) = from.bar_energy.get(bar_from..end_bar.min(from.bar_energy.len())) {
        let (lo, hi) = slice.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        if hi - lo > 6.0 || f64::from(hi) < from.loudness_db - 12.0 {
            return None;
        }
    }
    let repeats = ((bars - loop_bars) as f64 / loop_bars as f64).ceil().clamp(1.0, 3.0) as u8;
    let total_bars = loop_bars * (1 + u32::from(repeats));
    let seconds = length * f64::from(1 + u32::from(repeats));
    let release = c.release_bars * to.bar_period() * if f.tempo_gap > c.tempo_tier1 { 0.5 } else { 1.0 };
    let mut automation = eq_blend(f, f.ratio, seconds, release);
    automation.out_loop = Some(LoopRegion { start: l0, length, repeats });
    Some(MixPlan {
        kind: TransitionKind::Extend,
        out_start: l0,
        in_start,
        seconds,
        automation,
        bars: total_bars,
        tempo: f.ratio,
        score: beat_score(f, c, c.score_extend),
        label: format!("Extended · {loop_bars}-bar loop ×{}", repeats + 1),
        reason: format!("last {loop_bars} bar(s) repeated {repeats}× to blend {total_bars} bars at the very end"),
        candidates: Vec::new(),
    })
}

/// Low-pass wash out, high-pass opening in, on the beat (off by default).
fn filter(from: &MixProfile, to: &MixProfile, f: &PairFeatures, c: &PlannerConfig) -> Option<MixPlan> {
    if !beat_preconditions(from, to, f, c) {
        return None;
    }
    let bar = from.bar_period();
    let bars = 4u32;
    let seconds = f64::from(bars) * bar;
    let in_start = entry(to)?;
    let downbeats: Vec<usize> = (0..from.beats.len()).skip(from.downbeat_phase).step_by(4).collect();
    let out_index = downbeats.iter().rev().find(|&&i| from.beats[i] + seconds <= from.end)?;
    let out_start = grid_beat(&from.beats, *out_index);
    let mut automation = TransitionAutomation {
        outgoing: DeckAutomation {
            lowpass_hz: Envelope::ramp(20_000.0, 350.0, 0.0, 0.85, Shape::Power(0.3)),
            gain: Envelope::ramp(1.0, 0.0, 0.6, 1.0, Shape::SineOut),
            ..Default::default()
        },
        incoming: DeckAutomation {
            highpass_hz: Envelope::ramp(1_200.0, 20.0, 0.3, 1.0, Shape::Power(2.0)),
            gain: incoming_gain(f.gain_db, |p| Shape::SineIn.map((p / 0.4).min(1.0))),
            ..Default::default()
        },
        ..Default::default()
    };
    if (f.ratio - 1.0).abs() * seconds > 0.004 {
        automation.incoming.rate = Envelope::constant(f.ratio as f32);
    }
    automation.release_seconds = c.release_bars * to.bar_period();
    let clash = f.harmonic < 0.34;
    Some(MixPlan {
        kind: TransitionKind::Filter,
        out_start,
        in_start,
        seconds,
        automation,
        bars,
        tempo: f.ratio,
        score: beat_score(f, c, c.score_filter) + if clash { 0.15 } else { 0.0 },
        label: format!("Filter · {bars} bars"),
        reason: if clash { "keys clash: filtered handover".into() } else { "filtered handover".into() },
        candidates: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A steady profile: beats from `lead`, flat energy.
    pub(crate) fn profile(bpm: f64, seconds: f64, camelot: u8, lead: f64, tail: f64) -> MixProfile {
        let period = 60.0 / bpm;
        let beats: Vec<f64> = (0..).map(|i| lead + i as f64 * period).take_while(|t| *t < lead + seconds).collect();
        let bars = beats.len() / 4;
        let duration = lead + seconds + tail;
        MixProfile {
            version: crate::mix_analysis::PROFILE_VERSION,
            duration,
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
            envelope_db: (0..(duration * 2.0).ceil() as usize).map(|i| if (i as f64 * 0.5) < lead || (i as f64 * 0.5) >= lead + seconds { -120.0 } else { -12.0 }).collect(),
            bar_low: vec![0.3; bars],
            bar_tonal: vec![0.2; bars],
            intro_bars: 8,
            outro_bars: 8,
            ..Default::default()
        }
    }

    #[test]
    fn beatmatches_compatible_songs_on_a_phrase() {
        let from = profile(124.0, 180.0, 8, 0.5, 4.0);
        let to = profile(126.0, 200.0, 9, 1.2, 0.0);
        let plan = plan_mix(&from, &to);
        assert_eq!(plan.kind, TransitionKind::BeatMatched, "{plan:#?}");
        assert_eq!(plan.bars, 8);
        assert!(plan.out_start + plan.seconds <= from.end + 0.2, "ends in audio");
        assert!(from.end - (plan.out_start + plan.seconds) < 4.0 * from.bar_period(), "eats little of the outro");
        assert!((plan.in_start - 1.2).abs() < 1e-6, "skips only the incoming silence");
        assert!((plan.tempo - 124.0 / 126.0).abs() < 1e-9);
        assert!((f64::from(plan.automation.incoming.rate.first()) - 124.0 / 126.0).abs() < 1e-6);
        // On a downbeat of the 4-bar grid.
        let bars = (plan.out_start - 0.5) / from.bar_period();
        assert!((bars - bars.round()).abs() < 1e-6 && (bars.round() as i64) % 4 == 0);
        // Bass swap: one bassline at a time.
        let a = &plan.automation;
        for p in [0.2f32, 0.4, 0.6, 0.8] {
            assert!(a.outgoing.low.at(p) * a.incoming.low.at(p) < 0.1, "both basses at {p}");
        }
    }

    #[test]
    fn chooses_other_families_when_beats_do_not_fit() {
        let from = profile(124.0, 180.0, 8, 0.0, 0.0);
        // 90 vs 124 BPM, no silence to trim: an energy crossfade.
        let slow = profile(90.0, 180.0, 8, 0.0, 0.0);
        let plan = plan_mix(&from, &slow);
        assert_eq!(plan.kind, TransitionKind::EnergyCrossfade, "{plan:#?}");
        assert!(plan.out_start + plan.seconds <= from.end + 1e-6);
        assert!(beatmatch_blocker(&from, &slow).unwrap().contains("apart"));
        // A 9% gap is ridden briefly.
        let faster = profile(124.0 * 1.09, 180.0, 8, 0.0, 0.0);
        let ridden = plan_mix(&from, &faster);
        assert!(matches!(ridden.kind, TransitionKind::BeatMatched | TransitionKind::Extend));
        assert!(ridden.bars <= 4, "{}", ridden.bars);
        // Weak beats and a hard ending into a hard start with silence: trimmed.
        let mut weak = profile(124.0, 180.0, 8, 0.0, 2.0);
        weak.beat_confidence = 0.2;
        let mut next = profile(100.0, 180.0, 3, 0.8, 0.0);
        next.beat_confidence = 0.2;
        let plan = plan_mix(&weak, &next);
        assert_eq!(plan.kind, TransitionKind::TrimOnly, "{plan:#?}");
        assert!((plan.in_start - 0.8).abs() < 1e-9 && plan.out_start >= weak.end - 0.05);
        // Album order is never mixed by default; unmixable genres neither.
        let album = PairContext { album_in_order: true, ..Default::default() };
        assert_eq!(plan_transition(&from, &slow, &album, &PlannerConfig::default()).kind, TransitionKind::NoMix);
        let classical = PairContext { to_genre: Some("Classical".into()), ..Default::default() };
        assert_eq!(plan_transition(&from, &slow, &classical, &PlannerConfig::default()).kind, TransitionKind::NoMix);
    }

    #[test]
    fn energy_crossfade_follows_the_patent_shapes() {
        let config = PlannerConfig::default();
        let mut fading = profile(90.0, 180.0, 8, 0.0, 0.0);
        // The last 12 s fall from −12 to −40 dB.
        let n = fading.envelope_db.len();
        for i in n - 24..n {
            fading.envelope_db[i] = -12.0 - 28.0 * ((i - (n - 24)) as f32 / 24.0);
        }
        fading.fade_out = 12.0;
        let steady = profile(90.0, 180.0, 8, 0.0, 0.0);
        let other = profile(130.0, 180.0, 8, 0.0, 0.0);
        let quiet_end = energy_crossfade(&fading, &other, &pair_features(&fading, &other, &config), &config);
        let plain = energy_crossfade(&steady, &other, &pair_features(&steady, &other, &config), &config);
        assert!(quiet_end.seconds > plain.seconds * 1.4, "quiet ending fades longer: {} vs {}", quiet_end.seconds, plain.seconds);
        // Slower than linear on the way down: still high at the midpoint.
        let mid = quiet_end.automation.outgoing.gain.at(0.5);
        assert!(mid > 0.6, "quiet-ending fade-out at the middle {mid}");
        // A quiet start rises faster than linear.
        let mut soft = profile(130.0, 180.0, 8, 0.0, 0.0);
        for i in 0..20 {
            soft.envelope_db[i] = -30.0 + i as f32;
        }
        let rising = energy_crossfade(&steady, &soft, &pair_features(&steady, &soft, &config), &config);
        let quarter = rising.automation.incoming.gain.at(0.15);
        let plain_quarter = plain.automation.incoming.gain.at(0.15);
        assert!(quarter > plain_quarter, "quiet start rises faster: {quarter} vs {plain_quarter}");
    }

    #[test]
    fn extends_a_hard_ending_instead_of_eating_it() {
        // Room for only 2 bars after the last phrase boundary is not the
        // issue: a hard ending with steady last bars gets a loop when the
        // planner would otherwise have to start the blend early.
        // (A second of reverb tail after the last bar, as real files have.)
        let from = profile(124.0, 180.0, 8, 0.0, 1.0);
        let to = profile(124.0, 180.0, 8, 0.0, 0.0);
        let config = PlannerConfig::default();
        let f = pair_features(&from, &to, &config);
        let plan = extend(&from, &to, &f, &config).expect("extend");
        let region = plan.automation.out_loop.unwrap();
        assert!((region.length - 2.0 * from.bar_period()).abs() < 1e-6);
        assert!((region.start + region.length - from.end).abs() < 0.01, "loop ends with the last bar");
        assert_eq!(plan.bars, 2 * (1 + u32::from(region.repeats)));
        assert!((plan.seconds - f64::from(plan.bars) * from.bar_period()).abs() < 1e-6);
        assert!(from.end - plan.out_start < 3.0 * from.bar_period(), "starts at the very end");
    }

    #[test]
    fn config_round_trips_and_floors_to_no_mix() {
        let config = PlannerConfig { floor: 0.99, ..Default::default() };
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(PlannerConfig::from_json(&json).unwrap(), config);
        let partial = PlannerConfig::from_json(r#"{"tempoCeiling": 0.08}"#).unwrap();
        assert_eq!(partial.tempo_ceiling, 0.08);
        assert_eq!(partial.floor, PlannerConfig::default().floor);
        let from = profile(124.0, 180.0, 8, 0.0, 0.0);
        let plan = plan_transition(&from, &profile(125.0, 180.0, 8, 0.0, 0.0), &PairContext::default(), &config);
        assert_eq!(plan.kind, TransitionKind::NoMix);
        assert!(plan.reason.contains("convincing"), "{}", plan.reason);
        assert!(!plan.candidates.is_empty());
    }
}
