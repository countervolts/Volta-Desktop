//! `FullPlayer` from player-ui.tsx: the full-window Now Playing view
//! (`.fullscreen-dialog`). The record (artwork, title, artist, favorite,
//! seek with times, large transport, volume, quality and panel switches) sits
//! beside the embedded lyrics or queue, over the animated backdrop.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Bounds, BoxShadow, Context, Entity, FontWeight, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, Render, Styled, Window, canvas, div, img, point, prelude::*, px,
};
use volta_logic::format::{duration, is_lossless};

use crate::app_state::{AppState, Panel};
use crate::components::fullscreen_background::FullscreenBackground;
use crate::components::inspector;
use crate::components::lyrics_view::LyricsView;
use crate::components::motion::spinner;
use crate::components::playback_dock::{fraction, track};
use crate::components::ui::{IconButton, icon, icon_with, line};
use crate::player::{Player, Repeat};
use crate::theme::{self, fonts::tabular_nums, sz};

pub struct FullPlayer {
    state: Entity<AppState>,
    player: Entity<Player>,
    background: Entity<FullscreenBackground>,
    lyrics: Entity<LyricsView>,
    panel: Panel,
    seek_bounds: Rc<Cell<Bounds<Pixels>>>,
    volume_bounds: Rc<Cell<Bounds<Pixels>>>,
    dragging_seek: bool,
    dragging_volume: bool,
    volume_hover: bool,
    /// `.volume-value { transition: opacity/transform 0.12s ease }`.
    volume_reveal: crate::components::motion::Transition,
}

/// `quality(song, activeStream)`.
fn quality(song: Option<&volta_logic::model::Song>, compatible: bool) -> &'static str {
    if compatible {
        return "MP3 320 kbps";
    }
    let Some(song) = song else { return "Original Quality" };
    if !is_lossless(Some(song)) {
        return "Original";
    }
    if song.bit_depth.unwrap_or(0.0) > 16.0 || song.sampling_rate.unwrap_or(0.0) > 48000.0 {
        "Hi-Res Lossless"
    } else {
        "Lossless"
    }
}

impl FullPlayer {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let player = state.read(cx).player.clone();
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        let background = cx.new(|cx| FullscreenBackground::new(state.clone(), cx));
        let lyrics = cx.new(|cx| LyricsView::embedded(state.clone(), cx));
        Self {
            state,
            player,
            background,
            lyrics,
            panel: Panel::Lyrics,
            seek_bounds: Rc::new(Cell::new(Bounds::default())),
            volume_bounds: Rc::new(Cell::new(Bounds::default())),
            dragging_seek: false,
            dragging_volume: false,
            volume_hover: false,
            volume_reveal: crate::components::motion::Transition::new(120),
        }
    }

    fn seek_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let f = fraction(self.seek_bounds.get(), x);
        self.player.update(cx, |p, cx| {
            let target = f * p.duration.max(1.0);
            p.seek(target, cx)
        });
    }

    fn volume_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let f = fraction(self.volume_bounds.get(), x);
        self.player.update(cx, |p, cx| p.set_volume((f * 100.0).round() / 100.0, cx));
    }

}

