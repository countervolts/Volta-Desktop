//! `StatsView` from app/stats-view.tsx: period tabs and navigation, overview
//! cards, activity charts, genre donut and ranked lists, all from the local
//! listening history.

use chrono::{Local, TimeZone};
use gpui::{AnyElement, App, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window, div, img, prelude::*};
use volta_logic::format::duration;
use volta_logic::library_stats::LibraryStats;
use volta_logic::listening_stats::{RankedItem, StatsPeriod, summarize_listening_period};

use crate::app_state::{AppState, Page, Route};
use crate::components::charts::{area_chart, bar_chart, donut, graph_colors};
use crate::components::ui::{icon, line};
use crate::theme::{self, Tokens, mix, sz};
use crate::views::home::page_heading_with;
use crate::views::library::secondary_button;

const PERIODS: [(StatsPeriod, &str); 4] = [(StatsPeriod::Day, "Day"), (StatsPeriod::Week, "Week"), (StatsPeriod::Month, "Month"), (StatsPeriod::Year, "Year")];

fn compact_duration(seconds: f64) -> String {
    if seconds < 60.0 {
        format!("{}s", seconds.round() as i64)
    } else {
        duration(Some(seconds))
    }
}

fn thousands(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `Intl.DateTimeFormat({month: "short", day: "numeric", hour: "numeric", minute: "2-digit"})`.
fn time_label(at: f64) -> String {
    let Some(date) = Local.timestamp_millis_opt(at as i64).single() else { return "—".into() };
    date.format("%b %-d, %-I:%M %p").to_string()
}

fn card(tokens: &Tokens, glyph: &str, label: &str, value: String, sub: Option<String>, narrow: bool) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(sz(4.0))
        .min_h(sz(if narrow { 117.0 } else { 136.0 }))
        .p(sz(if narrow { 12.0 } else { 16.0 }))
        .border(sz(1.0))
        .border_color(tokens.line)
        .rounded(sz(8.0))
        .bg(tokens.surface)
        .child(div().size(sz(21.0)).mb(sz(2.0)).text_color(tokens.accent).child(icon(glyph, 18.0)))
        .child(div().text_size(sz(10.0)).font_weight(FontWeight(650.0)).text_color(tokens.muted).child(label.to_uppercase()))
        .child(
            line(value)
                .min_h(sz(27.0))
                .text_size(sz(if narrow { 20.0 } else { 23.0 }))
                .font_weight(FontWeight(680.0))
                .letter_spacing(sz(-0.5))
                .line_height(gpui::relative(1.15))
                .font_features(theme::fonts::tabular_nums()),
        )
        .when_some(sub, |d, sub| d.child(line(sub).text_size(sz(10.0)).text_color(tokens.muted)))
        .into_any_element()
}

/// The "Overview" heading above a card grid.
fn overview_heading(tokens: &Tokens, title: String) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(sz(7.0))
        .text_color(tokens.secondary)
        .child(icon("bar-chart3", 16.0))
        .child(div().text_size(sz(14.0)).font_weight(FontWeight(650.0)).text_color(tokens.text).child(title))
        .into_any_element()
}

