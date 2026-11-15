//! Search (`route.page === "search"`) and genre detail (`"genre"`) from
//! App.tsx and search-genres.tsx.

use gpui::{
    AnyElement, App, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, Transformation, Window, div,
    img, linear_color_stop, linear_gradient, prelude::*, radians,
};
use volta_logic::model::{AlbumRecord, Song};

use crate::app_state::{AppState, Page, Route};
use crate::components::album_grid::{GridKind, album_grid, auto_fill_columns};
use crate::components::track_table::{TableKind, track_table};
use crate::components::ui::{icon, icon_with, line};
use crate::theme::{self, sz};
use crate::views::home::{empty_state, page_heading, page_heading_with, section_heading};
use crate::views::library::secondary_button;

const GENRE_COLORS: [&str; 8] = ["#a92343", "#285c94", "#9b5622", "#247469", "#786321", "#7e4168", "#416b37", "#ad3630"];

/// The tile colour: `reduce((n, c) => (n * 31 + c.charCodeAt(0)) >>> 0, 0) % 8`
/// over the code points of the genre name.
pub fn genre_color(genre: &str) -> &'static str {
    let hash = genre.chars().fold(0u32, |n, c| {
        let mut units = [0u16; 2];
        let first = c.encode_utf16(&mut units)[0];
        n.wrapping_mul(31).wrapping_add(first as u32)
    });
    GENRE_COLORS[(hash % GENRE_COLORS.len() as u32) as usize]
}

fn genre_tile(state: &Entity<AppState>, genre: &str, width: f32, viewport: f32, cx: &mut App) -> AnyElement {
    let height = width * 9.0 / 16.0;
    let color = theme::hex(genre_color(genre));
    let has_cover = state.update(cx, |s, cx| s.genre_cover(genre, cx)).is_some();
    let art = state.update(cx, |s, cx| s.genre_tile_art(genre, color, cx));
    let target = state.clone();
    let route = Route { page: Page::Genre, id: Some(genre.to_string()), title: Some(genre.to_string()) };
    let symbol = (width * 0.45).min(height * 0.75);
    let compact = viewport <= 860.0;
    div()
        .id(SharedString::from(format!("genre-{genre}")))
        .relative()
        .w(sz(width))
        .h(sz(height))
        .rounded(sz(8.0))
        .overflow_hidden()
        .bg(theme::hex(genre_color(genre)))
        .cursor_pointer()
        .on_click(move |_, _, cx| target.update(cx, |s, cx| s.navigate(route.clone(), cx)))
        .map(|d| match (art, has_cover) {
            // `mix-blend-mode: luminosity` at .75, pre-composited (genre_art).
            (Some(image), _) => d.child(img(image).absolute().inset_0().size_full().rounded(sz(8.0)).object_fit(gpui::ObjectFit::Cover)),
            (None, true) => d,
            (None, false) => d.child(
                // `.genre-symbol`: right 14%, top 10%, 45% × 75%, rotate(-12deg), opacity .2.
                div()
                    .absolute()
                    .right(gpui::relative(0.14))
                    .top(gpui::relative(0.10))
                    .w(gpui::relative(0.45))
                    .h(gpui::relative(0.75))
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(0.2)
                    .text_color(gpui::white())
                    .child(icon("music2", symbol).with_transformation(Transformation::rotate(radians(-12f32.to_radians())))),
            ),
        })
        // `::after`: linear-gradient(0deg, #0009, transparent 70%).
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(gpui::relative(0.7))
                .bg(linear_gradient(0.0, linear_color_stop(theme::hex("#0009"), 0.0), linear_color_stop(theme::transparent(), 1.0))),
        )
        .child(
            div()
                .absolute()
                .bottom(sz(if compact { 10.0 } else { 12.0 }))
                .left(sz(if compact { 10.0 } else { 12.0 }))
                .right(sz(12.0))
                .text_color(gpui::white())
                .text_size(sz(if compact { 12.0 } else { 14.0 }))
                .font_weight(FontWeight::BOLD)
                .line_height(gpui::relative(1.2))
                .child(genre.to_string()),
        )
        .into_any_element()
}

