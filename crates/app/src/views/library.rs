//! Library list pages from App.tsx: Albums, Recently Added, Recently Played,
//! Artists, Songs, Favorites and Playlists.

use std::borrow::Cow;
use std::rc::Rc;

use gpui::{AnyElement, App, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window, div, img, prelude::*};
use volta_logic::format::album_name;
use volta_logic::library::locale_compare;
use volta_logic::model::{AlbumRecord, Artist, Song};

use crate::app_state::{AppState, Page, Route};
use crate::components::album_grid::{GridKind, album_grid, metrics};
use crate::components::select::select;
use crate::components::track_table::{TableKind, track_table};
use crate::components::ui::{icon, icon_with, line};
use crate::theme::{self, sz};
use crate::views::home::{empty_state, page_heading_with};

pub const ALBUM_SORTS: [(&str, &str); 4] = [
    ("alphabeticalByName", "Album"),
    ("alphabeticalByArtist", "Artist"),
    ("newest", "Recently Added"),
    ("frequent", "Most Played"),
];
pub const SONG_SORTS: [(&str, &str); 4] =
    [("default", "Default order"), ("title", "Title"), ("artist", "Artist"), ("album", "Album")];
pub const ARTIST_SORTS: [(&str, &str); 2] = [("name", "Name"), ("albums", "Most albums")];

/// `.secondary-button`.
pub fn secondary_button(id: &str, label: &str, icon_name: &str, cx: &App) -> gpui::Stateful<gpui::Div> {
    let tokens = theme::tokens(cx).clone();
    div()
        .id(SharedString::from(id.to_string()))
        .flex()
        .items_center()
        .justify_center()
        .gap(sz(8.0))
        .min_h(sz(34.0))
        .px(sz(20.0))
        .py(sz(7.0))
        .rounded(sz(7.0))
        .bg(tokens.surface)
        .text_color(tokens.accent)
        .text_size(sz(13.0))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|s| s.bg(tokens.hover))
        // Lucide's Play is always drawn `fill="currentColor"` in the web.
        .when(icon_name == "play", |d| d.child(crate::components::ui::icon_with("play", 14.0, None, true)))
        .when(!icon_name.is_empty() && icon_name != "play", |d| d.child(icon(icon_name, 15.0)))
        .child(label.to_string())
}

/// `.sort-control`: the sort icon beside a `Select`.
fn sort_control(
    state: &Entity<AppState>,
    id: &'static str,
    value: &'static str,
    options: &[(&'static str, &'static str)],
    apply: impl Fn(&mut AppState, &'static str, &mut gpui::Context<AppState>) + 'static,
    cx: &App,
) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let target = state.clone();
    let apply = Rc::new(apply);
    let on_select = Rc::new(move |value: &'static str, _: &mut Window, cx: &mut App| {
        let apply = apply.clone();
        target.update(cx, |s, cx| {
            apply(s, value, cx);
            cx.notify();
        });
    });
    div()
        .flex()
        .items_center()
        .gap(sz(5.0))
        .text_color(tokens.secondary)
        .text_size(sz(11.0))
        .child(icon("arrow-down-wide-narrow", 16.0))
        .child(select(state, id, value, options, on_select, cx))
        .into_any_element()
}

