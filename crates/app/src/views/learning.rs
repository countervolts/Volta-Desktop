//! The two learning dialogs: `ListeningHistoryView` (app-views.tsx) and
//! `RecommendationEngineDialog` (recommendation-engine-dialog.tsx).

use std::collections::HashSet;

use chrono::{Local, TimeZone};
use gpui::{AnyElement, App, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, div, prelude::*};
use volta_logic::format::duration;
use volta_logic::interactions::EngagementKind;
use volta_logic::learned_ranker::ranker_confidence;
use volta_logic::listening_history::EventKind;
use volta_logic::recommendation_tuning::{MAX_SLATE_SIZE, MIN_SLATE_SIZE, PRESETS, RecommendationTuning, describe, match_preset};
use volta_logic::storage as keys;

use crate::app_state::AppState;
use crate::components::range::range;
use crate::components::ui::icon;
use crate::theme::{self, Tokens, sz};
use crate::views::library::secondary_button;
use crate::views::settings::{checkbox, stepper};

/// `dateFormatter`: "Nov 13, 3:45 PM".
fn format_time(at: f64) -> String {
    Local.timestamp_millis_opt(at as i64).single().map(|d| d.format("%b %-d, %-I:%M %p").to_string()).unwrap_or_default()
}

fn listening_label(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Start => "Started",
        EventKind::Resume => "Resumed",
        EventKind::Pause => "Paused",
        EventKind::Skip => "Skipped",
        EventKind::Complete => "Finished",
        EventKind::Stop => "Stopped",
        EventKind::Seek => "Sought",
    }
}

fn engagement_label(kind: EngagementKind) -> String {
    use EngagementKind::*;
    match kind {
        AlbumView => "Opened album",
        ArtistView => "Opened artist",
        GenreView => "Browsed genre",
        FavoriteAdd => "Favorited",
        FavoriteRemove => "Unfavorited",
        QueueAdd => "Queued",
        PlayNext => "Play next",
        AlbumPlay => "Played album",
        AlbumShuffle => "Shuffled album",
        PlaylistAdd => "Added to playlist",
        Search => "Searched",
        RecommendationClick => "Opened a suggestion",
        Dislike => "Not interested",
        Undislike => "Undid not interested",
        ArtistMute => "Muted artist",
        ArtistUnmute => "Unmuted artist",
        RecommendationImpression => return kind.as_str(),
    }
    .to_string()
}

fn ellipsis(text: impl Into<SharedString>) -> gpui::Div {
    div().overflow_hidden().whitespace_nowrap().text_ellipsis().child(text.into())
}

