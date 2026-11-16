//! `AlbumGrid` from components.tsx: `.album-grid`, `.shelf-grid` (one
//! horizontally scrolling row) and `.featured-grid` (bigger shelf cards).
//!
//! Track sizes follow theme.css, including the viewport media queries:
//! - default: grid `repeat(auto-fill, minmax(160px, 1fr))`, gap 27/20;
//!   shelf `(100% − 80px) / 5`; featured `(100% − 60px) / 4`.
//! - `min-width: 1600px`: grid minmax 190px; shelf `(100% − 100px) / 6`.
//! - `max-width: 1150px`: grid minmax 145px, gap 23/17; shelf
//!   `(100% − 51px) / 4`; featured `(100% − 34px) / 3`.
//! - `max-width: 640px`: grid 2 columns, gap 25/15; shelf `(100% − 26px) / 2.3`;
//!   featured 70 %; caption 13px; play button always visible (32px).

use gpui::{AnyElement, App, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window, div, img, point, prelude::*, px};
use volta_logic::format::album_name;
use volta_logic::model::AlbumRecord;

use crate::app_state::{AppState, Page, Route};
use crate::components::ui::{icon_with, line};
use crate::theme::{self, sz};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GridKind {
    Grid,
    /// `albumGrid(albums, false, true)`: grid columns, featured captions.
    FeaturedGrid,
    Shelf,
    Featured,
}

/// `repeat(auto-fill, minmax(min, 1fr))` column count.
pub fn auto_fill_columns(available: f32, min: f32, gap: f32) -> usize {
    (((available + gap) / (min + gap)).floor() as usize).max(1)
}

/// Layout metrics for one grid kind at a viewport width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridMetrics {
    /// Column count (grid) — shelves scroll instead.
    pub columns: usize,
    /// Card width in CSS px for shelves; `None` for `1fr` grid columns.
    pub card_width: Option<f32>,
    pub column_gap: f32,
    pub row_gap: f32,
}

pub fn metrics(kind: GridKind, content_width: f32, viewport_width: f32) -> GridMetrics {
    metrics_with(kind, content_width, viewport_width, false)
}

/// `inspector_open` applies the `.inspector-open` shelf rules at ≤1150px.
pub fn metrics_with(kind: GridKind, content_width: f32, viewport_width: f32, inspector_open: bool) -> GridMetrics {
    let narrow = viewport_width <= 640.0;
    let compact = viewport_width <= 1150.0;
    let wide = viewport_width >= 1600.0;
    let (column_gap, row_gap) = if narrow {
        (15.0, 25.0)
    } else if compact {
        (17.0, 23.0)
    } else {
        (20.0, 27.0)
    };
    match kind {
        GridKind::FeaturedGrid => GridMetrics { columns: 1, card_width: None, column_gap, row_gap },
        GridKind::Grid => {
            let columns = if narrow {
                2
            } else {
                let min = if compact { 145.0 } else if wide { 190.0 } else { 160.0 };
                auto_fill_columns(content_width, min, column_gap)
            };
            GridMetrics { columns, card_width: None, column_gap, row_gap }
        }
        GridKind::Shelf | GridKind::Featured => {
            let width = if narrow {
                if kind == GridKind::Featured { content_width * 0.7 } else { (content_width - 26.0) / 2.3 }
            } else if kind == GridKind::Featured {
                if compact && inspector_open {
                    (content_width - 17.0) / 2.0
                } else if compact {
                    (content_width - 34.0) / 3.0
                } else {
                    (content_width - 60.0) / 4.0
                }
            } else if compact && inspector_open {
                (content_width - 34.0) / 3.0
            } else if compact {
                (content_width - 51.0) / 4.0
            } else if wide {
                (content_width - 100.0) / 6.0
            } else {
                (content_width - 80.0) / 5.0
            };
            GridMetrics { columns: 0, card_width: Some(width.max(0.0)), column_gap, row_gap }
        }
    }
}

fn open_album(state: &Entity<AppState>, album: &AlbumRecord, cx: &mut App) {
    let route = Route {
        page: Page::Album,
        id: Some(album.id.clone()),
        title: Some(album_name(album).to_string()),
    };
    state.update(cx, |state, cx| state.navigate(route, cx));
}

