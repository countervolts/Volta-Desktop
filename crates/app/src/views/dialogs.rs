//! Dialogs from app-dialogs.tsx on the shared `Modal` (components.tsx):
//! `.dialog-overlay` (#0009) and `.dialog` (470px, 22px padding, 12px radius,
//! sidebar fill, big shadow) with a heading row and a close button.

use gpui::{
    AnyElement, App, BoxShadow, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, deferred, div,
    img, point, prelude::*, px,
};
use volta_logic::format::{duration, is_lossless};
use volta_logic::model::Song;

use gpui_component::input::{Input, InputState};

use crate::app_state::{AppState, Dialog, Page, Route};
use crate::components::ui::{IconButton, icon};
use crate::theme::{self, sz};
use crate::views::library::secondary_button;

fn close(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |s, cx| {
        s.dialog = None;
        cx.notify();
    });
}

/// `Modal`: overlay + centred card. `width` is the CSS max width.
pub fn modal(state: &Entity<AppState>, title: &str, width: f32, body: AnyElement, cx: &App) -> AnyElement {
    modal_with(state, title, width, 22.0, body, None, cx)
}

/// `Modal` with a custom padding and an optional footer pinned below the
/// scrolling body (`position: sticky; bottom` in the web, e.g. `.engine-actions`).
pub fn modal_with(
    state: &Entity<AppState>,
    title: &str,
    width: f32,
    padding: f32,
    body: AnyElement,
    footer: Option<AnyElement>,
    cx: &App,
) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let (a, b) = (state.clone(), state.clone());
    let viewport = theme::viewport(cx);
    let content = div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(sz(20.0))
                .mb(sz(16.0))
                .child(div().text_size(sz(18.0)).font_weight(FontWeight::SEMIBOLD).letter_spacing(sz(-0.3)).child(title.to_string()))
                .child(IconButton::new("dialog-close", "Close", icon("x", 18.0)).on_click(move |_, _, cx| close(&b, cx))),
        )
        .child(body);
    let card = div()
        .id("dialog")
        .w(sz(width.min(viewport - 32.0)))
        // `max-height: calc(100dvh - 48px)`: the overlay pads 24px each side.
        .max_h_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .border(sz(1.0))
        .border_color(tokens.line)
        .rounded(sz(12.0))
        .bg(tokens.sidebar)
        .text_color(tokens.text)
        .shadow(vec![BoxShadow {
            color: theme::hex("#0008"),
            offset: point(px(0.0), px(22.0)),
            blur_radius: px(90.0),
            spread_radius: px(0.0),
            inset: false,
        }])
        // Clicks inside the card do not reach the overlay.
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(div().id("dialog-scroll").flex_1().min_h_0().overflow_y_scroll().p(sz(padding)).child(content))
        .children(footer);
    deferred(crate::components::motion::enter(
        div()
            .id("dialog-overlay")
            .absolute()
            .inset_0()
            .occlude()
            .bg(theme::hex("#0009"))
            .map(|d| crate::components::window_chrome::window_corners(d, cx))
            .flex()
            .items_center()
            .justify_center()
            .py(sz(24.0))
            .on_click(move |_, _, cx| close(&a, cx))
            .child(crate::components::motion::enter(card, "dialog-card-enter", 260, 14.0)),
        "dialog-overlay-enter",
        180,
        0.0,
    ))
    .with_priority(3)
    .into_any_element()
}

/// `audioCodecName`.
pub fn audio_codec_name(song: &Song) -> String {
    if let Some(codec) = song.codec.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        let lower = codec.to_lowercase();
        if lower.starts_with("mp4a") {
            return "AAC".into();
        }
        let known = match lower.as_str() {
            "aac" => Some("AAC"),
            "ac-3" => Some("Dolby Digital (AC-3)"),
            "alac" => Some("Apple Lossless (ALAC)"),
            "e-ac-3" => Some("Dolby Digital Plus (E-AC-3)"),
            "flac" => Some("FLAC"),
            "mp3" => Some("MP3"),
            "opus" => Some("Opus"),
            "pcm" => Some("PCM"),
            "vorbis" => Some("Vorbis"),
            _ => None,
        };
        return known.map(str::to_string).unwrap_or_else(|| codec.to_uppercase());
    }
    let suffix = song.suffix.as_deref().unwrap_or("").trim_start_matches('.').to_lowercase();
    let inferred = match suffix.as_str() {
        "aac" => Some("AAC"),
        "aif" | "aiff" | "wav" | "wave" => Some("PCM"),
        "alac" => Some("Apple Lossless (ALAC)"),
        "flac" => Some("FLAC"),
        "mp3" => Some("MP3"),
        "opus" => Some("Opus"),
        "wma" => Some("Windows Media Audio"),
        _ => None,
    };
    match inferred {
        Some(name) => name.to_string(),
        None if !suffix.is_empty() => format!("Unknown ({})", suffix.to_uppercase()),
        None => "Unknown".into(),
    }
}

