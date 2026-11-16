//! `Inspector` from player-ui.tsx: the right-hand panel with "Playing Next"
//! (now playing, the upcoming queue with drag reorder, previously played) or
//! "Lyrics".

use gpui::{
    AnyElement, App, Context, Entity, FontWeight, IntoElement, ParentElement, Render, SharedString, Styled, Window, div,
    img, prelude::*,
};
use volta_logic::model::Song;

use crate::app_state::{AppState, Panel};
use crate::components::ui::{IconButton, icon, line};
use crate::theme::{self, sz};

/// `--inspector-width`: 306px, 280px at ≤1150px.
pub fn inspector_width(viewport: f32) -> f32 {
    if viewport <= 1150.0 { 280.0 } else { 306.0 }
}

/// Drag payload for reordering upcoming songs (index into `upcoming`).
#[derive(Clone)]
struct DraggedQueueSong {
    from: usize,
    title: SharedString,
}

impl Render for DraggedQueueSong {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx).clone();
        div()
            .px(sz(10.0))
            .py(sz(6.0))
            .rounded(sz(6.0))
            .bg(tokens.surface)
            .border(sz(1.0))
            .border_color(tokens.line)
            .text_size(sz(12.0))
            .text_color(tokens.text)
            .opacity(0.9)
            .child(self.title.clone())
    }
}

fn artwork(state: &Entity<AppState>, song: &Song, size: u32, cx: &mut App) -> impl IntoElement {
    let tokens = theme::tokens(cx).clone();
    let cover = state.update(cx, |s, cx| s.cover(song.cover_art.as_deref(), size, cx));
    div()
        .size(sz(39.0))
        .flex_none()
        .rounded(sz(5.0))
        .bg(tokens.surface)
        .when_some(cover, |d, image| d.child(img(image).size_full().rounded(sz(5.0))))
}

fn caption(song: &Song, cx: &App) -> impl IntoElement {
    let tokens = theme::tokens(cx);
    div()
        .min_w_0()
        .flex_1()
        .child(line(song.title.clone()).text_size(sz(12.0)).font_weight(FontWeight::MEDIUM))
        .child(
            line(song.artist.clone().unwrap_or_default())
                .mt(sz(4.0))
                .text_size(sz(11.0))
                .text_color(tokens.secondary),
        )
}

/// `.queue-song`: min-height 58, padding 7/0, bottom rule.
fn queue_row() -> gpui::Div {
    div().flex().items_center().gap(sz(9.0)).min_h(sz(58.0)).w_full().py(sz(7.0)).border_b(sz(1.0))
}

fn heading(text: &str) -> impl IntoElement {
    div().mb(sz(9.0)).text_size(sz(12.0)).font_weight(FontWeight::SEMIBOLD).child(text.to_string())
}