fn album_card(
    state: &Entity<AppState>,
    album: &AlbumRecord,
    key: &str,
    kind: GridKind,
    narrow: bool,
    tile: f32,
    cx: &mut App,
) -> gpui::Stateful<gpui::Div> {
    let tokens = theme::tokens(cx).clone();
    let featured = matches!(kind, GridKind::Featured | GridKind::FeaturedGrid);
    let size = if featured { 600 } else { 400 };
    let cover = state.update(cx, |state, cx| state.cover(album.cover_art.as_deref(), size, cx));
    let group = SharedString::from(format!("album-card-{key}"));
    let (open_a, open_b, play) = (album.clone(), album.clone(), album.id.clone());
    let (state_a, state_b, state_c) = (state.clone(), state.clone(), state.clone());
    let (title_size, title_weight, sub_size, sub_mt) = if featured {
        (15.0, FontWeight::SEMIBOLD, 12.0, 3.0)
    } else {
        (if narrow { 13.0 } else { 12.0 }, FontWeight::NORMAL, 11.0, 1.0)
    };
    let play_size = if narrow { 32.0 } else { 35.0 };
    let context_items = crate::components::menu::album_context_items(state, album, cx);
    let context_state = state.clone();
    div()
        .id(SharedString::from(format!("album-{key}")))
        .group(group.clone())
        .min_w_0()
        .on_mouse_down(gpui::MouseButton::Right, move |event, _, cx| {
            cx.stop_propagation();
            crate::components::menu::open(&context_state, event.position, gpui::Anchor::TopLeft, context_items.clone(), cx);
        })
        .child(
            div()
                .relative()
                .child(
                    div()
                        .id(SharedString::from(format!("album-art-{key}")))
                        .relative()
                        .w_full()
                        .rounded(sz(7.0))
                        .overflow_hidden()
                        .bg(tokens.surface)
                        .cursor_pointer()
                        .on_click(move |_, _, cx| open_album(&state_a, &open_a, cx))
                        // `aspect-ratio: 1`.
                        .child(div().w_full().pb(gpui::relative(1.0)).when_some(cover, |d, image| {
                            d.child(img(image).absolute().top_0().left_0().size_full().rounded(sz(7.0)))
                        }))
                        // No artwork id: `<Music2>` at 38%, stroke 1, muted.
                        .when(album.cover_art.is_none(), |d| {
                            d.child(
                                div().absolute().inset_0().flex().items_center().justify_center().text_color(tokens.muted).child(
                                    crate::components::ui::icon_with("music2", tile * 0.38, Some(1.0), false),
                                ),
                            )
                        })
                        // `.artwork-link:hover { filter: brightness(.92) }`.
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .hover(|s| s.bg(gpui::black().opacity(0.08))),
                        ),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("album-play-{key}")))
                        .absolute()
                        .bottom(sz(10.0))
                        .right(sz(10.0))
                        .size(sz(play_size))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(gpui::white())
                        .bg(theme::hex("#303030b8"))
                        .when(!narrow, |d| d.opacity(0.0).group_hover(group, |s| s.opacity(1.0)))
                        .hover(|s| s.bg(tokens.accent))
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            let id = play.clone();
                            state_c.update(cx, |state, cx| state.play_album(id, cx));
                        })
                        .child(icon_with("play", 20.0, None, true)),
                ),
        )
        .child(
            div()
                .id(SharedString::from(format!("album-caption-{key}")))
                .pt(sz(8.0))
                .cursor_pointer()
                .on_click(move |_, _, cx| open_album(&state_b, &open_b, cx))
                .child(
                    line(album_name(album).to_string())
                        .text_size(sz(title_size))
                        .font_weight(title_weight)
                        .line_height(gpui::relative(1.4)),
                )
                .child(
                    line(album.artist.clone().filter(|a| !a.is_empty()).unwrap_or_else(|| "Unknown artist".into()))
                        .mt(sz(sub_mt))
                        .text_size(sz(sub_size))
                        .line_height(gpui::relative(1.4))
                        .text_color(tokens.secondary),
                ),
        )
}

thread_local! {
    /// Shelf scroll positions by shelf id, so the edge fades know where the
    /// shelf is and a shelf keeps its place across re-renders.
    static SHELF_SCROLL: std::cell::RefCell<std::collections::HashMap<String, gpui::ScrollHandle>> = Default::default();
}