/// `.favorites-tabs` (also used by Recently Played).
fn tabs(state: &Entity<AppState>, id: &str, items: &[(&'static str, &'static str)], selected: &str, set: fn(&mut AppState, &'static str), cx: &App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    div()
        .flex()
        .child(
            div()
                .flex()
                .gap(sz(2.0))
                .mb(sz(18.0))
                .p(sz(3.0))
                .rounded(sz(10.0))
                .bg(tokens.row)
                .children(items.iter().map(|&(value, label)| {
                    let active = value == selected;
                    let target = state.clone();
                    div()
                        .id(SharedString::from(format!("{id}-{value}")))
                        .px(sz(14.0))
                        .py(sz(6.0))
                        .rounded(sz(8.0))
                        .text_size(sz(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                        .when(active, |d| d.bg(tokens.hover).text_color(tokens.text))
                        .when(!active, |d| d.text_color(tokens.secondary).hover(|s| s.text_color(tokens.text)))
                        .on_click(move |_, _, cx| {
                            target.update(cx, |s, cx| {
                                set(s, value);
                                cx.notify();
                            })
                        })
                        .child(label)
                })),
        )
        .into_any_element()
}

/// `artistGrid`: `repeat(auto-fill, minmax(150px, 1fr))`, gap 30/28;
/// two columns (gap 26) at ≤640px.
pub fn artist_grid(state: &Entity<AppState>, artists: &[Artist], content_width: f32, viewport: f32, cx: &mut App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let narrow = viewport <= 640.0;
    let (columns, gap_x, gap_y) = if narrow {
        (2, 26.0, 26.0)
    } else {
        (crate::components::album_grid::auto_fill_columns(content_width, 150.0, 28.0), 28.0, 30.0)
    };
    let card = |i: usize, cx: &mut App| -> AnyElement {
            let artist = &artists[i];
            let cover = state.update(cx, |s, cx| s.cover(artist.cover_art.as_deref(), 300, cx));
            let target = state.clone();
            let route = Route { page: Page::Artist, id: Some(artist.id.clone()), title: Some(artist.name.clone()) };
            div()
                .id(("artist-card", i))
                .min_w_0()
                .flex()
                .flex_col()
                .items_center()
                .text_center()
                .cursor_pointer()
                .on_click(move |_, _, cx| target.update(cx, |s, cx| s.navigate(route.clone(), cx)))
                .child(
                    div()
                        .w_full()
                        .mb(sz(11.0))
                        .rounded_full()
                        .overflow_hidden()
                        .bg(tokens.surface)
                        .relative()
                        .child(div().w_full().pb(gpui::relative(1.0)).when_some(cover, |d, image| {
                            d.child(img(image).absolute().top_0().left_0().size_full().rounded_full())
                        })),
                )
                .child(line(artist.name.clone()).max_w_full().text_size(sz(13.0)).font_weight(FontWeight::MEDIUM))
                .child(
                    div()
                        .mt(sz(4.0))
                        .text_size(sz(11.0))
                        .text_color(tokens.muted)
                        .child(format!("{} albums", artist.album_count.unwrap_or(0.0) as i64)),
                )
                .into_any_element()
    };
    // One grid per row so the page renders only rows near the viewport.
    let rows = artists.len().div_ceil(columns.max(1));
    let tile = (content_width - gap_x * (columns as f32 - 1.0)) / columns as f32;
    crate::components::virtual_rows::virtual_rows("artist-grid", rows, tile + 40.0 + gap_y, gap_y, cx, |range, cx| {
        range
            .map(|row| {
                let cards: Vec<AnyElement> = (row * columns..((row + 1) * columns).min(artists.len())).map(|i| card(i, cx)).collect();
                div().grid().grid_cols(columns as u16).gap_x(sz(gap_x)).children(cards).into_any_element()
            })
            .collect()
    })
}

/// Playlists use album cards (`.album-grid` of `.album-card`).
fn playlist_grid(state: &Entity<AppState>, content_width: f32, viewport: f32, filter: &str, cx: &mut App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let m = metrics(GridKind::Grid, content_width, viewport);
    let playlists: Vec<_> = state
        .read(cx)
        .page
        .playlists
        .iter()
        .filter(|p| p.name.to_lowercase().contains(filter))
        .cloned()
        .collect();
    let cards: Vec<AnyElement> = playlists
        .iter()
        .enumerate()
        .map(|(i, playlist)| {
            let cover = state.update(cx, |s, cx| s.cover(playlist.cover_art.as_deref(), 400, cx));
            let has_cover = playlist.cover_art.as_deref().is_some_and(|c| !c.is_empty());
            let target = state.clone();
            let route = Route { page: Page::Playlist, id: Some(playlist.id.clone()), title: Some(playlist.name.clone()) };
            div()
                .id(("playlist-card", i))
                .min_w_0()
                .cursor_pointer()
                .on_click(move |_, _, cx| target.update(cx, |s, cx| s.navigate(route.clone(), cx)))
                .child(
                    div()
                        .relative()
                        .w_full()
                        .rounded(sz(7.0))
                        .overflow_hidden()
                        .bg(tokens.surface)
                        .child(div().w_full().pb(gpui::relative(1.0)).when_some(cover, |d, image| {
                            d.child(img(image).absolute().top_0().left_0().size_full().rounded(sz(7.0)))
                        }))
                        .when(!has_cover, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(tokens.muted)
                                    .child(icon_with("list-music", 54.0, Some(1.0), false)),
                            )
                        }),
                )
                .child(
                    div()
                        .pt(sz(8.0))
                        .child(line(playlist.name.clone()).text_size(sz(12.0)).line_height(gpui::relative(1.4)))
                        .child(
                            line(format!("{} songs", playlist.song_count.unwrap_or(0.0) as i64))
                                .mt(sz(1.0))
                                .text_size(sz(11.0))
                                .line_height(gpui::relative(1.4))
                                .text_color(tokens.secondary),
                        ),
                )
                .into_any_element()
        })
        .collect();
    if cards.is_empty() {
        return empty_state("No playlists yet", "Your Navidrome playlists will appear here.", cx).into_any_element();
    }
    div()
        .grid()
        .grid_cols(m.columns as u16)
        .gap_x(sz(m.column_gap))
        .gap_y(sz(m.row_gap))
        .children(cards)
        .into_any_element()
}

fn album_matches(album: &AlbumRecord, filter: &str) -> bool {
    filter.is_empty()
        || format!("{} {}", album_name(album), album.artist.as_deref().unwrap_or("")).to_lowercase().contains(filter)
}

fn song_matches(song: &Song, filter: &str) -> bool {
    filter.is_empty()
        || format!("{} {} {}", song.title, song.artist.as_deref().unwrap_or(""), song.album.as_deref().unwrap_or(""))
            .to_lowercase()
            .contains(filter)
}

fn sorted_songs(songs: Vec<Song>, sort: &str) -> Vec<Song> {
    if sort == "default" {
        return songs;
    }
    let mut songs = songs;
    fn field<'a>(s: &'a Song, sort: &str) -> &'a str {
        match sort {
            "title" => s.title.as_str(),
            "artist" => s.artist.as_deref().unwrap_or_default(),
            _ => s.album.as_deref().unwrap_or_default(),
        }
    }
    songs.sort_by(|a, b| locale_compare(field(a, sort), field(b, sort)));
    songs
}

