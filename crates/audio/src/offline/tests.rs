//! Objective transition bench: renders transitions with the product's own
//! mixer and checks them by measurement (beat alignment between per-deck
//! stems, loudness continuity, peaks, clicks at loop seams, flux at the
//! seam). Synthetic songs give exact ground truth.

use volta_logic::mix_automation::{DeckAutomation, Envelope, LoopRegion, Shape, TransitionAutomation};
use volta_logic::mix_metrics::{alignment_error, detect_clicks, seam_ratio, flux_spike_ratio, follow_beats, loudness_series, max_loudness_jump, peak, spectral_flux, to_mono};
use volta_logic::mix_synth::{SynthOptions, SynthSong, synth_song};

use super::*;

const RATE: u32 = 44_100;

fn song(bpm: f64, root: f64, seed: u32) -> SynthSong {
    synth_song(RATE, SynthOptions { bpm, seconds: 48.0, root_hz: root, seed, ..Default::default() })
}

/// Beat-matched automation: equal-power gains, bass swap at the midpoint,
/// incoming rate `ratio` (or a ramp on both decks when `ramp`).
fn beatmatched(ratio: f64, ramp: bool) -> TransitionAutomation {
    let mut a = TransitionAutomation::equal_power();
    a.outgoing.low = Envelope::ramp(1.0, 0.0, 0.45, 0.55, Shape::SCurve);
    a.incoming.low = Envelope::ramp(0.0, 1.0, 0.45, 0.55, Shape::SCurve);
    if ramp {
        let k = (1.0 / ratio) as f32;
        a.outgoing.rate = Envelope::ramp(1.0, k, 0.0, 1.0, Shape::SCurve);
        a.incoming.rate = Envelope::ramp(1.0 / k, 1.0, 0.0, 1.0, Shape::SCurve);
    } else {
        a.incoming.rate = Envelope::constant(ratio as f32);
    }
    a.release_seconds = 4.0;
    a
}

fn flat_gains(mut a: TransitionAutomation) -> TransitionAutomation {
    for deck in [&mut a.outgoing, &mut a.incoming] {
        deck.gain = Envelope::constant(1.0);
        deck.low = Envelope::constant(1.0);
    }
    a
}

fn request(out_start: f64, in_start: f64, seconds: f64, automation: TransitionAutomation) -> RenderRequest {
    RenderRequest { out_start, in_start, seconds, automation, lead: 6.0, tail: 6.0, out_gain: 1.0, in_gain: 1.0 }
}