fn song_details(state: &Entity<AppState>, inputs: &DialogInputs, song: &Song, cx: &mut App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let cover = state.update(cx, |s, cx| s.cover(song.cover_art.as_deref(), 420, cx));
    let cover_missing = song.cover_art.is_none();
    let row = |label: &str, value: String| {
        div()
            .flex()
            .justify_between()
            .gap(sz(16.0))
            .pb(sz(7.0))
            .border_b(sz(1.0))
            .border_color(tokens.line)
            .text_size(sz(11.0))
            .child(div().text_color(tokens.muted).child(label.to_string()))
            .child(div().text_color(tokens.text).text_right().child(value))
    };
    let mut rows = vec![
        row("Duration", duration(song.duration)),
        row("Audio codec", audio_codec_name(song)),
        row(
            "Format",
            if is_lossless(Some(song)) {
                "Lossless".into()
            } else {
                song.suffix.as_deref().map(str::to_uppercase).filter(|s| !s.is_empty()).unwrap_or_else(|| "Unknown".into())
            },
        ),
    ];
    if let Some(year) = song.year.filter(|y| *y != 0.0) {
        rows.push(row("Year", format!("{}", year as i64)));
    }
    if let Some(genre) = song.genre.clone().filter(|g| !g.is_empty()) {
        rows.push(row("Genre", genre));
    }
    if let Some(track) = song.track.filter(|t| *t != 0.0) {
        rows.push(row("Track", format!("{}", track as i64)));
    }
    let (local, editing) = (state.read(cx).is_local(), state.read(cx).local.editing);
    let mut actions = div().flex().flex_wrap().gap(sz(7.0));
    if local {
        let target = state.clone();
        actions = actions.child(secondary_button("edit-metadata", "Edit metadata", "", cx).on_click(move |_, _, cx| {
            target.update(cx, |s, cx| {
                s.local.editing = true;
                cx.notify();
            })
        }));
    }
    if let Some(album_id) = song.album_id.clone() {
        let target = state.clone();
        let title = song.album.clone();
        actions = actions.child(secondary_button("details-album", "Go to Album", "disc3", cx).on_click(move |_, _, cx| {
            let route = Route { page: Page::Album, id: Some(album_id.clone()), title: title.clone() };
            target.update(cx, |s, cx| {
                s.dialog = None;
                s.navigate(route, cx);
            });
        }));
    }
    if let Some(artist_id) = song.artist_id.clone() {
        let target = state.clone();
        let title = song.artist.clone();
        actions = actions.child(secondary_button("details-artist", "Go to Artist", "user-round", cx).on_click(move |_, _, cx| {
            let route = Route { page: Page::Artist, id: Some(artist_id.clone()), title: title.clone() };
            target.update(cx, |s, cx| {
                s.dialog = None;
                s.navigate(route, cx);
            });
        }));
    }
    let narrow = theme::viewport(cx) <= 640.0;
    div()
        .flex()
        .when(narrow, |d| d.flex_col())
        .items_start()
        .gap(sz(20.0))
        .child(
            div()
                .size(sz(155.0))
                .flex_none()
                .rounded(sz(7.0))
                .bg(tokens.surface)
                .when_some(cover, |d, image| d.child(img(image).size_full().rounded(sz(7.0))))
                .when(cover_missing, |d| {
                    d.flex().items_center().justify_center().text_color(tokens.muted).child(crate::components::ui::icon_with("music2", 155.0 * 0.38, Some(1.0), false))
                }),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .child(div().text_size(sz(18.0)).font_weight(FontWeight::SEMIBOLD).letter_spacing(sz(-0.3)).child(song.title.clone()))
                .when(local && editing, |d| d.child(metadata_editor(state, inputs, None, Some(song.clone()), cx)))
                .when(!(local && editing), |d| {
                    d.child(
                        div()
                            .mt(sz(7.0))
                            .text_size(sz(13.0))
                            .text_color(tokens.secondary)
                            .child(song.artist.clone().unwrap_or_else(|| "Unknown artist".into())),
                    )
                    .when_some(song.album.clone().filter(|a| !a.is_empty()), |d, album| {
                        d.child(div().mt(sz(4.0)).text_size(sz(11.0)).text_color(tokens.muted).child(album))
                    })
                    .child(div().flex().flex_col().gap(sz(8.0)).my(sz(20.0)).children(rows))
                    .child(credits_panel(state, song, cx))
                    .child(actions)
                }),
        )
        .into_any_element()
}

