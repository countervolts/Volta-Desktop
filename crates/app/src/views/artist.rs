//! Artist detail: the `route.page === "artist"` branch of App.tsx.
//!
//! Full-bleed `.artist-header` (cover art, shade gradients, name, album count,
//! pin / play / favorite, share action), then Latest Release beside Top Songs,
//! Albums, and "You might also like" artists.

use gpui::{
    AnyElement, App, BoxShadow, Entity, FontWeight, IntoElement, ObjectFit, ParentElement, SharedString, Styled,
    StyledImage, Window, div, img, linear_color_stop, linear_gradient, point, prelude::*,
};
use volta_logic::model::{AlbumRecord, Song};

use crate::app_state::AppState;
use crate::components::album_grid::{GridKind, album_grid};
use crate::components::track_table::{TableKind, track_table};
use crate::components::ui::{icon, icon_with};
use crate::theme::{self, sz};
use crate::views::home::{empty_state, section_heading};
use crate::views::library::secondary_button;

/// `.artist-actions > button` (38px glass circle) or the 58px play button.
fn round_action(id: &str, size: f32, play: bool, child: impl IntoElement) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .size(sz(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border(sz(1.0))
        .border_color(theme::hex(if play { "#ffffff40" } else { "#ffffff24" }))
        // `backdrop-filter: blur(18px) saturate(130%)`.
        .backdrop_blur_saturate(sz(18.0), 1.3)
        .bg(theme::hex(if play { "#ffffffeb" } else { "#ffffff20" }))
        .text_color(theme::hex(if play { "#171719" } else { "#ffffff" }))
        .shadow(vec![BoxShadow {
            color: theme::hex("#0003"),
            offset: point(gpui::px(0.0), gpui::px(5.0)),
            blur_radius: gpui::px(18.0),
            spread_radius: gpui::px(0.0),
            inset: false,
        }])
        .cursor_pointer()
        .hover(move |s| s.bg(theme::hex(if play { "#ffffff" } else { "#ffffff32" })))
        .child(child)
}

fn gradient_band(top: f32, height: f32, from: &str, to: &str) -> impl IntoElement {
    div()
        .absolute()
        .left_0()
        .right_0()
        .top(gpui::relative(top))
        .h(gpui::relative(height))
        .bg(linear_gradient(180.0, linear_color_stop(theme::hex(from), 0.0), linear_color_stop(theme::hex(to), 1.0)))
}

pub fn render(state: &Entity<AppState>, content_width: f32, window: &mut Window, cx: &mut App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let viewport = theme::viewport_css_width(window);
    let (page, loading, error, filter) = {
        let s = state.read(cx);
        (s.page.clone(), s.page_loading, s.page_error.clone(), s.filter.to_lowercase())
    };
    if let Some(error) = error {
        return empty_state("Unable to load music", &error, cx).into_any_element();
    }
    let Some(artist) = page.artist.clone() else {
        return div().when(loading, |d| d).into_any_element();
    };
    let (_, pad_x, _) = theme::main_padding(viewport);
    let header_width = content_width + 2.0 * pad_x;
    // `min-height: clamp(370px, 48vw, 540px)`; ≤640px: 350px.
    let header_height = if viewport <= 640.0 { 350.0 } else { (viewport * 0.48).clamp(370.0, 540.0) };
    // The CSS asks for `object-position: center 28%`, but the `.artwork`
    // wrapper keeps its square aspect, so in practice the web draws the
    // square image full-width from the top edge. Match what renders.
    let image_side = header_width.max(header_height);
    let image_top = 0.0;
    let cover = state.update(cx, |s, cx| s.cover(artist.cover_art.as_deref(), 1200, cx));
    let h1 = if viewport <= 640.0 { 34.0 } else { (viewport * 0.041).clamp(34.0, 54.0) };

    let albums: Vec<AlbumRecord> = page
        .albums
        .iter()
        .filter(|a| {
            filter.is_empty()
                || format!("{} {}", volta_logic::format::album_name(a), a.artist.as_deref().unwrap_or(""))
                    .to_lowercase()
                    .contains(&filter)
        })
        .cloned()
        .collect();
    let songs: Vec<Song> = page.artist_songs.clone();
    let play_songs = songs.clone();
    let play_state = state.clone();

    let header = div()
        .relative()
        .flex()
        .flex_col()
        .items_center()
        .justify_end()
        .min_h(sz(header_height))
        .mx(sz(-pad_x))
        .mb(sz(38.0))
        .overflow_hidden()
        .text_color(gpui::white())
        .bg(theme::hex("#171719"))
        .when_some(cover, |d, image| {
            d.child(
                img(image)
                    .absolute()
                    .left_0()
                    .top(sz(image_top))
                    .w(sz(image_side))
                    .h(sz(image_side))
                    .object_fit(ObjectFit::Cover),
            )
        })
        // `.artist-header-shade`.
        .child(gradient_band(0.0, 0.28, "#00000008", "#00000008"))
        .child(gradient_band(0.28, 0.35, "#00000008", "#0000004d"))
        .child(gradient_band(0.63, 0.37, "#0000004d", "#000000c7"))
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(gpui::relative(0.22))
                .bg(linear_gradient(90.0, linear_color_stop(theme::hex("#00000024"), 0.0), linear_color_stop(theme::transparent(), 1.0))),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(gpui::relative(0.22))
                .bg(linear_gradient(270.0, linear_color_stop(theme::hex("#00000024"), 0.0), linear_color_stop(theme::transparent(), 1.0))),
        )
        .child(
            div()
                .absolute()
                .top(sz(14.0))
                .right(sz(17.0))
                .child({
                    let (target, share) = (state.clone(), artist.clone());
                    div()
                        .id("artist-share-wrap")
                        .on_click(move |_, _, cx| target.update(cx, |s, cx| s.share_artist(&share, cx)))
                        .child(round_action("artist-share", 38.0, false, icon("copy", 17.0)))
                }),
        )
        .child(
            div()
                .relative()
                .w_full()
                .max_w(sz(720.0))
                .pt(sz(34.0))
                .px(sz(28.0))
                .pb(sz(25.0))
                .flex()
                .flex_col()
                .items_center()
                .text_center()
                // `.artist-header-content { text-shadow: 0 1px 18px #0009 }`.
                .text_shadow(gpui::TextShadow {
                    offset_x: sz(0.0).into(),
                    offset_y: sz(1.0).into(),
                    blur: sz(18.0).into(),
                    color: theme::hex("#0009"),
                })
                .child(
                    div()
                        .text_size(sz(h1))
                        .font_weight(FontWeight::BOLD)
                        .letter_spacing(sz(-1.6))
                        .line_height(gpui::relative(1.02))
                        .child(artist.name.clone()),
                )
                .child(
                    div()
                        .mt(sz(9.0))
                        .text_size(sz(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::hex("#ffffffe0"))
                        .child(format!("{} albums in your library", page.albums.len())),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(sz(16.0))
                        .mt(sz(18.0))
                        .child({
                            use volta_logic::storage::{Pin, PinKind};
                            let pinned = state.read(cx).is_pinned(PinKind::Artist, &artist.id);
                            let pin = Pin {
                                kind: PinKind::Artist,
                                id: artist.id.clone(),
                                name: artist.name.clone(),
                                cover_art: artist.cover_art.clone(),
                                image_url: artist.artist_image_url.clone(),
                            };
                            let target = state.clone();
                            div()
                                .id("artist-pin-wrap")
                                .on_click(move |_, _, cx| {
                                    let pin = pin.clone();
                                    target.update(cx, |s, cx| s.toggle_pin(pin, cx))
                                })
                                .child(round_action("artist-pin", 38.0, false, icon_with("pin", 17.0, None, pinned)))
                        })
                        .child(
                            div()
                                .id("artist-play-wrap")
                                .on_click(move |_, _, cx| {
                                    if !play_songs.is_empty() {
                                        let player = play_state.read(cx).player.clone();
                                        let songs = play_songs.clone();
                                        player.update(cx, |p, cx| p.play_queue(songs, 0, cx));
                                    }
                                })
                                .child(round_action(
                                    "artist-play",
                                    58.0,
                                    true,
                                    div().ml(sz(3.0)).child(icon_with("play", 25.0, None, true)),
                                )),
                        )
                        .child({
                            let starred = artist.starred.is_some();
                            let favorite = state.read(cx).is_favorite_id(&artist.id, starred);
                            let (target, id) = (state.clone(), artist.id.clone());
                            div()
                                .id("artist-favorite-wrap")
                                .on_click(move |_, _, cx| {
                                    let id = id.clone();
                                    target.update(cx, |s, cx| s.toggle_favorite("artist", id, starred, cx))
                                })
                                .child(round_action("artist-favorite", 38.0, false, icon_with("star", 18.0, None, favorite)))
                        }),
                ),
        );

    // `.artist-feature-grid`: `minmax(190px, .72fr) minmax(0, 2fr)`, gap 36;
    // one column at ≤640px.
    let narrow = viewport <= 640.0;
    let latest = {
        let mut sorted = albums.clone();
        sorted.sort_by(|a, b| b.year.unwrap_or(0.0).partial_cmp(&a.year.unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
        sorted.into_iter().next()
    };
    let left_width = if narrow { content_width } else { ((content_width - 36.0) * 0.72 / 2.72).max(190.0) };
    let right_width = if narrow { content_width } else { content_width - 36.0 - left_width };

    let latest_section = latest.map(|album| {
        let year = album.year.filter(|y| *y != 0.0).map(|y| format!("{}", y as i64));
        // `.artist-latest-release .album-grid`: `minmax(0, 210px)`; 180px at ≤640px.
        let card_width = left_width.min(if narrow { 180.0 } else { 210.0 });
        div()
            .w(sz(left_width))
            .flex_none()
            .mb(sz(32.0))
            .child(section_heading(state, "Latest Release", year.as_deref(), None, cx))
            .child(div().w(sz(card_width)).child(album_grid(
                state,
                "latest",
                std::slice::from_ref(&album),
                GridKind::FeaturedGrid,
                card_width,
                viewport,
                cx,
            )))
    });
    let expanded = state.read(cx).expanded_artist_songs.as_deref() == Some(artist.id.as_str());
    let shown: Vec<Song> = if expanded { songs.clone() } else { songs.iter().take(8).cloned().collect() };
    let toggle_state = state.clone();
    let toggle_id = artist.id.clone();
    let popularity = songs.iter().any(|s| s.play_count.is_some_and(f64::is_finite));
    let top_songs = div()
        .min_w_0()
        .when(!narrow, |d| d.w(sz(right_width)))
        .mb(sz(32.0))
        .child(section_heading(
            state,
            "Top Songs",
            Some(&format!("{}{}", if popularity { "Most played first · " } else { "" }, songs.len())),
            None,
            cx,
        ))
        .child(if songs.is_empty() {
            empty_state("No artist songs found", "This artist has no matching songs in the library.", cx).into_any_element()
        } else {
            div()
                .mt(sz(-3.0))
                .child(track_table(state, "artist-songs", &shown, TableKind::Full, viewport, cx))
                .when(songs.len() > 8, |d| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sz(10.0))
                            .mt(sz(14.0))
                            .child(
                                secondary_button("artist-songs-more", if expanded { "Show less" } else { "Show more" }, "", cx)
                                    .min_h(sz(31.0))
                                    .px(sz(15.0))
                                    .py(sz(6.0))
                                    .text_size(sz(12.0))
                                    .on_click(move |_, _, cx| {
                                        let id = toggle_id.clone();
                                        toggle_state.update(cx, |s, cx| {
                                            s.expanded_artist_songs = if expanded { None } else { Some(id) };
                                            cx.notify();
                                        })
                                    }),
                            )
                            .when(!expanded, |d| {
                                d.child(div().text_size(sz(11.0)).text_color(tokens.muted).child("Showing the first 8 songs"))
                            }),
                    )
                })
                .into_any_element()
        });

    let feature = div()
        .flex()
        .when(narrow, |d| d.flex_col().gap(sz(4.0)))
        .when(!narrow, |d| d.items_start().gap(sz(36.0)))
        .children(latest_section)
        .child(top_songs);

    let mut body = div().flex().flex_col().child(header).child(feature).child(
        div()
            .mb(sz(32.0))
            .child(section_heading(state, "Albums", None, None, cx))
            .child(album_grid(state, "artist-albums", &albums, GridKind::Grid, content_width, viewport, cx)),
    );
    if !page.similar_artists.is_empty() {
        body = body.child(
            div()
                .mb(sz(32.0))
                .child(section_heading(state, "You might also like", Some("Artists"), None, cx))
                .child(crate::views::library::artist_grid(state, &page.similar_artists, content_width, viewport, cx)),
        );
    }
    body.into_any_element()
}