fn shelf_scroll(id: &str) -> gpui::ScrollHandle {
    SHELF_SCROLL.with(|m| {
        m.borrow_mut()
            .entry(id.to_string())
            .or_insert_with(|| {
                let handle = gpui::ScrollHandle::default();
                // `VOLTA_SHOT_SHELF=<px>`: shelves start scrolled that far (screenshots).
                #[cfg(feature = "shots")]
                if let Some(x) = std::env::var("VOLTA_SHOT_SHELF").ok().and_then(|v| v.parse::<f32>().ok()) {
                    handle.set_offset(gpui::point(gpui::px(-x), gpui::px(0.0)));
                }
                handle
            })
            .clone()
    })
}

thread_local! {
    /// Latest page-scroll per shelf: (generation, target offset in px). A shelf that is
    /// still gliding when the next button press arrives continues from its target.
    static SHELF_GLIDE: std::cell::RefCell<std::collections::HashMap<String, (u64, f32)>> = Default::default();
}

const GLIDE_MS: f32 = 280.0;

/// Moves a shelf one page: `forward` shows the next covers. A page is as many whole covers as fit.
fn page_shelf(id: &str, handle: &gpui::ScrollHandle, forward: bool, pitch: f32, content_width: f32, gap: f32, window: &mut Window, cx: &mut App) {
    let rem = window.rem_size();
    let to_px = |css: f32| f32::from(sz(css).to_pixels(rem));
    let max = f32::from(handle.max_offset().x);
    let current = -f32::from(handle.offset().x);
    let (generation, base) = SHELF_GLIDE.with(|g| {
        let g = g.borrow();
        g.get(id).map_or((1, current), |(n, target)| (n + 1, *target))
    });
    let covers = ((content_width + gap) / pitch).floor().max(1.0);
    let step = covers * to_px(pitch);
    let target = (base + if forward { step } else { -step }).clamp(0.0, max);
    if (target - current).abs() < 0.5 {
        return;
    }
    SHELF_GLIDE.with(|g| g.borrow_mut().insert(id.to_string(), (generation, target)));
    if cx.reduce_motion() {
        handle.set_offset(point(px(-target), px(0.0)));
        SHELF_GLIDE.with(|g| g.borrow_mut().remove(id));
        window.refresh();
        return;
    }
    glide_shelf(id.to_string(), handle.clone(), generation, current, target, std::time::Instant::now(), window);
}

fn glide_shelf(id: String, handle: gpui::ScrollHandle, generation: u64, from: f32, to: f32, started: std::time::Instant, window: &mut Window) {
    // A newer press took over.
    if SHELF_GLIDE.with(|g| g.borrow().get(&id).is_none_or(|(n, _)| *n != generation)) {
        return;
    }
    let t = (started.elapsed().as_secs_f32() * 1000.0 / GLIDE_MS).min(1.0);
    let eased = 1.0 - (1.0 - t).powi(3);
    handle.set_offset(point(px(-(from + (to - from) * eased)), px(0.0)));
    window.refresh();
    if t < 1.0 {
        window.on_next_frame(move |window, _| glide_shelf(id, handle, generation, from, to, started, window));
    } else {
        SHELF_GLIDE.with(|g| g.borrow_mut().remove(&id));
    }
}