fn album_details(state: &Entity<AppState>, inputs: &DialogInputs, album: &volta_logic::model::AlbumRecord, cx: &mut App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let cover = state.update(cx, |s, cx| s.cover(album.cover_art.as_deref(), 420, cx));
    let cover_missing = album.cover_art.is_none();
    let row = |label: &str, value: String| {
        div()
            .flex()
            .justify_between()
            .gap(sz(16.0))
            .pb(sz(7.0))
            .border_b(sz(1.0))
            .border_color(tokens.line)
            .text_size(sz(11.0))
            .child(div().text_color(tokens.muted).child(label.to_string()))
            .child(div().text_color(tokens.text).text_right().child(value))
    };
    let mut rows = vec![
        row("Songs", album.song_count.map(|n| format!("{}", n as i64)).unwrap_or_else(|| "Unknown".into())),
        row("Duration", duration(album.duration)),
    ];
    if let Some(year) = album.year.filter(|y| *y != 0.0) {
        rows.push(row("Year", format!("{}", year as i64)));
    }
    if let Some(genre) = album.genre.clone().filter(|g| !g.is_empty()) {
        rows.push(row("Genre", genre));
    }
    let (local, editing) = (state.read(cx).is_local(), state.read(cx).local.editing);
    let mut actions = div().flex().flex_wrap().gap(sz(7.0));
    if local {
        let target = state.clone();
        actions = actions.child(secondary_button("edit-album-metadata", "Edit metadata", "", cx).on_click(move |_, _, cx| {
            target.update(cx, |s, cx| {
                s.local.editing = true;
                cx.notify();
            })
        }));
    }
    {
        let target = state.clone();
        let route = Route { page: Page::Album, id: Some(album.id.clone()), title: Some(volta_logic::format::album_name(album).to_string()) };
        actions = actions.child(secondary_button("album-details-open", "Open Album", "disc3", cx).on_click(move |_, _, cx| {
            let route = route.clone();
            target.update(cx, |s, cx| {
                s.dialog = None;
                s.navigate(route, cx);
            });
        }));
    }
    if let Some(artist_id) = album.artist_id.clone() {
        let target = state.clone();
        let title = album.artist.clone();
        actions = actions.child(secondary_button("album-details-artist", "Go to Artist", "user-round", cx).on_click(move |_, _, cx| {
            let route = Route { page: Page::Artist, id: Some(artist_id.clone()), title: title.clone() };
            target.update(cx, |s, cx| {
                s.dialog = None;
                s.navigate(route, cx);
            });
        }));
    }
    div()
        .flex()
        .items_start()
        .gap(sz(20.0))
        .child(
            div()
                .size(sz(155.0))
                .flex_none()
                .rounded(sz(7.0))
                .bg(tokens.surface)
                .when_some(cover, |d, image| d.child(img(image).size_full().rounded(sz(7.0))))
                .when(cover_missing, |d| {
                    d.flex().items_center().justify_center().text_color(tokens.muted).child(crate::components::ui::icon_with("music2", 155.0 * 0.38, Some(1.0), false))
                }),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .child(div().text_size(sz(18.0)).font_weight(FontWeight::SEMIBOLD).letter_spacing(sz(-0.3)).child(volta_logic::format::album_name(album).to_string()))
                .when(local && editing, |d| d.child(metadata_editor(state, inputs, Some(album.clone()), None, cx)))
                .when(!(local && editing), |d| {
                    d.child(
                        div()
                            .mt(sz(7.0))
                            .text_size(sz(13.0))
                            .text_color(tokens.secondary)
                            .child(album.artist.clone().unwrap_or_else(|| "Unknown artist".into())),
                    )
                    .child(div().flex().flex_col().gap(sz(8.0)).my(sz(20.0)).children(rows))
                    .child(actions)
                }),
        )
        .into_any_element()
}