/// A pill switch between the two stats pages.
fn page_switch(state: &Entity<AppState>, tokens: &Tokens, current: &'static str) -> AnyElement {
    div()
        .flex()
        .gap(sz(3.0))
        .p(sz(3.0))
        .border(sz(1.0))
        .border_color(tokens.line)
        .rounded(sz(8.0))
        .bg(tokens.surface)
        .children([("listening", "Listening"), ("library", "Library")].into_iter().map(|(id, label)| {
            let selected = id == current;
            let target = state.clone();
            div()
                .id(SharedString::from(format!("stats-tab-{id}")))
                .min_w(sz(72.0))
                .px(sz(10.0))
                .py(sz(6.0))
                .rounded(sz(5.0))
                .flex()
                .justify_center()
                .text_size(sz(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .when(selected, |d| {
                    d.text_color(tokens.text).bg(tokens.canvas).shadow(vec![gpui::BoxShadow {
                        color: theme::hex("#0002"),
                        offset: gpui::point(gpui::px(0.0), gpui::px(1.0)),
                        blur_radius: gpui::px(2.0),
                        spread_radius: gpui::px(0.0),
                        inset: false,
                    }])
                })
                .when(!selected, |d| d.text_color(tokens.muted).hover(|s| s.text_color(tokens.text).bg(tokens.hover)))
                .on_click(move |_, _, cx| {
                    target.update(cx, |s, cx| {
                        s.stats_tab = id;
                        cx.notify();
                    })
                })
                .child(label)
        }))
        .into_any_element()
}

fn size_label(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let (mut value, mut unit) = (bytes, 0);
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 || value >= 100.0 { format!("{:.0} {}", value, UNITS[unit]) } else { format!("{:.1} {}", value, UNITS[unit]) }
}

fn chart_card(tokens: &Tokens, title: &str, unit: &str, body: AnyElement, narrow: bool) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .min_h(sz(if narrow { 220.0 } else { 238.0 }))
        .p(sz(18.0))
        .border(sz(1.0))
        .border_color(tokens.line)
        .rounded(sz(8.0))
        .bg(tokens.surface)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .mb(sz(13.0))
                .child(div().text_size(sz(14.0)).font_weight(FontWeight(650.0)).child(title.to_string()))
                .child(div().text_size(sz(10.0)).font_weight(FontWeight(650.0)).text_color(tokens.muted).child(unit.to_uppercase())),
        )
        .child(body)
        .into_any_element()
}

fn ranked(state: &Entity<AppState>, tokens: &Tokens, title: &str, empty: &str, items: &[RankedItem], kind: &str, cx: &mut App) -> AnyElement {
    let mut list = div().flex().flex_col().gap(sz(2.0));
    for (index, item) in items.iter().enumerate() {
        let cover = state.update(cx, |s, cx| s.cover(item.cover_art.as_deref(), 100, cx));
        let round = if kind == "artist" { 17.0 } else { 4.0 };
        let (target, item_id, label) = (state.clone(), item.id.clone(), item.label.clone());
        let page = match kind {
            "album" => Some(Page::Album),
            "artist" => Some(Page::Artist),
            _ => None,
        };
        list = list.child(
            div()
                .id(SharedString::from(format!("rank-{kind}-{index}")))
                .flex()
                .items_center()
                .gap(sz(8.0))
                .min_h(sz(39.0))
                .px(sz(5.0))
                .py(sz(4.0))
                .rounded(sz(5.0))
                .hover(|s| s.bg(tokens.hover))
                .when(page.is_some(), |d| d.cursor_pointer())
                .on_click(move |_, _, cx| {
                    if let Some(page) = page {
                        let route = Route { page, id: Some(item_id.clone()), title: Some(label.clone()) };
                        target.update(cx, |s, cx| s.navigate(route, cx));
                    }
                })
                .child(
                    div()
                        .w(sz(21.0))
                        .flex_none()
                        .flex()
                        .justify_end()
                        .text_size(sz(10.0))
                        .text_color(tokens.muted)
                        .font_features(theme::fonts::tabular_nums())
                        .child((index + 1).to_string()),
                )
                .child(
                    div()
                        .size(sz(34.0))
                        .flex_none()
                        .rounded(sz(round))
                        .bg(tokens.hover)
                        .when_some(cover, |d, image| d.child(img(image).size_full().rounded(sz(round)))),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sz(1.0))
                        .flex_1()
                        .min_w_0()
                        .child(line(item.label.clone()).text_size(sz(11.0)).font_weight(FontWeight(550.0)))
                        .when_some(item.detail.clone(), |d, detail| d.child(line(detail).text_size(sz(10.0)).text_color(tokens.muted))),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(sz(10.0))
                        .font_weight(FontWeight(650.0))
                        .text_color(tokens.accent)
                        .font_features(theme::fonts::tabular_nums())
                        .child(format!("{} {}", item.plays, if item.plays == 1 { "play" } else { "plays" })),
                ),
        );
    }
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .p(sz(18.0))
        .border(sz(1.0))
        .border_color(tokens.line)
        .rounded(sz(8.0))
        .bg(tokens.surface)
        .child(div().mb(sz(9.0)).text_size(sz(10.0)).font_weight(FontWeight(650.0)).letter_spacing(sz(10.0 * 0.06)).text_color(tokens.muted).child(title.to_uppercase()))
        .child(if items.is_empty() {
            div().my(sz(20.0)).text_size(sz(11.0)).text_color(tokens.muted).child(empty.to_string()).into_any_element()
        } else {
            list.into_any_element()
        })
        .into_any_element()
}