/// Beat misalignment (ms) between the stems over the overlap: each source
/// beat is located in its stem by cross-correlation with the clean source,
/// then every incoming beat is compared with the nearest outgoing beat.
/// Returns `(median, p95, beats compared)`.
fn alignment_ms(a: &SynthSong, b: &SynthSong, req: &RenderRequest) -> (f64, f64, usize) {
    // Gains and bands do not move beats; flattening them keeps every beat
    // matchable against the clean source.
    let flat = RenderRequest { automation: flat_gains(req.automation), ..*req };
    let r = render_transition(&a.audio, &b.audio, RATE, &flat, true);
    let [out, inc] = r.stems.as_ref().unwrap();
    let rate = f64::from(RATE);
    // The last beat before the switch has no outgoing partner once it ends.
    let (from, to) = (r.start_frame as f64 / rate + 0.02, r.end_frame as f64 / rate - 0.75 * 60.0 / a.bpm);
    // The outgoing source as heard: with a loop, its region repeats, so the
    // follower walks a virtual timeline (audio to the loop end, the region
    // again per repeat) whose beats keep increasing.
    let (a_audio, a_all_beats) = match req.automation.out_loop {
        Some(region) => {
            let s0 = (region.start * rate).round() as usize * 2;
            // The region can end a hair past the file (its last bar's end).
            let s1 = (((region.start + region.length) * rate).round() as usize * 2).min(a.audio.len());
            let mut padded_region = a.audio[s0..s1].to_vec();
            padded_region.resize(((region.length * rate).round() as usize) * 2, 0.0);
            let mut audio = a.audio[..s1].to_vec();
            let mut beats: Vec<f64> = a.beats.iter().copied().filter(|t| *t < region.start + region.length - 1e-6).collect();
            let inside: Vec<f64> = a.beats.iter().copied().filter(|t| *t >= region.start - 1e-6 && *t < region.start + region.length - 1e-6).collect();
            audio.resize(s0 + padded_region.len(), 0.0);
            for k in 1..=u32::from(region.repeats) {
                audio.extend_from_slice(&padded_region);
                beats.extend(inside.iter().map(|t| t + f64::from(k) * region.length));
            }
            (audio, beats)
        }
        None => (a.audio.clone(), a.beats.clone()),
    };
    let a_beats: Vec<f64> = a_all_beats.iter().copied().filter(|t| *t >= req.out_start - 1.0).collect();
    let on_out = follow_beats(&to_mono(out), &to_mono(&a_audio), rate, &a_beats, a_beats[0] - r.out_base, 1.0, 0.03);
    let b_beats: Vec<f64> = b.beats.iter().copied().filter(|t| *t >= req.in_start - 0.05).collect();
    let in_rate = f64::from(req.automation.incoming.rate.first());
    let on_in = follow_beats(&to_mono(inc), &to_mono(&b.audio), rate, &b_beats, r.start_frame as f64 / rate + (b_beats[0] - req.in_start) / in_rate, 1.0 / in_rate, 0.03);
    let times = |v: &[(f64, f64, f64)]| v.iter().filter(|x| x.2 > 0.6).map(|x| x.1).collect::<Vec<_>>();
    // Pairs further apart than half a beat are a lost match, not an error.
    let half = 0.5 * 60.0 / a.bpm;
    let outs = times(&on_out);
    let ins: Vec<f64> = times(&on_in).into_iter().filter(|t| outs.iter().any(|o| (o - t).abs() < half)).collect();
    let (median, p95, n) = alignment_error(&outs, &ins, from, to);
    if std::env::var("BENCH_DEBUG").is_ok() {
    }
    if std::env::var("ALIGN_DEBUG").is_ok() {
        for x in on_out.iter().filter(|x| x.1 >= from - 0.5 && x.1 <= to + 0.5) {
            eprintln!("  out {:.3} at {:.4} corr {:.2}", x.0, x.1, x.2);
        }
        for x in on_in.iter().filter(|x| x.1 >= from - 0.5 && x.1 <= to + 0.5) {
            eprintln!("  in  {:.3} at {:.4} corr {:.2}", x.0, x.1, x.2);
        }
        eprintln!("  window {from:.3}..{to:.3}");
    }
    if std::env::var("BENCH_DEBUG").is_ok() {
        eprintln!("METRIC alignment median {:.2} ms p95 {:.2} ms n {n}", median * 1000.0, p95 * 1000.0);
    }
    (median * 1000.0, p95 * 1000.0, n)
}

#[test]
fn beat_matched_mix_aligns_within_10ms() {
    let (a, b) = (song(124.0, 110.0, 1), song(128.0, 123.5, 7));
    let period = 60.0 / a.bpm;
    let out_start = a.beats[64];
    let seconds = 8.0 * 4.0 * period;
    let req = request(out_start, b.beats[0], seconds, beatmatched(a.bpm / b.bpm, false));
    let (median, p95, n) = alignment_ms(&a, &b, &req);
    assert!(n >= 28, "beats compared: {n}");
    assert!(median < 10.0 && p95 < 10.0, "alignment median {median:.2} ms, p95 {p95:.2} ms");
    // Overlap length is what was asked for.
    let r = render_transition(&a.audio, &b.audio, RATE, &req, false);
    let overlap = (r.end_frame - r.start_frame) as f64 / f64::from(RATE);
    assert!((overlap - seconds).abs() < 0.01, "overlap {overlap}");
    let start = r.out_base + r.start_frame as f64 / f64::from(RATE);
    assert!((start - out_start).abs() < 1.0 / f64::from(RATE) * 2.0, "starts on the planned frame: {start}");
}