/// `SongCreditsPanel` (`.song-credits`).
fn credits_panel(state: &Entity<AppState>, song: &Song, cx: &App) -> AnyElement {
    use volta_logic::song_credits::credits_from_metadata;
    let tokens = theme::tokens(cx).clone();
    let metadata = credits_from_metadata(song);
    let lookup = state.read(cx).credits.clone().filter(|c| c.song_id == song.id);
    let loading = lookup.as_ref().is_some_and(|c| c.loading);
    let found = lookup.as_ref().and_then(|c| c.credits.clone());
    let looked_up = lookup.as_ref().is_some_and(|c| c.looked_up);
    // A failed lookup falls back to the metadata credits (`|| metadataCredits`).
    let visible = found.clone().or_else(|| metadata.clone());
    let target = state.clone();
    let song_for_lookup = song.clone();
    let label = if loading {
        "Finding…"
    } else if visible.is_some() {
        "Find more credits"
    } else {
        "Find credits"
    };
    let small = |text: &str| {
        div().mt(sz(5.0)).text_size(sz(11.0)).line_height(gpui::relative(1.45)).text_color(tokens.muted).child(text.to_string())
    };
    div()
        .flex()
        .flex_col()
        .gap(sz(10.0))
        .my(sz(20.0))
        .pt(sz(16.0))
        .border_t(sz(1.0))
        .border_color(tokens.line)
        .child(
            div()
                .flex()
                .justify_between()
                .items_start()
                .gap(sz(16.0))
                .child(
                    div()
                        .min_w_0()
                        .child(div().text_size(sz(12.0)).font_weight(FontWeight::BOLD).text_color(tokens.text).child("Song credits"))
                        .child(small(if metadata.is_some() {
                            "From this song’s metadata. Find more roles from MusicBrainz when available."
                        } else {
                            "MusicBrainz is searched only when you ask. Volta sends this song’s title and artist, never your library address or account."
                        })),
                )
                .child(
                    secondary_button("find-credits", label, if loading { "loader-circle" } else { "sparkles" }, cx)
                        .flex_none()
                        .gap(sz(6.0))
                        .when(loading, |d| d.opacity(0.6))
                        .on_click(move |_, _, cx| {
                            let song = song_for_lookup.clone();
                            target.update(cx, |s, cx| s.lookup_credits(song, cx));
                        }),
                ),
        )
        .when_some(visible, |d, credits| {
            // `.media-details-copy dl` (margin 20 0, gap 8) outranks `.song-credits-list`.
            d.child(div().flex().flex_col().gap(sz(8.0)).my(sz(20.0)).children(credits.credits.into_iter().map(|credit| {
                div()
                    .flex()
                    .justify_between()
                    .gap(sz(16.0))
                    .pb(sz(7.0))
                    .border_b(sz(1.0))
                    .border_color(tokens.line)
                    .text_size(sz(11.0))
                    .child(div().text_color(tokens.muted).child(credit.role))
                    .child(div().text_color(tokens.text).text_right().child(credit.people.join(", ")))
            })))
        })
        .when(looked_up && found.is_none(), |d| d.child(small("No detailed credits were available for this recording yet.")))
        .into_any_element()
}

fn add_to_playlist(state: &Entity<AppState>, songs: &[Song], cx: &mut App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let (playlists, busy) = {
        let s = state.read(cx);
        (s.playlists.clone(), s.playlist_busy)
    };
    let intro = if songs.len() == 1 {
        format!("Add “{}” to:", songs[0].title)
    } else {
        format!("Add {} songs to:", songs.len())
    };
    let list: AnyElement = if playlists.is_empty() {
        div().child("You do not have any editable playlists yet.").into_any_element()
    } else {
        div()
            .id("playlist-picker-list")
            .flex()
            .flex_col()
            .gap(sz(8.0))
            .max_h(sz(260.0))
            .overflow_y_scroll()
            .children(playlists.iter().enumerate().map(|(i, playlist)| {
                let disabled = busy || playlist.readonly == Some(true);
                let target = state.clone();
                let chosen = playlist.clone();
                let songs = songs.to_vec();
                secondary_button(&format!("pick-playlist-{i}"), &playlist.name, "list-music", cx)
                    .w_full()
                    .justify_start()
                    .when(disabled, |d| d.opacity(0.5))
                    .on_click(move |_, _, cx| {
                        if !disabled {
                            let (playlist, songs) = (chosen.clone(), songs.clone());
                            target.update(cx, |s, cx| s.add_songs_to_playlist(playlist, songs, cx));
                        }
                    })
            }))
            .into_any_element()
    };
    let first = songs.first().cloned();
    let target = state.clone();
    div()
        .flex()
        .flex_col()
        .gap(sz(16.0))
        .text_color(tokens.secondary)
        .text_size(sz(13.0))
        .line_height(gpui::relative(1.45))
        .child(div().child(intro))
        .child(list)
        .child(
            div()
                .id("new-playlist-from-picker")
                .flex()
                .items_center()
                .justify_center()
                .gap(sz(8.0))
                .min_h(sz(34.0))
                .px(sz(20.0))
                .py(sz(7.0))
                .rounded(sz(7.0))
                .bg(theme::primary_fill())
                .hover(|s| s.bg(theme::primary_fill_hover()))
                .text_color(gpui::white())
                .text_size(sz(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    let song = first.clone();
                    target.update(cx, |s, cx| s.create_playlist_with(song, cx));
                })
                .child(icon("list-plus", 16.0))
                .child("New Playlist"),
        )
        .into_any_element()
}

