//! Pure ports of the Volta web player's logic. No gpui, no I/O.
//!
//! Each module names the TypeScript file it ports. Tests reuse the inputs and
//! expected values from the Playwright suite in `tests/`.

pub mod folder_tree;
pub mod audio_analysis;
pub mod automix;
pub mod format;
pub mod infinite_play;
pub mod interactions;
pub mod js;
pub mod learned_ranker;
pub mod library;
pub mod listening_history;
pub mod listening_stats;
pub mod local_music;
pub mod lyrics;
pub mod mix_analysis;
pub mod mix_automation;
pub mod mix_metrics;
pub mod mix_synth;
pub mod mix_planner;
pub mod mix_planner_legacy;
pub mod model;
pub mod musical_key;
pub mod recommendation_tuning;
pub mod recommendations;
pub mod server;
pub mod share_links;
pub mod shortcuts;
pub mod song_credits;
pub mod storage;
pub mod update;
