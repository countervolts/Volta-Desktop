//! Windowed rendering for long lists inside the page scroller.
//!
//! A long page (every song, every album card) used to build, lay out and
//! paint each row on every frame, and scrolling re-renders the page, so a
//! 1,700-song library cost ~25 ms per scroll frame. [`virtual_rows`] renders
//! only the rows within [`OVERSCAN`] of the visible part of `#main-content`
//! and stands in for the rest with spacers of the same height, so the
//! scrollbar and scroll position are unchanged.
//!
//! Rows are measured, not assumed: after each paint the list records where
//! it starts in the scroll content and the average row pitch (row height plus
//! `gap`), and asks for one more frame if either changed. Rows of one list
//! must therefore share a height, which holds for track rows and grid rows.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;

use gpui::{AnyElement, App, Global, IntoElement, ParentElement, ScrollHandle, SharedString, Styled, canvas, div, px};

/// Extra content rendered above and below the viewport.
const OVERSCAN: f32 = 800.0;
/// Lists this short render every row.
const MIN_ROWS: usize = 24;

/// The page scroller (`#main-content`), set by the shell.
#[derive(Clone)]
pub struct MainScroll(pub ScrollHandle);
impl Global for MainScroll {}

#[derive(Clone, Copy)]
struct Measure {
    /// The list's top in scroll-content coordinates.
    top: f32,
    pitch: f32,
    /// Pixels per CSS pixel (`rem_size / 16`; the interface scale).
    scale: f32,
}

thread_local! {
    static MEASURES: RefCell<HashMap<SharedString, Measure>> = RefCell::new(HashMap::new());
}

/// Renders `total` rows of roughly `estimate` CSS px pitch (row plus `gap`,
/// which separates consecutive rows; both in CSS px as passed to `sz`).
/// `render` builds the rows of the given range.
pub fn virtual_rows(
    id: impl Into<SharedString>,
    total: usize,
    estimate: f32,
    gap: f32,
    cx: &mut App,
    render: impl FnOnce(Range<usize>, &mut App) -> Vec<AnyElement>,
) -> AnyElement {
    let id = id.into();
    let handle = cx.try_global::<MainScroll>().map(|s| s.0.clone());
    let Some(handle) = handle.filter(|_| total > MIN_ROWS) else {
        return rows_block(render(0..total, cx), 0, total, gap).into_any_element();
    };

    let known = MEASURES.with(|m| m.borrow().get(&id).copied());
    let measure = known.unwrap_or(Measure { top: 0.0, pitch: estimate.max(1.0), scale: 1.0 });
    let gap_px = gap * measure.scale;
    let viewport = handle.bounds().size.height;
    let viewport = if viewport > px(0.0) { f32::from(viewport) } else { 1200.0 };
    let scroll_y = -f32::from(handle.offset().y);
    let from = ((scroll_y - OVERSCAN - measure.top) / measure.pitch).floor().max(0.0) as usize;
    let to = (((scroll_y + viewport + OVERSCAN - measure.top) / measure.pitch).ceil().max(0.0) as usize).min(total);
    let from = from.min(to);

    let above = from as f32 * measure.pitch;
    let below = if to < total { (total - to) as f32 * measure.pitch - gap_px } else { 0.0 };
    let rendered = to - from;
    let rows = rows_block(render(from..to, cx), from, total, gap);

    let probe_id = id.clone();
    let probe = canvas(
        move |bounds, window, _| {
            if rendered == 0 {
                return;
            }
            // Content coordinates do not move as the page scrolls.
            let block_top = f32::from(bounds.origin.y - handle.bounds().origin.y - handle.offset().y);
            let scale = f32::from(window.rem_size()) / 16.0;
            let block_height = f32::from(bounds.size.height) + if from + rendered == total { gap * scale } else { 0.0 };
            let next = Measure { top: block_top - from as f32 * measure.pitch, pitch: block_height / rendered as f32, scale };
            let changed = known.is_none_or(|old| {
                (old.top - next.top).abs() > 0.5 || (old.pitch - next.pitch).abs() > 0.25 || old.scale != next.scale
            });
            if changed {
                MEASURES.with(|m| m.borrow_mut().insert(probe_id, next));
                window.request_animation_frame();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full();

    div()
        .flex()
        .flex_col()
        .child(div().h(px(above)).flex_none())
        .child(rows.relative().child(probe))
        .child(div().h(px(below.max(0.0))).flex_none())
        .into_any_element()
}

/// Rows `from..` stacked with `gap` between them (not after the list's last row).
fn rows_block(rows: Vec<AnyElement>, from: usize, total: usize, gap: f32) -> gpui::Div {
    div().flex().flex_col().children(rows.into_iter().enumerate().map(move |(i, row)| {
        let last = from + i + 1 == total;
        div().when(!last && gap > 0.0, |d| d.pb(crate::theme::sz(gap))).child(row).into_any_element()
    }))
}

use gpui::prelude::FluentBuilder as _;
