//! `input[type="range"]` as styled by `.engine-dial input[type="range"]`:
//! a 6px track filled to the value in `--secondary`, the rest in
//! `--text` at 16%, and a 14px thumb (2px `--surface` ring). Dragging keeps
//! tracking outside the track until the button is released.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, BoxShadow, DispatchPhase, IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent, ParentElement,
    Pixels, SharedString, Styled, canvas, div, point, prelude::*, px,
};

use crate::theme::{Tokens, mix, sz, transparent};

thread_local! {
    static BOUNDS: RefCell<HashMap<SharedString, Bounds<Pixels>>> = RefCell::new(HashMap::new());
    static ACTIVE: RefCell<Option<SharedString>> = const { RefCell::new(None) };
}

fn fraction(id: &SharedString, x: Pixels) -> Option<f32> {
    let bounds = BOUNDS.with(|b| b.borrow().get(id).copied())?;
    Some((f32::from(x - bounds.origin.x) / f32::from(bounds.size.width).max(1.0)).clamp(0.0, 1.0))
}

/// A 0–1 slider; `on_change` receives the new value, already rounded to
/// whole percent (the web's `min=0 max=100` integer steps).
pub fn range(id: impl Into<SharedString>, value: f32, tokens: &Tokens, on_change: impl Fn(f32, &mut App) + 'static) -> AnyElement {
    let id: SharedString = id.into();
    let on_change: Rc<dyn Fn(f32, &mut App)> = Rc::new(on_change);
    let value = value.clamp(0.0, 1.0);
    let last = Rc::new(Cell::new(value));
    let emit = {
        let on_change = on_change.clone();
        let last = last.clone();
        move |f: f32, cx: &mut App| {
            let stepped = (f * 100.0).round() / 100.0;
            if (stepped - last.get()).abs() > f32::EPSILON {
                last.set(stepped);
                on_change(stepped, cx);
            }
        }
    };
    let (down_id, paint_id, bounds_id) = (id.clone(), id.clone(), id.clone());
    let down_emit = emit.clone();
    let thumb = sz(14.0);
    div()
        .id(SharedString::from(format!("range-{id}")))
        .relative()
        .w_full()
        .h(sz(20.0))
        .flex()
        .items_center()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |e, _, cx| {
            ACTIVE.with(|a| *a.borrow_mut() = Some(down_id.clone()));
            if let Some(f) = fraction(&down_id, e.position.x) {
                down_emit(f, cx);
            }
        })
        .child(
            div()
                .relative()
                .w_full()
                .h(sz(6.0))
                .rounded_full()
                .overflow_hidden()
                .bg(mix(tokens.text, 16.0, transparent()))
                .child(div().absolute().left_0().top_0().bottom_0().w(gpui::relative(value)).bg(tokens.secondary))
                .child(
                    canvas(
                        move |b, _, _| {
                            BOUNDS.with(|m| m.borrow_mut().insert(bounds_id.clone(), b));
                        },
                        move |_, _, window, _| {
                            let id = paint_id.clone();
                            let emit = emit.clone();
                            window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                                if phase != DispatchPhase::Bubble || ACTIVE.with(|a| a.borrow().as_ref() != Some(&id)) {
                                    return;
                                }
                                if e.pressed_button != Some(MouseButton::Left) {
                                    ACTIVE.with(|a| *a.borrow_mut() = None);
                                    return;
                                }
                                if let Some(f) = fraction(&id, e.position.x) {
                                    emit(f, cx);
                                }
                            });
                            window.on_mouse_event(move |_: &MouseUpEvent, _, _, _| ACTIVE.with(|a| *a.borrow_mut() = None));
                        },
                    )
                    .absolute()
                    .size_full(),
                ),
        )
        .child(
            div()
                .absolute()
                .top(sz(3.0))
                .left(gpui::relative(value))
                .ml(-thumb / 2.0)
                .size(thumb)
                .rounded_full()
                .border(sz(2.0))
                .border_color(tokens.surface)
                .bg(tokens.text)
                .shadow(vec![BoxShadow {
                    color: mix(tokens.text, 32.0, transparent()),
                    offset: point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: px(1.0),
                    inset: false,
                }]),
        )
        .into_any_element()
}