fn breakdown(tokens: &Tokens, title: &str, empty_text: &str, items: &[RankedItem]) -> AnyElement {
    let colors = graph_colors();
    let body = if items.is_empty() {
        div().my(sz(20.0)).text_size(sz(11.0)).text_color(tokens.muted).child(empty_text.to_string()).into_any_element()
    } else {
        div()
            .flex()
            .items_center()
            .gap(sz(14.0))
            .min_h(sz(170.0))
            .child(div().flex_1().flex().justify_center().child(donut(format!("donut-{title}"), items.iter().map(|i| i.plays as f64).collect(), colors.clone(), 168.0)))
            .child(
                div().flex().flex_col().gap(sz(9.0)).flex_1().min_w_0().children(items.iter().enumerate().map(|(i, item)| {
                    div()
                        .flex()
                        .items_center()
                        .gap(sz(7.0))
                        .text_size(sz(10.0))
                        .text_color(tokens.muted)
                        .child(div().size(sz(7.0)).flex_none().rounded_full().bg(colors[i % colors.len()]))
                        .child(line(item.label.clone()).flex_1().text_color(tokens.secondary))
                        .child(div().w(sz(22.0)).flex_none().child(item.plays.to_string()))
                })),
            )
            .into_any_element()
    };
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .min_h(sz(224.0))
        .p(sz(18.0))
        .border(sz(1.0))
        .border_color(tokens.line)
        .rounded(sz(8.0))
        .bg(tokens.surface)
        .child(div().mb(sz(9.0)).text_size(sz(10.0)).font_weight(FontWeight(650.0)).text_color(tokens.muted).child(title.to_uppercase()))
        .child(body)
        .into_any_element()
}

