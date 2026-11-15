//! "Update available" banner. Volta only tells you a newer release exists and
//! opens its GitHub page; it never downloads or installs anything.
//!
//! The check runs at most once a day (`UPDATE_LAST_CHECK_KEY`), can be turned
//! off in Settings > Privacy, and "Skip" remembers the version so it stays
//! quiet until the next release.

use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{AnyElement, App, AppContext, BoxShadow, Context, Entity, IntoElement, ParentElement, Styled, deferred, div, point, prelude::*, px};
use volta_logic::storage as keys;
use volta_logic::update::{check_due, is_newer};

use crate::app_state::AppState;
use crate::components::connection_banner::Connection;
use crate::components::notifications::NoticeKind;
use crate::theme::{self, sz};
use crate::views::library::secondary_button;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl AppState {
    /// Asks GitHub for the latest release. Automatic checks respect the
    /// Settings toggle and the once-a-day limit; `manual` ones always run and
    /// report the outcome in a toast.
    pub fn check_for_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.update_checking {
            return;
        }
        if !manual
            && (self.storage.get(keys::UPDATE_CHECK_KEY) == "false"
                || !check_due(&self.storage.get(keys::UPDATE_LAST_CHECK_KEY), now_ms()))
        {
            return;
        }
        self.update_checking = true;
        cx.notify();
        let task = cx.background_spawn(async move { volta_services::update::latest_release() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| {
                state.update_checking = false;
                match result {
                    Ok(release) => {
                        state.storage.set(keys::UPDATE_LAST_CHECK_KEY, &now_ms().to_string());
                        match release.filter(|r| is_newer(env!("CARGO_PKG_VERSION"), &r.version)) {
                            Some(release) => {
                                let skipped = state.storage.get(keys::UPDATE_SKIPPED_KEY) == release.version;
                                // An explicit "Check now" shows a skipped version again.
                                if manual || !skipped {
                                    state.update = Some(release);
                                }
                            }
                            None => {
                                state.update = None;
                                if manual {
                                    state.notify(format!("Volta {} is up to date.", env!("CARGO_PKG_VERSION")), NoticeKind::Info, cx);
                                }
                            }
                        }
                    }
                    Err(_) if manual => state.notify("Couldn’t check for updates.", NoticeKind::Error, cx),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// "Skip": hide this version until a newer one is published.
    pub fn skip_update(&mut self, cx: &mut Context<Self>) {
        if let Some(release) = self.update.take() {
            self.storage.set(keys::UPDATE_SKIPPED_KEY, &release.version);
        }
        cx.notify();
    }
}

pub fn render(state: &Entity<AppState>, cx: &App) -> Option<AnyElement> {
    let release = state.read(cx).update.clone()?;
    // The connection banner owns this spot while the server is unreachable.
    if state.read(cx).connection != Connection::Online && state.read(cx).session.is_some() {
        return None;
    }
    let tokens = theme::tokens(cx).clone();
    let viewport = theme::viewport(cx);
    let (later, skip) = (state.clone(), state.clone());
    let url = release.url.clone();
    Some(
        deferred(
            div().absolute().top(sz(10.0)).left_0().right_0().flex().justify_center().child(
                div()
                    .id("update-banner")
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
                    .child(div().flex_1().min_w_0().child(format!(
                        "Volta {} is available (you have {}).",
                        release.version,
                        env!("CARGO_PKG_VERSION")
                    )))
                    .child(secondary_button("update-view", "View release", "", cx).min_h(sz(27.0)).py(sz(5.0)).px(sz(11.0)).text_size(sz(11.0)).on_click(
                        move |_, _, cx| cx.open_url(&url),
                    ))
                    .child(secondary_button("update-skip", "Skip", "", cx).min_h(sz(27.0)).py(sz(5.0)).px(sz(11.0)).text_size(sz(11.0)).on_click(
                        move |_, _, cx| skip.update(cx, |s, cx| s.skip_update(cx)),
                    ))
                    .child(secondary_button("update-later", "Later", "", cx).min_h(sz(27.0)).py(sz(5.0)).px(sz(11.0)).text_size(sz(11.0)).on_click(
                        move |_, _, cx| {
                            later.update(cx, |s, cx| {
                                s.update = None;
                                cx.notify();
                            })
                        },
                    )),
            ),
        )
        .with_priority(2)
        .into_any_element(),
    )
}
