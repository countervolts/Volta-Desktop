//! Port of `src/lib/learned-ranker.ts`: on-device logistic re-ranker trained
//! from recommendation impressions and the actions that follow them.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::interactions::FeatureVector;
use crate::js::encode_uri_component;
use crate::listening_history::{EventKind, now_ms};

pub const RANKER_MODEL_VERSION: u32 = 2;
pub const IMPRESSION_LOG_VERSION: u32 = 2;
const MAX_IMPRESSIONS: usize = 400;
const MAX_IMPRESSION_AGE_MS: f64 = 5.0 * 24.0 * 60.0 * 60.0 * 1000.0;
const ATTRIBUTION_WINDOW_MS: f64 = 6.0 * 60.0 * 60.0 * 1000.0;
const MAX_WEIGHT: f64 = 4.0;
const L2: f64 = 0.02;
const LEARNING_RATE: f64 = 0.12;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankerModel {
    pub version: u32,
    pub weights: IndexMap<String, f64>,
    pub bias: f64,
    pub samples: u64,
    pub positives: u64,
    pub updated_at: f64,
}

impl Default for RankerModel {
    fn default() -> Self {
        Self { version: RANKER_MODEL_VERSION, weights: IndexMap::new(), bias: 0.0, samples: 0, positives: 0, updated_at: 0.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingImpression {
    pub id: String,
    pub at: f64,
    pub album_id: String,
    pub artist_id: String,
    pub genres: Vec<String>,
    pub features: FeatureVector,
    pub position: f64,
}

#[derive(Debug, Clone)]
pub struct RankerExample {
    pub features: FeatureVector,
    pub reward: f64,
}

#[derive(Debug, Clone, Default)]
pub struct RewardTarget {
    pub album_id: Option<String>,
    pub artist_id: Option<String>,
}

pub fn ranker_model_storage_key(server: &str, username: &str) -> String {
    format!("volta-ranker-model:v1:{}:{}", encode_uri_component(server), encode_uri_component(username))
}

pub fn impression_storage_key(server: &str, username: &str) -> String {
    format!("volta-ranker-impressions:v1:{}:{}", encode_uri_component(server), encode_uri_component(username))
}

fn sigmoid(v: f64) -> f64 {
    1.0 / (1.0 + (-v.clamp(-30.0, 30.0)).exp())
}

fn finite_record(v: Option<&Value>) -> bool {
    v.and_then(Value::as_object).is_some_and(|o| o.values().all(|e| e.as_f64().is_some_and(f64::is_finite)))
}

/// `readRankerModel`.
pub fn read_ranker_model(raw: Option<&str>) -> RankerModel {
    let Some(v) = raw.and_then(|r| serde_json::from_str::<Value>(r).ok()) else { return RankerModel::default() };
    let bias = v.get("bias").and_then(Value::as_f64);
    if v.get("version").and_then(Value::as_u64) != Some(RANKER_MODEL_VERSION as u64) || !finite_record(v.get("weights")) || !bias.is_some_and(f64::is_finite) {
        return RankerModel::default();
    }
    let num = |k: &str| v.get(k).and_then(Value::as_f64).filter(|n| n.is_finite()).unwrap_or(0.0);
    RankerModel {
        version: RANKER_MODEL_VERSION,
        weights: v["weights"].as_object().unwrap().iter().map(|(k, x)| (k.clone(), x.as_f64().unwrap())).collect(),
        bias: bias.unwrap().clamp(-4.0, 4.0),
        samples: num("samples").trunc().max(0.0) as u64,
        positives: num("positives").trunc().max(0.0) as u64,
        updated_at: num("updatedAt"),
    }
}

pub fn write_ranker_model(model: &RankerModel) -> String {
    serde_json::to_string(model).unwrap_or_default()
}

/// `readImpressions`.
pub fn read_impressions(raw: Option<&str>, now: f64) -> Vec<PendingImpression> {
    let Some(v) = raw.and_then(|r| serde_json::from_str::<Value>(r).ok()) else { return Vec::new() };
    if v.get("version").and_then(Value::as_u64) != Some(IMPRESSION_LOG_VERSION as u64) {
        return Vec::new();
    }
    let Some(items) = v.get("items").and_then(Value::as_array) else { return Vec::new() };
    let list: Vec<PendingImpression> = items
        .iter()
        .filter(|i| finite_record(i.get("features")))
        .filter_map(|i| serde_json::from_value::<PendingImpression>(i.clone()).ok())
        .filter(|i| i.at.is_finite() && now - i.at <= MAX_IMPRESSION_AGE_MS)
        .collect();
    let skip = list.len().saturating_sub(MAX_IMPRESSIONS);
    list.into_iter().skip(skip).collect()
}

/// `writeImpressions`: the kept items and their JSON.
pub fn write_impressions(items: Vec<PendingImpression>, now: f64) -> (Vec<PendingImpression>, String) {
    let list: Vec<PendingImpression> = items.into_iter().filter(|i| now - i.at <= MAX_IMPRESSION_AGE_MS).collect();
    let skip = list.len().saturating_sub(MAX_IMPRESSIONS);
    let next: Vec<PendingImpression> = list.into_iter().skip(skip).collect();
    let json = serde_json::json!({"version": IMPRESSION_LOG_VERSION, "items": next}).to_string();
    (next, json)
}

/// `recordImpressions`: `None` when nothing new was added.
pub fn record_impressions(existing: Vec<PendingImpression>, impressions: &[PendingImpression], now: f64) -> Option<(Vec<PendingImpression>, String)> {
    let known: std::collections::HashSet<&str> = existing.iter().map(|i| i.id.as_str()).collect();
    let additions: Vec<PendingImpression> = impressions.iter().filter(|i| !known.contains(i.id.as_str())).cloned().collect();
    if additions.is_empty() {
        return None;
    }
    Some(write_impressions(existing.into_iter().chain(additions).collect(), now))
}

/// `attributeReward`.
pub fn attribute_reward(items: &[PendingImpression], target: &RewardTarget, reward: f64, now: f64) -> (Vec<RankerExample>, Vec<PendingImpression>) {
    if items.is_empty() || reward == 0.0 {
        return (Vec::new(), items.to_vec());
    }
    let album = target.album_id.as_deref().unwrap_or("").trim().to_lowercase();
    let artist = target.artist_id.as_deref().unwrap_or("").trim().to_lowercase();
    let (mut best_index, mut best_match): (Option<usize>, f64) = (None, 0.0);
    for (index, item) in items.iter().enumerate() {
        if item.at > now || now - item.at > ATTRIBUTION_WINDOW_MS {
            continue;
        }
        let m = if !album.is_empty() && item.album_id == album {
            1.0
        } else if album.is_empty() && !artist.is_empty() && item.artist_id == artist {
            0.45
        } else {
            0.0
        };
        if m > best_match || (m > 0.0 && m == best_match && best_index.is_some_and(|b| item.at > items[b].at)) {
            best_match = m;
            best_index = Some(index);
        }
    }
    let Some(best) = best_index.filter(|_| best_match > 0.0) else { return (Vec::new(), items.to_vec()) };
    let matched = &items[best];
    let examples = vec![RankerExample { features: matched.features.clone(), reward: (reward * best_match).clamp(-1.0, 1.0) }];
    let remaining = items.iter().filter(|i| i.album_id != matched.album_id).cloned().collect();
    (examples, remaining)
}

fn usable(feature: &str, value: f64) -> bool {
    feature != "learned" && value.is_finite() && value != 0.0
}

/// `trainRanker`.
pub fn train_ranker(model: &RankerModel, examples: &[RankerExample], passes: usize) -> RankerModel {
    if examples.is_empty() {
        return model.clone();
    }
    let mut weights = model.weights.clone();
    let mut bias = model.bias;
    for _ in 0..passes.max(1) {
        for example in examples {
            let target = ((example.reward + 1.0) / 2.0).clamp(0.0, 1.0);
            let mut logit = bias;
            for (f, &v) in &example.features {
                if usable(f, v) {
                    logit += weights.get(f).copied().unwrap_or(0.0) * v;
                }
            }
            let error = target - sigmoid(logit);
            bias = (bias + LEARNING_RATE * error).clamp(-4.0, 4.0);
            for (f, &v) in &example.features {
                if !usable(f, v) {
                    continue;
                }
                let current = weights.get(f).copied().unwrap_or(0.0);
                let gradient = error * v - L2 * current;
                weights.insert(f.clone(), (current + LEARNING_RATE * gradient).clamp(-MAX_WEIGHT, MAX_WEIGHT));
            }
        }
    }
    RankerModel {
        version: RANKER_MODEL_VERSION,
        weights,
        bias,
        samples: model.samples + examples.len() as u64,
        positives: model.positives + examples.iter().filter(|e| e.reward > 0.0).count() as u64,
        updated_at: now_ms(),
    }
}

pub fn ranker_confidence(model: Option<&RankerModel>) -> f64 {
    match model {
        Some(m) if m.samples > 0 => (m.samples as f64 / 80.0).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

pub fn predict_ranker(model: Option<&RankerModel>, features: &FeatureVector) -> f64 {
    let Some(m) = model.filter(|m| m.samples > 0) else { return 0.5 };
    let mut logit = m.bias;
    for (f, &v) in features {
        if usable(f, v) {
            logit += m.weights.get(f).copied().unwrap_or(0.0) * v;
        }
    }
    sigmoid(logit)
}

pub fn blend_learned_score(prior: f64, model: Option<&RankerModel>, features: &FeatureVector) -> f64 {
    let confidence = ranker_confidence(model);
    if confidence == 0.0 {
        return prior;
    }
    prior + (predict_ranker(model, features) - 0.5) * 0.6 * confidence
}

/// `playbackRecommendationReward`.
pub fn playback_reward(kind: EventKind, completion_ratio: f64) -> f64 {
    match kind {
        EventKind::Complete => 0.9,
        EventKind::Skip if completion_ratio >= 0.8 => 0.3,
        EventKind::Skip if completion_ratio <= 0.2 => -0.8,
        EventKind::Skip => -0.15,
        EventKind::Stop if completion_ratio > 0.5 => 0.2,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imp(id: &str, album: &str, at: f64) -> PendingImpression {
        PendingImpression {
            id: id.into(),
            at,
            album_id: album.into(),
            artist_id: "x".into(),
            genres: vec![],
            features: [("artist".to_string(), 1.0)].into_iter().collect(),
            position: 0.0,
        }
    }

    #[test]
    fn attribution_and_training() {
        let now = 1_800_000_000_000.0;
        let items = vec![imp("1", "a", now - 1000.0), imp("2", "a", now - 10.0), imp("3", "b", now)];
        let (ex, rest) = attribute_reward(&items, &RewardTarget { album_id: Some("A".into()), artist_id: None }, 0.9, now);
        assert_eq!(ex.len(), 1);
        assert_eq!(rest.len(), 1);
        let model = train_ranker(&RankerModel::default(), &ex, 3);
        assert_eq!(model.samples, 1);
        assert!(model.weights["artist"] > 0.0);
        assert!(predict_ranker(Some(&model), &ex[0].features) > 0.5);
        let (_, json) = write_impressions(items, now);
        assert_eq!(read_impressions(Some(&json), now).len(), 3);
        assert_eq!(read_ranker_model(Some(&write_ranker_model(&model))), model);
    }
}