fn genre_browser(state: &Entity<AppState>, content_width: f32, viewport: f32, cx: &mut App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let (genres, loading, error) = {
        let s = state.read(cx);
        (s.genres.clone(), s.genres_loading, s.genres_error.clone())
    };
    // `.genre-grid`: 4 columns (5 at ≥1600, 3 at ≤1150, 2 at ≤860), gap 16 (12 at ≤860).
    let (columns, gap) = if viewport <= 860.0 {
        (2.0, 12.0)
    } else if viewport <= 1150.0 {
        (3.0, 16.0)
    } else if viewport >= 1600.0 {
        (5.0, 16.0)
    } else {
        (4.0, 16.0)
    };
    let width = (content_width - gap * (columns - 1.0)) / columns;
    let tiles: Vec<AnyElement> = genres.iter().map(|g| genre_tile(state, &g.value, width, viewport, cx)).collect();
    let message = |text: String| div().my(sz(20.0)).text_size(sz(13.0)).text_color(tokens.secondary).child(text);
    let retry = state.clone();
    div()
        .pb(sz(24.0))
        .child(
            div()
                .mb(sz(12.0))
                .text_size(sz(16.0))
                .font_weight(FontWeight(650.0))
                .child("Browse Genres"),
        )
        .child(div().flex().flex_wrap().gap(sz(gap)).children(tiles))
        .when(loading, |d| d.child(message("Loading…".into())))
        .when_some(error, |d, error| {
            d.child(
                message(format!("{error} ")).child(
                    div()
                        .id("genres-retry")
                        .text_color(tokens.accent)
                        .cursor_pointer()
                        .on_click(move |_, _, cx| retry.update(cx, |s, cx| s.retry_genres(cx)))
                        .child("Retry"),
                ),
            )
        })
        .when(!loading && genres.is_empty() && state.read(cx).genres_error.is_none(), |d| {
            d.child(message("No genres found. Add genre tags to your music to browse them here.".into()))
        })
        .into_any_element()
}

fn search_history(state: &Entity<AppState>, content_width: f32, viewport: f32, cx: &mut App) -> Option<AnyElement> {
    let tokens = theme::tokens(cx).clone();
    let history = state.read(cx).search_history.clone();
    if history.is_empty() {
        return None;
    }
    let columns = if viewport <= 860.0 { 2 } else { auto_fill_columns(content_width, 190.0, 12.0) };
    let clear = state.clone();
    let items = history.into_iter().enumerate().map(|(i, term)| {
        let pick = state.clone();
        let query = term.clone();
        div()
            .id(("search-history", i))
            .on_click(move |_, _, cx| pick.update(cx, |s, cx| s.set_search_query(query.clone(), cx)))
            .flex()
            .items_center()
            .gap(sz(6.0))
            .min_w_0()
            .min_h(sz(58.0))
            .p(sz(12.0))
            .border(sz(1.0))
            .border_color(tokens.line)
            .rounded(sz(6.0))
            .bg(tokens.surface)
            .text_color(tokens.secondary)
            .text_size(sz(11.0))
            .cursor_pointer()
            .hover(|s| s.text_color(tokens.text).border_color(tokens.accent).bg(tokens.hover))
            .child(div().flex_none().child(icon("search", 14.0)))
            .child(line(term))
    });
    Some(
        div()
            .w_full()
            .mb(sz(32.0))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .mb(sz(9.0))
                    .child(div().text_size(sz(14.0)).font_weight(FontWeight(650.0)).child("Recent searches"))
                    .child(
                        div()
                            .id("clear-search-history")
                            .text_size(sz(11.0))
                            .text_color(tokens.accent)
                            .cursor_pointer()
                            .on_click(move |_, _, cx| clear.update(cx, |s, cx| s.clear_search_history(cx)))
                            .child("Clear"),
                    ),
            )
            .child(div().grid().grid_cols(columns as u16).gap(sz(12.0)).children(items))
            .into_any_element(),
    )
}