#[test]
fn tempo_ramp_on_both_decks_keeps_the_beats_locked() {
    let (a, b) = (song(120.0, 110.0, 3), song(131.0, 98.0, 9));
    let period = 60.0 / a.bpm;
    let req = request(a.beats[64], b.beats[0], 8.0 * 4.0 * period, beatmatched(a.bpm / b.bpm, true));
    let (median, p95, n) = alignment_ms(&a, &b, &req);
    assert!(n >= 24, "beats compared: {n}");
    // Both decks are stretched here, so two WSOLA position errors add up:
    // measured p95 ≈ 12–15 ms. Beat-matched plans therefore stretch only the
    // incoming deck (held through the overlap, eased back afterwards), where
    // the < 10 ms target holds; this bound guards the two-deck capability.
    assert!(median < 10.0 && p95 < 16.0, "ramp alignment median {median:.2} ms, p95 {p95:.2} ms");
}

#[test]
fn level_is_continuous_and_nothing_clips() {
    let (a, b) = (song(124.0, 110.0, 1), song(124.0, 130.8, 5));
    let period = 60.0 / a.bpm;
    let req = request(a.beats[64], b.beats[0], 8.0 * 4.0 * period, beatmatched(1.0, false));
    let r = render_transition(&a.audio, &b.audio, RATE, &req, false);
    let rate = f64::from(RATE);
    let series = loudness_series(&r.mix, rate, 3.0, 0.1);
    let (from, to) = (r.start_frame as f64 / rate - 2.0, r.end_frame as f64 / rate + 2.0);
    let (at, jump) = max_loudness_jump(&series, 1.0, from, to);
    if std::env::var("BENCH_DEBUG").is_ok() {
        eprintln!("METRIC loudness max 1 s jump {jump:+.2} LU; peak {:.3}", peak(&r.mix));
    }
    assert!(jump.abs() <= 2.0, "short-term loudness moves {jump:.2} LU in 1 s at {at:.1} s");
    assert!(peak(&r.mix) < 1.0, "peak {}", peak(&r.mix));
    // No spectral-flux spike at the seam beyond the music's own beats.
    let flux = spectral_flux(&to_mono(&r.mix), rate, 1024, 512);
    let seam = flux_spike_ratio(&flux, r.start_frame as f64 / rate, r.end_frame as f64 / rate);
    let body = flux_spike_ratio(&flux, 0.0, r.start_frame as f64 / rate - 0.1);
    if std::env::var("BENCH_DEBUG").is_ok() {
        eprintln!("METRIC flux spike seam {seam:.2}x vs body {body:.2}x");
    }
    assert!(seam <= body * 1.3, "seam flux {seam:.1}× median vs body {body:.1}×");
}