/// Renders `albums` as `kind`. `id` must be unique on the page (it keys the
/// shelf scroll position and the card hover groups).
pub fn album_grid(
    state: &Entity<AppState>,
    id: &str,
    albums: &[AlbumRecord],
    kind: GridKind,
    content_width: f32,
    viewport_width: f32,
    cx: &mut App,
) -> AnyElement {
    let inspector_open = state.read(cx).panel.is_some() && viewport_width > 860.0;
    let m = metrics_with(kind, content_width, viewport_width, inspector_open);
    let narrow = viewport_width <= 640.0;
    let tile = m.card_width.unwrap_or_else(|| {
        let columns = m.columns.max(1) as f32;
        (content_width - m.column_gap * (columns - 1.0)) / columns
    });
    let card = |i: usize, cx: &mut App| {
        let card = album_card(state, &albums[i], &format!("{id}-{i}"), kind, narrow, tile, cx);
        match m.card_width {
            Some(width) => card.w(sz(width)).flex_none().into_any_element(),
            None => card.into_any_element(),
        }
    };
    match kind {
        GridKind::Grid | GridKind::FeaturedGrid => {
            // One grid per row so long grids can render only the rows near
            // the viewport (virtual_rows); the layout matches one big grid.
            let columns = m.columns.max(1);
            let rows = albums.len().div_ceil(columns);
            // Square tile plus the title block, refined by measurement.
            let estimate = tile + 50.0 + m.row_gap;
            crate::components::virtual_rows::virtual_rows(format!("album-grid-{id}"), rows, estimate, m.row_gap, cx, |range, cx| {
                range
                    .map(|row| {
                        let cards: Vec<AnyElement> = (row * columns..((row + 1) * columns).min(albums.len())).map(|i| card(i, cx)).collect();
                        div().grid().grid_cols(columns as u16).gap_x(sz(m.column_gap)).children(cards).into_any_element()
                    })
                    .collect()
            })
        }
        _ => {
            let cards: Vec<AnyElement> = (0..albums.len()).map(|i| card(i, cx)).collect();
            // The shelf is clipped to the page content, so covers never run beneath the
            // sidebar or into the side margins. The wheel is not handled here, so it
            // scrolls the page; a bare chevron in each margin pages through the shelf,
            // shown only while the pointer is over the shelf.
            let (_, pad_x, _) = crate::theme::main_padding(viewport_width);
            let handle = shelf_scroll(id);
            let offset = -f32::from(handle.offset().x);
            let max = f32::from(handle.max_offset().x);
            // Scroll range is only known after the first layout: render once more.
            if max <= 0.0 && f32::from(handle.bounds().size.width) <= 0.0 {
                let state = state.clone();
                cx.defer(move |cx| state.update(cx, |_, cx| cx.notify()));
            }
            let tokens = crate::theme::tokens(cx).clone();
            let pitch = m.card_width.unwrap_or(tile) + m.column_gap;
            let (content, gap) = (content_width, m.column_gap);
            let hover_group = SharedString::from(format!("shelf-hover-{id}"));
            // Centred in the side margin (at least 30px wide) and on the artwork.
            let button_width = pad_x.max(30.0);
            let nav = |forward: bool| {
                let (handle, shelf_id, group) = (handle.clone(), id.to_string(), hover_group.clone());
                div()
                    .id(SharedString::from(format!("shelf-{id}-{}", if forward { "next" } else { "prev" })))
                    .absolute()
                    .top(sz(tile / 2.0 - 22.0))
                    .map(|d| {
                        let inset = sz((pad_x - button_width) / 2.0);
                        if forward { d.right(inset) } else { d.left(inset) }
                    })
                    .w(sz(button_width))
                    .h(sz(44.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(tokens.text)
                    .opacity(0.0)
                    .group_hover(group, |s| s.opacity(0.85))
                    .hover(|s| s.opacity(1.0))
                    .cursor_pointer()
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        page_shelf(&shelf_id, &handle, forward, pitch, content, gap, window, cx);
                    })
                    .child(icon_with(if forward { "chevron-right" } else { "chevron-left" }, 24.0, Some(2.0), false))
            };
            // Reaches into the side margins so they count as part of the hover area.
            div()
                .relative()
                .group(hover_group.clone())
                .ml(sz(-pad_x))
                .mr(sz(-pad_x))
                .px(sz(pad_x))
                .child(
                    div()
                        .id(SharedString::from(format!("shelf-{id}")))
                        .track_scroll(&handle)
                        .flex()
                        .gap(sz(m.column_gap))
                        .overflow_x_hidden()
                        .pb(sz(10.0))
                        .children(cards),
                )
                .when(offset > 0.5, |d| d.child(nav(false)))
                .when(max - offset > 0.5, |d| d.child(nav(true)))
                .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_fill_matches_css() {
        // 1440 window: 1440 − 234 sidebar − 74 padding = 1132 → 6 columns.
        assert_eq!(auto_fill_columns(1132.0, 160.0, 20.0), 6);
        assert_eq!(auto_fill_columns(100.0, 160.0, 20.0), 1);
        assert_eq!(auto_fill_columns(340.0, 160.0, 20.0), 2);
    }

    #[test]
    fn shelf_widths_follow_media_queries() {
        assert_eq!(metrics(GridKind::Shelf, 1132.0, 1440.0).card_width, Some(210.4));
        assert_eq!(metrics(GridKind::Featured, 1132.0, 1440.0).card_width, Some(268.0));
        assert_eq!(metrics(GridKind::Shelf, 1000.0, 1700.0).card_width, Some(150.0));
        assert_eq!(metrics(GridKind::Shelf, 851.0, 1100.0).card_width, Some(200.0));
        assert_eq!(metrics(GridKind::Grid, 350.0, 390.0).columns, 2);
    }
}