/// Text fields owned by the shell (they need a `Window` to create).
pub struct DialogInputs {
    pub name: Entity<InputState>,
    pub comment: Entity<InputState>,
    pub shortcut_query: Entity<InputState>,
    /// Local metadata editor fields, in `METADATA_FIELDS` order.
    pub metadata: Vec<Entity<InputState>>,
}

/// Editor fields: title, artist, album artist, album, composer, genre,
/// track, disc, year.
pub const METADATA_FIELDS: usize = 9;

/// Values for the editor fields from a song.
pub fn song_metadata_values(song: &Song) -> [String; METADATA_FIELDS] {
    let number = |v: Option<f64>| v.filter(|n| *n != 0.0).map(|n| format!("{}", n as i64)).unwrap_or_default();
    [
        song.title.clone(),
        song.artist.clone().unwrap_or_default(),
        song.album_artist.clone().unwrap_or_default(),
        song.album.clone().unwrap_or_default(),
        song.composer.clone().unwrap_or_default(),
        song.genre.clone().unwrap_or_default(),
        number(song.track),
        number(song.disc_number),
        number(song.year),
    ]
}

/// Values for the album editor (album, album artist, year, genre in their slots).
pub fn album_metadata_values(album: &volta_logic::model::AlbumRecord) -> [String; METADATA_FIELDS] {
    let mut values: [String; METADATA_FIELDS] = Default::default();
    values[3] = volta_logic::format::album_name(album).to_string();
    values[2] = album.artist.clone().unwrap_or_default();
    values[8] = album.year.filter(|n| *n != 0.0).map(|n| format!("{}", n as i64)).unwrap_or_default();
    values[5] = album.genre.clone().unwrap_or_default();
    values
}