#[test]
fn loop_seams_are_click_free_and_keep_the_grid() {
    let a = song(124.0, 110.0, 2);
    let b = song(124.0, 110.0, 4);
    let period = 60.0 / a.bpm;
    // Repeat the last 2 bars before beat 96 twice while the mix runs.
    let l0 = a.beats[88];
    let len = 8.0 * period;
    let mut automation = beatmatched(1.0, false);
    automation.out_loop = Some(LoopRegion { start: l0, length: len, repeats: 2 });
    let seconds = 8.0 * 4.0 * period;
    let req = request(a.beats[80], b.beats[0], seconds, automation);
    let r = render_transition(&a.audio, &b.audio, RATE, &req, true);
    let rate = f64::from(RATE);
    // Gains flattened, so a seam and the bar before it differ only by the seam.
    let flat = RenderRequest { automation: flat_gains(req.automation), ..req };
    let out = to_mono(&render_transition(&a.audio, &b.audio, RATE, &flat, true).stems.unwrap()[0]);
    // Seams in output time: the outgoing deck plays at rate 1.
    let first_seam = ((l0 + len - r.out_base) * rate).round() as usize;
    let seams = [first_seam, first_seam + (len * rate).round() as usize];
    // Each seam against the same spot one loop length earlier (same bar
    // parity, no seam there): the high-frequency burst a click would add.
    // Window: the 10 ms crossfade before the seam and 3 ms after.
    let bar = (len * rate).round() as usize;
    let (before, after) = ((0.012 * rate) as usize, (0.003 * rate) as usize);
    for seam in seams {
        let ratio = seam_ratio(&out, seam, bar, before, after);
        if std::env::var("BENCH_DEBUG").is_ok() {
            eprintln!("METRIC loop seam {seam}: hf ratio {ratio:.3}");
        }
        assert!(ratio < 2.0, "loop seam at {seam}: {ratio:.2}× the high-frequency energy of the bar before");
    }
    // The same check flags a hard splice at the same place.
    let mono_a = to_mono(&a.audio);
    let (s0, s1) = ((l0 * rate).round() as usize, ((l0 + len) * rate).round() as usize);
    let mut spliced = mono_a[..s1].to_vec();
    spliced.extend_from_slice(&mono_a[s0 + 1234..]);
    let splice = seam_ratio(&spliced, s1, bar, before, after);
    if std::env::var("BENCH_DEBUG").is_ok() {
        eprintln!("METRIC hard splice hf ratio {splice:.2}");
    }
    assert!(splice > 4.0, "a hard splice reads {splice:.2}");
    // The repeated bars keep the beat grid: the out stem's kicks continue
    // at exact beat spacing through both seams (no gap, no double beat).
    let flat_out = &out;
    // Source beats heard: up to the loop end, then the region twice more.
    let region: Vec<f64> = a.beats.iter().copied().filter(|t| *t >= l0 - 1e-6 && *t < l0 + len - 1e-6).collect();
    let mut heard: Vec<f64> = a.beats.iter().copied().filter(|t| *t >= a.beats[80] && *t < l0 + len - 1e-6).collect();
    let n_before = heard.len();
    heard.extend(region.iter().chain(region.iter()));
    let mono_src = to_mono(&a.audio);
    let mut times = Vec::new();
    let mut guess = a.beats[80] - r.out_base;
    for beat in &heard {
        let from = ((beat * rate) as usize).saturating_sub(441);
        let (offset, corr) = volta_logic::mix_metrics::locate(&mono_src, from, 2205, flat_out, (guess * rate) as i64 - 441, 1323).unwrap();
        assert!(corr > 0.8, "beat {beat} found with corr {corr}");
        times.push((offset + 441) as f64 / rate);
        guess = times.last().unwrap() + period;
    }
    let worst = times.windows(2).map(|w| ((w[1] - w[0]) - period).abs()).fold(0.0, f64::max);
    assert!(times.len() == n_before + 2 * region.len() && worst < 0.001, "grid broken by {worst} s");
}

#[test]
fn filter_sweeps_and_neutral_effects_behave() {
    let (a, b) = (song(124.0, 110.0, 1), song(124.0, 130.8, 5));
    let period = 60.0 / a.bpm;
    let mut automation = TransitionAutomation::equal_power();
    automation.outgoing.lowpass_hz = Envelope::ramp(20_000.0, 300.0, 0.0, 1.0, Shape::Power(0.3));
    automation.incoming.highpass_hz = Envelope::ramp(1_000.0, 20.0, 0.0, 1.0, Shape::Power(2.0));
    let req = request(a.beats[64], b.beats[0], 4.0 * 4.0 * period, automation);
    let r = render_transition(&a.audio, &b.audio, RATE, &req, false);
    let rate = f64::from(RATE);
    let mono = to_mono(&r.mix);
    assert!(peak(&r.mix) < 1.0);
    let before = detect_clicks(&mono[..r.start_frame], rate, 0.005, 0.0).iter().map(|c| c.1).fold(0.0, f64::max);
    let during = detect_clicks(&mono[r.start_frame..r.end_frame + 4410], rate, 0.005, 0.0).iter().map(|c| c.1).fold(0.0, f64::max);
    assert!(during <= before.max(4.0) * 1.2, "sweep clicks {during:.1} vs {before:.1}");
    // Neutral automation with the effects chain engaged changes no level.
    let mut neutral = DeckAutomation::default();
    neutral.low = Envelope::ramp(1.0, 1.0, 0.0, 1.0, Shape::Linear);
    neutral.high = Envelope::points(&[(0.0, 1.0, Shape::Linear), (0.5, 0.999_999, Shape::Linear), (1.0, 1.0, Shape::Linear)]);
    assert!(neutral.uses_bands());
    let silent_in = DeckAutomation { gain: Envelope::constant(0.0), ..Default::default() };
    let automation = TransitionAutomation { outgoing: neutral, incoming: silent_in, ..Default::default() };
    let req = request(a.beats[64], b.beats[0], 4.0 * 4.0 * period, automation);
    let r = render_transition(&a.audio, &b.audio, RATE, &req, false);
    let original = &a.audio[((r.out_base * rate) as usize) * 2..];
    let k = r.start_frame * 2 + 4410 * 2;
    let len = (r.end_frame - r.start_frame) * 2 - 8820;
    let energy = |x: &[f32]| x.iter().map(|v| f64::from(v * v)).sum::<f64>();
    let ratio = energy(&r.mix[k..k + len]) / energy(&original[k..k + len]);
    if std::env::var("BENCH_DEBUG").is_ok() {
        eprintln!("METRIC neutral fx energy ratio {ratio:.4}; sweep click {during:.1} vs before {before:.1}");
    }
    assert!((ratio - 1.0).abs() < 0.03, "neutral chain changes energy by {ratio}");
}

