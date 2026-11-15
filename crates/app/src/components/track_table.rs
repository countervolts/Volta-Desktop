//! `TrackTable` from components.tsx.
//!
//! Variants: the full table (`#`, Title with artwork, Album, Quality, Time,
//! menu), the album table (`.album-tracks`: `#`, Title, Time, menu) and the
//! playlist table (`.playlist-tracks`: album layout with artwork and artist).
//! CSS grid tracks are mapped to fixed widths plus `flex-grow` for the `fr`
//! columns; column sets switch at the 1150px and 640px media queries.

use gpui::{AnyElement, App, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, div, img, prelude::*};
use volta_logic::format::{duration, is_lossless};
use volta_logic::model::Song;

use crate::app_state::{AppState, Page, Route};
use crate::components::ui::{IconButton, icon, icon_with, line};
use crate::theme::{self, fonts, sz};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TableKind {
    Full,
    Album,
    Playlist,
}

struct Columns {
    number: f32,
    gap: f32,
    time: f32,
    menu: f32,
    /// `(min, fr)` for title and album; album is `None` when hidden.
    title: (f32, f32),
    album: Option<(f32, f32)>,
    quality: Option<f32>,
    padding_x: f32,
}

fn columns(kind: TableKind, viewport: f32) -> Columns {
    let narrow = viewport <= 640.0;
    let compact = viewport <= 1150.0;
    if kind != TableKind::Full {
        return if narrow {
            Columns { number: 22.0, gap: 7.0, time: 36.0, menu: 25.0, title: (0.0, 1.0), album: None, quality: None, padding_x: 4.0 }
        } else if compact {
            Columns { number: 24.0, gap: 8.0, time: 40.0, menu: 25.0, title: (0.0, 1.0), album: None, quality: None, padding_x: 9.0 }
        } else {
            Columns { number: 24.0, gap: 10.0, time: 45.0, menu: 28.0, title: (0.0, 1.0), album: None, quality: None, padding_x: 9.0 }
        };
    }
    if narrow {
        Columns { number: 22.0, gap: 7.0, time: 36.0, menu: 25.0, title: (0.0, 1.0), album: None, quality: None, padding_x: 4.0 }
    } else if compact {
        Columns {
            number: 24.0,
            gap: 8.0,
            time: 40.0,
            menu: 25.0,
            title: (100.0, 1.5),
            album: Some((75.0, 1.0)),
            quality: None,
            padding_x: 9.0,
        }
    } else {
        Columns {
            number: 30.0,
            gap: 10.0,
            time: 42.0,
            menu: 28.0,
            title: (120.0, 1.5),
            album: Some((100.0, 1.0)),
            quality: Some(64.0),
            padding_x: 9.0,
        }
    }
}

fn fr(div: gpui::Div, (min, grow): (f32, f32)) -> gpui::Div {
    div.flex_grow(grow).flex_basis(gpui::relative(0.0)).min_w(sz(min))
}

/// `.playing-indicator`: three bars bouncing (`bounce-bar`).
fn playing_indicator(id: &str, color: gpui::Hsla) -> impl IntoElement {
    use crate::components::motion::bounce_bar;
    div()
        .flex()
        .items_end()
        .justify_center()
        .gap(sz(2.0))
        .w(sz(16.0))
        .h(sz(14.0))
        .child(bounce_bar(SharedString::from(format!("{id}-bar1")), 0.55, 0.0, color))
        .child(bounce_bar(SharedString::from(format!("{id}-bar2")), 0.95, 0.5, color))
        .child(bounce_bar(SharedString::from(format!("{id}-bar3")), 0.7, 0.2, color))
}

