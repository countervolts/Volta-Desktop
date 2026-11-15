//! The interface scale preview (`.scale-preview` in App.tsx). Drawn in real
//! pixels so it keeps one size while the rest of the UI zooms beneath it.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{AnyElement, App, Bounds, BoxShadow, Entity, FontWeight, IntoElement, MouseButton, ParentElement, Pixels, SharedString, Styled, canvas, deferred, div, point, prelude::*, px};
use volta_logic::storage as keys;

use crate::app_state::AppState;
use crate::theme::{self, mix, transparent};

/// Applies `scale` (percent, 70–150) to the window immediately.
pub fn apply_scale(scale: f32, cx: &mut App) {
    cx.global_mut::<theme::ThemeState>().scale = scale.clamp(70.0, 150.0);
    theme::apply(cx);
    cx.refresh_windows();
}

pub fn adjust(state: &Entity<AppState>, delta: f32, cx: &mut App) {
    let Some((original, current)) = state.read(cx).scale_preview else { return };
    let next = (current + delta).clamp(70.0, 150.0);
    state.update(cx, |s, cx| {
        s.scale_preview = Some((original, next));
        cx.notify();
    });
    apply_scale(next, cx);
}

pub fn cancel(state: &Entity<AppState>, cx: &mut App) {
    let Some((original, _)) = state.read(cx).scale_preview else { return };
    state.update(cx, |s, cx| {
        s.scale_preview = None;
        cx.notify();
    });
    apply_scale(original, cx);
}

pub fn keep(state: &Entity<AppState>, cx: &mut App) {
    let Some((_, scale)) = state.read(cx).scale_preview else { return };
    state.update(cx, |s, cx| {
        s.scale_preview = None;
        s.storage.set(keys::INTERFACE_SCALE_KEY, &format!("{}", scale.round() as i32));
        cx.notify();
    });
    apply_scale(scale, cx);
}

thread_local! {
    static SLIDER: Rc<Cell<Bounds<Pixels>>> = Rc::new(Cell::new(Bounds::default()));
}

pub fn render(state: &Entity<AppState>, cx: &App) -> Option<AnyElement> {
    let (_, scale) = state.read(cx).scale_preview?;
    let tokens = theme::tokens(cx).clone();
    let slider_cell = SLIDER.with(Rc::clone);
    let progress = (scale - 70.0) / 80.0;
    let (s1, s2, s3, s4, s5) = (state.clone(), state.clone(), state.clone(), state.clone(), state.clone());
    let set_from = move |x: Pixels, cx: &mut App| {
        let bounds = SLIDER.with(|c| c.get());
        let f = (f32::from(x - bounds.origin.x) / f32::from(bounds.size.width).max(1.0)).clamp(0.0, 1.0);
        let target = (70.0 + f * 80.0).round();
        let current = s5.read(cx).scale_preview.map(|p| p.1).unwrap_or(target);
        adjust(&s5, target - current, cx);
    };
    let set_down = set_from.clone();
    let step = |label: &'static str, delta: f32, target: Entity<AppState>| {
        div()
            .id(SharedString::from(format!("scale-step-{label}")))
            .size(px(25.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(tokens.line)
            .rounded(px(6.0))
            .bg(tokens.canvas)
            .hover(|s| s.bg(tokens.hover))
            .text_color(tokens.text)
            .text_size(px(14.0))
            .cursor_pointer()
            .on_click(move |_, _, cx| adjust(&target, delta, cx))
            .child(label)
    };
    let button = |label: String, primary: bool| {
        div()
            .id(SharedString::from(format!("scale-{primary}")))
            .flex()
            .items_center()
            .justify_center()
            .min_h(px(34.0))
            .px(px(20.0))
            .py(px(7.0))
            .rounded(px(7.0))
            .text_size(px(13.0))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .map(|d| {
                if primary {
                    d.bg(theme::hex("#e73650")).text_color(gpui::white()).hover(|s| s.bg(theme::hex("#fa4059")))
                } else {
                    d.bg(tokens.surface).text_color(tokens.accent).hover(|s| s.bg(tokens.hover))
                }
            })
            .child(label)
    };
    Some(
        deferred(
            div().absolute().top(px(14.0)).left_0().right_0().flex().justify_center().child(
                div()
                    .id("scale-preview")
                    .occlude()
                    .flex()
                    .items_center()
                    .gap(px(18.0))
                    .w(px(620.0))
                    .px(px(15.0))
                    .py(px(13.0))
                    .border_1()
                    .border_color(mix(tokens.text, 22.0, transparent()))
                    .rounded(px(14.0))
                    .backdrop_blur(px(14.0))
                    .bg(mix(tokens.surface, 94.0, transparent()))
                    .shadow(vec![BoxShadow {
                        color: theme::hex("#00000059"),
                        offset: point(px(0.0), px(14.0)),
                        blur_radius: px(38.0),
                        spread_radius: px(0.0),
                        inset: false,
                    }])
                    .text_color(tokens.text)
                    .on_mouse_move(move |e: &gpui::MouseMoveEvent, _, cx| {
                        if e.pressed_button == Some(MouseButton::Left) && SLIDER.with(|c| c.get()).contains(&e.position) {
                            set_from(e.position.x, cx);
                        }
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .gap(px(6.0))
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(9.0))
                                    .child(
                                        div()
                                            .px(px(7.0))
                                            .py(px(2.0))
                                            .rounded_full()
                                            .bg(tokens.selection)
                                            .text_color(tokens.accent)
                                            .text_size(px(10.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .letter_spacing(px(10.0 * 0.07))
                                            .child("PREVIEW"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .font_features(theme::fonts::tabular_nums())
                                            .child(format!("{}%", scale.round() as i32)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .line_height(gpui::relative(1.45))
                                    .text_color(tokens.muted)
                                    .child("Use ← and → (hold Shift for 10%) or drag the slider. Browse around to see how it feels, then keep or cancel."),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(9.0))
                                    .child(step("−", -1.0, s1))
                                    .child(
                                        div()
                                            .id("scale-slider")
                                            .relative()
                                            .flex_1()
                                            .h(px(16.0))
                                            .flex()
                                            .items_center()
                                            .cursor_pointer()
                                            .on_mouse_down(MouseButton::Left, move |e, _, cx| set_down(e.position.x, cx))
                                            .child(
                                                div()
                                                    .w_full()
                                                    .h(px(4.0))
                                                    .rounded_full()
                                                    .bg(tokens.line)
                                                    .child(div().h_full().w(gpui::relative(progress)).rounded_full().bg(tokens.accent)),
                                            )
                                            .child(
                                                div()
                                                    .absolute()
                                                    .left(gpui::relative(progress))
                                                    .ml(px(-7.0))
                                                    .size(px(14.0))
                                                    .rounded_full()
                                                    .bg(tokens.accent),
                                            )
                                            .child(canvas(move |b, _, _| slider_cell.set(b), |_, _, _, _| {}).absolute().size_full()),
                                    )
                                    .child(step("+", 1.0, s2)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(button("Cancel".into(), false).on_click(move |_, _, cx| cancel(&s3, cx)))
                            .child(button(format!("Keep {}%", scale.round() as i32), true).on_click(move |_, _, cx| keep(&s4, cx))),
                    ),
            ),
        )
        .with_priority(4)
        .into_any_element(),
    )
}
