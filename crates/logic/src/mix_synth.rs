//! Synthetic "songs" with known ground truth (beat times, bars, sections),
//! for renderer tests and the measurement lab's self-test corpus. They are
//! test signals, not music: a kick on every beat (accented downbeats), a
//! closed hat between beats, a bass note per bar, a sustained chord, and a
//! seeded random note per beat so no two bars are identical (real songs are
//! not exactly periodic; perfectly periodic test audio would make positions
//! ambiguous).

/// A generated track and its ground truth.
#[derive(Debug, Clone)]
pub struct SynthSong {
    pub rate: u32,
    /// Interleaved stereo.
    pub audio: Vec<f32>,
    pub bpm: f64,
    /// Every beat time (s); beats `0, 4, 8, ...` are downbeats.
    pub beats: Vec<f64>,
    /// First and last audible moments (s).
    pub start: f64,
    pub end: f64,
}

/// Options for [`synth_song`].
#[derive(Debug, Clone, Copy)]
pub struct SynthOptions {
    pub bpm: f64,
    /// Seconds of music (whole bars are rendered).
    pub seconds: f64,
    /// Silence before and after.
    pub lead: f64,
    pub tail: f64,
    /// Root of the chord and bass (Hz).
    pub root_hz: f64,
    /// Overall level (linear).
    pub level: f32,
    /// Seconds of linear fade-out at the end (0 = hard ending).
    pub fade_out: f64,
    /// Different noise per song.
    pub seed: u32,
    /// Bars at the start with only kick and bass (no hats, no chord).
    pub intro_bars: usize,
    /// Level of those intro bars (linear, 1 = as loud as the rest).
    pub intro_gain: f32,
}

impl Default for SynthOptions {
    fn default() -> Self {
        Self { bpm: 124.0, seconds: 60.0, lead: 0.0, tail: 0.0, root_hz: 110.0, level: 0.5, fade_out: 0.0, seed: 1, intro_bars: 0, intro_gain: 1.0 }
    }
}

fn noise(seed: &mut u32) -> f32 {
    // xorshift32
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    (*seed as f32 / u32::MAX as f32) * 2.0 - 1.0
}

pub fn synth_song(rate: u32, o: SynthOptions) -> SynthSong {
    let r = f64::from(rate);
    let period = 60.0 / o.bpm;
    let bars = (o.seconds / (4.0 * period)).floor().max(1.0) as usize;
    let music = bars as f64 * 4.0 * period;
    let total = ((o.lead + music + o.tail) * r).ceil() as usize;
    let beats: Vec<f64> = (0..bars * 4).map(|i| o.lead + i as f64 * period).collect();
    let mut audio = vec![0f32; total * 2];
    let mut seed = o.seed.max(1);
    let kick_base = 45.0 + f64::from(o.seed % 7) * 2.5;
    let kick_decay = 0.12 + f64::from(o.seed % 5) * 0.015;
    let tau = 2.0 * std::f64::consts::PI;
    for i in 0..total {
        let t = i as f64 / r;
        let local = t - o.lead;
        if local < 0.0 || local >= music {
            continue;
        }
        let beat_index = (local / period).floor() as usize;
        let in_beat = local - beat_index as f64 * period;
        let accent = if beat_index % 4 == 0 { 1.0 } else { 0.6 };
        // Kick: 2 ms attack, pitch drop from ~120 Hz to its base, ~150 ms
        // decay, tapered to zero by 250 ms (a hard stop would itself be a
        // click). Base pitch and decay vary with the seed, as kicks differ
        // between real songs.
        let kick = if in_beat < 0.25 {
            let attack = (in_beat / 0.002).min(1.0);
            let taper = if in_beat > 0.2 { 0.5 + 0.5 * (std::f64::consts::PI * (in_beat - 0.2) / 0.05).cos() } else { 1.0 };
            let phase = tau * (kick_base * in_beat + 70.0 * 0.03 * (1.0 - (-in_beat / 0.03).exp()));
            phase.sin() * attack * taper * (-in_beat / kick_decay).exp() * accent
        } else {
            0.0
        };
        // Hat on the off-beat: 1 ms attack, 30 ms decay of noise.
        let off = (in_beat - period / 2.0).rem_euclid(period);
        let n = noise(&mut seed);
        let hat = if off < 0.08 { f64::from(n) * (off / 0.001).min(1.0) * (-off / 0.03).exp() * 0.25 } else { 0.0 };
        // Bass note per bar (root, fifth alternating), chord pad.
        let bar = beat_index / 4;
        let bass_hz = if bar % 2 == 0 { o.root_hz / 2.0 } else { o.root_hz * 0.75 };
        let bass = (tau * bass_hz * t).sin() * 0.25;
        let pad = [1.0, 1.26, 1.5].iter().map(|m| (tau * o.root_hz * 2.0 * m * t).sin()).sum::<f64>() * 0.05;
        // A plucked note per beat, its pitch drawn from a pentatonic scale by
        // a hash of (seed, beat): unique bars, same for both channels.
        let note = {
            let h = (beat_index as u32).wrapping_mul(2_654_435_761) ^ o.seed.wrapping_mul(40_503);
            let degree = [0.0, 2.0, 4.0, 7.0, 9.0, 12.0, 14.0, 16.0][(h >> 7) as usize % 8];
            let f = o.root_hz * 2.0 * 2f64.powf(degree / 12.0);
            (tau * f * in_beat).sin() * (-in_beat / 0.18).exp() * (in_beat / 0.003).min(1.0) * 0.12
        };
        let (hat, pad, note, layer) = if bar < o.intro_bars { (0.0, 0.0, 0.0, f64::from(o.intro_gain)) } else { (hat, pad, note, 1.0) };
        let mut v = (kick * 0.7 + hat + bass + pad + note) * f64::from(o.level) * layer;
        if o.fade_out > 0.0 && local > music - o.fade_out {
            v *= (music - local) / o.fade_out;
        }
        // Slight stereo difference so channels are not identical.
        audio[2 * i] = v as f32;
        audio[2 * i + 1] = (v * 0.97 + pad * 0.01) as f32;
    }
    SynthSong { rate, audio, bpm: o.bpm, beats, start: o.lead, end: o.lead + music }
}

/// Mono at 11.025 kHz (for `mix_analysis::analyze_track`) by plain
/// decimation of a song rendered at 44.1 kHz with a crude low-pass.
pub fn to_analysis_rate(song: &SynthSong) -> Vec<f32> {
    let step = (song.rate / 11_025).max(1) as usize;
    let mono: Vec<f32> = song.audio.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect();
    mono.chunks(step).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn songs_have_their_ground_truth() {
        let song = synth_song(44_100, SynthOptions { bpm: 120.0, seconds: 20.0, lead: 1.0, tail: 1.0, ..Default::default() });
        assert_eq!(song.beats.len(), 40);
        assert!((song.beats[1] - song.beats[0] - 0.5).abs() < 1e-9);
        assert!((song.end - 21.0).abs() < 1e-9);
        let peak = song.audio.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.2 && peak < 1.0, "peak {peak}");
        let profile = crate::mix_analysis::analyze_track(&to_analysis_rate(&song)).unwrap();
        assert!((profile.bpm - 120.0).abs() < 1.0, "bpm {}", profile.bpm);
    }
}
