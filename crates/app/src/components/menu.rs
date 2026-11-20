//! Menus: the Radix `SongMenu` dropdown and `ContextMenu` from
//! components.tsx (`.context-menu`, `.custom-context-menu`). One menu is open
//! at a time; the shell draws it above everything and closes it on an
//! outside click or Escape.

use std::rc::Rc;

use gpui::{
    AnyElement, App, BoxShadow, Anchor, Entity, IntoElement, ParentElement, Pixels, Point, SharedString, Styled,
    Window, anchored, deferred, div, point, prelude::*, px,
};
use volta_logic::model::Song;

use crate::app_state::{AppState, Page, Route};
use crate::components::ui::icon;
use crate::theme::{self, sz};

pub type MenuAction = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
pub enum MenuItem {
    Item { icon: &'static str, label: SharedString, action: MenuAction },
    Separator,
}

impl MenuItem {
    pub fn new(icon: &'static str, label: impl Into<SharedString>, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        MenuItem::Item { icon, label: label.into(), action: Rc::new(action) }
    }
}

#[derive(Clone)]
pub struct OpenMenu {
    pub position: Point<Pixels>,
    /// Which corner of the menu sits at `position`.
    pub corner: Anchor,
    pub items: Vec<MenuItem>,
}

pub fn open(state: &Entity<AppState>, position: Point<Pixels>, corner: Anchor, items: Vec<MenuItem>, cx: &mut App) {
    state.update(cx, |s, cx| {
        s.menu = Some(OpenMenu { position, corner, items });
        cx.notify();
    });
}

fn close(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |s, cx| {
        s.menu = None;
        cx.notify();
    });
}

/// `SongMenu` items (Play Next, Play Last, View Details, —, Add to Playlist,
/// Favorite, Copy Share Link, Remove from Playlist, Go to Album).
pub fn song_items(state: &Entity<AppState>, song: &Song, cx: &App) -> Vec<MenuItem> {
    let favorite = state.read(cx).is_favorite_song(song);
    let mut items = Vec::new();
    let (s1, s2, s3) = (state.clone(), state.clone(), state.clone());
    let (a, b, c) = (song.clone(), song.clone(), song.clone());
    items.push(MenuItem::new("list-plus", "Play Next", move |_, cx| {
        let song = a.clone();
        s1.update(cx, |s, cx| s.queue_song(song, true, cx));
    }));
    items.push(MenuItem::new("list-end", "Play Last", move |_, cx| {
        let song = b.clone();
        s2.update(cx, |s, cx| s.queue_song(song, false, cx));
    }));
    {
        let state = state.clone();
        let song = song.clone();
        items.push(MenuItem::new("info", "View Details", move |_, cx| {
            let song = song.clone();
            state.update(cx, |s, cx| s.open_song_details(song, cx));
        }));
    }
    items.push(MenuItem::Separator);
    {
        let state = state.clone();
        let song = song.clone();
        items.push(MenuItem::new("list-music", "Add to Playlist", move |_, cx| {
            let song = song.clone();
            state.update(cx, |s, cx| s.open_add_to_playlist(vec![song], cx));
        }));
    }
    items.push(MenuItem::new("star", if favorite { "Remove Favorite" } else { "Favorite" }, move |_, cx| {
        let song = c.clone();
        s3.update(cx, |s, cx| s.toggle_song_favorite(&song, cx));
    }));
    {
        let state = state.clone();
        let song = song.clone();
        items.push(MenuItem::new("share2", "Copy Share Link", move |_, cx| {
            let song = song.clone();
            state.update(cx, |s, cx| s.share_song(&song, cx));
        }));
    }
    if let Some(album_id) = song.album_id.clone() {
        let state = state.clone();
        let title = song.album.clone();
        items.push(MenuItem::new("disc3", "Go to Album", move |_, cx| {
            let route = Route { page: Page::Album, id: Some(album_id.clone()), title: title.clone() };
            state.update(cx, |s, cx| s.navigate(route, cx));
        }));
    }
    items
}