#[allow(clippy::too_many_arguments)]
pub fn track_table(
    state: &Entity<AppState>,
    id: &str,
    songs: &[Song],
    kind: TableKind,
    viewport: f32,
    cx: &mut App,
) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let c = columns(kind, viewport);
    let (current_id, playing) = {
        let player = state.read(cx).player.read(cx);
        (player.current_song().map(|s| s.id.clone()), player.playing)
    };
    let with_art = kind != TableKind::Album;
    let row_min_h = match kind {
        TableKind::Album => {
            if viewport <= 640.0 {
                44.0
            } else {
                43.0
            }
        }
        TableKind::Playlist => 54.0,
        TableKind::Full => 52.0,
    };
    let art = if kind == TableKind::Playlist { 40.0 } else if viewport <= 640.0 { 33.0 } else { 36.0 };
    let all: std::sync::Arc<Vec<Song>> = std::sync::Arc::new(songs.to_vec());
    // Select mode (`.track-table.selecting`): a checkbox column first.
    let selecting = crate::views::selection::selectable_table(id) && state.read(cx).selection.selecting;
    let selected_set = if selecting {
        state.update(cx, |s, _| s.selection.pool = songs.to_vec());
        state.read(cx).selection.selected.clone()
    } else {
        Default::default()
    };
    let select_w = if viewport <= 720.0 { 24.0 } else { 26.0 };
    let checkbox = |checked: bool, tokens: &theme::Tokens| {
        div()
            .size(sz(15.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(sz(3.0))
            .border(sz(1.0))
            .border_color(if checked { tokens.accent } else { tokens.muted })
            .when(checked, |d| d.bg(tokens.accent).text_color(gpui::white()).child(icon_with("check", 11.0, Some(3.0), false)))
    };

    let mut table = div().w_full().text_size(sz(12.0)).flex().flex_col();
    if kind == TableKind::Full {
        let mut heading = div()
            .flex()
            .items_center()
            .gap(sz(c.gap))
            .h(sz(30.0))
            .px(sz(c.padding_x))
            .border_b(sz(1.0))
            .border_color(tokens.line)
            .text_color(tokens.muted)
            .text_size(sz(10.0))
            .when(selecting, |d| {
                let all_selected = !songs.is_empty() && selected_set.len() == songs.len();
                let target = state.clone();
                d.child(
                    div()
                        .id(SharedString::from(format!("select-all-{id}")))
                        .w(sz(select_w))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .cursor_pointer()
                        .on_click(move |_, _, cx| target.update(cx, |s, cx| s.toggle_select_all(cx)))
                        .child(checkbox(all_selected, &tokens)),
                )
            })
            .child(div().w(sz(c.number)).flex_none().child("#"))
            .child(fr(div(), c.title).child("Title"));
        if let Some(album) = c.album {
            heading = heading.child(fr(div(), album).text_size(sz(11.0)).text_color(tokens.secondary).child("Album"));
        }
        if let Some(q) = c.quality {
            heading = heading.child(div().w(sz(q)).flex_none().text_size(sz(9.0)).child("Quality"));
        }
        heading = heading
            .child(div().w(sz(c.time)).flex_none().child("Time"))
            .child(div().w(sz(c.menu)).flex_none());
        table = table.child(heading);
    }

    let build_row = |index: usize, cx: &mut App| -> AnyElement {
        let song = &songs[index];
        let current = current_id.as_deref() == Some(song.id.as_str());
        let group = SharedString::from(format!("song-row-{id}-{index}"));
        let number_text = match (song.track, kind) {
            (Some(track), TableKind::Album) => format!("{}", track as i64),
            _ => format!("{}", index + 1),
        };
        let play_state = state.clone();
        let queue = all.clone();
        let number: AnyElement = if current && playing {
            playing_indicator(&format!("playing-{id}"), tokens.accent).into_any_element()
        } else {
            div()
                .child(div().group_hover(group.clone(), |s| s.invisible()).child(number_text))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(tokens.accent)
                        .invisible()
                        .group_hover(group.clone(), |s| s.visible())
                        .child(icon_with("play", 13.0, None, true)),
                )
                .into_any_element()
        };
        let favorite = state.read(cx).is_favorite_song(song);
        // Menus are built when opened, not for every row on every frame.
        let (menu_songs, context_songs) = (all.clone(), all.clone());
        let menu_state = state.clone();
        let context_state = state.clone();
        let cover = if with_art {
            state.update(cx, |s, cx| s.cover(song.cover_art.as_deref(), 80, cx))
        } else {
            None
        };
        let title = div()
            .flex()
            .items_center()
            .gap(sz(10.0))
            .min_w_0()
            .when(with_art, |d| {
                d.child(
                    div()
                        .size(sz(art))
                        .flex_none()
                        .rounded(sz(4.0))
                        .overflow_hidden()
                        .bg(tokens.surface)
                        .when_some(cover, |d, image| d.child(img(image).size_full().rounded(sz(4.0))))
                        .when(song.cover_art.is_none(), |d| {
                            d.flex().items_center().justify_center().text_color(tokens.muted).child(icon_with("music2", art * 0.38, Some(1.0), false))
                        }),
                )
            })
            .child(
                div()
                    .min_w_0()
                    .child(line(song.title.clone()).text_size(sz(12.0)).font_weight(FontWeight::NORMAL))
                    .when(with_art, |d| {
                        d.child(
                            line(song.artist.clone().filter(|a| !a.is_empty()).unwrap_or_else(|| "Unknown artist".into()))
                                .mt(sz(3.0))
                                .text_size(sz(10.0))
                                .text_color(tokens.secondary),
                        )
                    }),
            )
            .when(favorite, |d| {
                d.child(div().flex_none().text_color(tokens.accent).child(icon_with("star", 12.0, None, true)))
            });

        let mut row = div()
            .id(SharedString::from(format!("song-{id}-{index}")))
            .group(group.clone())
            .flex()
            .items_center()
            .gap(sz(c.gap))
            .min_h(sz(row_min_h))
            .px(sz(c.padding_x))
            .py(sz(4.0))
            .border_b(sz(1.0))
            .border_color(tokens.line)
            .rounded(sz(5.0))
            .cursor_pointer()
            .when(index % 2 == 1, |d| d.bg(tokens.row))
            .when(current, |d| d.text_color(tokens.accent).bg(tokens.selection))
            .when(selected_set.contains(&index), |d| d.bg(tokens.selection))
            .when(!current, |d| d.hover(|s| s.bg(tokens.surface)))
            .on_mouse_down(gpui::MouseButton::Right, move |event, _, cx| {
                cx.stop_propagation();
                let items = crate::components::menu::song_context_items(&context_state, &context_songs[index], cx);
                crate::components::menu::open(&context_state, event.position, gpui::Anchor::TopLeft, items, cx);
            })
            .on_click(move |event, _, cx| {
                if selecting {
                    let shift = event.modifiers().shift;
                    play_state.update(cx, |s, cx| s.toggle_song_selection(index, shift, cx));
                    return;
                }
                let songs = queue.as_ref().clone();
                let player = play_state.read(cx).player.clone();
                player.update(cx, |p, cx| p.play_queue(songs, index, cx));
            })
            .when(selecting, |d| {
                d.child(div().w(sz(select_w)).flex_none().flex().justify_center().child(checkbox(selected_set.contains(&index), &tokens)))
            })
            .child(
                div()
                    .relative()
                    .w(sz(c.number))
                    .flex_none()
                    .min_h(sz(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(sz(11.0))
                    .text_color(tokens.muted)
                    .child(number),
            )
            .child(fr(div(), c.title).child(title));
        if let Some(album_col) = c.album {
            let album_id = song.album_id.clone();
            let album_state = state.clone();
            let album_title = song.album.clone();
            row = row.child(
                fr(div(), album_col).min_w_0().child(
                    div()
                        .id(SharedString::from(format!("song-album-{id}-{index}")))
                        .text_size(sz(11.0))
                        .text_color(tokens.secondary)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            if let Some(album_id) = album_id.clone() {
                                let route = Route { page: Page::Album, id: Some(album_id), title: album_title.clone() };
                                album_state.update(cx, |s, cx| s.navigate(route, cx));
                            }
                        })
                        .child(song.album.clone().filter(|a| !a.is_empty()).unwrap_or_else(|| "—".into())),
                ),
            );
        }
        if let Some(q) = c.quality {
            let label = if is_lossless(Some(song)) {
                "Lossless".to_string()
            } else {
                song.suffix.as_deref().map(str::to_uppercase).filter(|s| !s.is_empty()).unwrap_or_else(|| "—".into())
            };
            row = row.child(div().w(sz(q)).flex_none().text_size(sz(9.0)).text_color(tokens.muted).child(label));
        }
        row = row
            .child(
                div()
                    .w(sz(c.time))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .text_size(sz(11.0))
                    .text_color(tokens.secondary)
                    .font_features(fonts::tabular_nums())
                    .child(duration(song.duration)),
            )
            .child(
                div().w(sz(c.menu)).flex_none().opacity(0.65).child(
                    IconButton::new(SharedString::from(format!("song-menu-{id}-{index}")), "Options", icon("ellipsis", 18.0))
                        .width(c.menu)
                        .height(26.0)
                        .on_click(move |event, _, cx| {
                            cx.stop_propagation();
                            // Radix: `align="end"`, `sideOffset={5}` under the trigger.
                            let at = event.position() + gpui::point(gpui::px(14.0), gpui::px(18.0));
                            let items = crate::components::menu::song_items(&menu_state, &menu_songs[index], cx);
                            crate::components::menu::open(&menu_state, at, gpui::Anchor::TopRight, items, cx);
                        }),
                ),
            );
        row.into_any_element()
    };
    // Long tables render only the rows near the viewport (virtual_rows).
    let pitch = row_min_h + 1.0;
    table = table.child(crate::components::virtual_rows::virtual_rows(
        format!("track-table-{id}"),
        songs.len(),
        pitch,
        0.0,
        cx,
        |range, cx| range.map(|index| build_row(index, cx)).collect(),
    ));
    table.into_any_element()
}
