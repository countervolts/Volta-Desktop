//! `.connection-banner`: shown while the server is unreachable.
//!
//! The web flips to "reconnecting" when a page load fails to connect, and
//! relies on the browser's `online` event to retry. Native has no such
//! event, so while not online a ping retries every 5 seconds; a failed manual
//! Retry shows the offline wording with Retry enabled.

use std::time::Duration;

use gpui::{AnyElement, App, AppContext, BoxShadow, Context, Entity, IntoElement, ParentElement, Styled, deferred, div, point, prelude::*, px};

use crate::app_state::{AppState, Page};
use crate::components::motion::spinner;
use crate::theme::{self, sz};
use crate::views::library::secondary_button;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Connection {
    #[default]
    Online,
    Offline,
    Reconnecting,
}

const AUTO_RETRY: Duration = Duration::from_secs(5);

impl AppState {
    /// Records a load result: connection failures start reconnecting.
    pub fn note_connection(&mut self, failed: Option<&volta_services::NavidromeError>, cx: &mut Context<Self>) {
        match failed {
            None => self.connection = Connection::Online,
            Some(error) if error.is_connection_failure() => {
                if self.connection == Connection::Online {
                    self.connection = Connection::Reconnecting;
                    self.schedule_reconnect(cx);
                }
            }
            Some(_) => {}
        }
    }

    fn schedule_reconnect(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AUTO_RETRY).await;
            let _ = this.update(cx, |state, cx| {
                if state.connection != Connection::Online {
                    state.retry_connection(true, cx);
                }
            });
        })
        .detach();
    }

    /// `retryConnection`: ping, then reload the view.
    pub fn retry_connection(&mut self, automatic: bool, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        if !automatic {
            self.connection = Connection::Reconnecting;
            cx.notify();
        }
        let task = cx.background_spawn(async move { client.ping() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| {
                match result {
                    Ok(()) => {
                        state.connection = Connection::Online;
                        state.page_error = None;
                        if state.route().page == Page::Home {
                            state.load_home(cx);
                        } else {
                            state.load_page(cx);
                        }
                    }
                    Err(_) => {
                        state.connection = Connection::Offline;
                        state.schedule_reconnect(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

pub fn render(state: &Entity<AppState>, cx: &App) -> Option<AnyElement> {
    let connection = state.read(cx).connection;
    if connection == Connection::Online || state.read(cx).session.is_none() {
        return None;
    }
    let tokens = theme::tokens(cx).clone();
    let reconnecting = connection == Connection::Reconnecting;
    let viewport = theme::viewport(cx);
    let target = state.clone();
    let badge: AnyElement = if reconnecting {
        spinner(15.0).into_any_element()
    } else {
        div()
            .size(sz(17.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(tokens.accent)
            .text_color(tokens.canvas)
            .text_size(sz(12.0))
            .font_weight(gpui::FontWeight::BOLD)
            .child("!")
            .into_any_element()
    };
    Some(
        deferred(
            div().absolute().top(sz(10.0)).left_0().right_0().flex().justify_center().child(
                div()
                    .id("connection-banner")
                    .occlude()
                    .flex()
                    .items_center()
                    .gap(sz(9.0))
                    .w(sz(560f32.min(viewport - 32.0)))
                    .min_h(sz(36.0))
                    .py(sz(6.0))
                    .pr(sz(8.0))
                    .pl(sz(12.0))
                    .border(sz(1.0))
                    .border_color(tokens.line)
                    .rounded(sz(8.0))
                    .bg(tokens.sidebar)
                    .shadow(vec![BoxShadow {
                        color: theme::hex("#0005"),
                        offset: point(px(0.0), px(8.0)),
                        blur_radius: px(28.0),
                        spread_radius: px(0.0),
                        inset: false,
                    }])
                    .text_color(tokens.secondary)
                    .text_size(sz(11.0))
                    .child(badge)
                    .child(div().flex_1().min_w_0().child(if reconnecting {
                        "Reconnecting to Navidrome…"
                    } else {
                        "You’re offline. Volta will reconnect when the network returns."
                    }))
                    .child(
                        secondary_button("connection-retry", "Retry", "", cx)
                            .min_h(sz(27.0))
                            .py(sz(5.0))
                            .px(sz(11.0))
                            .text_size(sz(11.0))
                            .when(reconnecting, |d| d.opacity(0.38).cursor_default())
                            .when(!reconnecting, move |d| d.on_click(move |_, _, cx| target.update(cx, |s, cx| s.retry_connection(false, cx)))),
                    ),
            ),
        )
        .with_priority(2)
        .into_any_element(),
    )
}
