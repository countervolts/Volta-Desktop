//! Album and playlist detail: the `route.page === "album" || "playlist"`
//! branch of App.tsx (`.collection-header`, track table, footer, related).

use gpui::{AnyElement, App, BoxShadow, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window, div, img, point, prelude::*};
use rand::seq::SliceRandom;
use volta_logic::format::album_name;
use volta_logic::model::Song;

use crate::app_state::{AppState, Page, Route};
use crate::components::album_grid::{GridKind, album_grid};
use crate::components::track_table::{TableKind, track_table};
use crate::components::ui::icon_with;
use crate::theme::{self, sz};
use crate::views::home::{empty_state, section_heading};

/// `.collection-actions button`.
pub fn pill(
    id: &str,
    label: &str,
    icon: Option<(&str, f32, bool)>,
    disabled: bool,
    viewport: f32,
    inspector_compact: bool,
    cx: &App,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let (min_w, h, px, text) = if viewport <= 640.0 {
        (64.0, 28.0, 11.0, 11.0)
    } else if viewport <= 860.0 {
        (76.0, 29.0, 14.0, 12.0)
    } else if inspector_compact {
        (70.0, 28.0, 12.0, 12.0)
    } else {
        (106.0, 33.0, 20.0, 12.0)
    };
    div()
        .id(SharedString::from(format!("collection-{id}")))
        .flex()
        .items_center()
        .justify_center()
        .gap(sz(5.0))
        .min_w(sz(min_w))
        // Flex items shrink to `min-width` and wrap their label, as on the web.
        .flex_shrink(1.0)
        .text_center()
        .h(sz(h))
        // These buttons also carry `.secondary-button`, whose
        // `min-height: 34px` outranks the collection heights.
        .when(!matches!(id, "play" | "shuffle"), |d| d.min_h(sz(34.0)))
        .px(sz(px))
        .rounded(sz(20.0))
        .bg(tokens.surface)
        .text_color(tokens.accent)
        .text_size(sz(text))
        .font_weight(FontWeight::SEMIBOLD)
        .when(disabled, |d| d.opacity(0.5))
        .when(!disabled, |d| d.cursor_pointer().hover(|s| s.bg(tokens.hover)))
        .on_click(move |_, window, cx| {
            if !disabled {
                on_click(window, cx)
            }
        })
        .when_some(icon, |d, (name, size, fill)| d.child(icon_with(name, size, None, fill)))
        // A flex item never shrinks below its longest word (CSS min-content),
        // so only multi-word labels may wrap.
        .child(
            div()
                .line_height(gpui::relative(1.15))
                .map(|d| if label.contains(' ') { d.min_w_0().flex_shrink(1.0) } else { d.flex_none().whitespace_nowrap() })
                .child(label.to_string()),
        )
        .into_any_element()
}

fn play_songs(state: &Entity<AppState>, songs: Vec<Song>, shuffle: bool, cx: &mut App) {
    let mut songs = songs;
    if shuffle {
        songs.shuffle(&mut rand::rng());
    }
    let player = state.read(cx).player.clone();
    player.update(cx, |p, cx| p.play_queue(songs, 0, cx));
}