/// `SongMetadataEditor` / `AlbumMetadataEditor` (`.local-metadata-form`).
fn metadata_editor(state: &Entity<AppState>, inputs: &DialogInputs, album: Option<volta_logic::model::AlbumRecord>, song: Option<Song>, cx: &App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let fields: Vec<(&str, usize)> = if album.is_some() {
        vec![("Album", 3), ("Album artist", 2), ("Year", 8), ("Genre", 5)]
    } else {
        vec![
            ("Title", 0),
            ("Artist", 1),
            ("Album artist", 2),
            ("Album", 3),
            ("Composer", 4),
            ("Genre", 5),
            ("Track number", 6),
            ("Disc number", 7),
            ("Year", 8),
        ]
    };
    let field = |label: &str, input: &Entity<InputState>| {
        div()
            .flex()
            .flex_col()
            .gap(sz(5.0))
            .min_w_0()
            .text_color(tokens.secondary)
            .text_size(sz(10.0))
            .font_weight(FontWeight::SEMIBOLD)
            .child(label.to_string())
            .child(
                // `font: inherit` keeps the label's 600 weight in the input.
                div()
                    .h(sz(34.0))
                    .flex()
                    .items_center()
                    .px(sz(9.0))
                    .border(sz(1.0))
                    .border_color(tokens.line)
                    .rounded(sz(6.0))
                    .bg(tokens.canvas)
                    .text_size(sz(12.0))
                    .child(
                        Input::new(input)
                            .appearance(false)
                            .bordered(false)
                            .focus_bordered(false)
                            .p_0()
                            .h(sz(32.0))
                            .text_size(sz(12.0))
                            .text_color(tokens.text),
                    ),
            )
            .into_any_element()
    };
    let mut rows = div().flex().flex_col().gap(sz(10.0));
    for pair in fields.chunks(2) {
        let mut row = div().flex().gap(sz(10.0));
        for (label, index) in pair {
            row = row.child(div().flex_1().flex_basis(sz(0.0)).min_w_0().child(field(label, &inputs.metadata[*index])));
        }
        if pair.len() == 1 {
            row = row.child(div().flex_1().flex_basis(sz(0.0)));
        }
        rows = rows.child(row);
    }
    let cancel = state.clone();
    let save = state.clone();
    let metadata = inputs.metadata.clone();
    let is_album = album.is_some();
    div()
        .flex()
        .flex_col()
        .gap(sz(12.0))
        .mt(sz(14.0))
        .child(div().text_size(sz(10.5)).line_height(gpui::relative(1.45)).text_color(tokens.muted).child(if is_album {
            "Saved in Volta on this device. Your audio files stay untouched."
        } else {
            "Saved in Volta on this device. Your audio file stays untouched."
        }))
        .child(rows)
        .child(
            div()
                .flex()
                .justify_end()
                .gap(sz(7.0))
                .child(secondary_button("metadata-cancel", "Cancel", "", cx).on_click(move |_, _, cx| {
                    cancel.update(cx, |s, cx| {
                        s.local.editing = false;
                        cx.notify();
                    })
                }))
                .child(primary_button("metadata-save", "Save metadata", cx).on_click(move |_, _, cx| {
                    let values: Vec<String> = metadata.iter().map(|i| i.read(cx).value().to_string()).collect();
                    let number = |v: &str| v.trim().parse::<f64>().ok().filter(|n| n.is_finite());
                    use volta_logic::local_music::MetadataPatch;
                    if let Some(album) = album.clone() {
                        let patch = MetadataPatch {
                            album: Some(values[3].clone()),
                            album_artist: Some(values[2].clone()),
                            year: number(&values[8]),
                            genre: Some(values[5].clone()),
                            ..Default::default()
                        };
                        save.update(cx, |s, cx| s.save_local_album(&album, patch, cx));
                    } else if let Some(song) = song.clone() {
                        let patch = MetadataPatch {
                            title: Some(values[0].clone()),
                            artist: Some(values[1].clone()),
                            album_artist: Some(values[2].clone()),
                            album: Some(values[3].clone()),
                            composer: Some(values[4].clone()),
                            genre: Some(values[5].clone()),
                            track: number(&values[6]),
                            disc_number: number(&values[7]),
                            year: number(&values[8]),
                        };
                        save.update(cx, |s, cx| s.save_local_song(&song, patch, cx));
                    }
                })),
        )
        .into_any_element()
}

/// `.primary-button`.
fn primary_button(id: &str, label: &str, cx: &App) -> gpui::Stateful<gpui::Div> {
    let _ = cx;
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
        .bg(theme::primary_fill())
        .hover(|s| s.bg(theme::primary_fill_hover()))
        .text_color(gpui::white())
        .text_size(sz(13.0))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .child(label.to_string())
}

/// `.playlist-form` text field (label above a 38px input).
fn field(label: &str, input: &Entity<InputState>, multiline: bool, cx: &App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    div()
        .flex()
        .flex_col()
        .gap(sz(7.0))
        .text_color(tokens.text)
        .text_size(sz(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label.to_string())
        .child(
            div()
                .min_h(sz(if multiline { 82.0 } else { 38.0 }))
                .px(sz(10.0))
                .py(sz(8.0))
                .border(sz(1.0))
                .border_color(tokens.line)
                .rounded(sz(7.0))
                .bg(tokens.canvas)
                .font_weight(FontWeight::NORMAL)
                .text_size(sz(13.0))
                .child(Input::new(input).appearance(false).bordered(false).focus_bordered(false).p_0().text_color(tokens.text)),
        )
        .into_any_element()
}

fn playlist_form(
    state: &Entity<AppState>,
    inputs: &DialogInputs,
    editing: bool,
    song: Option<&Song>,
    cx: &App,
) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let busy = state.read(cx).playlist_busy;
    let (cancel, save) = (state.clone(), state.clone());
    let (name, comment) = (inputs.name.clone(), inputs.comment.clone());
    div()
        .flex()
        .flex_col()
        .gap(sz(16.0))
        .text_color(tokens.secondary)
        .text_size(sz(13.0))
        .line_height(gpui::relative(1.45))
        .child(field("Playlist name", &inputs.name, false, cx))
        .when(editing, |d| d.child(field("Description", &inputs.comment, true, cx)))
        .when_some(song, |d, song| d.child(div().child(format!("Adds “{}” to this playlist.", song.title))))
        .child(
            div()
                .flex()
                .justify_end()
                .gap(sz(9.0))
                .child(secondary_button("playlist-cancel", "Cancel", "", cx).on_click(move |_, _, cx| close(&cancel, cx)))
                .child(
                    primary_button(
                        "playlist-save",
                        if busy {
                            "Saving…"
                        } else if editing {
                            "Save Changes"
                        } else {
                            "Create Playlist"
                        },
                        cx,
                    )
                    .on_click(move |_, _, cx| {
                        let name = name.read(cx).value().to_string();
                        let comment = comment.read(cx).value().to_string();
                        save.update(cx, |s, cx| s.save_playlist(name, comment, cx));
                    }),
                ),
        )
        .into_any_element()
}