pub fn render(state: &Entity<AppState>, content_width: f32, window: &mut Window, cx: &mut App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let viewport = theme::viewport_css_width(window);
    let narrow = viewport <= 640.0;
    let (period, offset, enabled) = {
        let s = state.read(cx);
        (s.stats_period, s.stats_offset, s.storage.get(volta_logic::storage::LISTENING_HISTORY_ENABLED_KEY) != "false")
    };
    let events = state.read(cx).listening_events.clone();
    let stats = summarize_listening_period(&events, period, offset, Local::now().date_naive());
    let period_name = PERIODS.iter().find(|(p, _)| *p == period).map(|(_, n)| *n).unwrap_or("Week");

    let switch = page_switch(state, &tokens, state.read(cx).stats_tab);
    if state.read(cx).stats_tab == "library" {
        let body = library_view(state, &tokens, content_width, narrow, cx);
        return div().flex().flex_col().child(page_heading_with("Stats", vec![switch], cx)).child(body).into_any_element();
    }

    let tabs = div()
        .flex()
        .gap(sz(3.0))
        .p(sz(3.0))
        .border(sz(1.0))
        .border_color(tokens.line)
        .rounded(sz(8.0))
        .bg(tokens.surface)
        .when(narrow, |d| d.w_full())
        .children(PERIODS.iter().map(|&(p, label)| {
            let selected = p == period;
            let target = state.clone();
            div()
                .id(SharedString::from(format!("stats-period-{label}")))
                .when(narrow, |d| d.flex_1())
                .min_w(sz(if narrow { 0.0 } else { 58.0 }))
                .px(sz(10.0))
                .py(sz(6.0))
                .rounded(sz(5.0))
                .flex()
                .justify_center()
                .text_size(sz(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .when(selected, |d| {
                    d.text_color(tokens.text).bg(tokens.canvas).shadow(vec![gpui::BoxShadow {
                        color: theme::hex("#0002"),
                        offset: gpui::point(gpui::px(0.0), gpui::px(1.0)),
                        blur_radius: gpui::px(2.0),
                        spread_radius: gpui::px(0.0),
                        inset: false,
                    }])
                })
                .when(!selected, |d| d.text_color(tokens.muted).hover(|s| s.text_color(tokens.text).bg(tokens.hover)))
                .on_click(move |_, _, cx| {
                    target.update(cx, |s, cx| {
                        s.stats_period = p;
                        s.stats_offset = 0;
                        cx.notify();
                    })
                })
                .child(label)
        }));

    let nav_button = |id: &str, glyph: &str, delta: i32, disabled: bool| {
        let target = state.clone();
        div()
            .id(SharedString::from(id.to_string()))
            .w(sz(if narrow { 31.0 } else { 32.0 }))
            .min_h(sz(30.0))
            .flex()
            .items_center()
            .justify_center()
            .border(sz(1.0))
            .border_color(tokens.line)
            .rounded(sz(6.0))
            .text_color(tokens.secondary)
            .when(disabled, |d| d.opacity(0.5))
            .when(!disabled, |d| {
                d.cursor_pointer().hover(|s| s.bg(tokens.hover).text_color(tokens.text).border_color(mix(tokens.accent, 45.0, tokens.line)))
            })
            .on_click(move |_, _, cx| {
                if !disabled {
                    target.update(cx, |s, cx| {
                        s.stats_offset = (s.stats_offset + delta).min(0);
                        cx.notify();
                    })
                }
            })
            .child(icon(glyph, 16.0))
    };
    let current_target = state.clone();
    let nav = div()
        .flex()
        .items_center()
        .gap(sz(10.0))
        .px(sz(12.0))
        .py(sz(10.0))
        .border(sz(1.0))
        .border_color(tokens.line)
        .rounded(sz(8.0))
        .bg(tokens.surface)
        .child(nav_button("stats-prev", "arrow-left", -1, false))
        .child(
            line(stats.range_label.clone())
                .flex_1()
                .flex()
                .justify_center()
                .text_size(sz(if narrow { 12.0 } else { 14.0 }))
                .font_weight(FontWeight(650.0)),
        )
        .when(!narrow, |d| {
            d.child(
                div()
                    .id("stats-current")
                    .min_w(sz(73.0))
                    .min_h(sz(30.0))
                    .px(sz(10.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border(sz(1.0))
                    .rounded(sz(6.0))
                    .text_size(sz(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .map(|d| {
                        if stats.current {
                            d.text_color(tokens.accent).border_color(mix(tokens.accent, 60.0, tokens.line))
                        } else {
                            d.text_color(tokens.secondary).border_color(tokens.line)
                        }
                    })
                    .hover(|s| s.bg(tokens.hover))
                    .on_click(move |_, _, cx| {
                        current_target.update(cx, |s, cx| {
                            s.stats_offset = 0;
                            cx.notify();
                        })
                    })
                    .child(match period {
                        StatsPeriod::Day => "Today",
                        StatsPeriod::Week => "This week",
                        _ => "Current",
                    }),
            )
        })
        .child(nav_button("stats-next", "arrow-right", 1, stats.current));

    let empty = |title: String, body: &str, button: Option<AnyElement>| {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(sz(9.0))
            .min_h(sz(238.0))
            .px(sz(24.0))
            .py(sz(52.0))
            .border(sz(1.0))
            .border_color(tokens.line)
            .rounded(sz(9.0))
            .text_center()
            .text_color(tokens.muted)
            .child(div().text_color(tokens.accent).opacity(0.85).child(icon("bar-chart3", 31.0)))
            .child(div().mt(sz(5.0)).text_size(sz(15.0)).font_weight(FontWeight(650.0)).text_color(tokens.text).child(title))
            .child(div().max_w(sz(390.0)).text_size(sz(12.0)).line_height(gpui::relative(1.5)).child(body.to_string()))
            .children(button)
            .into_any_element()
    };

    let mut view = div()
        .flex()
        .flex_col()
        .gap(sz(if narrow { 18.0 } else { 20.0 }))
        .pb(sz(24.0))
        .child(
            div()
                .mt(sz(-5.0))
                .max_w(sz(660.0))
                .text_size(sz(12.0))
                .line_height(gpui::relative(1.55))
                .text_color(tokens.muted)
                .child("Listening activity saved locally by Volta. Browse day, week, month, and year views without sending your history anywhere."),
        )
        .child(div().flex().child(tabs))
        .child(nav);

    if !enabled {
        let target = state.clone();
        view = view.child(empty(
            "Listening stats are paused".into(),
            "Turn on personalized recommendations to start keeping a local listening history.",
            Some(
                secondary_button("stats-open-settings", "Open Settings", "", cx)
                    .mt(sz(5.0))
                    .on_click(move |_, _, cx| {
                        target.update(cx, |s, cx| {
                            s.settings_tab = "privacy";
                            s.navigate(Route::page(Page::Settings), cx)
                        })
                    })
                    .into_any_element(),
            ),
        ));
    } else if stats.total_plays == 0 {
        view = view.child(empty(
            format!("No listening data for this {}", period_name.to_lowercase()),
            if stats.current { "Start playing music and your stats will appear here automatically." } else { "No tracks were recorded for this period." },
            None,
        ));
    } else {
        let streak = stats.longest_streak;
        let active = stats.active_days;
        let cards = vec![
            card(&tokens, "music2", "Tracks played", thousands(stats.total_plays), None, narrow),
            card(&tokens, "clock3", "Listen time", compact_duration(stats.total_seconds), Some(format!("{} min", thousands((stats.total_seconds / 60.0).round() as usize))), narrow),
            card(&tokens, "disc3", "Unique songs", thousands(stats.unique_songs), None, narrow),
            card(&tokens, "user-round", "Unique artists", thousands(stats.unique_artists), None, narrow),
            card(&tokens, "album", "Unique albums", thousands(stats.unique_albums), None, narrow),
            card(&tokens, "list-music", "Sessions", thousands(stats.sessions), None, narrow),
            card(&tokens, "calendar-days", "Average listen", compact_duration(stats.average_listen_seconds), Some("per track start".into()), narrow),
            card(&tokens, "flame", "Best streak", format!("{streak} day{}", if streak == 1 { "" } else { "s" }), Some(format!("{active} active day{}", if active == 1 { "" } else { "s" })), narrow),
        ];
        let columns = if narrow { 2 } else { crate::components::album_grid::auto_fill_columns(content_width, 152.0, 12.0).min(8) };
        view = view.child(
            div()
                .flex()
                .flex_col()
                .gap(sz(12.0))
                .child(overview_heading(
                    &tokens,
                    if period == StatsPeriod::Year { format!("{} summary", stats.range_label) } else { "Overview".into() },
                ))
                .child(div().grid().grid_cols(columns as u16).gap(sz(if narrow { 8.0 } else { 12.0 })).children(cards)),
        );
        let info = |label: &str, value: String| {
            div().flex().gap(sz(4.0)).child(label.to_string()).child(div().text_color(tokens.text).font_weight(FontWeight::SEMIBOLD).child(value))
        };
        view = view.child(
            div()
                .flex()
                .flex_wrap()
                .gap_x(sz(25.0))
                .gap_y(sz(8.0))
                .px(sz(14.0))
                .py(sz(10.0))
                .border(sz(1.0))
                .border_color(tokens.line)
                .rounded(sz(7.0))
                .bg(tokens.surface)
                .text_size(sz(11.0))
                .text_color(tokens.muted)
                .child(info("First play", if stats.first_played_at > 0.0 { time_label(stats.first_played_at) } else { "—".into() }))
                .child(info("Last play", if stats.last_played_at > 0.0 { time_label(stats.last_played_at) } else { "—".into() }))
                .child(info("Active days", stats.active_days.to_string())),
        );
        let labels: Vec<String> = stats.timeline.iter().map(|p| p.label.clone()).collect();
        let half = if narrow { content_width - 38.0 } else { (content_width - 16.0) / 2.0 - 38.0 };
        let activity = match period {
            StatsPeriod::Day => "Plays by hour",
            StatsPeriod::Week => "Plays by day",
            StatsPeriod::Month => "Daily plays",
            StatsPeriod::Year => "Monthly plays",
        };
        let plays = area_chart("stats-plays", labels.clone(), stats.timeline.iter().map(|p| p.plays as f64).collect(), half, 183.0, &tokens);
        let minutes = bar_chart("stats-minutes", labels, stats.timeline.iter().map(|p| (p.seconds / 60.0).round()).collect(), half, 183.0, &tokens);
        let pair = |a: AnyElement, b: AnyElement| {
            if narrow {
                div().flex().flex_col().gap(sz(10.0)).child(a).child(b).into_any_element()
            } else {
                div()
                    .flex()
                    .items_start()
                    .gap(sz(16.0))
                    .child(div().flex_1().min_w_0().child(a))
                    .child(div().flex_1().min_w_0().child(b))
                    .into_any_element()
            }
        };
        view = view
            .child(pair(
                chart_card(&tokens, activity, "Tracks", plays, narrow),
                chart_card(&tokens, "Listening time", "Minutes", minutes, narrow),
            ))
            .child(pair(
                breakdown(&tokens, "Genre breakdown", "No genres were recorded for this period.", &stats.top_genres),
                ranked(state, &tokens, "Top songs", "No songs were recorded for this period.", &stats.top_songs, "song", cx),
            ))
            .child(pair(
                ranked(state, &tokens, "Top albums", "No albums were recorded for this period.", &stats.top_albums, "album", cx),
                ranked(state, &tokens, "Top artists", "No artists were recorded for this period.", &stats.top_artists, "artist", cx),
            ));
    }
    div().flex().flex_col().child(page_heading_with("Stats", vec![switch], cx)).child(view).into_any_element()
}

/// A ranked list of plain counts with a share bar under each row.
fn count_bars(tokens: &Tokens, title: &str, items: &[(String, usize)], unit: &str) -> AnyElement {
    let most = items.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1) as f32;
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .p(sz(18.0))
        .border(sz(1.0))
        .border_color(tokens.line)
        .rounded(sz(8.0))
        .bg(tokens.surface)
        .child(div().mb(sz(10.0)).text_size(sz(10.0)).font_weight(FontWeight(650.0)).letter_spacing(sz(10.0 * 0.06)).text_color(tokens.muted).child(title.to_uppercase()))
        .child(div().flex().flex_col().gap(sz(9.0)).children(items.iter().enumerate().map(|(i, (label, count))| {
            div()
                .flex()
                .flex_col()
                .gap(sz(4.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sz(8.0))
                        .child(div().w(sz(16.0)).flex_none().text_size(sz(10.0)).text_color(tokens.muted).child((i + 1).to_string()))
                        .child(line(label.clone()).flex_1().text_size(sz(12.0)).font_weight(FontWeight(550.0)))
                        .child(
                            div()
                                .flex_none()
                                .text_size(sz(10.0))
                                .font_weight(FontWeight(650.0))
                                .text_color(tokens.accent)
                                .font_features(theme::fonts::tabular_nums())
                                .child(format!("{} {unit}", thousands(*count))),
                        ),
                )
                .child(
                    div().ml(sz(24.0)).h(sz(4.0)).rounded_full().bg(tokens.hover).child(
                        div().h_full().w(gpui::relative((*count as f32 / most).clamp(0.03, 1.0))).rounded_full().bg(tokens.accent),
                    ),
                )
        })))
        .into_any_element()
}

fn ranked_counts(items: &[(String, usize)]) -> Vec<RankedItem> {
    items
        .iter()
        .map(|(label, count)| RankedItem { id: label.clone(), label: label.clone(), detail: None, cover_art: None, image_url: None, plays: *count as u32, seconds: 0.0 })
        .collect()
}

fn notice(tokens: &Tokens, glyph: &str, title: &str, body: &str, button: Option<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(sz(9.0))
        .min_h(sz(238.0))
        .px(sz(24.0))
        .py(sz(52.0))
        .border(sz(1.0))
        .border_color(tokens.line)
        .rounded(sz(12.0))
        .text_center()
        .text_color(tokens.muted)
        .child(div().text_color(tokens.accent).opacity(0.85).child(icon(glyph, 31.0)))
        .child(div().mt(sz(5.0)).text_size(sz(15.0)).font_weight(FontWeight(650.0)).text_color(tokens.text).child(title.to_string()))
        .child(div().max_w(sz(390.0)).text_size(sz(12.0)).line_height(gpui::relative(1.5)).child(body.to_string()))
        .children(button)
        .into_any_element()
}

/// Stats → Library: totals and breakdowns for everything in the library.
fn library_view(state: &Entity<AppState>, tokens: &Tokens, content_width: f32, narrow: bool, cx: &mut App) -> AnyElement {
    state.update(cx, |s, cx| s.load_library_stats(cx));
    let (cached, loading, playlists) = {
        let s = state.read(cx);
        (s.library_stats.clone(), s.library_stats_loading, s.playlists.len())
    };
    let intro = div()
        .mt(sz(-5.0))
        .max_w(sz(660.0))
        .text_size(sz(12.0))
        .line_height(gpui::relative(1.55))
        .text_color(tokens.muted)
        .child("What is in your library right now: how much music there is, how it is tagged, and what it is made of.");
    let body = match cached {
        None => notice(tokens, "disc3", "Counting your library…", "Large libraries take a moment, since every song has to be read once.", None),
        Some(_) if loading => notice(tokens, "disc3", "Counting your library…", "Large libraries take a moment, since every song has to be read once.", None),
        Some((_, Err(error))) => {
            let target = state.clone();
            notice(
                tokens,
                "triangle-alert",
                "Library stats could not be loaded",
                &error,
                Some(
                    secondary_button("library-stats-retry", "Try again", "refresh-cw", cx)
                        .mt(sz(5.0))
                        .on_click(move |_, _, cx| {
                            target.update(cx, |s, cx| {
                                s.library_stats = None;
                                cx.notify();
                            })
                        })
                        .into_any_element(),
                ),
            )
        }
        Some((_, Ok(stats))) if stats.songs == 0 => notice(tokens, "disc3", "Your library is empty", "Add music, or connect a server, and your library stats will appear here.", None),
        Some((_, Ok(stats))) => library_stats_body(state, tokens, &stats, playlists, content_width, narrow, cx),
    };
    div().flex().flex_col().gap(sz(if narrow { 18.0 } else { 20.0 })).pb(sz(24.0)).child(intro).child(body).into_any_element()
}

fn library_stats_body(_state: &Entity<AppState>, tokens: &Tokens, stats: &LibraryStats, playlists: usize, content_width: f32, narrow: bool, _cx: &mut App) -> AnyElement {
    let percent = |part: usize| if stats.songs == 0 { 0 } else { (part as f64 / stats.songs as f64 * 100.0).round() as usize };
    let years = match (stats.oldest_year, stats.newest_year) {
        (Some(a), Some(b)) if a != b => format!("{a} – {b}"),
        (Some(a), _) => a.to_string(),
        _ => "—".into(),
    };
    let heading = overview_heading(tokens, format!("{} songs · {} of music", thousands(stats.songs), compact_duration(stats.total_seconds)));
    let cards = vec![
        card(tokens, "album", "Albums", thousands(stats.albums), None, narrow),
        card(tokens, "user-round", "Artists", thousands(stats.artists), None, narrow),
        card(tokens, "sparkles", "Genres", thousands(stats.genres), None, narrow),
        card(tokens, "list-music", "Playlists", thousands(playlists), None, narrow),
        card(tokens, "clock3", "Total time", compact_duration(stats.total_seconds), Some(format!("{} avg per song", compact_duration(stats.total_seconds / stats.songs.max(1) as f64))), narrow),
        card(tokens, "hard-drive", "Library size", if stats.total_bytes > 0.0 { size_label(stats.total_bytes) } else { "—".into() }, None, narrow),
        card(tokens, "star", "Favorites", thousands(stats.favorites), Some(format!("{}% of songs", percent(stats.favorites))), narrow),
        card(tokens, "audio-lines", "Lossless", thousands(stats.lossless), Some(format!("{} hi-res", thousands(stats.hi_res))), narrow),
        card(tokens, "gauge", "Average bit rate", stats.average_bitrate.map(|b| format!("{} kbps", b.round() as i64)).unwrap_or_else(|| "—".into()), None, narrow),
        card(
            tokens,
            "calendar-days",
            "Release years",
            years,
            stats.newest_year.zip(stats.oldest_year).map(|(n, o)| format!("{} years apart", n - o)),
            narrow,
        ),
        card(
            tokens,
            "music2",
            "Longest song",
            stats.longest.as_ref().map(|(_, s)| compact_duration(*s)).unwrap_or_else(|| "—".into()),
            stats.longest.as_ref().map(|(t, _)| t.clone()),
            narrow,
        ),
    ];
    let columns = if narrow { 2 } else { crate::components::album_grid::auto_fill_columns(content_width, 152.0, 12.0).min(6) };
    let half = if narrow { content_width - 38.0 } else { (content_width - 16.0) / 2.0 - 38.0 };
    let pair = |a: AnyElement, b: AnyElement| {
        if narrow {
            div().flex().flex_col().gap(sz(10.0)).child(a).child(b).into_any_element()
        } else {
            div().flex().items_start().gap(sz(16.0)).child(div().flex_1().min_w_0().child(a)).child(div().flex_1().min_w_0().child(b)).into_any_element()
        }
    };
    let decades = bar_chart(
        "library-decades",
        stats.decades.iter().map(|(d, _)| d.clone()).collect(),
        stats.decades.iter().map(|(_, n)| *n as f64).collect(),
        half,
        183.0,
        tokens,
    );
    let mut view = div()
        .flex()
        .flex_col()
        .gap(sz(16.0))
        .child(heading)
        .child(div().grid().grid_cols(columns as u16).gap(sz(if narrow { 8.0 } else { 12.0 })).children(cards));
    if !stats.decades.is_empty() {
        view = view.child(pair(
            chart_card(tokens, "Songs by decade", "Songs", decades, narrow),
            breakdown(tokens, "File formats", "No format information.", &ranked_counts(&stats.formats)),
        ));
    } else {
        view = view.child(breakdown(tokens, "File formats", "No format information.", &ranked_counts(&stats.formats)));
    }
    view = view.child(pair(
        breakdown(tokens, "Genre breakdown", "No genres are tagged in this library.", &ranked_counts(&stats.top_genres)),
        count_bars(tokens, "Most songs by artist", &stats.top_artists, "songs"),
    ));
    view.into_any_element()
}

impl AppState {
    /// The key the cached library stats belong to: switching between this
    /// device and a server, or reconnecting, recomputes them.
    fn library_stats_key(&self) -> String {
        if self.is_local() {
            "local".into()
        } else {
            self.client().map(|c| format!("{:p}", std::sync::Arc::as_ptr(&c))).unwrap_or_default()
        }
    }

    /// Loads every song in the background and summarizes it. Does nothing when
    /// the stats are already current, in flight, or already failed for this source.
    pub fn load_library_stats(&mut self, cx: &mut gpui::Context<Self>) {
        let key = self.library_stats_key();
        if self.library_stats_loading || key.is_empty() || self.library_stats.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        self.library_stats_loading = true;
        let started = std::time::Instant::now();
        let source = if self.is_local() {
            let songs = self.local.library.as_ref().map(|l| l.tracks.clone()).unwrap_or_default();
            Ok(songs)
        } else {
            Err(self.client())
        };
        let task = cx.background_spawn(async move {
            let songs = match source {
                Ok(songs) => songs,
                Err(Some(client)) => client.all_songs(500, None).map_err(|e| format!("{e:#}"))?,
                Err(None) => return Err("Not connected".to_string()),
            };
            Ok::<_, String>(volta_logic::library_stats::summarize(&songs))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| {
                match &result {
                    Ok(stats) => log::info!(target: crate::logs::PERF, "library stats: {} songs summarized, {:.0} ms (includes fetching them)", stats.songs, started.elapsed().as_secs_f64() * 1000.0),
                    Err(error) => log::warn!("library stats failed: {error}"),
                }
                state.library_stats = Some((key, result));
                state.library_stats_loading = false;
                cx.notify();
            });
        })
        .detach();
    }
}