fn search_tabs(state: &Entity<AppState>, counts: [(&'static str, &'static str, usize); 4], selected: &str, cx: &App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    div()
        .flex()
        .child(
            div()
                .flex()
                .gap(sz(2.0))
                .mt(sz(4.0))
                .mb(sz(18.0))
                .p(sz(3.0))
                .rounded(sz(10.0))
                .bg(tokens.row)
                .children(counts.into_iter().map(|(value, label, count)| {
                    let active = value == selected;
                    let target = state.clone();
                    div()
                        .id(SharedString::from(format!("search-tab-{value}")))
                        .flex()
                        .items_center()
                        .gap(sz(6.0))
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
                                s.search_tab = value;
                                cx.notify();
                            })
                        })
                        .child(label)
                        .child(
                            div()
                                .text_size(sz(11.0))
                                .text_color(tokens.muted)
                                .font_features(theme::fonts::tabular_nums())
                                .child(count.to_string()),
                        )
                })),
        )
        .into_any_element()
}

fn search_results(state: &Entity<AppState>, content_width: f32, viewport: f32, cx: &mut App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let (query, debounced, page, tab, loading, error) = {
        let s = state.read(cx);
        (s.search_query.trim().to_string(), s.search_debounced.clone(), s.page.clone(), s.search_tab, s.page_loading, s.page_error.clone())
    };
    let pending = query != debounced || loading;
    let title = format!("Search results for “{debounced}”");
    let heading = page_heading_with(
        &title,
        if page.songs.is_empty() { Vec::new() } else { vec![crate::views::selection::select_button(state, "search-select", false, cx)] },
        cx,
    );
    let mut body = div().flex().flex_col().child(heading).children(crate::views::selection::bulk_bar(state, cx));
    if let Some(error) = error {
        return body.child(empty_state("Unable to load music", &error, cx)).into_any_element();
    }
    if page.albums.is_empty() && page.songs.is_empty() && page.artists.is_empty() {
        return body
            .child(if pending {
                empty_state("Searching…", "Finding matches in your library.", cx)
            } else {
                empty_state("No results", "Try another artist, album, or song.", cx)
            })
            .into_any_element();
    }
    body = body
        .child(
            div()
                .mt(sz(-6.0))
                .mb(sz(28.0))
                .text_size(sz(12.0))
                .text_color(tokens.secondary)
                .child(if pending { "Searching your library…" } else { "Results from your Navidrome library" }),
        )
        .child(search_tabs(
            state,
            [
                ("all", "All", page.artists.len() + page.albums.len() + page.songs.len()),
                ("artists", "Artists", page.artists.len()),
                ("albums", "Albums", page.albums.len()),
                ("songs", "Songs", page.songs.len()),
            ],
            tab,
            cx,
        ));
    let empty_tab = |text: &str| div().py(sz(24.0)).text_color(tokens.secondary).child(text.to_string());
    match tab {
        "artists" if page.artists.is_empty() => body = body.child(empty_tab("No matching artists in your library.")),
        "albums" if page.albums.is_empty() => body = body.child(empty_tab("No matching albums in your library.")),
        "songs" if page.songs.is_empty() => body = body.child(empty_tab("No matching songs in your library.")),
        _ => {}
    }
    if matches!(tab, "all" | "artists") && !page.artists.is_empty() {
        let heading = section_heading(state, "Artists", None, None, cx).into_any_element();
        let artists: Vec<AnyElement> = page.artists.iter().enumerate().map(|(i, artist)| {
            let cover = state.update(cx, |s, cx| s.cover(artist.cover_art.as_deref(), 160, cx));
            let target = state.clone();
            let route = Route { page: Page::Artist, id: Some(artist.id.clone()), title: Some(artist.name.clone()) };
            div()
                .id(("search-artist", i))
                .flex()
                .items_center()
                .gap(sz(11.0))
                .min_w(sz(170.0))
                .text_size(sz(13.0))
                .cursor_pointer()
                .on_click(move |_, _, cx| target.update(cx, |s, cx| s.navigate(route.clone(), cx)))
                .child(
                    div()
                        .size(sz(56.0))
                        .flex_none()
                        .rounded_full()
                        .bg(tokens.surface)
                        .when_some(cover, |d, image| d.child(img(image).size_full().rounded_full())),
                )
                .child(
                    div()
                        .child(artist.name.clone())
                        .child(div().mt(sz(5.0)).text_size(sz(11.0)).text_color(tokens.secondary).child("Artist")),
                )
                .into_any_element()
        }).collect();
        body = body.child(
            div()
                .mb(sz(32.0))
                .child(heading)
                .child(div().flex().flex_wrap().gap(sz(25.0)).children(artists)),
        );
    }
    if matches!(tab, "all" | "albums") && !page.albums.is_empty() {
        body = body.child(
            div()
                .mb(sz(32.0))
                .child(section_heading(state, "Albums", None, None, cx))
                .child(album_grid(state, "search-albums", &page.albums, GridKind::Shelf, content_width, viewport, cx)),
        );
    }
    if matches!(tab, "all" | "songs") && !page.songs.is_empty() {
        body = body.child(
            div()
                .mb(sz(32.0))
                .child(section_heading(state, "Songs", None, None, cx))
                .child(track_table(state, "search-songs", &page.songs, TableKind::Full, viewport, cx)),
        );
    }
    body.into_any_element()
}