fn delete_playlist(state: &Entity<AppState>, playlist: &volta_logic::model::Playlist, cx: &App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let busy = state.read(cx).playlist_busy;
    let (cancel, confirm) = (state.clone(), state.clone());
    let target = playlist.clone();
    div()
        .flex()
        .flex_col()
        .gap(sz(16.0))
        .text_color(tokens.secondary)
        .text_size(sz(13.0))
        .line_height(gpui::relative(1.45))
        .child(div().child(format!(
            "Delete “{}”? This only removes the playlist, not the music in it.",
            playlist.name
        )))
        .child(
            div()
                .flex()
                .justify_end()
                .gap(sz(9.0))
                .child(secondary_button("delete-cancel", "Cancel", "", cx).on_click(move |_, _, cx| close(&cancel, cx)))
                .child(
                    primary_button("delete-confirm", if busy { "Deleting…" } else { "Delete Playlist" }, cx)
                        .on_click(move |_, _, cx| {
                            let playlist = target.clone();
                            confirm.update(cx, |s, cx| s.delete_playlist(playlist, cx));
                        }),
                ),
        )
        .into_any_element()
}

/// The keyboard shortcuts reference and editor (`.shortcut-manager`).
fn shortcuts(state: &Entity<AppState>, inputs: &DialogInputs, cx: &App) -> AnyElement {
    use volta_logic::shortcuts::{definitions, format_shortcut};
    let tokens = theme::tokens(cx).clone();
    let mod_label = theme::mod_label();
    let (bindings, recording) = {
        let s = state.read(cx);
        (s.shortcuts.clone(), s.recording_shortcut)
    };
    let query = inputs.shortcut_query.read(cx).value().trim().to_lowercase();
    let binding_of = |id: &str| bindings.iter().find(|(i, _)| *i == id).map(|(_, b)| b.clone());
    let filtered: Vec<_> = definitions()
        .into_iter()
        .filter(|d| {
            let label = binding_of(d.id).map(|b| format_shortcut(&b, mod_label)).unwrap_or_default();
            query.is_empty()
                || format!("{} {} {} {}", d.category, d.label, d.description, label).to_lowercase().contains(&query)
        })
        .collect();
    let total = definitions().len();
    let mut list = div().id("shortcut-list").max_h(sz(510.0)).overflow_y_scroll().pt(sz(1.0)).pr(sz(4.0)).pb(sz(2.0));
    if filtered.is_empty() {
        list = list.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(sz(8.0))
                .py(sz(30.0))
                .text_color(tokens.muted)
                .child(icon("search", 22.0))
                .child(div().text_size(sz(12.0)).child(format!("No shortcuts match “{query}”."))),
        );
    }
    let mut last_category = "";
    for (index, def) in filtered.iter().enumerate() {
        if def.category != last_category {
            list = list.child(
                div()
                    .mt(sz(if index == 0 { 7.0 } else { 17.0 }))
                    .mb(sz(7.0))
                    .mx(sz(3.0))
                    .text_size(sz(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(tokens.secondary)
                    .child(def.category),
            );
            last_category = def.category;
        } else {
            list = list.child(div().h(sz(7.0)));
        }
        let is_recording = recording == Some(def.id);
        let label = binding_of(def.id).map(|b| format_shortcut(&b, mod_label)).unwrap_or_default();
        let target = state.clone();
        let id = def.id;
        list = list.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(sz(15.0))
                .min_h(sz(52.0))
                .px(sz(11.0))
                .py(sz(9.0))
                .rounded(sz(7.0))
                .bg(tokens.canvas)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sz(4.0))
                        .min_w_0()
                        .child(div().text_size(sz(12.0)).font_weight(FontWeight::MEDIUM).child(def.label))
                        .child(div().text_size(sz(11.0)).text_color(tokens.muted).child(def.description)),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("binding-{id}")))
                        .flex_none()
                        .min_w(sz(126.0))
                        .min_h(sz(30.0))
                        .px(sz(9.0))
                        .py(sz(5.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .border(sz(1.0))
                        .rounded(sz(6.0))
                        .font_family("monospace")
                        .text_size(sz(11.0))
                        .cursor_pointer()
                        .map(|d| {
                            if is_recording {
                                d.border_color(tokens.accent).text_color(tokens.accent).bg(tokens.selection)
                            } else {
                                d.border_color(tokens.line).text_color(tokens.text).bg(tokens.surface)
                            }
                        })
                        .on_click(move |_, _, cx| {
                            target.update(cx, |s, cx| {
                                s.recording_shortcut = Some(id);
                                cx.notify();
                            })
                        })
                        .child(if is_recording { "Press keys…".to_string() } else { label }),
                ),
        );
    }
    let reset = state.clone();
    let clear = inputs.shortcut_query.clone();
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .gap(sz(9.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sz(7.0))
                        .flex_1()
                        .min_w_0()
                        .min_h(sz(34.0))
                        .px(sz(9.0))
                        .py(sz(6.0))
                        .border(sz(1.0))
                        .border_color(tokens.line)
                        .rounded(sz(7.0))
                        .bg(tokens.canvas)
                        .text_color(tokens.muted)
                        .child(icon("search", 15.0))
                        .child(
                            div().flex_1().text_size(sz(12.0)).text_color(tokens.text).child(
                                Input::new(&inputs.shortcut_query).appearance(false).bordered(false).focus_bordered(false).p_0(),
                            ),
                        )
                        .when(!query.is_empty(), |d| {
                            d.child(IconButton::new("clear-shortcut-search", "Clear shortcut search", icon("x", 14.0)).size(21.0).on_click(
                                move |_, window, cx| clear.update(cx, |input, cx| input.set_value("", window, cx)),
                            ))
                        }),
                )
                .child(
                    secondary_button("reset-shortcuts", "Reset defaults", "", cx)
                        .min_h(sz(34.0))
                        .px(sz(12.0))
                        .text_size(sz(11.0))
                        .on_click(move |_, _, cx| reset.update(cx, |s, cx| s.reset_shortcuts(cx))),
                ),
        )
        .child(
            div()
                .mt(sz(13.0))
                .mb(sz(7.0))
                .mx(sz(3.0))
                .text_size(sz(11.0))
                .text_color(tokens.muted)
                .child(format!(
                    "{} of {total} shortcuts{}",
                    filtered.len(),
                    if recording.is_some() { " · Press a key combination to assign it" } else { "" }
                )),
        )
        .child(list)
        .child(
            div().mt(sz(12.0)).text_size(sz(11.0)).line_height(gpui::relative(1.45)).text_color(tokens.muted).child(format!(
                "Select a binding, then press the new key combination. On this device, the modifier key is {mod_label}. Shortcuts pause while you type in a field."
            )),
        )
        .into_any_element()
}