fn sorted_artists(artists: Vec<Artist>, sort: &str) -> Vec<Artist> {
    if sort != "albums" {
        return artists;
    }
    let mut artists = artists;
    artists.sort_by(|a, b| {
        b.album_count.unwrap_or(0.0).partial_cmp(&a.album_count.unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal)
    });
    artists
}

pub fn render(state: &Entity<AppState>, content_width: f32, window: &mut Window, cx: &mut App) -> AnyElement {
    let viewport = theme::viewport_css_width(window);
    let (route, page, loading, error, filter, album_sort, song_sort, artist_sort, played_tab, favorites_tab) = {
        let s = state.read(cx);
        (
            s.route().clone(),
            s.page.clone(),
            s.page_loading,
            s.page_error.clone(),
            s.filter.to_lowercase(),
            s.album_sort,
            s.song_sort,
            s.artist_sort,
            s.played_tab,
            s.favorites_tab,
        )
    };
    let title = route.page.title();

    // Heading actions, as in App.tsx's `.page-heading-actions`.
    let songs_view = route.page == Page::Songs || (route.page == Page::Favorites && favorites_tab == "songs");
    let artists_view = route.page == Page::Artists || (route.page == Page::Favorites && favorites_tab == "artists");
    let mut actions: Vec<AnyElement> = Vec::new();
    if route.page == Page::Albums {
        actions.push(sort_control(state, "album-sort", album_sort, &ALBUM_SORTS, |s, v, cx| s.set_album_sort(v, cx), cx));
    }
    if songs_view {
        actions.push(sort_control(state, "song-sort", song_sort, &SONG_SORTS, |s, v, _| s.song_sort = v, cx));
    }
    if artists_view {
        actions.push(sort_control(state, "artist-sort", artist_sort, &ARTIST_SORTS, |s, v, _| s.artist_sort = v, cx));
    }
    if route.page == Page::Playlists {
        let target = state.clone();
        actions.push(
            secondary_button("new-playlist", "New Playlist", "list-plus", cx)
                .on_click(move |_, _, cx| target.update(cx, |s, cx| s.create_playlist_with(None, cx)))
                .into_any_element(),
        );
    }
    let can_select = songs_view || (route.page == Page::Played && played_tab == "songs");
    if can_select {
        actions.push(crate::views::selection::select_button(state, "select-songs", false, cx));
    }

    let mut body = div()
        .flex()
        .flex_col()
        .child(page_heading_with(title, actions, cx))
        .children(crate::views::selection::bulk_bar(state, cx));
    if loading && page.albums.is_empty() && page.songs.is_empty() && page.artists.is_empty() && page.playlists.is_empty() {
        return body.into_any_element();
    }
    if let Some(error) = error {
        return body.child(empty_state("Unable to load music", &error, cx)).into_any_element();
    }

    // Borrow the page's lists unless a filter or sort needs a new order.
    let albums: Cow<[AlbumRecord]> = if filter.is_empty() {
        Cow::Borrowed(&page.albums)
    } else {
        Cow::Owned(page.albums.iter().filter(|a| album_matches(a, &filter)).cloned().collect())
    };
    let songs: Cow<[Song]> = if filter.is_empty() && song_sort == "default" {
        Cow::Borrowed(&page.songs)
    } else {
        Cow::Owned(sorted_songs(page.songs.iter().filter(|s| song_matches(s, &filter)).cloned().collect(), song_sort))
    };
    let artists: Cow<[Artist]> = if filter.is_empty() && artist_sort != "albums" {
        Cow::Borrowed(&page.artists)
    } else {
        Cow::Owned(sorted_artists(
            page.artists.iter().filter(|a| filter.is_empty() || a.name.to_lowercase().contains(&filter)).cloned().collect(),
            artist_sort,
        ))
    };
    let grid = |albums: &[AlbumRecord], cx: &mut App| album_grid(state, "page", albums, GridKind::Grid, content_width, viewport, cx);

    body = match route.page {
        Page::Albums | Page::Recent => body.child(if albums.is_empty() {
            empty_state(
                "No albums found",
                if filter.is_empty() { "Albums appear here after Navidrome scans your music." } else { "Try a different filter." },
                cx,
            )
            .into_any_element()
        } else {
            grid(&albums, cx)
        }),
        Page::Played => {
            let body = body.child(tabs(state, "played-tab", &[("albums", "Albums"), ("songs", "Songs")], played_tab, |s, v| s.played_tab = v, cx));
            if played_tab == "albums" {
                body.child(if albums.is_empty() {
                    empty_state("No recently played albums", "Play music in Navidrome to build your recent history.", cx).into_any_element()
                } else {
                    grid(&albums, cx)
                })
            } else if songs.is_empty() {
                let enabled = state.read(cx).storage.get(volta_logic::storage::LISTENING_HISTORY_ENABLED_KEY) != "false";
                body.child(empty_state(
                    "No recently played songs",
                    if enabled {
                        "Play a few songs and they will show up here, most recent first."
                    } else {
                        "Turn on Personalized recommendations in Settings to keep a local listening history."
                    },
                    cx,
                ))
            } else {
                body.child(track_table(state, "played-songs", &songs, TableKind::Full, viewport, cx))
            }
        }
        Page::Artists => body.child(if artists.is_empty() {
            empty_state(
                "No artists found",
                if filter.is_empty() { "Artists appear here after Navidrome scans your music." } else { "Try a different filter." },
                cx,
            )
            .into_any_element()
        } else {
            {
                // `visibleArtists.slice(0, renderLimit)`.
                let limit = state.read(cx).paging.render_limit.max(60);
                artist_grid(state, &artists[..artists.len().min(limit)], content_width, viewport, cx)
            }
        }),
        Page::Songs => body.child(if songs.is_empty() {
            empty_state("No songs found", "Try another filter or refresh your library.", cx).into_any_element()
        } else if state.read(cx).local_songs_sliced() {
            // `sortedSongs.slice(0, renderLimit)` for local libraries.
            let limit = state.read(cx).paging.render_limit.max(60);
            track_table(state, "songs", &songs[..songs.len().min(limit)], TableKind::Full, viewport, cx)
        } else {
            track_table(state, "songs", &songs, TableKind::Full, viewport, cx)
        }),
        Page::Favorites => {
            let body = body.child(tabs(
                state,
                "favorites-tab",
                &[("songs", "Songs"), ("albums", "Albums"), ("artists", "Artists")],
                favorites_tab,
                |s, v| s.favorites_tab = v,
                cx,
            ));
            match favorites_tab {
                "songs" => body.child(if songs.is_empty() {
                    empty_state("No favorite songs", "Favorite a song from its menu to find it here.", cx).into_any_element()
                } else {
                    {
                        let limit = state.read(cx).paging.render_limit.max(60);
                        track_table(state, "favorites", &songs[..songs.len().min(limit)], TableKind::Full, viewport, cx)
                    }
                }),
                "albums" => body.child(if albums.is_empty() {
                    empty_state("No favorite albums", "Open an album and choose Favorite to find it here.", cx).into_any_element()
                } else {
                    grid(&albums, cx)
                }),
                _ => body.child(if artists.is_empty() {
                    empty_state("No favorite artists", "Open an artist and choose Favorite to find them here.", cx).into_any_element()
                } else {
                    {
                // `visibleArtists.slice(0, renderLimit)`.
                let limit = state.read(cx).paging.render_limit.max(60);
                artist_grid(state, &artists[..artists.len().min(limit)], content_width, viewport, cx)
            }
                }),
            }
        }
        Page::Playlists => body.child(playlist_grid(state, content_width, viewport, &filter, cx)),
        _ => body,
    };
    body.into_any_element()
}