/// Groups genre songs into albums (`genreAlbums` in search-genres.tsx).
fn genre_albums(songs: &[Song], genre: &str) -> Vec<AlbumRecord> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, AlbumRecord> = std::collections::HashMap::new();
    for song in songs {
        let key = |v: Option<&str>| v.unwrap_or("").trim().to_lowercase();
        let id = song.album_id.clone().filter(|i| !i.is_empty()).unwrap_or_else(|| {
            format!("genre-album:{}-{}", key(song.artist.as_deref()), key(song.album.as_deref().or(Some(song.title.as_str()))))
        });
        let album = groups.entry(id.clone()).or_insert_with(|| {
            order.push(id.clone());
            AlbumRecord {
                id: id.clone(),
                name: Some(song.album.clone().unwrap_or_else(|| "Unknown album".into())),
                artist: Some(song.album_artist.clone().or(song.artist.clone()).unwrap_or_else(|| "Unknown artist".into())),
                artist_id: song.artist_id.clone(),
                cover_art: song.cover_art.clone(),
                year: song.year,
                genre: Some(genre.to_string()),
                song: Some(Vec::new()),
                ..Default::default()
            }
        });
        album.song.get_or_insert_with(Vec::new).push(song.clone());
        album.song_count = Some(album.song.as_ref().map_or(0, Vec::len) as f64);
        if album.cover_art.is_none() {
            album.cover_art = song.cover_art.clone();
        }
    }
    order.into_iter().filter_map(|id| groups.remove(&id)).collect()
}