fn queue(state: &Entity<AppState>, cx: &mut App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let player_entity = state.read(cx).player.clone();
    let (queue, index, playing) = {
        let p = player_entity.read(cx);
        (p.queue.clone(), p.index, p.playing)
    };
    let current = index.and_then(|i| queue.get(i).cloned());
    let split = index.map_or(0, |i| i + 1);
    let upcoming: Vec<Song> = queue.iter().skip(split).cloned().collect();

    let mut content = div()
        .id("queue-content")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .pt(sz(11.0))
        .pr(sz(14.0))
        .pb(sz(30.0))
        .pl(sz(8.0));

    if let Some(song) = current.as_ref() {
        let bars = |pct: f32| div().w(sz(3.0)).h(gpui::relative(pct)).rounded(sz(2.0)).bg(tokens.accent);
        content = content.child(heading("Now Playing")).child(
            queue_row()
                .border_color(tokens.line)
                .child(artwork(state, song, 100, cx))
                .child(caption(song, cx))
                .child(
                    div()
                        .w(sz(16.0))
                        .h(sz(14.0))
                        .flex_none()
                        .flex()
                        .items_end()
                        .justify_center()
                        .gap(sz(2.0))
                        .when(playing, |d| d.child(bars(0.55)).child(bars(0.95)).child(bars(0.7))),
                ),
        );
    }

    // Up Next AutoMix button (per queue) and why the coming transition is
    // what it is.
    let (automix_mode, automix_on, status) = {
        let p = player_entity.read(cx);
        let next = index.map(|i| i + 1);
        let status = p.mix_status.clone().filter(|(i, _)| Some(*i) == next || (*i == 0 && next == Some(queue.len()))).map(|(_, t)| t);
        (p.transition == crate::player::Transition::Automix, p.automix_queue, status)
    };
    if automix_mode && current.is_some() {
        let toggle_player = player_entity.clone();
        content = content.child(
            div()
                .mt(sz(18.0))
                .flex()
                .items_center()
                .gap(sz(10.0))
                .child(
                    div()
                        .id("queue-automix")
                        .flex_none()
                        .px(sz(10.0))
                        .py(sz(4.0))
                        .rounded(sz(12.0))
                        .border(sz(1.0))
                        .border_color(if automix_on { tokens.accent } else { tokens.line })
                        .when(automix_on, |d| d.bg(tokens.accent).text_color(gpui::white()))
                        .text_size(sz(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .cursor_pointer()
                        .on_click(move |_, _, cx| toggle_player.update(cx, |p, cx| p.set_automix_queue(!automix_on, cx)))
                        .child("AutoMix"),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_size(sz(11.0))
                        .text_color(tokens.secondary)
                        .child(if automix_on { status.unwrap_or_else(|| "Next transition is planned near the end of the song".into()) } else { "Songs play without transitions in this queue".into() }),
                ),
        );
    }

    let clear_player = player_entity.clone();
    content = content.child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .mt(sz(28.0))
            .mb(sz(9.0))
            .child(div().text_size(sz(12.0)).font_weight(FontWeight::SEMIBOLD).child("Queue"))
            .when(!upcoming.is_empty(), |d| {
                d.child(
                    div()
                        .id("queue-clear")
                        .text_size(sz(11.0))
                        .text_color(tokens.accent)
                        .cursor_pointer()
                        .on_click(move |_, _, cx| clear_player.update(cx, |p, cx| p.clear_upcoming(cx)))
                        .child("Clear"),
                )
            }),
    );

    for (i, song) in upcoming.iter().enumerate() {
        let absolute = split + i;
        let group = SharedString::from(format!("queue-row-{i}"));
        let (jump, remove, drop) = (player_entity.clone(), player_entity.clone(), player_entity.clone());
        let payload = DraggedQueueSong { from: i, title: song.title.clone().into() };
        content = content.child(
            queue_row()
                .id(("queue-song", i))
                .group(group.clone())
                .border_color(tokens.line)
                .cursor_grab()
                .on_drag(payload, |payload, _, _, cx| cx.new(|_| payload.clone()))
                .drag_over::<DraggedQueueSong>(move |style, _, _, _| {
                    style.bg(tokens.selection).border_color(tokens.accent)
                })
                .on_drop(move |dragged: &DraggedQueueSong, _, cx| {
                    let from = split + dragged.from;
                    drop.update(cx, |p, cx| p.reorder_queue(from, absolute, cx));
                })
                .child(div().w(sz(14.0)).flex_none().flex().justify_center().text_color(tokens.muted).child(icon("grip-vertical", 14.0)))
                .child(
                    div()
                        .id(("queue-jump", i))
                        .flex()
                        .items_center()
                        .gap(sz(9.0))
                        .flex_1()
                        .min_w_0()
                        .cursor_pointer()
                        .on_click(move |_, _, cx| jump.update(cx, |p, cx| p.jump_to(absolute, cx)))
                        .child(artwork(state, song, 100, cx))
                        .child(caption(song, cx)),
                )
                .child(
                    div().flex_none().opacity(0.0).group_hover(group, |s| s.opacity(1.0)).child(
                        IconButton::new(("queue-remove", i), format!("Remove {} from queue", song.title), icon("x", 14.0))
                            .width(20.0)
                            .height(26.0)
                            .on_click(move |_, _, cx| remove.update(cx, |p, cx| p.remove_from_queue(absolute, cx))),
                    ),
                ),
        );
    }

    if upcoming.is_empty() {
        content = content.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .px(sz(15.0))
                .py(sz(27.0))
                .text_center()
                .text_color(tokens.muted)
                .child(icon("list-music", 30.0))
                .child(div().mt(sz(12.0)).mb(sz(5.0)).text_size(sz(12.0)).text_color(tokens.secondary).child("Your queue is clear."))
                .child(
                    div()
                        .text_size(sz(11.0))
                        .line_height(gpui::relative(1.5))
                        .child("Choose Play Next or Play Last from a song’s menu."),
                ),
        );
    }

    if let Some(current_index) = index.filter(|i| *i > 0) {
        content = content.child(div().mt(sz(30.0)).child(heading("Previously Played")));
        for (i, song) in queue.iter().take(current_index).enumerate() {
            let jump = player_entity.clone();
            content = content.child(
                queue_row()
                    .id(("queue-history", i))
                    .border_color(tokens.line)
                    .opacity(0.65)
                    .cursor_pointer()
                    .on_click(move |_, _, cx| jump.update(cx, |p, cx| p.jump_to(i, cx)))
                    .child(artwork(state, song, 80, cx))
                    .child(caption(song, cx)),
            );
        }
    }
    content.into_any_element()
}

