//! The floating pill player (`PlaybackDock`, `Transport`, `Seek`,
//! `VolumeControl` in `src/player-ui.tsx`; `.playback-dock` in theme.css).

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Bounds, Context, Entity, FontWeight, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, Render, ScrollEvent, Styled, Window, canvas, div, img, prelude::*, px,
};
use volta_logic::format::duration;

use crate::app_state::{AppState, Panel};
use crate::components::motion::spinner;
use crate::components::ui::{IconButton, icon, icon_with};
use crate::player::{Player, Repeat};
use crate::theme::{self, fonts::tabular_nums, mix, sz};

/// `volta-volume-scroll-step` / `-shift-scroll-step` defaults (percent).
const VOLUME_SCROLL_STEP: f64 = 5.0;
const VOLUME_SHIFT_SCROLL_STEP: f64 = 10.0;

pub struct PlaybackDock {
    state: Entity<AppState>,
    player: Entity<Player>,
    seek_hover: bool,
    volume_hover: bool,
    /// Bounds of the seek and volume tracks from the last paint, for pointer maths.
    seek_bounds: Rc<Cell<Bounds<Pixels>>>,
    volume_bounds: Rc<Cell<Bounds<Pixels>>>,
    dragging_seek: bool,
    dragging_volume: bool,
    /// `.hover-times` transitions: the track (180 ms) and the time labels and
    /// caption blur (160 ms).
    seek_grow: crate::components::motion::Transition,
    seek_reveal: crate::components::motion::Transition,
}

/// The wheel's volume step in percent: the "Volume scroll step" settings
/// (Alt/Option or Ctrl selects the large step; Shift can't, because the OS
/// turns Shift+wheel into a horizontal scroll and the vertical delta is lost).
pub(crate) fn volume_scroll_step(state: &Entity<AppState>, shift: bool, cx: &gpui::App) -> f64 {
    let storage = &state.read(cx).storage;
    let (key, fallback) = if shift {
        (volta_logic::storage::VOLUME_SHIFT_SCROLL_STEP_KEY, VOLUME_SHIFT_SCROLL_STEP)
    } else {
        (volta_logic::storage::VOLUME_SCROLL_STEP_KEY, VOLUME_SCROLL_STEP)
    };
    storage.get(key).parse::<f64>().ok().filter(|v| (1.0..=10.0).contains(v)).map(f64::round).unwrap_or(fallback)
}

pub(crate) fn fraction(bounds: Bounds<Pixels>, x: Pixels) -> f64 {
    let width = f32::from(bounds.size.width).max(1.0);
    (f32::from(x - bounds.origin.x) / width).clamp(0.0, 1.0) as f64
}

/// A range input track: 3px tall, `--secondary` fill on `--line`.
pub(crate) fn track(progress: f64, height: f32, fill: gpui::Hsla, rest: gpui::Hsla) -> gpui::Div {
    let progress = progress.clamp(0.0, 1.0) as f32;
    div()
        .w_full()
        .h(sz(height))
        .rounded(sz(999.0))
        .bg(rest)
        .overflow_hidden()
        // gpui clips to rectangles, so the fill rounds its own ends to sit
        // inside the track's rounded ends.
        .child(div().h_full().w(gpui::relative(progress)).rounded(sz(999.0)).bg(fill))
}