#[test]
fn equal_power_crossfade_renders_and_exports() {
    let (a, b) = (song(124.0, 110.0, 1), song(90.0, 130.8, 5));
    let req = request(40.0, 0.0, 6.0, TransitionAutomation::equal_power());
    let r = render_transition(&a.audio, &b.audio, RATE, &req, false);
    assert!(((r.end_frame - r.start_frame) as f64 / f64::from(RATE) - 6.0).abs() < 0.01);
    let wav = wav_bytes(&r.mix, RATE);
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(wav.len(), 44 + r.mix.len() * 2);
}

/// Analysis → planner → renderer → metrics, with nothing assumed: the
/// plan's cue points come from our own beat tracker, so its errors count.
mod end_to_end {
    use super::*;
    use volta_logic::mix_analysis::analyze_track;
    use volta_logic::mix_planner::{TransitionKind, plan_mix};
    use volta_logic::mix_synth::to_analysis_rate;

    fn planned(a: &SynthSong, b: &SynthSong) -> (volta_logic::mix_planner::MixPlan, RenderRequest) {
        let (pa, pb) = (analyze_track(&to_analysis_rate(a)).unwrap(), analyze_track(&to_analysis_rate(b)).unwrap());
        let plan = plan_mix(&pa, &pb);
        let req = RenderRequest { out_start: plan.out_start, in_start: plan.in_start, seconds: plan.seconds, automation: plan.automation, lead: 6.0, tail: 6.0, out_gain: 1.0, in_gain: 1.0 };
        (plan, req)
    }

    fn report(name: &str, plan: &volta_logic::mix_planner::MixPlan, values: &str) {
        if std::env::var("BENCH_DEBUG").is_ok() {
            eprintln!("METRIC {name}: {} {} bars {:.2} s ({}) {values}", plan.kind.id(), plan.bars, plan.seconds, plan.reason);
        }
    }

    fn level_and_peak(a: &SynthSong, b: &SynthSong, req: &RenderRequest) -> (f64, f32) {
        let r = render_transition(&a.audio, &b.audio, RATE, req, false);
        let rate = f64::from(RATE);
        let series = loudness_series(&r.mix, rate, 3.0, 0.1);
        let (_, jump) = max_loudness_jump(&series, 1.0, r.start_frame as f64 / rate - 2.0, r.end_frame as f64 / rate + 2.0);
        (jump, peak(&r.mix))
    }