pub fn render(state: &Entity<AppState>, inputs: &DialogInputs, cx: &mut App) -> Option<AnyElement> {
    let dialog = state.read(cx).dialog.clone()?;
    Some(match dialog {
        Dialog::PlaylistDraft { playlist, song } => {
            let body = playlist_form(state, inputs, playlist.is_some(), song.as_ref(), cx);
            modal(state, if playlist.is_some() { "Edit Playlist" } else { "New Playlist" }, 470.0, body, cx)
        }
        Dialog::ListeningHistory => {
            let body = crate::views::learning::listening_history(state, cx);
            modal(state, "Listening history", 700.0, body, cx)
        }
        Dialog::RecommendationEngine => {
            let (body, footer) = crate::views::learning::engine(state, cx);
            modal_with(state, "Recommendation engine", 940.0, 20.0, body, Some(footer), cx)
        }
        Dialog::Shortcuts => {
            let body = shortcuts(state, inputs, cx);
            modal(state, "Keyboard shortcuts", 700.0, body, cx)
        }
        Dialog::DeletePlaylist(playlist) => {
            let body = delete_playlist(state, &playlist, cx);
            modal(state, "Delete Playlist", 470.0, body, cx)
        }
        Dialog::SongDetails(song) => {
            let body = song_details(state, inputs, &song, cx);
            modal(state, "Song details", 620.0, body, cx)
        }
        Dialog::AlbumDetails(album) => {
            let body = album_details(state, inputs, &album, cx);
            modal(state, "Album details", 620.0, body, cx)
        }
        Dialog::AddToPlaylist(songs) => {
            let body = add_to_playlist(state, &songs, cx);
            modal(state, "Add to Playlist", 470.0, body, cx)
        }
    })
}