pub fn render(state: &Entity<AppState>, content_width: f32, window: &mut Window, cx: &mut App) -> impl IntoElement {
    let tokens = theme::tokens(cx).clone();
    let viewport = theme::viewport_css_width(window);
    let route = state.read(cx).route().clone();
    let page = state.read(cx).page.clone();
    let loading = state.read(cx).page_loading;
    let is_album = route.page == Page::Album;
    if loading && page.album.is_none() && page.playlist.is_none() {
        return div().into_any_element();
    }
    if let Some(error) = state.read(cx).page_error.clone() {
        return empty_state("Unable to load music", &error, cx).into_any_element();
    }

    let cover_id = page
        .album
        .as_ref()
        .and_then(|a| a.cover_art.clone())
        .or_else(|| page.playlist.as_ref().and_then(|p| p.cover_art.clone()));
    let cover = state.update(cx, |s, cx| s.cover(cover_id.as_deref(), 700, cx));
    let title = page
        .album
        .as_ref()
        .map(|a| album_name(a).to_string())
        .or_else(|| page.playlist.as_ref().map(|p| p.name.clone()))
        .or(route.title.clone())
        .unwrap_or_default();
    let meta = {
        let lead = page
            .album
            .as_ref()
            .and_then(|a| a.genre.clone())
            .or_else(|| page.playlist.as_ref().and_then(|p| p.owner.clone()))
            .unwrap_or_default();
        let year = page.album.as_ref().and_then(|a| a.year).filter(|y| *y != 0.0);
        format!("{lead}{}", year.map(|y| format!(" · {}", y as i64)).unwrap_or_default())
    };

    // `grid-template-columns: minmax(190px, 254px) minmax(0, 1fr)`, gap 29;
    // `max-width: 860px` → 155px, gap 20; 640px → 126px, gap 17.
    let inspector_compact = state.read(cx).panel.is_some() && viewport <= 1150.0 && viewport > 860.0;
    let (art, gap) = if viewport <= 640.0 {
        (126.0, 17.0)
    } else if viewport <= 860.0 {
        (155.0, 20.0)
    } else if inspector_compact {
        (165.0, 18.0)
    } else {
        ((content_width - 29.0).clamp(190.0, 254.0), 29.0)
    };
    let (h1, artist_size, info_pt) = if viewport <= 640.0 {
        (22.0, 17.0, 0.0)
    } else if viewport <= 860.0 {
        (24.0, 19.0, 0.0)
    } else if inspector_compact {
        (24.0, 19.0, 3.0)
    } else {
        (28.0, 23.0, 25.0)
    };
    let narrow = viewport <= 640.0;

    let songs = page.songs.clone();
    let has_songs = !songs.is_empty();
    let mut actions = div()
        // Constrained to the info column so the pills shrink (and wrap their
        // labels) instead of overflowing, like the web's flex row.
        .max_w_full()
        .flex()
        .when(viewport <= 860.0 || inspector_compact, |d| d.flex_wrap())
        .gap(sz(if narrow || inspector_compact { 5.0 } else if viewport <= 860.0 { 6.0 } else { 9.0 }))
        .mt_auto()
        .pt(sz(if narrow { 13.0 } else if viewport <= 860.0 || inspector_compact { 15.0 } else { 22.0 }))
        .pb(sz(1.0));
    {
        let (s1, s2, songs1, songs2) = (state.clone(), state.clone(), songs.clone(), songs.clone());
        actions = actions
            .child(pill("play", "Play", Some(("play", 15.0, true)), !has_songs, viewport, inspector_compact, cx, move |_, cx| {
                play_songs(&s1, songs1.clone(), false, cx)
            }))
            .child(pill("shuffle", "Shuffle", Some(("shuffle", 16.0, false)), !has_songs, viewport, inspector_compact, cx, move |_, cx| {
                play_songs(&s2, songs2.clone(), true, cx)
            }));
    }
    if let Some(album) = page.album.as_ref() {
        let favorite = state.read(cx).is_favorite_id(&album.id, album.starred.is_some());
        let (fav_state, fav_id, fav_starred) = (state.clone(), album.id.clone(), album.starred.is_some());
        actions = actions
            .child(pill(
                "favorite",
                if favorite { "Favorited" } else { "Favorite" },
                Some(("star", 15.0, favorite)),
                false,
                viewport,
                inspector_compact,
                cx,
                move |_, cx| fav_state.update(cx, |s, cx| s.toggle_favorite("album", fav_id.clone(), fav_starred, cx)),
            ))
            .child({
                use volta_logic::storage::{Pin, PinKind};
                let pinned = state.read(cx).is_pinned(PinKind::Album, &album.id);
                let pin = Pin {
                    kind: PinKind::Album,
                    id: album.id.clone(),
                    name: album_name(album).to_string(),
                    cover_art: album.cover_art.clone(),
                    image_url: None,
                };
                let target = state.clone();
                pill("pin", if pinned { "Unpin" } else { "Pin" }, Some(("pin", 15.0, pinned)), false, viewport, inspector_compact, cx, move |_, cx| {
                    let pin = pin.clone();
                    target.update(cx, |s, cx| s.toggle_pin(pin, cx))
                })
            })
            .child({
                let (target, album) = (state.clone(), album.clone());
                pill("share", "Copy Share Link", Some(("copy", 15.0, false)), false, viewport, inspector_compact, cx, move |_, cx| {
                    target.update(cx, |s, cx| s.share_album(&album, cx))
                })
            });
    }
    if let Some(playlist) = page.playlist.clone().filter(|p| state.read(cx).can_edit_playlist(p)) {
        let (edit_state, delete_state, edit_pl, delete_pl) = (state.clone(), state.clone(), playlist.clone(), playlist);
        actions = actions
            .child(pill("edit-playlist", "Edit Playlist", None, false, viewport, inspector_compact, cx, move |_, cx| {
                let p = edit_pl.clone();
                edit_state.update(cx, |s, cx| s.edit_playlist(p, cx))
            }))
            .child(pill("delete-playlist", "Delete Playlist", None, false, viewport, inspector_compact, cx, move |_, cx| {
                let p = delete_pl.clone();
                delete_state.update(cx, |s, cx| {
                    s.dialog = Some(crate::app_state::Dialog::DeletePlaylist(p));
                    cx.notify();
                })
            }));
    }
    let selecting = state.read(cx).selection.selecting;
    let select_target = state.clone();
    actions = actions.child(
        div()
            .flex_shrink(1.0)
            .rounded(sz(7.0))
            .border(sz(1.0))
            .border_color(if selecting { theme::tokens(cx).accent } else { gpui::transparent_black() })
            .child(pill("select", if selecting { "Done" } else { "Select" }, Some(("check-square", 15.0, false)), !has_songs, viewport, inspector_compact, cx, move |_, cx| {
                select_target.update(cx, |s, cx| s.toggle_selecting(cx))
            })),
    );
    if page.playlist.is_some() {
        let playlist = page.playlist.clone().unwrap_or_default();
        use volta_logic::storage::{Pin, PinKind};
        let pinned = state.read(cx).is_pinned(PinKind::Playlist, &playlist.id);
        let pin = Pin { kind: PinKind::Playlist, id: playlist.id.clone(), name: playlist.name.clone(), cover_art: playlist.cover_art.clone(), image_url: None };
        let target = state.clone();
        actions = actions.child(pill("pin-playlist", if pinned { "Unpin" } else { "Pin" }, Some(("pin", 15.0, false)), false, viewport, inspector_compact, cx, move |_, cx| {
            let pin = pin.clone();
            target.update(cx, |s, cx| s.toggle_pin(pin, cx))
        }));
    }

    let artist_link = page.album.as_ref().map(|album| {
        let artist_id = album.artist_id.clone();
        let artist_name = album.artist.clone();
        let link_state = state.clone();
        div()
            .id("collection-artist")
            .text_size(sz(artist_size))
            .text_color(tokens.accent)
            .when(artist_id.is_some(), |d| d.cursor_pointer())
            .on_click(move |_, _, cx| {
                if let Some(id) = artist_id.clone() {
                    let route = Route { page: Page::Artist, id: Some(id), title: artist_name.clone() };
                    link_state.update(cx, |s, cx| s.navigate(route, cx));
                }
            })
            .child(album.artist.clone().filter(|a| !a.is_empty()).unwrap_or_else(|| "Unknown artist".into()))
    });

    let header = div()
        .flex()
        .items_center()
        .gap(sz(gap))
        .pt(sz(9.0))
        .pb(sz(20.0))
        .child(
            div()
                .size(sz(art))
                .flex_none()
                .rounded(sz(7.0))
                .overflow_hidden()
                .bg(tokens.surface)
                .shadow(vec![BoxShadow {
                    color: theme::hex("#0002"),
                    offset: point(gpui::px(0.0), gpui::px(7.0)),
                    blur_radius: gpui::px(15.0),
                    spread_radius: gpui::px(0.0),
                    inset: false,
                }])
                .when_some(cover, |d, image| d.child(img(image).size_full().rounded(sz(7.0)))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_start()
                .min_w_0()
                .flex_1()
                // `height: 100%` of a grid row at least as tall as the artwork.
                .min_h(sz(art))
                .pt(sz(info_pt))
                .child(
                    div()
                        .text_size(sz(if narrow { 10.0 } else { 11.0 }))
                        .text_color(tokens.secondary)
                        .mb(sz(3.0))
                        .child(if is_album { "Album" } else { "Playlist" }),
                )
                .child(
                    div()
                        .mt(sz(3.0))
                        .mb(sz(5.0))
                        .max_w_full()
                        .text_size(sz(h1))
                        .font_weight(FontWeight::BOLD)
                        .letter_spacing(sz(-0.7))
                        .line_height(gpui::relative(if narrow { 1.12 } else { 1.1 }))
                        .child(title),
                )
                .children(artist_link)
                .when_some(page.playlist.as_ref().and_then(|p| p.comment.clone()).filter(|c| !c.is_empty()), |d, comment| {
                    d.child(
                        div()
                            .mt(sz(6.0))
                            .max_w(sz(380.0))
                            .text_size(sz(if narrow { 11.0 } else { 12.0 }))
                            .text_color(tokens.secondary)
                            .line_height(gpui::relative(1.45))
                            .child(comment),
                    )
                })
                .child(
                    div()
                        .mt(sz(8.0))
                        .text_size(sz(if narrow { 10.0 } else { 11.0 }))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(tokens.secondary)
                        .child(meta),
                )
                .child(actions),
        );

    let filter = state.read(cx).filter.to_lowercase();
    let visible: Vec<Song> = songs
        .iter()
        .filter(|s| {
            filter.is_empty()
                || format!("{} {} {}", s.title, s.artist.as_deref().unwrap_or(""), s.album.as_deref().unwrap_or(""))
                    .to_lowercase()
                    .contains(&filter)
        })
        .cloned()
        .collect();
    let kind = if is_album { TableKind::Album } else { TableKind::Playlist };
    let minutes = (songs.iter().map(|s| s.duration.unwrap_or(0.0)).sum::<f64>() / 60.0).round() as i64;

    let mut page_el = div()
        .flex()
        .flex_col()
        // `{bulkBar}` sits above the page content.
        .children(crate::views::selection::bulk_bar(state, cx))
        .child(header)
        .child(track_table(state, "collection", &visible, kind, viewport, cx))
        .child(
            div()
                .my(sz(20.0))
                .text_size(sz(11.0))
                .text_color(tokens.muted)
                .child(format!("{} songs, {minutes} minutes", songs.len())),
        );
    if is_album && !page.similar_albums.is_empty() {
        page_el = page_el.child(
            div()
                .mb(sz(32.0))
                .child(section_heading(state, "You might also like", Some("Albums"), None, cx))
                .child(album_grid(state, "related", &page.similar_albums, GridKind::Shelf, content_width, viewport, cx)),
        );
    }
    page_el.into_any_element()
}