/// The right-click song menu (`contextItems` for songs in App.tsx).
pub fn song_context_items(state: &Entity<AppState>, song: &Song, cx: &App) -> Vec<MenuItem> {
    let favorite = state.read(cx).is_favorite_song(song);
    let mk = |f: fn(&mut AppState, Song, &mut gpui::Context<AppState>)| {
        let (state, song) = (state.clone(), song.clone());
        move |_: &mut Window, cx: &mut App| {
            let song = song.clone();
            state.update(cx, |s, cx| f(s, song, cx));
        }
    };
    let mut items = vec![
        MenuItem::new("list-plus", "Play Next", mk(|s, song, cx| s.queue_song(song, true, cx))),
        MenuItem::new("list-end", "Play Last", mk(|s, song, cx| s.queue_song(song, false, cx))),
        MenuItem::Separator,
        MenuItem::new("list-music", "Add to Playlist", mk(|s, song, cx| s.open_add_to_playlist(vec![song], cx))),
        MenuItem::Separator,
        MenuItem::new("star", if favorite { "Remove Favorite" } else { "Favorite" }, mk(|s, song, cx| s.toggle_song_favorite(&song, cx))),
        MenuItem::new("copy", "Copy Share Link", mk(|s, song, cx| s.share_song(&song, cx))),
        MenuItem::new("thumbs-down", "Not Interested", mk(|s, song, cx| s.not_interested_song(&song, cx))),
    ];
    if song.artist_id.is_some() {
        items.push(MenuItem::new(
            "ban",
            "Mute This Artist",
            mk(|s, song, cx| s.set_artist_muted(song.artist_id.clone(), song.artist.clone(), true, cx)),
        ));
    }
    items.push(MenuItem::Separator);
    items.push(MenuItem::new("info", "View Details", mk(|s, song, cx| s.open_song_details(song, cx))));
    if let Some(album_id) = song.album_id.clone() {
        let (state, title) = (state.clone(), song.album.clone());
        items.push(MenuItem::new("disc3", "Go to Album", move |_, cx| {
            let route = Route { page: Page::Album, id: Some(album_id.clone()), title: title.clone() };
            state.update(cx, |s, cx| s.navigate(route, cx));
        }));
    }
    if let Some(artist_id) = song.artist_id.clone() {
        let (state, title) = (state.clone(), song.artist.clone());
        items.push(MenuItem::new("user-round", "Go to Artist", move |_, cx| {
            let route = Route { page: Page::Artist, id: Some(artist_id.clone()), title: title.clone() };
            state.update(cx, |s, cx| s.navigate(route, cx));
        }));
    }
    items
}

