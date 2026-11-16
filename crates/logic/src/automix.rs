//! Port of `src/lib/automix.ts`: plans from measured analysis (with the
//! beat-matched branch) or server tempo/key tags.

use crate::audio_analysis::TrackAnalysis;
use crate::model::Song;
use crate::musical_key::{camelot_distance, harmonic_compatibility, parse_musical_key, tempo_compatibility};

pub const AUTOMIX_MAX_OVERLAP: f64 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomixStyle {
    Beatmatch,
    Crossfade,
    Cut,
    Gapless,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutomixPlan {
    pub style: AutomixStyle,
    pub overlap_seconds: f64,
    pub tempo_ratio: f64,
    /// Absolute outgoing start time when a measured grid is available.
    pub outgoing_start_seconds: Option<f64>,
    pub linear: bool,
    pub harmonic: f64,
    pub tempo_compatibility: f64,
    pub reason: String,
}

pub const AUTOMIX_MAX_TEMPO_SHIFT: f64 = 0.03;

struct Profile {
    bpm: f64,
    key: Option<crate::musical_key::CamelotKey>,
    energy: f64,
    /// `"analysis" | "server" | "estimated"`.
    source: &'static str,
}

/// `automixProfile`: measured analysis first, then server tags.
fn profile(song: &Song, analysis: Option<&TrackAnalysis>) -> Profile {
    if let Some(a) = analysis.filter(|a| a.bpm.is_finite() && a.bpm > 0.0 && a.energy.is_finite() && a.confidence.is_finite()) {
        return Profile {
            bpm: a.bpm,
            key: Some(crate::musical_key::CamelotKey { camelot: a.camelot, minor: a.mode == "A" }),
            energy: a.energy,
            source: "analysis",
        };
    }
    let bpm = song.bpm.unwrap_or(f64::NAN);
    let key = parse_musical_key(song.musical_key.as_deref());
    let has_tempo = bpm.is_finite() && (40.0..=240.0).contains(&bpm);
    let server = has_tempo && key.is_some();
    Profile { bpm: if has_tempo { bpm } else { 0.0 }, key, energy: 0.5, source: if server { "server" } else { "estimated" } }
}

/// `planAutomixTransition`. `duration` is the outgoing media duration.
pub fn plan_automix_transition(
    from: &Song,
    from_analysis: Option<&TrackAnalysis>,
    to: &Song,
    to_analysis: Option<&TrackAnalysis>,
    duration: Option<f64>,
) -> AutomixPlan {
    let duration = duration.or(from_analysis.and_then(|a| a.duration)).or(from.duration).unwrap_or(0.0);
    let incoming = to_analysis.and_then(|a| a.duration).or(to.duration).unwrap_or(0.0);
    let max_overlap = AUTOMIX_MAX_OVERLAP
        .min(if duration > 0.0 { duration / 4.0 } else { AUTOMIX_MAX_OVERLAP })
        .min(if incoming > 0.0 { incoming / 4.0 } else { AUTOMIX_MAX_OVERLAP });
    let (a, b) = (profile(from, from_analysis), profile(to, to_analysis));
    let known_key = |p: &Profile, an: Option<&TrackAnalysis>| {
        p.source == "server" || (p.source == "analysis" && an.and_then(|a| a.key_confidence).unwrap_or(0.0) >= 0.5)
    };
    let known_tempo = |p: &Profile, an: Option<&TrackAnalysis>| {
        p.source == "server" || (p.source == "analysis" && an.and_then(|a| a.tempo_confidence).unwrap_or(0.0) >= 0.7)
    };
    let harmonic = match (known_key(&a, from_analysis) && known_key(&b, to_analysis), a.key, b.key) {
        (true, Some(ka), Some(kb)) => harmonic_compatibility(camelot_distance(ka, kb)),
        _ => 0.5,
    };
    let tempo_known = known_tempo(&a, from_analysis) && known_tempo(&b, to_analysis);
    let tempo = if tempo_known { tempo_compatibility(a.bpm, b.bpm) } else { 0.5 };
    let outro = from_analysis.and_then(|x| x.outro);
    let intro = to_analysis.and_then(|x| x.intro);
    let energy_gap = (outro.map_or(a.energy, |o| o.energy) - intro.map_or(b.energy, |i| i.energy)).abs();
    let plan = AutomixPlan {
        style: AutomixStyle::Crossfade,
        overlap_seconds: 2f64.min(max_overlap),
        tempo_ratio: 1.0,
        outgoing_start_seconds: None,
        linear: false,
        harmonic,
        tempo_compatibility: tempo,
        reason: "Gentle blend at original tempo".into(),
    };
    if harmonic < 0.34
        || (tempo_known && tempo < 0.55)
        || energy_gap > 0.55
        || from.id == to.id
        || (duration > 0.0 && duration < 12.0)
        || (incoming > 0.0 && incoming < 12.0)
    {
        return AutomixPlan { style: AutomixStyle::Gapless, overlap_seconds: 0.0, reason: "Preserving the natural song boundary".into(), ..plan };
    }
    let (Some(outro), Some(intro)) = (outro, intro) else { return plan };
    if outro.confidence < 0.7
        || intro.confidence < 0.7
        || !(outro.bpm.is_finite() && intro.bpm.is_finite())
        || outro.bpm <= 0.0
        || intro.bpm <= 0.0
        || !(outro.beat_offset.is_finite() && intro.beat_offset.is_finite())
        || duration <= 0.0
        || energy_gap > 0.3
    {
        return plan;
    }
    let ratio = outro.bpm / intro.bpm;
    if (ratio - 1.0).abs() > AUTOMIX_MAX_TEMPO_SHIFT {
        return plan;
    }
    let beat = 60.0 / outro.bpm;
    let desired = (if harmonic >= 0.67 { beat * 8.0 } else { beat * 4.0 }).min(max_overlap);
    let phase = outro.beat_offset - intro.beat_offset / ratio;
    let earliest = duration - desired;
    let start = phase + ((earliest - phase) / beat).ceil() * beat;
    let overlap = duration - start;
    if overlap < 1.0 || start < 0.0 || start < duration - max_overlap {
        return plan;
    }
    AutomixPlan {
        style: AutomixStyle::Beatmatch,
        overlap_seconds: overlap,
        outgoing_start_seconds: Some(start),
        tempo_ratio: ratio,
        reason: format!("Measured beats aligned near {} BPM", crate::js::js_round(outro.bpm)),
        ..plan
    }

}

/// `isAlbumSequence`: one album in track order.
pub fn is_album_sequence(queue: &[&Song]) -> bool {
    if queue.len() < 2 {
        return false;
    }
    let Some(album) = queue[0].album_id.as_deref() else { return false };
    let (mut previous_track, mut previous_disc) = (None::<f64>, 1.0);
    for song in queue {
        if song.album_id.as_deref() != Some(album) {
            return false;
        }
        let Some(track) = song.track.filter(|t| t.is_finite()) else { return false };
        let disc = song.disc_number.unwrap_or(1.0);
        if let Some(prev) = previous_track {
            let next_in_disc = disc == previous_disc && track == prev + 1.0;
            let next_disc = disc == previous_disc + 1.0 && track == 1.0;
            if !next_in_disc && !next_disc {
                return false;
            }
        }
        previous_track = Some(track);
        previous_disc = disc;
    }
    true
}

/// `automixEligible`. Local files qualify: the native app profiles them from
/// disk, so they are mixed like any other song. With `mix_albums` off, an
/// album played in track order keeps its natural boundaries.
pub fn automix_eligible(queue: &[&Song], mix_albums: bool) -> bool {
    queue.len() >= 2 && (mix_albums || !is_album_sequence(queue))
}

/// `describeAutomixStyle`.
pub fn describe_automix_style(style: AutomixStyle) -> &'static str {
    match style {
        AutomixStyle::Gapless => "Natural transition",
        AutomixStyle::Beatmatch => "Beat-matched",
        AutomixStyle::Cut => "Quick blend",
        AutomixStyle::Crossfade => "Crossfade",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Source;

    fn song(id: &str, album: &str, track: f64, duration: f64) -> Song {
        Song { id: id.into(), album_id: Some(album.into()), track: Some(track), duration: Some(duration), ..Default::default() }
    }

    #[test]
    fn plans_gentle_blend_and_respects_album_order() {
        let (a, b) = (song("a", "x", 1.0, 200.0), song("b", "y", 1.0, 200.0));
        let plan = plan_automix_transition(&a, None, &b, None, None);
        assert_eq!(plan.style, AutomixStyle::Crossfade);
        assert_eq!(plan.overlap_seconds, 2.0);
        assert!(automix_eligible(&[&a, &b], false));
        let c = song("c", "x", 2.0, 200.0);
        assert!(!automix_eligible(&[&a, &c], false));
        assert!(automix_eligible(&[&a, &c], true), "mixing albums is opt-in");
        // Local files are profiled from disk, so they mix like any other song.
        let local = Song { source: Some(Source::Local), ..song("l", "w", 1.0, 200.0) };
        assert!(automix_eligible(&[&a, &local], false));
        let short = song("s", "z", 1.0, 10.0);
        assert_eq!(plan_automix_transition(&a, None, &short, None, None).style, AutomixStyle::Gapless);
    }

    #[test]
    fn measured_edges_plan_a_beat_match() {
        let region = |bpm: f64, offset: f64| crate::audio_analysis::TransitionRegion { bpm, beat_offset: offset, confidence: 0.9, energy: 0.6 };
        let analysis = |intro, outro| TrackAnalysis {
            bpm: 120.0,
            camelot: 8,
            mode: "B".into(),
            energy: 0.6,
            confidence: 0.9,
            source: "analysis".into(),
            tempo_confidence: Some(0.9),
            key_confidence: Some(0.8),
            duration: Some(200.0),
            intro: Some(intro),
            outro: Some(outro),
        };
        let (a, b) = (song("a", "x", 1.0, 200.0), song("b", "y", 1.0, 200.0));
        let from = analysis(region(120.0, 0.1), region(120.0, 184.25));
        let to = analysis(region(121.0, 0.2), region(121.0, 184.0));
        let plan = plan_automix_transition(&a, Some(&from), &b, Some(&to), Some(200.0));
        assert_eq!(plan.style, AutomixStyle::Beatmatch);
        assert!(plan.overlap_seconds >= 1.0 && plan.overlap_seconds <= 8.0);
        assert!((plan.tempo_ratio - 120.0 / 121.0).abs() < 1e-9);
    }
}