    #[test]
    fn planned_beat_matched_mix_is_aligned_level_and_clean() {
        let a = synth_song(RATE, SynthOptions { bpm: 124.0, seconds: 96.0, root_hz: 110.0, seed: 3, ..Default::default() });
        // No level step of its own near the seam: the loudness check must
        // measure the transition, not the song (an intro that is quieter
        // than the rest would end right at the overlap's end).
        let b = synth_song(RATE, SynthOptions { bpm: 127.0, seconds: 96.0, root_hz: 110.0, seed: 8, ..Default::default() });
        let (plan, req) = planned(&a, &b);
        assert!(matches!(plan.kind, TransitionKind::BeatMatched | TransitionKind::Extend), "{plan:#?}");
        if std::env::var("BENCH_DEBUG").is_ok() {
            let pb = analyze_track(&to_analysis_rate(&b)).unwrap();
            let near = |t: f64, beats: &[f64]| beats.iter().map(|u| t - u).min_by(|x, y| x.abs().total_cmp(&y.abs())).unwrap();
            eprintln!("out_start {:.4} (err {:+.1} ms, on bar {:?}) in_start {:.4} (err {:+.1} ms) b phase {} first beats {:?}", plan.out_start, near(plan.out_start, &a.beats) * 1000.0, a.beats.iter().position(|u| (u - plan.out_start).abs() < 0.05).map(|i| i % 4), plan.in_start, near(plan.in_start, &b.beats) * 1000.0, pb.downbeat_phase, &pb.beats[..5]);
        }
        let (median, p95, n) = alignment_ms(&a, &b, &req);
        let (jump, top) = level_and_peak(&a, &b, &req);
        report("planned beat-matched", &plan, &format!("alignment median {median:.2} ms p95 {p95:.2} ms n {n}; loudness jump {jump:+.2} LU; peak {top:.3}"));
        assert!(n >= 4 * plan.bars as usize - 4, "beats compared {n}");
        assert!(median < 10.0 && p95 < 10.0, "alignment median {median:.2} p95 {p95:.2}");
        assert!(jump.abs() <= 2.0, "loudness jump {jump:.2}");
        assert!(top < 1.0);
        // The incoming song starts at its first downbeat: nothing skipped.
        assert!(plan.in_start - b.start < 60.0 / b.bpm, "intro skipped: {}", plan.in_start);
    }

    #[test]
    fn planned_extension_loops_cleanly_at_the_end() {
        let a = synth_song(RATE, SynthOptions { bpm: 124.0, seconds: 96.0, root_hz: 110.0, seed: 3, ..Default::default() });
        let b = synth_song(RATE, SynthOptions { bpm: 124.0, seconds: 96.0, root_hz: 110.0, seed: 8, ..Default::default() });
        let (pa, pb) = (analyze_track(&to_analysis_rate(&a)).unwrap(), analyze_track(&to_analysis_rate(&b)).unwrap());
        let config = volta_logic::mix_planner::PlannerConfig::default();
        let plan = volta_logic::mix_planner::plan_transition(&pa, &pb, &Default::default(), &config);
        let candidates: Vec<_> = plan.candidates.iter().map(|c| (c.kind, c.score)).collect();
        // Build the extension candidate explicitly (it competes with a plain
        // beat-matched mix, which may score higher here).
        let extended = volta_logic::mix_planner::plan_transition(&pa, &pb, &Default::default(), &volta_logic::mix_planner::PlannerConfig { score_beatmatch: 0.0, ..config });
        assert_eq!(extended.kind, TransitionKind::Extend, "{candidates:?}");
        let req = RenderRequest { out_start: extended.out_start, in_start: extended.in_start, seconds: extended.seconds, automation: extended.automation, lead: 6.0, tail: 6.0, out_gain: 1.0, in_gain: 1.0 };
        let (median, p95, n) = alignment_ms(&a, &b, &req);
        let (jump, top) = level_and_peak(&a, &b, &req);
        report("planned extend", &extended, &format!("alignment median {median:.2} ms p95 {p95:.2} ms n {n}; loudness jump {jump:+.2} LU; peak {top:.3}"));
        assert!(n >= 4 * extended.bars as usize - 4, "beats compared {n}");
        assert!(median < 10.0 && p95 < 10.0, "alignment median {median:.2} p95 {p95:.2}");
        assert!(jump.abs() <= 2.0 && top < 1.0, "jump {jump} peak {top}");
        // It ends with the song: nothing of the outgoing audio left unheard
        // beyond the final bar.
        let region = extended.automation.out_loop.unwrap();
        assert!(a.end - (region.start + region.length) < 4.0 * 60.0 / a.bpm + 0.1);
        assert!(region.start + region.length <= a.end, "loop ends inside the file");
    }

