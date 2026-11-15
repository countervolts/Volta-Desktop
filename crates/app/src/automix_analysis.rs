//! Background analysis for the native AutoMix: the whole current track and
//! the next two are decoded and profiled (beats, downbeats, key, loudness,
//! audible span) one at a time. Profiles are stored per track under
//! `volta-mix-profile:v3:<hash>` with a 600-entry LRU index. Local files are
//! analysed from disk too.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{AppContext, Context};
use serde_json::Value;
use volta_logic::mix_analysis::{MixProfile, PROFILE_VERSION, analyze_track};
use volta_logic::model::Song;

use crate::app_state::AppState;
use crate::player::Transition;

const INDEX_KEY: &str = "volta-mix-profile:v3:index";
const CACHE_LIMIT: usize = 600;
const MAX_ANALYSIS_BYTES: f64 = 256.0 * 1024.0 * 1024.0;

#[derive(Default)]
pub struct Analyzer {
    in_flight: HashSet<String>,
    failed_until: HashMap<String, Instant>,
    queue: Vec<(String, Song)>,
    active: bool,
    signature: String,
}

/// JSON number text as JavaScript writes it (`60`, not `60.0`).
fn js_number(value: Option<f64>) -> Value {
    match value {
        Some(v) if v.is_finite() && v.fract() == 0.0 && v.abs() < 9e15 => Value::from(v as i64),
        Some(v) if v.is_finite() => Value::from(v),
        _ => Value::Null,
    }
}

/// Identity of the audio a profile describes.
pub fn analysis_key(server: &str, username: &str, song: &Song, original: bool) -> String {
    if song.is_local() {
        return serde_json::json!(["local", song.local_file_path, js_number(song.size), js_number(song.local_modified)]).to_string();
    }
    serde_json::json!([server, username, song.id, js_number(song.duration), js_number(song.size), original]).to_string()
}

fn storage_key(key: &str) -> String {
    let hash = key.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3));
    format!("volta-mix-profile:v3:{hash:016x}")
}

impl AppState {
    fn read_profile(&self, key: &str) -> Option<MixProfile> {
        serde_json::from_str::<MixProfile>(&self.storage.get(&storage_key(key))).ok().filter(|p| p.version == PROFILE_VERSION && p.bpm > 0.0)
    }

    fn store_profile(&mut self, key: String, profile: MixProfile, cx: &mut Context<Self>) {
        let slot = storage_key(&key);
        self.storage.set(&slot, &serde_json::to_string(&profile).unwrap_or_default());
        let mut index: Vec<(String, f64)> = serde_json::from_str(&self.storage.get(INDEX_KEY)).unwrap_or_default();
        index.retain(|(k, _)| *k != slot);
        index.push((slot, volta_logic::listening_history::now_ms()));
        while index.len() > CACHE_LIMIT {
            let (old, _) = index.remove(0);
            self.storage.remove(&old);
        }
        self.storage.set(INDEX_KEY, &serde_json::to_string(&index).unwrap_or_default());
        self.player.update(cx, |p, _| {
            p.profiles.insert(key, profile);
        });
    }

    /// Profiles the current track and the next two while AutoMix is on.
    pub fn prime_automix_analysis(&mut self, cx: &mut Context<Self>) {
        let player = self.player.read(cx);
        if player.transition != Transition::Automix {
            return;
        }
        let Some(index) = player.index else { return };
        let (server, username) = self.client().map(|c| (c.server.clone(), c.username.clone())).unwrap_or_default();
        let original = player.original;
        let wanted: Vec<(String, Song)> =
            player.queue.iter().skip(index).take(3).map(|s| (analysis_key(&server, &username, s, original), s.clone())).collect();
        let signature = wanted.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join("|");
        if signature == self.analyzer.signature {
            return;
        }
        self.analyzer.signature = signature;
        let keys: HashSet<&String> = wanted.iter().map(|(k, _)| k).collect();
        let dropped: Vec<String> = self.analyzer.queue.iter().filter(|(k, _)| !keys.contains(k)).map(|(k, _)| k.clone()).collect();
        self.analyzer.queue.retain(|(k, _)| keys.contains(k));
        for key in dropped {
            self.analyzer.in_flight.remove(&key);
        }
        let now = Instant::now();
        for (key, song) in wanted {
            if self.player.read(cx).profiles.contains_key(&key) {
                continue;
            }
            if let Some(profile) = self.read_profile(&key) {
                self.player.update(cx, |p, _| {
                    p.profiles.insert(key, profile);
                });
                continue;
            }
            if self.analyzer.in_flight.contains(&key) || self.analyzer.failed_until.get(&key).is_some_and(|until| *until > now) {
                continue;
            }
            self.analyzer.in_flight.insert(key.clone());
            self.analyzer.queue.push((key, song));
        }
        self.pump_analysis(cx);
    }