/// `.listening-history-section-heading`.
fn section_heading(title: &str, note: String, tokens: &Tokens) -> AnyElement {
    div()
        .flex()
        .items_baseline()
        .justify_between()
        .gap(sz(12.0))
        .child(div().text_size(sz(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(tokens.text).child(title.to_string()))
        .child(div().text_size(sz(10.0)).text_color(tokens.muted).child(note))
        .into_any_element()
}

/// `.listening-history-event`: copy on the left, meta on the right (stacked
/// at ≤640px).
fn event_row(title: String, subtitle: String, meta: Vec<AnyElement>, narrow: bool, tokens: &Tokens) -> AnyElement {
    let copy = div()
        .flex()
        .flex_col()
        .justify_center()
        .gap(sz(2.0))
        .min_w_0()
        .flex_1()
        .child(ellipsis(title).text_size(sz(12.0)).font_weight(FontWeight::MEDIUM).text_color(tokens.text))
        .child(ellipsis(subtitle).text_size(sz(10.0)).text_color(tokens.muted));
    let meta = div()
        .flex()
        .flex_wrap()
        .items_center()
        .when(!narrow, |d| d.justify_end().max_w(sz(220.0)))
        .gap_x(sz(8.0))
        .gap_y(sz(2.0))
        .text_size(sz(10.0))
        .text_color(tokens.muted)
        .children(meta);
    div()
        .flex()
        .when(narrow, |d| d.flex_col().gap(sz(5.0)))
        .when(!narrow, |d| d.items_center().gap(sz(12.0)))
        .py(sz(10.0))
        .px(sz(2.0))
        .border_b(sz(1.0))
        .border_color(tokens.line)
        .child(copy)
        .child(meta)
        .into_any_element()
}

fn list(id: &'static str, rows: Vec<AnyElement>, tokens: &Tokens) -> AnyElement {
    div()
        .id(id)
        .flex()
        .flex_col()
        .max_h(sz(330.0))
        .overflow_y_scroll()
        .border_t(sz(1.0))
        .border_color(tokens.line)
        .children(rows)
        .into_any_element()
}

pub fn listening_history(state: &Entity<AppState>, cx: &mut App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let narrow = theme::viewport(cx) <= 640.0;
    let s = state.read(cx);
    let enabled = s.storage.get(keys::LISTENING_HISTORY_ENABLED_KEY) != "false";
    let persistent = s.storage.get(keys::LISTENING_HISTORY_PERSIST_KEY) != "false";
    let events = &s.listening_events;
    let profile = volta_logic::listening_history::summarize_listening(events);
    let mut recent: Vec<_> = events.iter().collect();
    recent.sort_by(|a, b| b.at.partial_cmp(&a.at).unwrap_or(std::cmp::Ordering::Equal));
    recent.truncate(60);
    let total_seconds: f64 = events.iter().map(|e| e.delta_seconds.max(0.0)).sum();
    let unique_songs = events.iter().map(|e| e.song_id.as_str()).collect::<HashSet<_>>().len();
    let completion = (profile.average_completion_ratio * 100.0).round() as i64;
    let interactions: Vec<_> = s.learning.engagement.iter().filter(|e| e.kind != EngagementKind::RecommendationImpression).collect();
    let mut recent_interactions = interactions.clone();
    recent_interactions.sort_by(|a, b| b.at.partial_cmp(&a.at).unwrap_or(std::cmp::Ordering::Equal));
    recent_interactions.truncate(12);
    let samples = s.learning.ranker.samples;
    let confidence = (ranker_confidence(Some(&s.learning.ranker)) * 100.0).round() as i64;
    let mutes = s.learning.engagement_profile.mutes.len();

    let stat = |value: String, label: &str| {
        div()
            .flex()
            .flex_col()
            .gap(sz(2.0))
            .min_w_0()
            .p(sz(12.0))
            .border(sz(1.0))
            .border_color(tokens.line)
            .rounded(sz(8.0))
            .bg(tokens.canvas)
            .child(
                div()
                    .text_size(sz(17.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(tokens.text)
                    .font_features(theme::fonts::tabular_nums())
                    .child(value),
            )
            .child(ellipsis(label.to_string()).text_size(sz(10.0)).text_color(tokens.muted))
    };
    let stats = [
        (events.len().to_string(), "events retained"),
        (profile.sessions.to_string(), "listening sessions"),
        (unique_songs.to_string(), "songs heard"),
        (duration(Some(total_seconds)), "audible time"),
        (interactions.len().to_string(), "actions tracked"),
        (samples.to_string(), "learning samples"),
    ];
    let columns = if narrow { 2 } else { 3 };
    let mut grid = div().flex().flex_col().gap(sz(8.0));
    for chunk in stats.chunks(columns) {
        grid = grid.child(div().flex().gap(sz(8.0)).children(chunk.iter().map(|(v, l)| stat(v.clone(), l).flex_1().flex_basis(sz(0.0)))));
    }

    let bullets = [
        "Longer listens and finished songs count as stronger interest.",
        "Early skips and \"not interested\" reduce that artist and genre.",
        "Favorites, queue adds, playlist adds, and album views add weight.",
        "Songs played together in one sitting teach item-to-item similarity.",
        "Time of day, weekday, and typical track length tune the context.",
        "A local model learns which signals predict what you actually play.",
    ];
    let can_reset = samples > 0 || !interactions.is_empty();
    let reset_state = state.clone();
    let method = div()
        .flex()
        .flex_col()
        .gap(sz(7.0))
        .py(sz(14.0))
        .px(sz(16.0))
        .border_l(sz(2.0))
        .border_color(tokens.accent)
        .bg(tokens.canvas)
        .child(div().text_size(sz(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(tokens.text).child("How recommendations use this"))
        .child(div().flex().flex_col().gap(sz(4.0)).children(bullets.iter().map(|b| {
            div()
                .flex()
                .child(div().w(sz(17.0)).flex_none().text_color(tokens.muted).child("•"))
                .child(div().flex_1().min_w_0().child(b.to_string()))
        })))
        .when(profile.events > 0, |d| {
            let muted = if mutes > 0 { format!(" · {mutes} muted artist{}", if mutes == 1 { "" } else { "s" }) } else { String::new() };
            d.child(div().text_size(sz(10.0)).text_color(tokens.muted).child(format!(
                "Average completion across recorded outcomes: {completion}%. Local model confidence: {confidence}%{muted}."
            )))
        })
        .child(
            div().mt(sz(2.0)).flex().child(
                secondary_button("reset-learning", "Reset what Volta has learned", "", cx)
                    .when(!can_reset, |d| d.opacity(0.38).cursor_default())
                    .when(can_reset, move |d| d.on_click(move |_, _, cx| reset_state.update(cx, |s, cx| s.reset_learning(cx)))),
            ),
        );

    let s = state.read(cx);
    let interaction_rows: Vec<AnyElement> = recent_interactions
        .iter()
        .map(|e| {
            let entity = e.entity.as_ref();
            let title = entity.and_then(|en| en.name.clone()).or_else(|| e.text.clone()).unwrap_or_else(|| "Library".into());
            let artist = entity.and_then(|en| en.artist_name.clone()).map(|a| format!(" · {a}")).unwrap_or_default();
            event_row(
                title,
                format!("{}{artist}", engagement_label(e.kind)),
                vec![div().child(format_time(e.at)).into_any_element()],
                narrow,
                &tokens,
            )
        })
        .collect();
    let activity_rows: Vec<AnyElement> = recent
        .iter()
        .map(|e| {
            let color = match e.kind {
                EventKind::Complete => theme::hex("#9ed3ab"),
                EventKind::Skip => theme::hex("#e5b18f"),
                EventKind::Pause | EventKind::Stop => tokens.muted,
                _ => tokens.secondary,
            };
            let mut meta = vec![
                div().text_color(color).font_weight(FontWeight::SEMIBOLD).child(listening_label(e.kind)).into_any_element(),
                div().child(format_time(e.at)).into_any_element(),
            ];
            if e.delta_seconds > 0.0 {
                meta.push(div().when(!narrow, |d| d.w_full().flex().justify_end()).child(format!("{} heard", duration(Some(e.delta_seconds)))).into_any_element());
            }
            if e.kind == EventKind::Skip {
                meta.push(div().when(!narrow, |d| d.w_full().flex().justify_end()).child(format!("at {}", duration(Some(e.total_seconds)))).into_any_element());
            }
            let album = e.album.as_ref().map(|a| format!(" · {a}")).unwrap_or_default();
            event_row(e.title.clone(), format!("{}{album}", e.artist.clone().unwrap_or_else(|| "Unknown artist".into())), meta, narrow, &tokens)
        })
        .collect();
    let _ = s;
    let toggle_state = state.clone();
    div()
        .flex()
        .flex_col()
        .gap(sz(18.0))
        .text_size(sz(12.0))
        .line_height(gpui::relative(1.5))
        .text_color(tokens.secondary)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sz(4.0))
                .px(sz(2.0))
                .pt(sz(2.0))
                .child(if enabled {
                    "This is the local listening profile Volta uses to shape recommendations."
                } else {
                    "Personalized recommendation tracking is turned off."
                })
                // `<small>` is `font-size: smaller` (10px under 12px).
                .child(div().text_size(sz(10.0)).text_color(tokens.muted).child("Playback history and interactions stay on this device. Normal Navidrome scrobbling is separate.")),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(sz(16.0))
                .py(sz(12.0))
                .px(sz(14.0))
                .border(sz(1.0))
                .border_color(tokens.line)
                .rounded(sz(8.0))
                .bg(tokens.canvas)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sz(3.0))
                        .min_w_0()
                        .child(div().text_size(sz(12.0)).font_weight(FontWeight::MEDIUM).text_color(tokens.text).child("Keep history across logins and restarts"))
                        .child(div().text_size(sz(10.0)).text_color(tokens.muted).child(if persistent {
                            "Your profile will return when you sign in again."
                        } else {
                            "Keep it only for this app session; logout clears it."
                        })),
                )
                .child(checkbox("history-persist", persistent, &tokens, move |value, cx| {
                    toggle_state.update(cx, |s, cx| s.set_listening_persistent(value, cx))
                })),
        )
        .child(grid)
        .child(method)
        .when(!interaction_rows.is_empty(), |d| {
            d.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sz(9.0))
                    .child(section_heading("Actions", format!("Showing {}", interaction_rows.len()), &tokens))
                    .child(list("history-actions", interaction_rows, &tokens)),
            )
        })
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sz(9.0))
                .child(section_heading(
                    "Recent activity",
                    if activity_rows.is_empty() { "Nothing recorded yet".into() } else { format!("Showing {}", activity_rows.len()) },
                    &tokens,
                ))
                .child(if activity_rows.is_empty() {
                    div().child("Play a song for the profile to start learning your habits.").into_any_element()
                } else {
                    list("history-activity", activity_rows, &tokens)
                }),
        )
        .into_any_element()
}