/// The right-click album menu.
pub fn album_context_items(state: &Entity<AppState>, album: &volta_logic::model::AlbumRecord, cx: &App) -> Vec<MenuItem> {
    use volta_logic::storage::{Pin, PinKind};
    let s = state.read(cx);
    let favorite = s.is_favorite_id(&album.id, album.starred.is_some());
    let pinned = s.is_pinned(PinKind::Album, &album.id);
    let mk = |f: fn(&mut AppState, volta_logic::model::AlbumRecord, &mut gpui::Context<AppState>)| {
        let (state, album) = (state.clone(), album.clone());
        move |_: &mut Window, cx: &mut App| {
            let album = album.clone();
            state.update(cx, |s, cx| f(s, album, cx));
        }
    };
    let mut items = vec![
        MenuItem::new("play", "Play Album", mk(|s, a, cx| s.play_album(a.id, cx))),
        MenuItem::new("shuffle", "Shuffle Album", mk(|s, a, cx| s.shuffle_album(a.id, cx))),
        MenuItem::Separator,
        MenuItem::new("list-plus", "Play Next", mk(|s, a, cx| s.queue_album(a, true, cx))),
        MenuItem::new("list-end", "Play Last", mk(|s, a, cx| s.queue_album(a, false, cx))),
        MenuItem::Separator,
        MenuItem::new(
            "star",
            if favorite { "Remove Favorite" } else { "Favorite" },
            mk(|s, a, cx| {
                let starred = a.starred.is_some();
                s.toggle_favorite("album", a.id, starred, cx)
            }),
        ),
        MenuItem::new(
            "pin",
            if pinned { "Unpin Album" } else { "Pin Album" },
            mk(|s, a, cx| {
                let pin = Pin {
                    kind: PinKind::Album,
                    name: volta_logic::format::album_name(&a).to_string(),
                    id: a.id,
                    cover_art: a.cover_art,
                    image_url: None,
                };
                s.toggle_pin(pin, cx)
            }),
        ),
        MenuItem::new("copy", "Copy Share Link", mk(|s, a, cx| s.share_album(&a, cx))),
        MenuItem::Separator,
        MenuItem::new("thumbs-down", "Not Interested", mk(|s, a, cx| s.not_interested_album(&a, cx))),
    ];
    if album.artist_id.is_some() {
        items.push(MenuItem::new(
            "ban",
            "Mute This Artist",
            mk(|s, a, cx| s.set_artist_muted(a.artist_id.clone(), a.artist.clone(), true, cx)),
        ));
    }
    items.extend([
        MenuItem::Separator,
        MenuItem::new(
            "info",
            "View Details",
            mk(|s, a, cx| {
                s.dialog = Some(crate::app_state::Dialog::AlbumDetails(a));
                cx.notify();
            }),
        ),
    ]);
    if let Some(artist_id) = album.artist_id.clone() {
        let (state, title) = (state.clone(), album.artist.clone());
        items.push(MenuItem::new("user-round", "Go to Artist", move |_, cx| {
            let route = Route { page: Page::Artist, id: Some(artist_id.clone()), title: title.clone() };
            state.update(cx, |s, cx| s.navigate(route, cx));
        }));
    }
    items
}

/// The open menu, positioned and clamped to the window.
pub fn render(state: &Entity<AppState>, cx: &mut App) -> Option<AnyElement> {
    let menu = state.read(cx).menu.clone()?;
    let tokens = theme::tokens(cx).clone();
    let close_state = state.clone();
    let rows = menu.items.into_iter().enumerate().map(|(i, item)| match item {
        MenuItem::Separator => div().h(sz(1.0)).m(sz(5.0)).bg(tokens.line).into_any_element(),
        MenuItem::Item { icon: name, label, action } => {
            let state = state.clone();
            div()
                .id(("menu-item", i))
                .flex()
                .items_center()
                .gap(sz(9.0))
                .min_h(sz(29.0))
                .px(sz(9.0))
                .py(sz(5.0))
                .rounded(sz(5.0))
                .cursor_pointer()
                .hover(|s| s.bg(tokens.accent).text_color(gpui::white()))
                .on_click(move |_, window, cx| {
                    close(&state, cx);
                    action(window, cx);
                })
                .child(div().flex_none().child(icon(name, 16.0)))
                .child(label)
                .into_any_element()
        }
    });
    Some(
        deferred(
            anchored().position(menu.position).anchor(menu.corner).snap_to_window_with_margin(px(8.0)).child(
                crate::components::motion::enter(
                div()
                    .id("context-menu")
                    .occlude()
                    .min_w(sz(187.0))
                    .p(sz(5.0))
                    .border(sz(1.0))
                    .border_color(tokens.line)
                    .rounded(sz(8.0))
                    .bg(tokens.sidebar)
                    .text_color(tokens.text)
                    .text_size(sz(12.0))
                    .shadow(vec![BoxShadow {
                        color: theme::hex("#0006"),
                        offset: point(px(0.0), px(8.0)),
                        blur_radius: px(26.0),
                        spread_radius: px(0.0),
                        inset: false,
                    }])
                    .on_mouse_down_out(move |_, _, cx| close(&close_state, cx))
                    .children(rows),
                "context-menu-enter",
                150,
                6.0,
                ),
            ),
        )
        .with_priority(2)
        .into_any_element(),
    )
}