fn genre_detail(state: &Entity<AppState>, genre: &str, content_width: f32, viewport: f32, cx: &mut App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let (songs, loading) = {
        let s = state.read(cx);
        (s.page.songs.clone(), s.page_loading)
    };
    let more = songs.len() == 100;
    let albums = genre_albums(&songs, genre);
    let back = state.clone();
    let (s1, s2, a, b) = (state.clone(), state.clone(), songs.clone(), songs.clone());
    let plus = if more { "+" } else { "" };
    div()
        .pb(sz(24.0))
        .child(
            div()
                .id("genre-back")
                .flex()
                .items_center()
                .gap(sz(6.0))
                .mb(sz(20.0))
                .text_color(tokens.accent)
                .cursor_pointer()
                .on_click(move |_, _, cx| back.update(cx, |s, cx| s.navigate(Route::page(Page::Search), cx)))
                .child(icon("chevron-left", 16.0))
                .child("Browse Genres"),
        )
        .child(
            div()
                .flex()
                .items_end()
                .justify_between()
                .gap(sz(20.0))
                .mt(sz(18.0))
                .mb(sz(30.0))
                .pb(sz(22.0))
                .border_b(sz(1.0))
                .border_color(tokens.line)
                .child(
                    div()
                        .child(div().text_size(sz(11.0)).text_color(tokens.secondary).mb(sz(3.0)).child("Genre"))
                        .child(
                            div()
                                .mt(sz(3.0))
                                .mb(sz(7.0))
                                .text_size(sz(if viewport <= 860.0 { 27.0 } else { 31.0 }))
                                .font_weight(FontWeight::BOLD)
                                .line_height(gpui::relative(1.1))
                                .child(genre.to_string()),
                        )
                        .child(
                            div()
                                .text_size(sz(12.0))
                                .text_color(tokens.secondary)
                                .child(format!("{}{plus} songs · {}{plus} albums", songs.len(), albums.len())),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(sz(8.0))
                        .child(
                            div()
                                .id("genre-play")
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap(sz(8.0))
                                .min_h(sz(34.0))
                                .px(sz(20.0))
                                .py(sz(7.0))
                                .rounded(sz(7.0))
                                .bg(theme::hex("#e73650"))
                                .hover(|s| s.bg(theme::hex("#fa4059")))
                                .text_color(gpui::white())
                                .text_size(sz(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .cursor_pointer()
                                .on_click(move |_, _, cx| {
                                    let player = s1.read(cx).player.clone();
                                    let songs = a.clone();
                                    player.update(cx, |p, cx| p.play_queue(songs, 0, cx));
                                })
                                .child(icon_with("play", 14.0, None, true))
                                .child("Play all"),
                        )
                        .child(secondary_button("genre-shuffle", "Shuffle all", "shuffle", cx).on_click(move |_, _, cx| {
                            use rand::seq::SliceRandom;
                            let mut songs = b.clone();
                            songs.shuffle(&mut rand::rng());
                            let player = s2.read(cx).player.clone();
                            player.update(cx, |p, cx| p.play_queue(songs, 0, cx));
                        })),
                ),
        )
        .child(
            div()
                .mb(sz(24.0))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .justify_between()
                        .mb(sz(14.0))
                        .child(div().text_size(sz(18.0)).font_weight(FontWeight(650.0)).child("Albums"))
                        .when(viewport > 860.0, |d| {
                            d.child(div().text_size(sz(12.0)).text_color(tokens.muted).child("From your collection"))
                        }),
                )
                .child(album_grid(state, "genre-albums", &albums, GridKind::Grid, content_width, viewport, cx)),
        )
        .when(loading, |d| d.child(div().my(sz(20.0)).text_size(sz(13.0)).text_color(tokens.secondary).child("Loading…")))
        .when(!loading && songs.is_empty(), |d| {
            d.child(div().my(sz(20.0)).text_size(sz(13.0)).text_color(tokens.secondary).child("No songs in this genre."))
        })
        .into_any_element()
}

pub fn render(state: &Entity<AppState>, content_width: f32, window: &mut Window, cx: &mut App) -> AnyElement {
    let viewport = theme::viewport_css_width(window);
    let route = state.read(cx).route().clone();
    if route.page == Page::Genre {
        let genre = route.id.clone().unwrap_or_default();
        return genre_detail(state, &genre, content_width, viewport, cx);
    }
    if state.read(cx).search_debounced.is_empty() {
        let history = search_history(state, content_width, viewport, cx);
        return div()
            .flex()
            .flex_col()
            .child(page_heading("Search", cx))
            .children(history)
            .child(genre_browser(state, content_width, viewport, cx))
            .into_any_element();
    }
    search_results(state, content_width, viewport, cx)
}

#[cfg(test)]
mod tests {
    #[test]
    fn genre_colors_follow_the_web_hash() {
        // ((74·31 + 97)·31 + 122)·31 + 122 = 2301655 → % 8 = 7 (the red Jazz tile).
        assert_eq!(super::genre_color("Jazz"), "#ad3630");
        assert_eq!(super::genre_color(""), "#a92343");
    }
}