struct Dial {
    key: &'static str,
    label: &'static str,
    hint: &'static str,
    low: &'static str,
    high: &'static str,
}

const DIALS: [Dial; 7] = [
    Dial {
        key: "adventurousness",
        label: "Risk appetite",
        hint: "At 0 the shelf is what it is sure you like. At 100 it only shows albums you have never played and pushes out anything already in heavy rotation.",
        low: "Records I know",
        high: "Only what I have not heard",
    },
    Dial {
        key: "familiarity",
        label: "Familiarity",
        hint: "Weight for artists, genres, and albums already in your rotation.",
        low: "Strangers welcome",
        high: "Only what I know",
    },
    Dial {
        key: "diversity",
        label: "Spread",
        hint: "How hard the slate pushes for different artists instead of repeats.",
        low: "Same artist is fine",
        high: "One per artist",
    },
    Dial {
        key: "recency",
        label: "Recency",
        hint: "Balance what you are playing this week against your long-term taste.",
        low: "My all-time taste",
        high: "My last few days",
    },
    Dial {
        key: "freshness",
        label: "New arrivals",
        hint: "Boost records that were recently added to your collection.",
        low: "Ignore new adds",
        high: "Surface new adds",
    },
    Dial {
        key: "caution",
        label: "Caution",
        hint: "How strongly skips, dislikes, and mutes hold something down.",
        low: "Forgiving",
        high: "Remember every skip",
    },
    Dial {
        key: "learning",
        label: "Learned model",
        hint: "How much the on-device model may overrule the hand-tuned ranking.",
        low: "Trust the defaults",
        high: "Trust what I taught it",
    },
];