    #[test]
    fn planned_crossfade_between_unmatched_tempos_is_level() {
        let a = synth_song(RATE, SynthOptions { bpm: 124.0, seconds: 96.0, root_hz: 110.0, seed: 3, fade_out: 12.0, ..Default::default() });
        let b = synth_song(RATE, SynthOptions { bpm: 92.0, seconds: 96.0, root_hz: 130.8, seed: 8, ..Default::default() });
        let (plan, req) = planned(&a, &b);
        assert_eq!(plan.kind, TransitionKind::EnergyCrossfade, "{plan:#?}");
        let (jump, top) = level_and_peak(&a, &b, &req);
        report("planned crossfade", &plan, &format!("loudness jump {jump:+.2} LU; peak {top:.3}"));
        // The outgoing song is already fading: its fade is long and late.
        assert!(plan.seconds > 6.0, "{}", plan.seconds);
        assert!(top < 1.0);
        // Incoming at full level while the outgoing has faded: allow the
        // rise of the second song's own entrance, but no jump beyond 6 LU.
        assert!(jump.abs() <= 6.0, "loudness jump {jump:.2}");
    }
}

#[test]
#[ignore]
fn debug_analysis_accuracy() {
    use volta_logic::mix_analysis::analyze_track;
    use volta_logic::mix_synth::to_analysis_rate;
    for (bpm, intro) in [(124.0, 0usize), (127.0, 8)] {
        let s = synth_song(RATE, SynthOptions { bpm, seconds: 96.0, root_hz: 110.0, seed: 3, intro_bars: intro, intro_gain: 0.6, ..Default::default() });
        let p = analyze_track(&to_analysis_rate(&s)).unwrap();
        let errs: Vec<f64> = p.beats.iter().map(|t| s.beats.iter().map(|u| t - u).min_by(|x, y| x.abs().total_cmp(&y.abs())).unwrap()).collect();
        let mean = errs.iter().sum::<f64>() / errs.len() as f64;
        let refined: Vec<f64> = (0..p.beats.len()).map(|i| volta_logic::mix_planner::grid_beat(&p.beats, i)).collect();
        let rerrs: Vec<f64> = refined.iter().map(|t| s.beats.iter().map(|u| t - u).min_by(|x, y| x.abs().total_cmp(&y.abs())).unwrap()).collect();
        let rmean = rerrs.iter().sum::<f64>() / rerrs.len() as f64;
        let rmax = rerrs.iter().fold(0f64, |m, e| m.max(e.abs()));
        eprintln!("truth {bpm} analysed {:.4} conf {:.2} phase {} beats {} (truth {}) mean err {:+.1} ms, refined mean {:+.1} max {:.1} ms; first {:.3} truth first {:.3}", p.bpm, p.beat_confidence, p.downbeat_phase, p.beats.len(), s.beats.len(), mean * 1000.0, rmean * 1000.0, rmax * 1000.0, p.beats[0], s.beats[0]);
    }
}

#[test]
#[ignore]
fn debug_extend_inputs() {
    use volta_logic::mix_analysis::analyze_track;
    use volta_logic::mix_synth::to_analysis_rate;
    let a = synth_song(RATE, SynthOptions { bpm: 124.0, seconds: 96.0, root_hz: 110.0, seed: 3, ..Default::default() });
    let p = analyze_track(&to_analysis_rate(&a)).unwrap();
    let n = p.bar_energy.len();
    eprintln!("downbeat conf {:.2} end {:.3} duration {:.3} last beats {:?} bar_energy tail {:?} loudness {:.1} bars {}", p.downbeat_confidence, p.end, p.duration, &p.beats[p.beats.len() - 5..], &p.bar_energy[n - 4..], p.loudness_db, n);
}
