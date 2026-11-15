//! Home: the `route.page === "home"` branch of App.tsx.
//!
//! From Your Collection (featured shelf) → Recently Played (shelf, 8) →
//! Made for you (recommendations) → Recently Added (shelf) →
//! Heavy Rotation (shelf) → empty state.

use gpui::{AnyElement, App, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window, div, prelude::*};

use crate::app_state::{AppState, Page, Route};
use crate::components::album_grid::{GridKind, album_grid};
use crate::components::motion::spinner;
use crate::components::ui::{icon, icon_with};
use crate::theme::{self, sz};

pub fn page_heading(title: &str, cx: &App) -> impl IntoElement {
    page_heading_with(title, Vec::new(), cx)
}

/// `.page-heading` with `.page-heading-actions` (gap 10, trailing).
pub fn page_heading_with(title: &str, actions: Vec<AnyElement>, cx: &App) -> impl IntoElement {
    let tokens = theme::tokens(cx);
    let viewport = theme::viewport(cx);
    let narrow = viewport <= 640.0;
    // h1: 31px; 27px at ≤860px; 29px at ≤640px.
    let h1 = if narrow { 29.0 } else if viewport <= 860.0 { 27.0 } else { 31.0 };
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(sz(14.0))
        .min_h(sz(if narrow { 46.0 } else { 54.0 }))
        .mb(sz(21.0))
        .pb(sz(if narrow { 12.0 } else { 15.0 }))
        .border_b(sz(1.0))
        .border_color(tokens.line)
        .child(
            div()
                .min_w_0()
                .flex_shrink(1.0)
                .text_size(sz(h1))
                .font_weight(FontWeight::BOLD)
                .letter_spacing(sz(-0.65))
                .line_height(gpui::relative(1.2))
                .child(title.to_string()),
        )
        .when(!actions.is_empty(), |d| {
            d.child(div().flex().items_center().gap(sz(10.0)).ml_auto().flex_none().children(actions))
        })
}

/// `.section-heading`. With `link`, the title is a button with a chevron that
/// opens that page.
pub fn section_heading(
    state: &Entity<AppState>,
    title: &str,
    note: Option<&str>,
    link: Option<Page>,
    cx: &App,
) -> impl IntoElement {
    let tokens = theme::tokens(cx).clone();
    let h2 = div()
        .text_size(sz(18.0))
        .font_weight(FontWeight(650.0))
        .letter_spacing(sz(-0.2))
        .child(title.to_string());
    let title_el: AnyElement = match link {
        Some(page) => {
            let state = state.clone();
            div()
                .id(SharedString::from(format!("section-{title}")))
                .flex()
                .items_center()
                .gap(sz(4.0))
                .cursor_pointer()
                .on_click(move |_, _, cx| state.update(cx, |s, cx| s.navigate(Route::page(page), cx)))
                .child(h2)
                .child(div().text_color(tokens.muted).child(icon("chevron-right", 18.0)))
                .into_any_element()
        }
        None => h2.into_any_element(),
    };
    div()
        .flex()
        .items_baseline()
        .justify_between()
        .gap(sz(15.0))
        .mb(sz(12.0))
        .child(title_el)
        // `.section-heading > span { display: none }` at ≤860px.
        .when_some(note.filter(|_| theme::viewport(cx) > 860.0), |d, note| {
            d.child(div().text_size(sz(12.0)).text_color(tokens.muted).child(note.to_string()))
        })
}

/// `.music-section`.
fn section(children: impl IntoIterator<Item = AnyElement>, cx: &App) -> gpui::Div {
    div().mb(sz(if theme::viewport(cx) <= 640.0 { 28.0 } else { 32.0 })).min_w_0().children(children)
}