impl PlaybackDock {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let player = state.read(cx).player.clone();
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            player,
            seek_hover: false,
            volume_hover: false,
            seek_bounds: Rc::new(Cell::new(Bounds::default())),
            volume_bounds: Rc::new(Cell::new(Bounds::default())),
            dragging_seek: false,
            seek_grow: crate::components::motion::Transition::new(180),
            seek_reveal: crate::components::motion::Transition::new(160),
            dragging_volume: false,
        }
    }

    /// `volta-volume-scroll-step` / `-shift-scroll-step` (1–10 %).
    fn scroll_step(&self, shift: bool, cx: &gpui::App) -> f64 {
        volume_scroll_step(&self.state, shift, cx)
    }

    fn seek_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let fraction = fraction(self.seek_bounds.get(), x);
        self.player.update(cx, |player, cx| {
            let target = fraction * player.duration.max(1.0);
            player.seek(target, cx);
        });
    }

    fn volume_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let fraction = fraction(self.volume_bounds.get(), x);
        self.player.update(cx, |player, cx| player.set_volume((fraction * 100.0).round() / 100.0, cx));
    }

    fn transport(&self, viewport: f32, inspector_compact: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx).clone();
        let player = self.player.read(cx);
        let has_song = player.current_song().is_some();
        let has_queue = !player.queue.is_empty();
        let p = self.player.clone();
        // ≤860px hides shuffle and repeat; ≤640px also hides previous and
        // makes the remaining buttons 32×38.
        let narrow = viewport <= 640.0;
        let compact = viewport <= 860.0 || inspector_compact;
        let button = |id: &'static str, label: String, child: gpui::AnyElement, width: f32| {
            let b = IconButton::new(id, label, child).flat().color(tokens.secondary);
            if narrow { b.width(32.0).height(38.0) } else { b.width(width) }
        };
        let play_icon = if player.loading {
            spinner(23.0).into_any_element()
        } else if player.playing {
            icon_with("pause", 24.0, None, true).into_any_element()
        } else {
            icon_with("play", 24.0, None, true).into_any_element()
        };
        div()
            .flex()
            .items_center()
            .gap(sz(1.0))
            .when(!compact, |d| d.child(
                button("shuffle", "Shuffle".into(), icon("shuffle", 16.0).into_any_element(), 22.0)
                    .color(tokens.muted)
                    .active(player.shuffle)
                    .disabled(!has_queue)
                    .on_click({
                        let p = p.clone();
                        move |_, _, cx| p.update(cx, |p, cx| p.toggle_shuffle(cx))
                    }),
            ))
            .when(!narrow, |d| d.child(
                button("previous", "Previous track".into(), icon_with("skip-back", 20.0, None, true).into_any_element(), 27.0)
                    .disabled(!has_song)
                    .on_click({
                        let p = p.clone();
                        move |_, _, cx| p.update(cx, |p, cx| p.previous(cx))
                    }),
            ))
            .child(
                button("play-pause", if player.playing { "Pause" } else { "Play" }.into(), play_icon, 27.0)
                    .disabled(!has_song)
                    .on_click({
                        let p = p.clone();
                        move |_, _, cx| p.update(cx, |p, cx| p.toggle(cx))
                    }),
            )
            .child(
                button("next", "Next track".into(), icon_with("skip-forward", 20.0, None, true).into_any_element(), 27.0)
                    .disabled(!has_song)
                    .on_click({
                        let p = p.clone();
                        move |_, _, cx| p.update(cx, |p, cx| p.next(cx))
                    }),
            )
            .when(!compact, |d| d.child(
                button(
                    "repeat",
                    format!("Repeat: {}", player.repeat.label()),
                    icon(if player.repeat == Repeat::One { "repeat1" } else { "repeat" }, 16.0).into_any_element(),
                    22.0,
                )
                .color(tokens.muted)
                .active(player.repeat != Repeat::Off)
                .disabled(!has_queue)
                .on_click(move |_, _, cx| p.update(cx, |p, cx| p.cycle_repeat(cx))),
            ))
    }

    /// `.seek-control.hover-times`: a 3px track that grows to 7px with time
    /// labels above it while hovered.
    fn seek(&self, grow: f32, reveal: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx).clone();
        let player = self.player.read(cx);
        let has_song = player.current_song().is_some();
        let duration_value = player.duration.max(0.0);
        let progress = if duration_value > 0.0 { player.position / duration_value } else { 0.0 };
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let bounds_cell = self.seek_bounds.clone();
        let times = |text: String| {
            div()
                .text_size(sz(10.0))
                .line_height(gpui::relative(1.0))
                .text_color(tokens.text)
                .font_features(tabular_nums())
                .child(text)
        };
        div()
            .id("seek")
            .relative()
            .w_full()
            .h(sz(8.0))
            .flex()
            .items_center()
            .when(has_song, |s| s.cursor_pointer())
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.seek_hover = *hovered;
                cx.notify();
            }))
            .when(has_song, |s| {
                s.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.dragging_seek = true;
                        this.seek_to(event.position.x, cx);
                    }),
                )
            })
            .child(
                div()
                    .w_full()
                    .mt(sz(lerp(3.0, 0.0, grow)))
                    .child(track(progress, lerp(3.0, 7.0, grow), mix(tokens.text, grow * 100.0, tokens.secondary), tokens.line)),
            )
            .child(
                canvas(move |bounds, _, _| bounds_cell.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            // `opacity 0 → 1, translateY(4px) → 0`.
            .when(reveal > 0.0, |s| {
                let bottom = sz(12.0 - 4.0 * (1.0 - reveal));
                s.child(div().absolute().left_0().bottom(bottom).opacity(reveal).child(times(duration(Some(player.position)))))
                    .child(
                        div()
                            .absolute()
                            .right_0()
                            .bottom(bottom)
                            .opacity(reveal)
                            .child(times(format!("-{}", duration(Some((duration_value - player.position).max(0.0)))))),
                    )
            })
    }

    fn volume(&self, viewport: f32, inspector_compact: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx).clone();
        let player = self.player.read(cx);
        let muted = player.muted;
        let volume = player.volume;
        let percentage = (volume * 100.0).round();
        let p = self.player.clone();
        let bounds_cell = self.volume_bounds.clone();
        let hovering = self.volume_hover || self.dragging_volume;
        div()
            .id("dock-volume")
            .flex()
            .items_center()
            .gap(sz(2.0))
            .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
                if delta == 0.0 {
                    return;
                }
                let step = this.scroll_step(event.modifiers.alt || event.modifiers.control, cx);
                let direction = if delta > 0.0 { 1.0 } else { -1.0 };
                this.player.update(cx, |player, cx| {
                    let next = ((player.volume + direction * step / 100.0) * 100.0).round() / 100.0;
                    player.set_volume(next, cx);
                });
                cx.stop_propagation();
            }))
            .child(
                IconButton::new("mute", if muted { "Unmute" } else { "Mute" }, icon(if muted { "volume-x" } else { "volume2" }, 19.0))
                    .flat()
                    .on_click(move |_, _, cx| p.update(cx, |p, cx| p.toggle_mute(cx))),
            )
            .child(
                div()
                    .id("volume-slider")
                    // `.dock-volume input { display: none }` at ≤1150px.
                    .when(viewport <= 1150.0 && !inspector_compact, |d| d.hidden())
                    .relative()
                    .w(sz(56.0))
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
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.dragging_volume = true;
                            this.volume_to(event.position.x, cx);
                        }),
                    )
                    .child(track(volume, if hovering { 5.0 } else { 3.0 }, tokens.secondary, tokens.line))
                    .child(
                        canvas(move |bounds, _, _| bounds_cell.set(bounds), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .when(hovering, |s| {
                        // `.volume-value` bubble, clamped inside the track.
                        let left = (56.0 * volume as f32).clamp(17.0, 56.0 - 17.0) - 17.0;
                        s.child(
                            div()
                                .absolute()
                                .left(sz(left))
                                .bottom(gpui::relative(1.0))
                                .mb(sz(4.0))
                                .min_w(sz(34.0))
                                .px(sz(5.0))
                                .py(sz(2.0))
                                .border(sz(1.0))
                                .border_color(tokens.line)
                                .rounded(sz(4.0))
                                .bg(tokens.surface)
                                .text_size(sz(10.0))
                                .line_height(gpui::relative(1.3))
                                .text_color(tokens.text)
                                .font_features(tabular_nums())
                                .flex()
                                .justify_center()
                                .child(format!("{percentage}%")),
                        )
                    }),
            )
    }
}