    fn pump_analysis(&mut self, cx: &mut Context<Self>) {
        if self.analyzer.active || self.analyzer.queue.is_empty() {
            return;
        }
        let (key, song) = self.analyzer.queue.remove(0);
        let original = self.player.read(cx).original;
        let source = if song.is_local() {
            song.local_file_path.clone().map(|p| volta_audio::TrackSource::File(p.into()))
        } else {
            self.client().map(|c| volta_audio::TrackSource::Url(c.stream_url(&song.id, original)))
        };
        let Some(source) = source else {
            self.analyzer.in_flight.remove(&key);
            return;
        };
        self.analyzer.active = true;
        let extension = if original || song.is_local() { song.suffix.clone() } else { Some("mp3".into()) };
        let task = cx.background_spawn(async move {
            if song.size.unwrap_or(0.0) > MAX_ANALYSIS_BYTES {
                return None;
            }
            let mono = volta_audio::decode_mono_11k(&source, extension.as_deref(), &volta_audio::http_client())?;
            analyze_track(&mono)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| {
                state.analyzer.active = false;
                state.analyzer.in_flight.remove(&key);
                match result {
                    Some(profile) => state.store_profile(key, profile, cx),
                    None => {
                        state.analyzer.failed_until.insert(key, Instant::now() + Duration::from_secs(5 * 60));
                    }
                }
                state.pump_analysis(cx);
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &std::path::Path, bpm: f64, seconds: f64) {
        let rate = 44100u32;
        let n = (seconds * f64::from(rate)) as usize;
        let beat = 60.0 / bpm;
        let mut data = Vec::with_capacity(n * 2);
        for i in 0..n {
            let t = i as f64 / f64::from(rate);
            let in_beat = t % beat;
            let accent = if ((t / beat).floor() as i64) % 4 == 0 { 1.0 } else { 0.55 };
            let kick = if in_beat < 0.12 { (2.0 * std::f64::consts::PI * (60.0 - in_beat * 200.0) * in_beat).sin() * (1.0 - in_beat / 0.12) * accent } else { 0.0 };
            let pad = (2.0 * std::f64::consts::PI * 220.0 * t).sin() * 0.06;
            data.extend_from_slice(&(((kick * 0.8 + pad) * 30000.0) as i16).to_le_bytes());
        }
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend(data);
        std::fs::write(path, wav).unwrap();
    }

    #[test]
    fn profiles_a_decoded_file() {
        let path = std::env::temp_dir().join(format!("volta-mix-{}.wav", std::process::id()));
        write_wav(&path, 126.0, 60.0);
        let mono = volta_audio::decode_mono_11k(&volta_audio::TrackSource::File(path.clone()), Some("wav"), &volta_audio::http_client()).expect("decoded");
        assert!((mono.len() as f64 / 11025.0 - 60.0).abs() < 0.1);
        let profile = analyze_track(&mono).expect("profile");
        assert!((profile.bpm - 126.0).abs() < 1.5, "bpm {}", profile.bpm);
        assert!(profile.beat_confidence > 0.5, "confidence {}", profile.beat_confidence);
        std::fs::remove_file(path).ok();
        assert_eq!(
            analysis_key("https://m", "me", &Song { id: "s".into(), duration: Some(60.0), ..Default::default() }, true),
            r#"["https://m","me","s",60,null,true]"#
        );
    }
}