pub fn render(
    state: &Entity<AppState>,
    lyrics: &Entity<crate::components::lyrics_view::LyricsView>,
    panel: Panel,
    viewport: f32,
    cx: &mut App,
) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let close = state.clone();
    let overlay = viewport <= 860.0;
    let width = if overlay { 280.0 } else { inspector_width(viewport) };
    div()
        .id("inspector")
        .w(sz(width))
        .flex_none()
        .flex()
        .flex_col()
        .overflow_hidden()
        .min_h_0()
        .when(!overlay, |d| d.h_full().border_l(sz(1.0)).border_color(tokens.line).bg(tokens.row))
        .when(overlay, |d| {
            d.absolute().right_0().top(sz(53.0)).bottom(sz(85.0)).bg(tokens.canvas).shadow(vec![gpui::BoxShadow {
                color: theme::hex("#0003"),
                offset: gpui::point(gpui::px(-10.0), gpui::px(0.0)),
                blur_radius: gpui::px(25.0),
                spread_radius: gpui::px(0.0),
                inset: false,
            }])
        })
        .child(
            div()
                .flex_none()
                .min_h(sz(55.0))
                .pl(sz(22.0))
                .pr(sz(18.0))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(sz(15.0))
                        .font_weight(FontWeight::BOLD)
                        .letter_spacing(sz(-0.15))
                        .child(if panel == Panel::Queue { "Playing Next" } else { "Lyrics" }),
                )
                .child(IconButton::new("close-panel", "Close panel", icon("x", 17.0)).on_click(move |_, _, cx| {
                    close.update(cx, |s, cx| {
                        s.panel = None;
                        cx.notify();
                    })
                })),
        )
        .child(match panel {
            Panel::Queue => queue(state, cx),
            Panel::Lyrics => div().flex_1().min_h_0().flex().flex_col().child(lyrics.clone()).into_any_element(),
        })
        .into_any_element()
}

/// `<Inspector embedded>` in the full player: a quieter heading with the
/// lyrics import button, no close button, transparent background.
pub fn render_embedded(
    state: &Entity<AppState>,
    lyrics: &Entity<crate::components::lyrics_view::LyricsView>,
    panel: Panel,
    cx: &mut App,
) -> AnyElement {
    let body = match panel {
        Panel::Queue => div().flex_1().min_h_0().flex().flex_col().pt(sz(12.0)).child(queue(state, cx)).into_any_element(),
        Panel::Lyrics => div().flex_1().min_h_0().flex().flex_col().child(lyrics.clone()).into_any_element(),
    };
    div()
        .id("inspector-embedded")
        .size_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .min_h_0()
        .child(
            div()
                .flex_none()
                .min_h(sz(35.0))
                .py(sz(10.0))
                .flex()
                .items_center()
                .gap(sz(8.0))
                .text_color(theme::hex("#ffffff9f"))
                .child(
                    div()
                        .text_size(sz(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .child(if panel == Panel::Queue { "Playing Next" } else { "Lyrics" }),
                )
                .when(panel == Panel::Lyrics, |d| {
                    let target = lyrics.clone();
                    let status = lyrics.read(cx).import_status.as_ref().map(|(text, _)| text.clone());
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sz(7.0))
                            .min_w_0()
                            .child(
                                IconButton::new("import-lyrics", "Import lyrics from clipboard", icon("clipboard-paste", 16.0))
                                    .on_click(move |_, _, cx| target.update(cx, |v, cx| v.import_from_clipboard(cx))),
                            )
                            .when_some(status, |d, status| {
                                d.child(div().text_size(sz(10.0)).whitespace_nowrap().text_color(theme::hex("#ffffffa8")).child(status))
                            }),
                    )
                }),
        )
        .child(body)
        .into_any_element()
}