impl Render for PlaybackDock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx).clone();
        let player = self.player.read(cx);
        let song = player.current_song().cloned();
        let has_song = song.is_some();
        let cover_id = song.as_ref().and_then(|s| s.cover_art.clone());
        let cover = self.state.update(cx, |state, cx| state.cover(cover_id.as_deref(), 100, cx));
        let seek_hovering = (self.seek_hover || self.dragging_seek) && has_song;
        self.seek_grow.set(seek_hovering);
        self.seek_reveal.set(seek_hovering);
        let (grow, growing) = self.seek_grow.value();
        let (reveal, revealing) = self.seek_reveal.value();
        if growing || revealing {
            window.request_animation_frame();
        }
        let viewport = theme::viewport_css_width(window);
        let narrow = viewport <= 640.0;
        let panel = self.state.read(cx).panel;
        // `.inspector-open` at ≤1150px drops the same extras as ≤860px.
        let inspector_compact = panel.is_some() && viewport <= 1150.0 && viewport > 860.0;
        let wide = viewport > 860.0 && !inspector_compact;

        let mixing = self.state.read(cx).player.read(cx).automix_label.is_some();
        // During a mix the incoming cover dissolves in over the current one.
        let morph = {
            let player = self.state.read(cx).player.read(cx);
            player.mix_progress().zip(player.mixing_into().map(|s| s.cover_art.clone()))
        };
        let morph = morph.and_then(|(progress, id)| self.state.update(cx, |state, cx| state.cover(id.as_deref(), 100, cx)).map(|image| (image, progress)));
        let title = song.as_ref().map(|s| s.title.clone()).unwrap_or_else(|| "Volta".into());
        let subtitle = match &song {
            Some(s) => {
                let artist = s.artist.clone().unwrap_or_else(|| "Unknown artist".into());
                match &s.album {
                    Some(album) if !album.is_empty() => format!("{artist} — {album}"),
                    _ => artist,
                }
            }
            None => "Choose something to play".into(),
        };

        let art_size = if narrow { 38.0 } else { 35.0 };
        let art_radius = if narrow { 5.0 } else { 4.0 };
        // `.dock-artwork` is a button: "Open Now Playing".
        let open_state = self.state.clone();
        let artwork = div()
            .id("dock-artwork")
            .when(has_song, |a| {
                a.cursor_pointer().on_click(move |_, _, cx| {
                    open_state.update(cx, |s, cx| {
                        s.full_player_open = true;
                        cx.notify();
                    })
                })
            })
            .w(sz(art_size))
            .h(sz(art_size))
            .flex_none()
            .rounded(sz(art_radius))
            .overflow_hidden()
            .bg(tokens.surface)
            .flex()
            .items_center()
            .justify_center()
            .text_color(tokens.muted)
            .relative()
            .when_some(cover, |a, image| a.child(img(image).size_full().rounded(sz(art_radius))))
            .when_some(morph, |a, (image, progress)| {
                a.child(img(image).absolute().top_0().left_0().size_full().rounded(sz(art_radius)).opacity(progress.clamp(0.0, 1.0)))
            })
            // `filter: blur(3px); opacity: .55` while seeking.
            .when(reveal > 0.0, |a| a.opacity(1.0 - 0.45 * reveal).filter_blur(sz(3.0 * reveal)));

        let caption = div()
            .w_full()
            .min_w_0()
            .when(!has_song, |c| c.opacity(0.6))
            .when(reveal > 0.0, |c| c.opacity(1.0 - 0.45 * reveal).filter_blur(sz(3.0 * reveal)))
            .child(
                div()
                    .text_size(sz(if narrow { 12.0 } else { 11.0 }))
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_height(gpui::relative(1.25))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(title),
            )
            .child(
                div()
                    .text_size(sz(if narrow { 11.0 } else { 10.0 }))
                    .line_height(gpui::relative(1.3))
                    .text_color(tokens.secondary)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(subtitle),
            );

        let favorite = song.as_ref().is_some_and(|s| self.state.read(cx).is_favorite_song(s));
        let fav_state = self.state.clone();
        let fav_song = song.clone();
        let now_playing = div()
            .relative()
            .flex()
            .items_center()
            .gap(sz(9.0))
            .flex_1()
            .min_w(sz(80.0))
            .child(artwork)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(caption)
                    .when(mixing, |d| d.child(div().flex().child(crate::components::motion::mixing_indicator("dock-mixing", false, &tokens))))
                    .child(div().when(narrow, |d| d.mt(sz(2.0))).child(self.seek(grow, reveal, cx))),
            )
            .when(wide, |d| d.child(
                div().when(seek_hovering && wide, |d| d.invisible()).child(
                    IconButton::new(
                        "favorite",
                        "Favorite current song",
                        icon_with("star", 17.0, None, favorite),
                    )
                    .flat()
                    .width(24.0)
                    .active(favorite)
                    .disabled(!has_song)
                    .on_click(move |_, _, cx| {
                        if let Some(song) = fav_song.clone() {
                            fav_state.update(cx, |s, cx| s.toggle_song_favorite(&song, cx));
                        }
                    }),
                ),
            ));

        let hide_extras = seek_hovering && wide;
        let actions = div()
            .flex()
            .items_center()
            .gap(sz(2.0))
            .when(!hide_extras && wide, |a| {
                let state = self.state.clone();
                let (enabled, busy) = {
                    let s = self.state.read(cx);
                    (s.infinite_enabled(), s.infinite.busy)
                };
                a.child(
                    IconButton::new("infinite-play", if busy { "Adding songs to queue" } else { "Infinite Play" }, icon("infinity", 19.0))
                        .flat()
                        .width(27.0)
                        .active(enabled)
                        .disabled(busy)
                        .on_click(move |_, _, cx| state.update(cx, |s, cx| s.toggle_infinite_play(cx))),
                )
            })
            .child({
                let state = self.state.clone();
                IconButton::new("lyrics", "Lyrics", icon("message-square-text", 18.0))
                    .flat()
                    .width(27.0)
                    .active(panel == Some(Panel::Lyrics))
                    .on_click(move |_, _, cx| state.update(cx, |s, cx| s.toggle_panel(Panel::Lyrics, cx)))
            })
            .child({
                let state = self.state.clone();
                IconButton::new("queue", "Playing Next", icon("list-music", 19.0))
                    .flat()
                    .width(27.0)
                    .active(panel == Some(Panel::Queue))
                    .on_click(move |_, _, cx| state.update(cx, |s, cx| s.toggle_panel(Panel::Queue, cx)))
            })
            .child(self.volume(viewport, inspector_compact, cx))
            // `.inspector-open .dock-actions > .icon-button:last-child { display: none }`.
            .when(!hide_extras && wide && panel.is_none(), |a| {
                a.child(
                    IconButton::new("expand", "Expand player", icon("expand", 17.0))
                        .flat()
                        .width(27.0)
                        .disabled(!has_song)
                        .on_click({
                            let state = self.state.clone();
                            move |_, _, cx| {
                                state.update(cx, |s, cx| {
                                    s.full_player_open = true;
                                    cx.notify();
                                })
                            }
                        }),
                )
            });

        div()
            .id("playback-dock")
            // The dock floats over the page: clicks and hovers on it must not
            // reach whatever is underneath.
            .occlude()
            .w_full()
            .max_w(sz(680.0))
            .min_h(sz(if narrow { 67.0 } else { 52.0 }))
            .mx_auto()
            .flex()
            .when(narrow, |d| d.flex_row_reverse())
            .items_center()
            .gap(sz(if narrow { 9.0 } else if wide && panel.is_none() { 12.0 } else { 8.0 }))
            .px(sz(if narrow { 13.0 } else if wide { 12.0 } else { 10.0 }))
            .py(sz(if narrow { 10.0 } else { 6.0 }))
            .border(sz(1.0))
            .rounded(sz(if narrow { 16.0 } else { 32.0 }))
            // Liquid Glass (rim light and shadow included); themes without it (high
            // contrast) keep the opaque blurred pill.
            .map(|d| match theme::glass::dock(&tokens, narrow) {
                Some(glass) => d.border_color(theme::transparent()).glass(glass),
                None => d
                    .border_color(tokens.line)
                    .backdrop_blur(sz(20.0))
                    .bg(tokens.dock)
                    .shadow(vec![
                        gpui::BoxShadow {
                            color: theme::hex("#0003"),
                            offset: gpui::point(px(0.0), px(10.0)),
                            blur_radius: px(28.0),
                            spread_radius: px(0.0),
                            inset: false,
                        },
                        gpui::BoxShadow {
                            color: theme::hex("#ffffff06"),
                            offset: gpui::point(px(0.0), px(1.0)),
                            blur_radius: px(0.0),
                            spread_radius: px(0.0),
                            inset: true,
                        },
                    ]),
            })
            .text_color(tokens.text)
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if event.pressed_button != Some(MouseButton::Left) {
                    if this.dragging_seek || this.dragging_volume {
                        this.dragging_seek = false;
                        this.dragging_volume = false;
                        cx.notify();
                    }
                    return;
                }
                if this.dragging_seek {
                    this.seek_to(event.position.x, cx);
                } else if this.dragging_volume {
                    this.volume_to(event.position.x, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dragging_seek = false;
                    this.dragging_volume = false;
                    cx.notify();
                }),
            )
            .child(self.transport(viewport, inspector_compact, cx))
            .child(now_playing)
            .when(!narrow, |d| d.child(actions))
    }
}