/// `EmptyState` from components.tsx (`.empty-state`).
pub fn empty_state(title: &str, message: &str, cx: &App) -> impl IntoElement {
    let tokens = theme::tokens(cx);
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .min_h(sz(300.0))
        .px(sz(20.0))
        .py(sz(45.0))
        .text_center()
        .child(div().mb(sz(18.0)).text_color(tokens.muted).child(icon_with("music2", 42.0, Some(1.3), false)))
        .child(div().mb(sz(9.0)).text_size(sz(21.0)).font_weight(FontWeight::BOLD).letter_spacing(sz(-0.3)).child(title.to_string()))
        .child(
            div()
                .max_w(sz(340.0))
                .text_size(sz(13.0))
                .line_height(gpui::relative(1.5))
                .text_color(tokens.secondary)
                .child(message.to_string()),
        )
}

pub fn render(state: &Entity<AppState>, content_width: f32, window: &mut Window, cx: &mut App) -> impl IntoElement {
    let tokens = theme::tokens(cx).clone();
    let viewport = theme::viewport_css_width(window);
    // `recommendedResults`, cached until an input changes; exposures logged.
    state.update(cx, |s, cx| {
        s.refresh_recommendations(cx);
        s.record_impressions();
    });
    let recommended: Vec<_> = state.read(cx).learning.slate.iter().map(|r| r.album.clone()).collect();
    let home = &state.read(cx).home;
    let loading = home.loading;
    let error = home.error.clone();
    let albums = home.albums.clone();
    let recent = home.recent.clone();
    let played: Vec<_> = home.recently_played.iter().take(8).cloned().collect();
    let frequent = home.frequent.clone();
    let filter = state.read(cx).filter.to_lowercase();
    let matches = |album: &volta_logic::model::AlbumRecord| {
        filter.is_empty()
            || format!("{} {}", volta_logic::format::album_name(album), album.artist.as_deref().unwrap_or(""))
                .to_lowercase()
                .contains(&filter)
    };
    let recent: Vec<_> = recent.into_iter().filter(|a| matches(a)).collect();
    let frequent_visible: Vec<_> = frequent.iter().filter(|a| matches(a)).cloned().collect();

    let mut page = div().flex().flex_col().child(page_heading("Home", cx));
    if loading && albums.is_empty() && recent.is_empty() {
        return page
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sz(10.0))
                    .text_size(sz(13.0))
                    .text_color(tokens.secondary)
                    .child(spinner(17.0))
                    .child("Loading your library…"),
            )
            .into_any_element();
    }
    if let Some(error) = error {
        page = page.child(div().mb(sz(20.0)).text_color(tokens.accent).text_size(sz(13.0)).child(error));
    }
    let grid = |id: &str, list: &[_], kind, cx: &mut App| album_grid(state, id, list, kind, content_width, viewport, cx);

    page = page.child(section([
        section_heading(state, "From Your Collection", Some("Rediscover something good."), None, cx).into_any_element(),
        grid("collection", &albums, GridKind::Featured, cx),
    ], cx));
    if !played.is_empty() {
        page = page.child(section([
            section_heading(state, "Recently Played", Some("Your latest listens."), Some(Page::Played), cx)
                .into_any_element(),
            grid("played", &played, GridKind::Shelf, cx),
        ], cx));
    }
    if !recommended.is_empty() {
        page = page.child(section([
            section_heading(state, "Made for you", Some("Based on your listening, with room to wander."), None, cx)
                .into_any_element(),
            grid("recommended", &recommended, GridKind::Shelf, cx),
        ], cx));
    }
    page = page.child(section([
        section_heading(state, "Recently Added", None, Some(Page::Recent), cx).into_any_element(),
        grid("recent", &recent, GridKind::Shelf, cx),
    ], cx));
    if !frequent.is_empty() {
        page = page.child(section([
            section_heading(state, "Heavy Rotation", None, None, cx).into_any_element(),
            grid("frequent", &frequent_visible, GridKind::Shelf, cx),
        ], cx));
    }
    if albums.is_empty() && recent.is_empty() && !loading {
        page = page.child(empty_state(
            "Your music belongs here",
            "Add music to Navidrome, then refresh your library.",
            cx,
        ));
    }
    page.into_any_element()
}
