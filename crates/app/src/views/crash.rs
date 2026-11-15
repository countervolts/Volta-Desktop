//! `ErrorBoundary`: a panic while building the page shows this card instead
//! of taking the app down. Nothing is uploaded; the panic is logged.

use gpui::{AnyElement, App, FontWeight, IntoElement, ParentElement, SharedString, Styled, div, prelude::*};

use crate::theme::{self, sz};
use crate::views::library::secondary_button;

/// The panic payload as text.
pub fn message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "Unknown error".into())
}

/// `.crash-screen` / `.crash-card`.
pub fn render(
    error: &str,
    on_reload: impl Fn(&mut gpui::Window, &mut App) + 'static,
    on_retry: impl Fn(&mut gpui::Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    div()
        .size_full()
        .min_h(sz(400.0))
        .flex()
        .items_center()
        .justify_center()
        .p(sz(32.0))
        .child(
            div()
                .w_full()
                .max_w(sz(520.0))
                .flex()
                .flex_col()
                .gap(sz(10.0))
                .p(sz(28.0))
                .border(sz(1.0))
                .border_color(tokens.line)
                .rounded(sz(16.0))
                .bg(tokens.surface)
                .text_color(tokens.text)
                .child(div().text_size(sz(12.0)).font_weight(FontWeight::SEMIBOLD).letter_spacing(sz(12.0 * 0.08)).text_color(tokens.accent).child("SOMETHING WENT WRONG"))
                .child(div().text_size(sz(22.0)).font_weight(FontWeight::BOLD).child("Volta hit an unexpected error"))
                .child(div().text_color(tokens.secondary).line_height(gpui::relative(1.5)).child(
                    "Your music and sign-in are safe. Try again, and if the problem keeps happening, reload the player.",
                ))
                .child(
                    div()
                        .id("crash-detail")
                        .max_h(sz(160.0))
                        .overflow_y_scroll()
                        .py(sz(10.0))
                        .px(sz(12.0))
                        .rounded(sz(10.0))
                        .bg(tokens.row)
                        .text_color(tokens.muted)
                        .text_size(sz(12.0))
                        .font_family(SharedString::from("monospace"))
                        .child(error.to_string()),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .flex_wrap()
                        .gap(sz(10.0))
                        .mt(sz(4.0))
                        .child(secondary_button("crash-reload", "Reload Volta", "", cx).on_click(move |_, w, cx| on_reload(w, cx)))
                        .child(
                            div()
                                .id("crash-retry")
                                .flex()
                                .items_center()
                                .justify_center()
                                .min_h(sz(34.0))
                                .px(sz(20.0))
                                .py(sz(7.0))
                                .rounded(sz(7.0))
                                .bg(theme::hex("#e73650"))
                                .hover(|s| s.bg(theme::hex("#fa4059")))
                                .text_color(gpui::white())
                                .text_size(sz(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .cursor_pointer()
                                .on_click(move |_, w, cx| on_retry(w, cx))
                                .child("Try again"),
                        ),
                ),
        )
        .into_any_element()
}