impl Render for FullPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let base = theme::tokens(cx).clone();
        let tokens = theme::fullscreen_tokens(&base);
        let viewport = theme::viewport_css_width(window);
        let scale = f32::from(window.rem_size()) / 16.0;
        let viewport_h = f32::from(theme::content_size(window).height) / scale;
        let narrow = viewport <= 640.0;
        let player = self.player.read(cx);
        let song = player.current_song().cloned();
        let (position, total, playing, loading, volume, muted, shuffle, repeat, compatible, has_queue) = (
            player.position,
            player.duration.max(0.0),
            player.playing,
            player.loading,
            player.volume,
            player.muted,
            player.shuffle,
            player.repeat,
            player.compatible,
            !player.queue.is_empty(),
        );
        let Some(song) = song else {
            return div().into_any_element();
        };
        let cover = self.state.update(cx, |s, cx| s.cover(song.cover_art.as_deref(), 900, cx));

        // `.full-player-layout`: columns .85fr / 1.15fr, gap clamp(80, 5vw, 130).
        let gap = (viewport * 0.05).clamp(80.0, 130.0);
        let inner = (viewport - 48.0).min(1880.0);
        let left_col = (inner - gap) * 0.85 / 2.0;
        let record_width = left_col.min(620.0);
        // `.full-record > .artwork`: min(100%, clamp(460px, 46dvh, 620px)).
        let art = record_width.min((viewport_h * 0.46).clamp(460.0, 620.0));

        let p = self.player.clone();
        let mixing = self.state.read(cx).player.read(cx).automix_label.is_some();
        // During a mix the incoming cover dissolves in over the current one.
        let morph = {
            let player = self.state.read(cx).player.read(cx);
            player.mix_progress().zip(player.mixing_into().map(|s| s.cover_art.clone()))
        };
        let morph = morph.and_then(|(progress, id)| self.state.update(cx, |state, cx| state.cover(id.as_deref(), 900, cx)).map(|image| (image, progress)));
        let favorite = self.state.read(cx).is_favorite_song(&song);
        let (fav_state, fav_song) = (self.state.clone(), song.clone());
        let button = |id: &'static str, label: String, child: gpui::AnyElement| {
            IconButton::new(id, label, child).bare().width(33.0).height(39.0).color(theme::hex("#c2c2c7"))
        };
        let play_icon = if loading {
            spinner(23.0).into_any_element()
        } else if playing {
            icon_with("pause", 24.0, None, true).into_any_element()
        } else {
            icon_with("play", 24.0, None, true).into_any_element()
        };
        let transport = div()
            .flex()
            .items_center()
            .justify_center()
            .when(narrow, |d| d.justify_between())
            .gap(sz(16.0))
            .mt(sz(if narrow { 19.0 } else { 14.0 }))
            .child({
                let p = p.clone();
                button("fs-shuffle", "Shuffle".into(), icon("shuffle", 16.0).into_any_element())
                    .color(if shuffle { theme::accent_on_dark() } else { theme::hex("#c2c2c7") })
                    .disabled(!has_queue)
                    .on_click(move |_, _, cx| p.update(cx, |p, cx| p.toggle_shuffle(cx)))
            })
            .child({
                let p = p.clone();
                button("fs-previous", "Previous track".into(), icon_with("skip-back", 20.0, None, true).into_any_element())
                    .on_click(move |_, _, cx| p.update(cx, |p, cx| p.previous(cx)))
            })
            .child({
                let p = p.clone();
                button("fs-play", if playing { "Pause" } else { "Play" }.into(), play_icon)
                    .on_click(move |_, _, cx| p.update(cx, |p, cx| p.toggle(cx)))
            })
            .child({
                let p = p.clone();
                button("fs-next", "Next track".into(), icon_with("skip-forward", 20.0, None, true).into_any_element())
                    .on_click(move |_, _, cx| p.update(cx, |p, cx| p.next(cx)))
            })
            .child({
                let p = p.clone();
                button(
                    "fs-repeat",
                    "Repeat".into(),
                    icon(if repeat == Repeat::One { "repeat1" } else { "repeat" }, 16.0).into_any_element(),
                )
                .color(if repeat != Repeat::Off { theme::accent_on_dark() } else { theme::hex("#c2c2c7") })
                .disabled(!has_queue)
                .on_click(move |_, _, cx| p.update(cx, |p, cx| p.cycle_repeat(cx)))
            });

        // `.show-times`: the track starts 39px in; times sit at left 8 and right −32.
        let progress = if total > 0.0 { position / total } else { 0.0 };
        let seek_cell = self.seek_bounds.clone();
        let time = |text: String| {
            div()
                .text_size(sz(10.0))
                .text_color(tokens.secondary)
                .font_features(tabular_nums())
                .child(text)
        };
        let seek = div()
            .id("fs-seek")
            .relative()
            .h(sz(18.0))
            .w_full()
            .child(div().absolute().top(sz(-1.0)).left(sz(8.0)).child(time(duration(Some(position)))))
            .child(
                div()
                    .absolute()
                    .top(sz(-1.0))
                    .right(sz(-32.0))
                    .child(time(format!("-{}", duration(Some((total - position).max(0.0)))))),
            )
            .child(
                div()
                    .id("fs-seek-track")
                    .absolute()
                    .top(sz(0.0))
                    .left(sz(39.0))
                    .right(sz(2.0))
                    .h(sz(10.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, e: &MouseDownEvent, _, cx| {
                            this.dragging_seek = true;
                            this.seek_to(e.position.x, cx);
                        }),
                    )
                    .child(track(progress, 3.0, theme::hex("#ffffffcc"), tokens.line))
                    .child(canvas(move |b, _, _| seek_cell.set(b), |_, _, _, _| {}).absolute().size_full()),
            );

        let volume_cell = self.volume_bounds.clone();
        let mute = p.clone();
        self.volume_reveal.set(self.volume_hover || self.dragging_volume);
        let (reveal, revealing) = self.volume_reveal.value();
        if revealing {
            window.request_animation_frame();
        }
        let shown_volume = if muted { 0.0 } else { volume };
        let percentage = (shown_volume * 100.0).round();
        // `.volume-value`: centred over the thumb, clamped inside the track.
        let track_width = f32::from(self.volume_bounds.get().size.width) / (f32::from(window.rem_size()) / 16.0);
        let bubble_left = (track_width * shown_volume as f32).clamp(17.0, (track_width - 17.0).max(17.0)) - 17.0;
        let volume_row = div()
            .id("fs-volume-row")
            // Wheel over the volume control steps it, as in the dock.
            .on_scroll(cx.listener(|this, event: &gpui::ScrollEvent, window, cx| {
                let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
                if delta == 0.0 {
                    return;
                }
                let step = crate::components::playback_dock::volume_scroll_step(&this.state, event.modifiers.alt || event.modifiers.control, cx);
                let direction = if delta > 0.0 { 1.0 } else { -1.0 };
                this.player.update(cx, |player, cx| {
                    let next = ((player.volume + direction * step / 100.0) * 100.0).round() / 100.0;
                    player.set_volume(next, cx);
                });
                cx.stop_propagation();
            }))
            .flex()
            .items_center()
            .gap(sz(9.0))
            .w_full()
            .mt(sz(9.0))
            .child(
                IconButton::new("fs-mute", if muted { "Unmute" } else { "Mute" }, icon(if muted { "volume-x" } else { "volume2" }, 18.0)).bare()
                    .width(28.0)
                    .height(30.0)
                    .color(theme::hex("#c2c2c7"))
                    .on_click(move |_, _, cx| mute.update(cx, |p, cx| p.toggle_mute(cx))),
            )
            .child(
                div()
                    .id("fs-volume")
                    .relative()
                    .flex_1()
                    .h(sz(30.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        this.volume_hover = *hovered;
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, e: &MouseDownEvent, _, cx| {
                            this.dragging_volume = true;
                            this.volume_to(e.position.x, cx);
                        }),
                    )
                    .child(track(shown_volume, 3.0, theme::hex("#ffffffcc"), tokens.line))
                    .child(canvas(move |b, _, _| volume_cell.set(b), |_, _, _, _| {}).absolute().size_full())
                    // `opacity 0 → 1, translate(-50%, 3px) → (-50%, 0)`.
                    .when(reveal > 0.0, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left(sz(bubble_left))
                                .bottom(gpui::relative(1.0))
                                .mb(sz(4.0 - 3.0 * (1.0 - reveal)))
                                .opacity(reveal)
                                .min_w(sz(34.0))
                                .px(sz(5.0))
                                .py(sz(2.0))
                                .border(sz(1.0))
                                .border_color(tokens.line)
                                .rounded(sz(4.0))
                                .bg(tokens.surface)
                                .shadow(vec![gpui::BoxShadow {
                                    color: gpui::black().opacity(0.2),
                                    offset: gpui::point(px(0.0), sz(3.0).to_pixels(window.rem_size())),
                                    blur_radius: sz(9.0).to_pixels(window.rem_size()),
                                    spread_radius: px(0.0),
                                    inset: false,
                                }])
                                .text_size(sz(10.0))
                                .line_height(gpui::relative(1.3))
                                .text_color(tokens.text)
                                .font_features(crate::theme::fonts::tabular_nums())
                                .flex()
                                .justify_center()
                                .child(format!("{percentage}%")),
                        )
                    }),
            );

        let panel = self.panel;
        let footer = div()
            .flex()
            .items_center()
            .justify_between()
            .mt(sz(if narrow { 18.0 } else { 14.0 }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sz(6.0))
                    .text_size(sz(10.0))
                    .text_color(tokens.secondary)
                    .child(icon("audio-lines", 15.0))
                    .child(quality(Some(&song), compatible)),
            )
            .child(
                div()
                    .flex()
                    .gap(sz(9.0))
                    .child(
                        IconButton::new("fs-lyrics", "Show lyrics", icon("message-square-text", 19.0)).bare()
                            .color(if panel == Panel::Lyrics { tokens.accent } else { theme::hex("#c2c2c7") })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.panel = Panel::Lyrics;
                                cx.notify();
                            })),
                    )
                    .child(
                        IconButton::new("fs-queue", "Show queue", icon("list-music", 20.0)).bare()
                            .color(if panel == Panel::Queue { tokens.accent } else { theme::hex("#c2c2c7") })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.panel = Panel::Queue;
                                cx.notify();
                            })),
                    ),
            );

        let artist_state = self.state.clone();
        let artist_route = song.artist_id.clone().map(|id| crate::app_state::Route {
            page: crate::app_state::Page::Artist,
            id: Some(id),
            title: song.artist.clone(),
        });
        let record = div()
            .w(sz(record_width))
            .flex_none()
            .child(
                div()
                    .size(sz(art))
                    .mx_auto()
                    .rounded(sz(9.0))
                    .bg(base.surface)
                    .shadow(vec![BoxShadow {
                        color: theme::hex("#0002"),
                        offset: point(px(0.0), px(13.0)),
                        blur_radius: px(26.0),
                        spread_radius: px(0.0),
                        inset: false,
                    }])
                    .relative()
                    .when_some(cover, |d, image| d.child(img(image).size_full().rounded(sz(9.0))))
                    .when_some(morph, |d, (image, progress)| {
                        d.child(img(image).absolute().top_0().left_0().size_full().rounded(sz(9.0)).opacity(progress.clamp(0.0, 1.0)))
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sz(10.0))
                    .mt(sz(21.0))
                    .mb(sz(20.0))
                    .child(
                        div()
                            .min_w_0()
                            .child(
                                line(song.title.clone())
                                    .mb(sz(5.0))
                                    .text_size(sz(if narrow { 23.0 } else { 21.0 }))
                                    .font_weight(FontWeight::BOLD)
                                    .letter_spacing(sz(-0.45)),
                            )
                            .child(
                                div()
                                    .id("fs-artist")
                                    .text_size(sz(if narrow { 15.0 } else { 14.0 }))
                                    .text_color(tokens.secondary)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .when(artist_route.is_some(), |d| {
                                        d.cursor_pointer().hover(|s| s.text_color(gpui::white()).underline())
                                    })
                                    .on_click(move |_, _, cx| {
                                        if let Some(route) = artist_route.clone() {
                                            artist_state.update(cx, |s, cx| {
                                                s.full_player_open = false;
                                                s.navigate(route, cx);
                                            });
                                        }
                                    })
                                    .child(song.artist.clone().unwrap_or_else(|| "Unknown artist".into())),
                            )
                            .when(mixing, |d| {
                                d.child(div().flex().child(crate::components::motion::mixing_indicator("fs-mixing", true, &tokens)))
                            }),
                    )
                    .child(
                        IconButton::new("fs-favorite", "Favorite current song", icon_with("star", 23.0, None, favorite)).bare()
                            .color(theme::hex("#c2c2c7"))
                            .on_click(move |_, _, cx| fav_state.update(cx, |s, cx| s.toggle_song_favorite(&fav_song, cx))),
                    ),
            )
            .child(seek)
            .child(transport)
            .child(volume_row)
            .child(footer);

        let lyrics = self.lyrics.clone();
        let state = self.state.clone();
        let panel_el = theme::with_tokens(cx, tokens.clone(), |cx| inspector::render_embedded(&state, &lyrics, panel, cx));

        let close_state = self.state.clone();
        let client_frame = crate::components::window_chrome::custom_titlebar(window);
        // macOS: keep the close button clear of the traffic lights.
        let top_inset = crate::components::window_chrome::traffic_light_inset();
        crate::components::motion::enter(
        div()
            .id("full-player")
            .absolute()
            .inset_0()
            .occlude()
            .bg(theme::hex("#242426"))
            .map(|d| crate::components::window_chrome::window_corners(d, cx))
            .text_color(tokens.text)
            .child(self.background.clone())
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if e.pressed_button != Some(MouseButton::Left) {
                    this.dragging_seek = false;
                    this.dragging_volume = false;
                    return;
                }
                if this.dragging_seek {
                    this.seek_to(e.position.x, cx);
                } else if this.dragging_volume {
                    this.volume_to(e.position.x, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.dragging_seek = false;
                    this.dragging_volume = false;
                }),
            )
            .child(
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .flex_col()
                    .px(sz(24.0))
                    .py(sz(19.0))
                    .child(
                        div().h(sz(36.0)).flex_none().flex().items_center().mt(sz(top_inset)).child(
                            IconButton::new("fs-close", "Close", icon("chevron-left", 22.0)).bare()
                                .color(theme::hex("#c2c2c7"))
                                .on_click(move |_, _, cx| {
                                    close_state.update(cx, |s, cx| {
                                        s.full_player_open = false;
                                        cx.notify();
                                    })
                                }),
                        )
                        // The rest of the top bar moves the window, like a
                        // title bar (Linux client-side frame).
                        .when(client_frame, |d| {
                            // Windows: leave room for the floating window controls, whose
                            // hit regions would otherwise lose to this drag area.
                            d.child(
                                crate::components::window_chrome::drag_area(div().id("fs-drag"))
                                    .flex_1()
                                    .h_full()
                                    .mr(sz(if cfg!(target_os = "windows") { 110.0 } else { 0.0 })),
                            )
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .max_w(sz(1880.0))
                            .mx_auto()
                            .flex()
                            .items_center()
                            .gap(sz(gap))
                            .child(div().w(sz(left_col)).flex_none().flex().justify_center().child(record))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .max_h(sz(viewport_h - 135.0))
                                    .child(panel_el),
                            ),
                    ),
            ),
            "full-player-enter",
            260,
            0.0,
        )
    }
}
