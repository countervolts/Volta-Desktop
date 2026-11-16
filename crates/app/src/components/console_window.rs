//! The developer console: Volta's log output in a window of its own.

use std::time::Duration;

use gpui::{Context, FontWeight, IntoElement, ParentElement, Render, Styled, Window, div, point, prelude::*, px};

use crate::console;
use crate::theme::{self, sz};
use crate::views::library::secondary_button;

pub struct ConsoleView {
    seen: u64,
    scroll: gpui::ScrollHandle,
}

impl ConsoleView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Pick up new log lines a few times a second; nothing is drawn when idle.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let changed = this.update(cx, |view, cx| {
                    if console::revision() != view.seen {
                        cx.notify();
                    }
                });
                if changed.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self { seen: u64::MAX, scroll: gpui::ScrollHandle::new() }
    }
}

impl Render for ConsoleView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx).clone();
        // Follow the output, unless the user scrolled up to read something.
        let following = {
            let (offset, max) = (self.scroll.offset(), self.scroll.max_offset());
            self.seen == u64::MAX || offset.y <= -max.y + px(8.0)
        };
        self.seen = console::revision();
        let lines = console::snapshot();
        let count = lines.len();
        let text = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
        if following {
            let scroll = self.scroll.clone();
            cx.on_next_frame(_window, move |_, _, _| {
                scroll.set_offset(point(px(0.0), -scroll.max_offset().y));
            });
        }
        let color = |level: log::Level| match level {
            log::Level::Error => theme::hex("#ff7b72"),
            log::Level::Warn => theme::hex("#e3b341"),
            log::Level::Info => tokens.text,
            _ => tokens.muted,
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(tokens.canvas)
            .text_color(tokens.text)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sz(12.0))
                    .px(sz(14.0))
                    .py(sz(10.0))
                    .border_b(sz(1.0))
                    .border_color(tokens.line)
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(sz(8.0))
                            .child(div().text_size(sz(14.0)).font_weight(FontWeight::BOLD).child("Console"))
                            .child(div().text_size(sz(11.0)).text_color(tokens.muted).child(format!("{count} lines"))),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(sz(8.0))
                            .child(secondary_button("console-copy", "Copy", "", cx).min_h(sz(28.0)).px(sz(12.0)).on_click(move |_, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                            }))
                            .child(secondary_button("console-clear", "Clear", "", cx).min_h(sz(28.0)).px(sz(12.0)).on_click(|_, _, _| console::clear())),
                    ),
            )
            .child(
                div()
                    .id("console-lines")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px(sz(14.0))
                    .py(sz(8.0))
                    .font_family(theme::fonts::mono_font_family(cx))
                    .text_size(sz(11.0))
                    .line_height(gpui::relative(1.45))
                    .children(lines.into_iter().map(|line| div().text_color(color(line.level)).child(line.text)))
                    .when(count == 0, |d| d.child(div().text_color(tokens.muted).child("Nothing logged yet."))),
            )
    }
}
