//! Port of `src/lib/recommendation-tuning.ts`. Stored under
//! `volta-recommendation-tuning` as `{version: 1, …dials}`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const RECOMMENDATION_TUNING_VERSION: u32 = 1;
pub const RECOMMENDATION_TUNING_KEY: &str = "volta-recommendation-tuning";
pub const MIN_SLATE_SIZE: f64 = 4.0;
pub const MAX_SLATE_SIZE: f64 = 16.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecommendationTuning {
    pub version: u32,
    pub adventurousness: f64,
    pub familiarity: f64,
    pub learning: f64,
    pub diversity: f64,
    pub recency: f64,
    pub freshness: f64,
    pub caution: f64,
    pub slate_size: f64,
}

impl Default for RecommendationTuning {
    fn default() -> Self {
        Self {
            version: RECOMMENDATION_TUNING_VERSION,
            adventurousness: 0.45,
            familiarity: 0.5,
            learning: 0.6,
            diversity: 0.55,
            recency: 0.5,
            freshness: 0.45,
            caution: 0.6,
            slate_size: 8.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub tuning: RecommendationTuning,
}

#[allow(clippy::too_many_arguments)]
const fn t(a: f64, f: f64, l: f64, d: f64, r: f64, fr: f64, c: f64, s: f64) -> RecommendationTuning {
    RecommendationTuning {
        version: RECOMMENDATION_TUNING_VERSION,
        adventurousness: a,
        familiarity: f,
        learning: l,
        diversity: d,
        recency: r,
        freshness: fr,
        caution: c,
        slate_size: s,
    }
}

pub const PRESETS: [Preset; 4] = [
    Preset {
        id: "safe",
        label: "Safe",
        description: "Mostly records you already play, with the odd adjacent pick.",
        tuning: t(0.1, 0.9, 0.4, 0.3, 0.6, 0.3, 0.9, 8.0),
    },
    Preset {
        id: "balanced",
        label: "Balanced",
        description: "A couple of records you know alongside things you have not played.",
        tuning: t(0.45, 0.5, 0.6, 0.55, 0.5, 0.45, 0.6, 8.0),
    },
    Preset {
        id: "adventurous",
        label: "Adventurous",
        description: "Drops records you have worn out and leads with what you have not heard.",
        tuning: t(0.8, 0.35, 0.7, 0.75, 0.45, 0.6, 0.35, 10.0),
    },
    Preset {
        id: "explorer",
        label: "Explorer",
        description: "Prioritizes overlooked and lightly played albums, with familiar fallback in small libraries.",
        tuning: t(1.0, 0.15, 0.85, 0.9, 0.35, 0.75, 0.2, 12.0),
    },
];

fn clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

fn round6(v: f64) -> f64 {
    crate::js::js_round(v * 1e6) / 1e6
}

fn finite_or(value: Option<&Value>, fallback: f64) -> f64 {
    value.and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(fallback)
}

/// `normalizeRecommendationTuning` over arbitrary JSON.
pub fn normalize_value(value: &Value) -> RecommendationTuning {
    let d = RecommendationTuning::default();
    let g = |k: &str, f: f64| clamp01(finite_or(value.get(k), f));
    RecommendationTuning {
        version: RECOMMENDATION_TUNING_VERSION,
        adventurousness: g("adventurousness", d.adventurousness),
        familiarity: g("familiarity", d.familiarity),
        learning: g("learning", d.learning),
        diversity: g("diversity", d.diversity),
        recency: g("recency", d.recency),
        freshness: g("freshness", d.freshness),
        caution: g("caution", d.caution),
        slate_size: crate::js::js_round(finite_or(value.get("slateSize"), d.slate_size).clamp(MIN_SLATE_SIZE, MAX_SLATE_SIZE)),
    }
}

pub fn normalize(tuning: &RecommendationTuning) -> RecommendationTuning {
    normalize_value(&serde_json::to_value(tuning).unwrap_or(Value::Null))
}

/// `readRecommendationTuning` from the raw stored string.
pub fn read_tuning(raw: Option<&str>) -> RecommendationTuning {
    let Some(value) = raw.and_then(|r| serde_json::from_str::<Value>(r).ok()) else { return RecommendationTuning::default() };
    if value.get("version").and_then(Value::as_u64) != Some(RECOMMENDATION_TUNING_VERSION as u64) {
        return RecommendationTuning::default();
    }
    normalize_value(&value)
}

/// `writeRecommendationTuning`: the normalized tuning and its JSON.
pub fn write_tuning(tuning: &RecommendationTuning) -> (RecommendationTuning, String) {
    let n = normalize(tuning);
    (n, serde_json::to_string(&n).unwrap_or_default())
}

pub fn match_preset(tuning: &RecommendationTuning) -> Option<&'static Preset> {
    let n = normalize(tuning);
    PRESETS.iter().find(|p| normalize(&p.tuning) == n)
}

/// `describeRecommendationTuning`: (label, risk percent).
pub fn describe(tuning: &RecommendationTuning) -> (&'static str, u32) {
    let risk = crate::js::js_round(clamp01(tuning.adventurousness * 0.6 + (1.0 - tuning.familiarity) * 0.25 + tuning.diversity * 0.15) * 100.0) as u32;
    let label = match_preset(tuning).map(|p| p.label).unwrap_or(match risk {
        75.. => "Explorer",
        55.. => "Adventurous",
        30.. => "Balanced",
        _ => "Safe",
    });
    (label, risk)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weights {
    pub artist: f64,
    pub genre: f64,
    pub album: f64,
    pub frequent: f64,
    pub favorite: f64,
    pub history: f64,
    pub engagement: f64,
    pub cooccurrence: f64,
    pub loyalty: f64,
    pub replay: f64,
    pub rediscovery: f64,
    pub era: f64,
    pub recent: f64,
    pub recent_penalty: f64,
    pub freshness: f64,
    pub skip_penalty: f64,
    pub aversion: f64,
    pub mute_penalty: f64,
    pub learned: f64,
    pub diversity: f64,
}

/// `recommendationTuningWeights`.
pub fn weights(tuning: &RecommendationTuning) -> Weights {
    let adventure = clamp01(tuning.adventurousness);
    let familiarity = clamp01(tuning.familiarity);
    let learning = clamp01(tuning.learning);
    let diversity = clamp01(tuning.diversity);
    let recency = clamp01(tuning.recency);
    let freshness = clamp01(tuning.freshness);
    let caution = clamp01(tuning.caution);
    let fam = round6(0.35 + familiarity * 1.3);
    Weights {
        artist: fam,
        genre: fam,
        album: fam,
        frequent: fam,
        favorite: round6(0.6 + familiarity * 0.8),
        history: fam,
        engagement: round6(0.5 + familiarity),
        cooccurrence: round6(0.4 + familiarity * 1.2),
        loyalty: fam,
        replay: fam,
        rediscovery: round6(0.5 + familiarity),
        era: round6(0.5 + adventure * (10.0 / 9.0)),
        recent: round6(0.3 + recency * 1.4),
        recent_penalty: round6(0.3 + recency * 1.4),
        freshness: round6(0.3 + freshness * (14.0 / 9.0)),
        skip_penalty: round6(0.3 + caution * (7.0 / 6.0)),
        aversion: round6(0.3 + caution * (7.0 / 6.0)),
        mute_penalty: round6(0.4 + caution),
        learned: round6(learning * (5.0 / 3.0)),
        diversity: round6(0.3 + diversity * (14.0 / 11.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_presets_and_weights() {
        assert_eq!(read_tuning(None), RecommendationTuning::default());
        assert_eq!(read_tuning(Some("{\"version\":1,\"slateSize\":99,\"caution\":-3}")).slate_size, 16.0);
        assert_eq!(read_tuning(Some("{\"version\":1,\"caution\":-3}")).caution, 0.0);
        assert_eq!(match_preset(&RecommendationTuning::default()).unwrap().id, "balanced");
        assert_eq!(describe(&PRESETS[3].tuning).0, "Explorer");
        let w = weights(&RecommendationTuning::default());
        assert_eq!(w.artist, 1.0);
        assert_eq!(w.learned, 1.0);
    }
}
