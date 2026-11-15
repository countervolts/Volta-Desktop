//! `Select` from app/custom-select.tsx: a `.custom-select` trigger and a
//! `.custom-select-menu` popover with a check beside the current option.
//! Which menu is open lives in `AppState::open_select` (one at a time).

use std::rc::Rc;

use gpui::{App, BoxShadow, Entity, IntoElement, ParentElement, SharedString, Styled, Window, deferred, div, point, prelude::*};

use crate::app_state::AppState;
use crate::components::ui::{icon, line};
use crate::theme::{self, sz};

pub type OnSelect = Rc<dyn Fn(&'static str, &mut Window, &mut App)>;

pub fn select(
    state: &Entity<AppState>,
    id: &'static str,
    value: &'static str,
    options: &[(&'static str, &'static str)],
    on_select: OnSelect,
    cx: &App,
) -> impl IntoElement {
    let tokens = theme::tokens(cx).clone();
    let open = state.read(cx).open_select == Some(id);
    let label = options.iter().find(|(v, _)| *v == value).map(|(_, l)| *l).unwrap_or(value);
    let toggle_state = state.clone();
    let trigger = div()
        .id(SharedString::from(format!("select-{id}")))
        .flex()
        .items_center()
        .gap(sz(8.0))
        .max_w(sz(260.0))
        .min_h(sz(30.0))
        .px(sz(9.0))
        .py(sz(5.0))
        .border(sz(1.0))
        .border_color(tokens.line)
        .hover(|s| s.border_color(theme::mix(tokens.accent, 40.0, tokens.line)))
        .rounded(sz(7.0))
        .bg(tokens.sidebar)
        .text_color(tokens.text)
        .text_size(sz(11.0))
        .on_click(move |_, _, cx| {
            toggle_state.update(cx, |s, cx| {
                s.open_select = if s.open_select == Some(id) { None } else { Some(id) };
                cx.notify();
            })
        })
        .child(line(label))
        .child(div().flex_none().text_color(tokens.muted).child(icon("chevron-down", 14.0)));

    let menu = open.then(|| {
        let close_state = state.clone();
        let items = options.iter().map(|&(option, text)| {
            let pick_state = state.clone();
            let on_select = on_select.clone();
            let checked = option == value;
            div()
                .id(SharedString::from(format!("select-{id}-{option}")))
                .flex()
                .items_center()
                .gap(sz(8.0))
                .min_h(sz(30.0))
                .px(sz(9.0))
                .py(sz(6.0))
                .rounded(sz(5.0))
                .text_color(tokens.text)
                .text_size(sz(12.0))
                .cursor_pointer()
                .hover(|s| s.bg(tokens.accent).text_color(gpui::white()))
                .on_click(move |_, window, cx| {
                    pick_state.update(cx, |s, cx| {
                        s.open_select = None;
                        cx.notify();
                    });
                    on_select(option, window, cx);
                })
                .child(
                    div()
                        .w(sz(14.0))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .when(checked, |d| d.child(icon("check", 13.0))),
                )
                .child(text)
        });
        deferred(
            div()
                .id(SharedString::from(format!("select-menu-{id}")))
                .absolute()
                .top(gpui::relative(1.0))
                .mt(sz(5.0))
                .right_0()
                .min_w(gpui::relative(1.0))
                .p(sz(5.0))
                .border(sz(1.0))
                .border_color(tokens.line)
                .rounded(sz(9.0))
                .bg(tokens.sidebar)
                .shadow(vec![BoxShadow {
                    color: theme::hex("#0007"),
                    offset: point(gpui::px(0.0), gpui::px(16.0)),
                    blur_radius: gpui::px(40.0),
                    spread_radius: gpui::px(0.0),
                    inset: false,
                }])
                .occlude()
                .on_mouse_down_out(move |_, _, cx| {
                    close_state.update(cx, |s, cx| {
                        if s.open_select == Some(id) {
                            s.open_select = None;
                            cx.notify();
                        }
                    })
                })
                .children(items),
        )
        .with_priority(1)
    });
    div().relative().child(trigger).children(menu)
}