fn dial_value(t: &RecommendationTuning, key: &str) -> f64 {
    match key {
        "adventurousness" => t.adventurousness,
        "familiarity" => t.familiarity,
        "learning" => t.learning,
        "diversity" => t.diversity,
        "recency" => t.recency,
        "freshness" => t.freshness,
        _ => t.caution,
    }
}

fn with_dial(mut t: RecommendationTuning, key: &str, value: f64) -> RecommendationTuning {
    match key {
        "adventurousness" => t.adventurousness = value,
        "familiarity" => t.familiarity = value,
        "learning" => t.learning = value,
        "diversity" => t.diversity = value,
        "recency" => t.recency = value,
        "freshness" => t.freshness = value,
        _ => t.caution = value,
    }
    t
}

/// `.engine-block-heading`.
fn block_heading(title: &str, note: String, tokens: &Tokens) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(sz(2.0))
        .child(div().text_size(sz(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(tokens.text).child(title.to_string()))
        .child(div().text_size(sz(11.0)).line_height(gpui::relative(1.4)).text_color(tokens.muted).child(note))
        .into_any_element()
}

/// Columns for `repeat(auto-fit, minmax(min(<min>px, 100%), 1fr))`.
fn auto_fit(width: f32, min: f32, gap: f32) -> usize {
    (((width + gap) / (min + gap)).floor() as usize).max(1)
}

/// Rows of `columns` equal-width cells (an auto-fit grid).
fn grid(items: Vec<AnyElement>, columns: usize, gap: f32, stretch: bool) -> AnyElement {
    let mut rows = div().flex().flex_col().gap(sz(gap));
    let mut items = items.into_iter().peekable();
    while items.peek().is_some() {
        let mut row = div().flex().when(!stretch, |d| d.items_start()).gap(sz(gap));
        for _ in 0..columns {
            row = row.child(match items.next() {
                Some(item) => div().flex().flex_col().flex_1().flex_basis(sz(0.0)).min_w_0().child(item),
                None => div().flex_1().flex_basis(sz(0.0)),
            });
        }
        rows = rows.child(row);
    }
    rows.into_any_element()
}

/// The engine dialog's body and its pinned `.engine-actions` footer.
pub fn engine(state: &Entity<AppState>, cx: &mut App) -> (AnyElement, AnyElement) {
    let tokens = theme::tokens(cx).clone();
    let viewport = theme::viewport(cx);
    let inner = 940f32.min(viewport - 32.0) - 40.0;
    let tuning = state.read(cx).learning.tuning;
    let samples = state.read(cx).learning.ranker.samples;
    let preset = match_preset(&tuning).map(|p| p.id);
    let (label, risk) = describe(&tuning);
    let preview: Vec<_> = state.update(cx, |s, _| s.engine_preview().to_vec());
    let candidate_count = {
        let home = &state.read(cx).home;
        if home.recommendation_pool.is_empty() { home.recommendation_candidates.len() } else { home.recommendation_pool.len() }
    };
    let set = {
        let state = state.clone();
        move |t: RecommendationTuning, cx: &mut App| state.update(cx, |s, cx| s.set_tuning(t, cx))
    };

    let hero = div()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap(sz(18.0))
        .py(sz(14.0))
        .px(sz(15.0))
        .border(sz(1.0))
        .border_color(tokens.line)
        .rounded(sz(10.0))
        .bg(tokens.canvas)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sz(5.0))
                .flex_grow(1.0)
                .flex_shrink(1.0)
                .flex_basis(sz(380.0))
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sz(6.0))
                        // `.engine-hero-copy p` outranks `.engine-hero-kicker`, so
                        // the kicker renders muted at 11.5px in the web.
                        .text_color(tokens.muted)
                        .text_size(sz(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .letter_spacing(sz(11.5 * 0.08))
                        .child(icon("sparkles", 13.0))
                        .child("LOCAL AND EXPLAINABLE"),
                )
                .child(div().text_size(sz(17.0)).font_weight(FontWeight::SEMIBOLD).letter_spacing(sz(-0.3)).text_color(tokens.text).child("Shape how Volta suggests music"))
                .child(div().text_size(sz(11.5)).line_height(gpui::relative(1.5)).text_color(tokens.muted).child(
                    "These dials trade certainty for discovery. Higher risk means the shelf is more willing to be wrong in exchange for finding something you would never have searched for.",
                )),
        )
        .child({
            let narrow = viewport <= 640.0;
            div()
                .flex()
                .flex_col()
                .flex_none()
                .when(!narrow, |d| d.items_end().min_w(sz(170.0)))
                .when(narrow, |d| d.items_start())
                .gap(sz(3.0))
                .child(div().text_size(sz(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(tokens.text).child(label))
                .child(
                    div()
                        .text_size(sz(17.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(tokens.accent)
                        .font_features(theme::fonts::tabular_nums())
                        .child(format!("{risk}%")),
                )
                .child(
                    div()
                        .w_full()
                        .h(sz(5.0))
                        .rounded_full()
                        .overflow_hidden()
                        .bg(tokens.sidebar)
                        .child(div().h_full().rounded_full().bg(tokens.accent).w(gpui::relative(risk as f32 / 100.0))),
                )
                .child(div().text_size(sz(10.0)).text_color(tokens.muted).child("appetite for risk"))
        });

    let preset_cards: Vec<AnyElement> = PRESETS
        .iter()
        .map(|p| {
            let pressed = preset == Some(p.id);
            let set = set.clone();
            let tuning = p.tuning;
            div()
                .id(SharedString::from(format!("preset-{}", p.id)))
                .flex_1()
                .flex()
                .flex_col()
                .gap(sz(3.0))
                .py(sz(10.0))
                .px(sz(11.0))
                .border(sz(1.0))
                .border_color(if pressed { tokens.accent } else { tokens.line })
                .rounded(sz(9.0))
                .bg(if pressed { tokens.selection } else { tokens.canvas })
                .when(!pressed, |d| d.hover(|s| s.bg(tokens.hover)))
                .cursor_pointer()
                .on_click(move |_, _, cx| set(tuning, cx))
                .child(
                    div()
                        .text_size(sz(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if pressed { tokens.accent } else { tokens.text })
                        .child(p.label),
                )
                .child(div().text_size(sz(10.5)).line_height(gpui::relative(1.4)).text_color(tokens.muted).child(p.description))
                .into_any_element()
        })
        .collect();
    let presets = div()
        .flex()
        .flex_col()
        .gap(sz(10.0))
        .child(block_heading(
            "Presets",
            match_preset(&tuning).map(|p| p.description.to_string()).unwrap_or_else(|| "Custom mix — tuned by hand below.".into()),
            &tokens,
        ))
        .child(grid(preset_cards, auto_fit(inner, 180.0, 8.0).min(PRESETS.len()), 8.0, true));

    let mut dials: Vec<AnyElement> = DIALS
        .iter()
        .map(|dial| {
            let value = dial_value(&tuning, dial.key);
            let percent = crate::views::learning::round_percent(value);
            let hint = if dial.key == "learning" {
                format!(
                    "{} {}",
                    dial.hint,
                    if samples > 0 {
                        format!("{samples} local samples trained so far.")
                    } else {
                        "No local learning yet, so this dial has no effect until you listen.".into()
                    }
                )
            } else {
                dial.hint.to_string()
            };
            let set = set.clone();
            let key = dial.key;
            div()
                .flex()
                .flex_col()
                .gap(sz(4.0))
                .py(sz(9.0))
                .px(sz(11.0))
                .rounded(sz(8.0))
                .bg(tokens.canvas)
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .justify_between()
                        .gap(sz(10.0))
                        .child(div().text_size(sz(12.0)).font_weight(FontWeight::MEDIUM).text_color(tokens.text).child(dial.label))
                        .child(
                            div()
                                .text_size(sz(11.5))
                                .text_color(tokens.accent)
                                .font_features(theme::fonts::tabular_nums())
                                .child(format!("{percent}%")),
                        ),
                )
                .child(div().text_size(sz(10.5)).line_height(gpui::relative(1.4)).text_color(tokens.muted).child(hint))
                .child(range(format!("dial-{key}"), value as f32, &tokens, move |v, cx| {
                    set(with_dial(tuning, key, v as f64), cx);
                }))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .gap(sz(10.0))
                        .text_size(sz(10.0))
                        .italic()
                        .text_color(tokens.muted)
                        .child(dial.low)
                        .child(dial.high),
                )
                .into_any_element()
        })
        .collect();
    let shelf_set = set.clone();
    dials.push(
        div()
            .flex()
            .items_center()
            .gap(sz(10.0))
            .py(sz(9.0))
            .px(sz(11.0))
            .rounded(sz(8.0))
            .bg(tokens.canvas)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sz(4.0))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .justify_between()
                            .gap(sz(10.0))
                            .child(div().text_size(sz(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(tokens.text).child("Shelf size"))
                            .child(
                                div()
                                    .text_size(sz(11.5))
                                    .text_color(tokens.accent)
                                    .font_features(theme::fonts::tabular_nums())
                                    .child(format!("{}", tuning.slate_size)),
                            ),
                    )
                    .child(div().text_size(sz(10.5)).line_height(gpui::relative(1.4)).text_color(tokens.muted).child("How many albums “Made for you” asks the engine for.")),
            )
            .child(stepper("shelf-size", format!("{}", tuning.slate_size), &tokens, move |delta, cx| {
                let next = (tuning.slate_size + delta as f64).clamp(MIN_SLATE_SIZE, MAX_SLATE_SIZE);
                shelf_set(RecommendationTuning { slate_size: next, ..tuning }, cx);
            }))
            .into_any_element(),
    );
    let dials_block = div()
        .flex()
        .flex_col()
        .gap(sz(10.0))
        .child(block_heading("Dials", "Every change re-ranks the preview immediately.".into(), &tokens))
        .child(div().flex().flex_col().gap(sz(7.0)).children(dials))
        .into_any_element();

    let preview_rows: Vec<AnyElement> = preview
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let exposure = item.signals.exposure;
            let (level, text) = if exposure == 0.0 {
                ("new", "NO PLAYS")
            } else if exposure >= 0.6 {
                ("played", "FAMILIAR")
            } else {
                ("some", "LIGHTLY PLAYED")
            };
            let new = level == "new";
            div()
                .flex()
                .items_center()
                .gap(sz(9.0))
                .min_w_0()
                .py(sz(5.0))
                .px(sz(7.0))
                .rounded(sz(6.0))
                .bg(tokens.canvas)
                .child(
                    div()
                        .w(sz(17.0))
                        .flex_none()
                        .flex()
                        .justify_end()
                        .text_size(sz(10.5))
                        .text_color(tokens.muted)
                        .font_features(theme::fonts::tabular_nums())
                        .child(format!("{}", index + 1)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sz(1.0))
                        .flex_1()
                        .min_w_0()
                        .child(
                            ellipsis(volta_logic::format::album_name(&item.album).to_string())
                                .text_size(sz(11.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(tokens.text),
                        )
                        .child(
                            ellipsis(format!("{} · {}", item.album.artist.as_deref().unwrap_or("Unknown artist"), item.reason))
                                .text_size(sz(10.5))
                                .text_color(tokens.muted),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .min_w(sz(34.0))
                        .px(sz(6.0))
                        .py(sz(1.0))
                        .rounded_full()
                        .bg(if new { tokens.selection } else { tokens.sidebar })
                        .text_color(if new { tokens.accent } else { tokens.muted })
                        .text_size(sz(9.5))
                        .letter_spacing(sz(9.5 * 0.05))
                        .flex()
                        .justify_center()
                        .child(text),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(sz(10.5))
                        .text_color(tokens.muted)
                        .font_features(theme::fonts::tabular_nums())
                        .child(format!("{:.2}", item.score)),
                )
                .into_any_element()
        })
        .collect();
    let preview_block = div()
        .flex()
        .flex_col()
        .gap(sz(10.0))
        .child(block_heading(
            "Live preview",
            if preview_rows.is_empty() {
                "Open Home once to load candidates, then tune here.".into()
            } else {
                format!("Re-ranked from {candidate_count} candidates as you tune.")
            },
            &tokens,
        ))
        .when(!preview_rows.is_empty(), |d| d.child(div().flex().flex_col().gap(sz(4.0)).children(preview_rows)))
        .into_any_element();
    let columns = grid(vec![dials_block, preview_block], auto_fit(inner, 340.0, 18.0).min(2), 18.0, false);

    let body = div()
        .flex()
        .flex_col()
        .gap(sz(16.0))
        .text_size(sz(12.0))
        .text_color(tokens.secondary)
        .child(hero)
        .child(presets)
        .child(columns)
        .into_any_element();

    let reset = state.clone();
    let footer = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(sz(14.0))
        .py(sz(12.0))
        .px(sz(20.0))
        .border_t(sz(1.0))
        .border_color(tokens.line)
        .bg(tokens.sidebar)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sz(2.0))
                .min_w_0()
                .child(div().text_size(sz(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(tokens.text).child(label))
                .child(div().text_size(sz(10.5)).text_color(tokens.muted).child(if samples > 0 {
                    format!("{samples} local samples trained.")
                } else {
                    "The engine is running on its defaults so far.".into()
                })),
        )
        .child(secondary_button("reset-dials", "Reset dials", "rotate-ccw", cx).on_click(move |_, _, cx| reset.update(cx, |s, cx| s.reset_tuning(cx))))
        .into_any_element();
    (body, footer)
}

/// `Math.round(value * 100)`.
pub fn round_percent(value: f64) -> i64 {
    volta_logic::js::js_round(value * 100.0) as i64
}
