//! `NotificationToast` from app/notifications.tsx: one toast at a time,
//! bottom-right above the dock, auto-dismissed after 4.7 s with a 220 ms exit.
//! Enter: 460 ms `cubic-bezier(.2,.82,.2,1)` from (14, 9) px, scale .98 →
//! 1.006 → 1; exit: 210 ms to (8, 3) px and transparent.

use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, App, BoxShadow, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, div,
    point, prelude::*, px,
};

use crate::app_state::AppState;
use crate::components::motion::cubic_bezier;
use crate::components::ui::{IconButton, icon, icon_with};
use crate::theme::{self, mix, sz, transparent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Success,
    Playback,
    Error,
}

#[derive(Clone, Debug)]
pub struct Notice {
    pub id: u64,
    pub kind: NoticeKind,
    pub message: String,
    pub leaving: Option<Instant>,
}

pub const VISIBLE: Duration = Duration::from_millis(4700);
pub const EXIT: Duration = Duration::from_millis(220);

pub fn render(state: &Entity<AppState>, cx: &App) -> Option<gpui::AnyElement> {
    let notice = state.read(cx).notice.clone()?;
    let tokens = theme::tokens(cx).clone();
    let headline = match notice.kind {
        NoticeKind::Info => "Update",
        NoticeKind::Success => "Saved",
        NoticeKind::Playback => "Now Playing",
        NoticeKind::Error => "Couldn’t complete",
    };
    let (mark_color, mark_icon) = match notice.kind {
        NoticeKind::Info => (tokens.text, icon_with("info", 21.0, Some(1.8), false)),
        NoticeKind::Success => (theme::hex("#30d158"), icon_with("check", 21.0, Some(2.2), false)),
        NoticeKind::Playback => (tokens.accent, icon_with("music2", 21.0, Some(1.9), false)),
        NoticeKind::Error => (theme::hex("#ff6258"), icon_with("triangle-alert", 20.0, Some(1.9), false)),
    };
    let tint = match notice.kind {
        NoticeKind::Info => tokens.text,
        NoticeKind::Error => theme::hex("#ff453a"),
        _ => mark_color,
    };
    let (mark_border, mark_fill) = if notice.kind == NoticeKind::Info {
        (mix(tokens.text, 10.0, transparent()), mix(tokens.text, 8.0, transparent()))
    } else {
        (mix(tint, 26.0, transparent()), mix(tint, 16.0, transparent()))
    };
    let dismiss = state.clone();
    let id = notice.id;
    let card = div()
        .id(SharedString::from(format!("toast-{id}")))
        // Clicks on the card (including its dismiss button) must not reach what is behind it.
        .occlude()
        .flex()
        .items_center()
        .gap(sz(12.0))
        .w(sz(390.0f32.min(theme::viewport(cx) - 28.0)))
        .min_h(sz(76.0))
        .pt(sz(12.0))
        .pb(sz(12.0))
        .pl(sz(12.0))
        .pr(sz(11.0))
        .overflow_hidden()
        .border(sz(1.0))
        .border_color(mix(tokens.text, 12.0, transparent()))
        .rounded(sz(16.0))
        .backdrop_blur_saturate(sz(24.0), 1.35)
        .bg(mix(tokens.sidebar, 88.0, transparent()))
        .shadow(vec![
            BoxShadow { color: theme::hex("#0006"), offset: point(px(0.0), px(18.0)), blur_radius: px(48.0), spread_radius: px(0.0), inset: false },
            BoxShadow { color: theme::hex("#0003"), offset: point(px(0.0), px(2.0)), blur_radius: px(8.0), spread_radius: px(0.0), inset: false },
        ])
        .text_color(tokens.text)
        .child(
            div()
                .size(sz(44.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .border(sz(1.0))
                .border_color(mark_border)
                .rounded(sz(13.0))
                .bg(mark_fill)
                .text_color(mark_color)
                .child(mark_icon),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(sz(2.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sz(5.0))
                        .text_color(tokens.secondary)
                        .text_size(sz(9.0))
                        .font_weight(FontWeight(650.0))
                        .letter_spacing(sz(9.0 * 0.08))
                        .line_height(gpui::relative(1.2))
                        .child("VOLTA")
                        .child("·")
                        .child(div().text_color(tokens.muted).text_size(sz(10.0)).font_weight(FontWeight(450.0)).child("now")),
                )
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(sz(13.0))
                        .font_weight(FontWeight(620.0))
                        .line_height(gpui::relative(1.3))
                        .child(headline),
                )
                .child(
                    div()
                        .text_color(tokens.secondary)
                        .text_size(sz(11.5))
                        .line_height(gpui::relative(1.35))
                        .line_clamp(2)
                        .child(notice.message.clone()),
                ),
        )
        .child(
            div().self_start().mt(sz(-3.0)).mr(sz(-2.0)).child(
                IconButton::new("toast-dismiss", "Dismiss notification", icon("x", 15.0))
                    .size(28.0)
                    .on_click(move |_, _, cx| dismiss.update(cx, |s, cx| s.dismiss_notice(id, cx))),
            ),
        );

    let viewport = theme::viewport(cx);
    let positioned = |content: gpui::AnyElement| {
        div().absolute().right(sz(16.0)).bottom(sz(if viewport <= 640.0 { 92.0 } else { 92.0 })).child(content)
    };
    let enter = cubic_bezier(0.2, 0.82, 0.2, 1.0);
    if crate::components::motion::reduced() {
        // `notification-fade-in/out 120ms`.
        let leaving = notice.leaving.is_some();
        return Some(
            positioned(
                card.with_animation(
                    SharedString::from(format!("toast-fade-{id}-{leaving}")),
                    Animation::new(Duration::from_millis(120)),
                    move |card, t| card.opacity(if leaving { 1.0 - t } else { t }),
                )
                .into_any_element(),
            )
            .into_any_element(),
        );
    }
    Some(match notice.leaving {
        None => positioned(
            card.with_animation(
                SharedString::from(format!("toast-enter-{id}")),
                Animation::new(Duration::from_millis(460)).with_easing(enter),
                |card, t| {
                    // 0 → 72 %: fade in and settle from (14, 9) px; scale is not drawable.
                    let k = (t / 0.72).min(1.0);
                    card.opacity(k).relative().left(px(14.0 * (1.0 - k))).top(px(9.0 * (1.0 - k)))
                },
            )
            .into_any_element(),
        )
        .into_any_element(),
        Some(_) => positioned(
            card.with_animation(
                SharedString::from(format!("toast-exit-{id}")),
                Animation::new(Duration::from_millis(210)).with_easing(cubic_bezier(0.4, 0.0, 1.0, 1.0)),
                |card, t| card.opacity(1.0 - t).relative().left(px(8.0 * t)).top(px(3.0 * t)),
            )
            .into_any_element(),
        )
        .into_any_element(),
    })
}
