//! Song multi-select: the Select/Done toggle, `.bulk-bar`, and the selection
//! state (`selecting`, `selectedIndexes`, `toggleSongSelection`, `bulkRun`).

use std::collections::BTreeSet;

use gpui::{AnyElement, App, Context, Entity, FontWeight, IntoElement, ParentElement, Styled, div, prelude::*};
use volta_logic::model::Song;

use crate::app_state::{AppState, Page};
use crate::components::notifications::NoticeKind;
use crate::theme::{self, sz};
use crate::views::library::secondary_button;

#[derive(Default)]
pub struct Selection {
    pub selecting: bool,
    pub selected: BTreeSet<usize>,
    anchor: Option<usize>,
    /// The list the selection indexes refer to (`selectionSongs`).
    pub pool: Vec<Song>,
}

/// Tables whose rows can be selected (`canSelectSongs`).
pub fn selectable_table(id: &str) -> bool {
    matches!(id, "collection" | "songs" | "favorites" | "played-songs" | "search-songs")
}

impl AppState {
    pub fn selected_songs(&self) -> Vec<Song> {
        self.selection.selected.iter().filter_map(|i| self.selection.pool.get(*i).cloned()).collect()
    }

    /// Selection only makes sense for the list on screen.
    pub fn reset_selection(&mut self) {
        self.selection.selecting = false;
        self.selection.selected.clear();
        self.selection.anchor = None;
    }

    pub fn toggle_selecting(&mut self, cx: &mut Context<Self>) {
        self.selection.selecting = !self.selection.selecting;
        self.selection.selected.clear();
        self.selection.anchor = None;
        cx.notify();
    }

    /// `toggleSongSelection`: shift extends from the anchor.
    pub fn toggle_song_selection(&mut self, index: usize, shift: bool, cx: &mut Context<Self>) {
        let sel = &mut self.selection;
        match (shift, sel.anchor) {
            (true, Some(anchor)) => sel.selected.extend(anchor.min(index)..=anchor.max(index)),
            _ => {
                if !sel.selected.remove(&index) {
                    sel.selected.insert(index);
                }
                sel.anchor = Some(index);
            }
        }
        cx.notify();
    }

    pub fn toggle_select_all(&mut self, cx: &mut Context<Self>) {
        let len = self.selection.pool.len();
        self.selection.selected = if self.selection.selected.len() == len { BTreeSet::new() } else { (0..len).collect() };
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selection.selected.clear();
        self.selection.anchor = None;
        cx.notify();
    }

    /// `bulkRun`: act on the selected songs, then leave select mode.
    fn bulk_run(&mut self, action: impl FnOnce(&mut Self, Vec<Song>, &mut Context<Self>), cx: &mut Context<Self>) {
        let songs = self.selected_songs();
        if songs.is_empty() {
            return;
        }
        action(self, songs, cx);
        self.reset_selection();
        cx.notify();
    }

    fn bulk_queue(&mut self, songs: Vec<Song>, next: bool, cx: &mut Context<Self>) {
        use volta_logic::interactions::EngagementKind;
        let count = songs.len();
        let ordered: Vec<Song> = if next { songs.into_iter().rev().collect() } else { songs };
        for song in ordered {
            self.engage(if next { EngagementKind::PlayNext } else { EngagementKind::QueueAdd }, crate::recommend::entity_for_song(&song));
            self.player.update(cx, |p, cx| p.append(song, next, cx));
        }
        let plural = if count == 1 { "" } else { "s" };
        let message =
            if next { format!("{count} song{plural} will play next.") } else { format!("{count} song{plural} added to the queue.") };
        self.notify(message, NoticeKind::Success, cx);
    }

    /// `bulkFavorite`: favorite all, or unfavorite all when all are favorites.
    fn bulk_favorite(&mut self, songs: Vec<Song>, cx: &mut Context<Self>) {
        let all = songs.iter().all(|s| self.is_favorite_song(s));
        let mut updated = 0;
        for song in &songs {
            if self.is_favorite_song(song) == all {
                self.toggle_song_favorite(song, cx);
                updated += 1;
            }
        }
        if updated > 0 {
            let plural = if updated == 1 { "" } else { "s" };
            let message = if all { format!("Removed {updated} favorite{plural}.") } else { format!("Favorited {updated} song{plural}.") };
            self.notify(message, NoticeKind::Success, cx);
        }
    }

    /// `bulkRemoveFromPlaylist`: by original index, highest first.
    fn bulk_remove_from_playlist(&mut self, songs: Vec<Song>, cx: &mut Context<Self>) {
        let Some(playlist) = self.page.playlist.clone().filter(|p| self.can_edit_playlist(p)) else { return };
        let Some(client) = self.client() else { return };
        let indexes: Vec<usize> = self.selection.selected.iter().rev().copied().collect();
        let count = songs.len();
        let id = playlist.id.clone();
        let task = gpui::AppContext::background_spawn(cx, async move {
            client.update_playlist(&id, &volta_services::PlaylistChanges { song_indexes_to_remove: indexes, ..Default::default() })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| {
                match result {
                    Ok(()) => {
                        state.load_playlists(cx);
                        state.load_page(cx);
                        state.notify(format!("Removed {count} song{} from the playlist.", if count == 1 { "" } else { "s" }), NoticeKind::Info, cx);
                    }
                    Err(_) => state.notify("Could not update this playlist. Try again.", NoticeKind::Info, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// `.select-button`: "Select", or "Done" (accent) while selecting.
pub fn select_button(state: &Entity<AppState>, id: &str, disabled: bool, cx: &App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let selecting = state.read(cx).selection.selecting;
    let target = state.clone();
    secondary_button(id, if selecting { "Done" } else { "Select" }, "check-square", cx)
        .border(sz(1.0))
        .border_color(if selecting { tokens.accent } else { gpui::transparent_black() })
        .when(disabled, |d| d.opacity(0.38).cursor_default())
        .when(!disabled, |d| d.on_click(move |_, _, cx| target.update(cx, |s, cx| s.toggle_selecting(cx))))
        .into_any_element()
}

thread_local! {
    /// Window y of the in-flow bar's top, from the last paint.
    static BAR_TOP: std::cell::Cell<Option<f32>> = const { std::cell::Cell::new(None) };
}

/// `.bulk-bar` in the page flow; it records where it sits so the shell can
/// pin a copy to the top of the scroll area (`position: sticky; top: 0`).
pub fn bulk_bar(state: &Entity<AppState>, cx: &App) -> Option<AnyElement> {
    let bar = bar_element(state, cx)?;
    Some(
        div()
            .relative()
            .child(bar)
            .child(
                gpui::canvas(|bounds, _, _| BAR_TOP.with(|c| c.set(Some(f32::from(bounds.origin.y)))), |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            )
            .into_any_element(),
    )
}

/// The pinned copy once the in-flow bar has scrolled past `top` (window y of
/// the scroll content box). The strip above it is the `::before` cap.
pub fn pinned_bulk_bar(state: &Entity<AppState>, top: f32, cap: f32, cx: &App) -> Option<AnyElement> {
    if !state.read(cx).selection.selecting {
        BAR_TOP.with(|c| c.set(None));
        return None;
    }
    let stuck = BAR_TOP.with(|c| c.get()).is_some_and(|y| y < top - 0.5);
    if !stuck {
        return None;
    }
    let tokens = theme::tokens(cx).clone();
    Some(div().flex().flex_col().child(div().h(sz(cap)).bg(tokens.canvas)).children(bar_element(state, cx)).into_any_element())
}

fn bar_element(state: &Entity<AppState>, cx: &App) -> Option<AnyElement> {
    let s = state.read(cx);
    if !s.selection.selecting {
        return None;
    }
    let tokens = theme::tokens(cx).clone();
    let narrow = theme::viewport(cx) <= 720.0;
    let count = s.selection.selected.len();
    let can_remove = s.route().page == Page::Playlist && s.page.playlist.as_ref().is_some_and(|p| s.can_edit_playlist(p));
    let button = |id: &str, label: &str, icon_name: &str, enabled: bool, action: fn(&mut AppState, Vec<Song>, &mut Context<AppState>)| {
        let target = state.clone();
        secondary_button(id, label, icon_name, cx)
            .px(sz(10.0))
            .py(sz(6.0))
            .text_size(sz(12.0))
            .when(!enabled, |d| d.opacity(0.38).cursor_default())
            .when(enabled, move |d| d.on_click(move |_, _, cx| target.update(cx, |s, cx| s.bulk_run(action, cx))))
            .into_any_element()
    };
    let any = count > 0;
    let mut actions = vec![
        button("bulk-play", "Play", "play", any, |s, songs, cx| s.player.update(cx, |p, cx| p.play_queue(songs, 0, cx))),
        button("bulk-next", "Play Next", "list-plus", any, |s, songs, cx| s.bulk_queue(songs, true, cx)),
        button("bulk-queue", "Add to Queue", "list-end", any, |s, songs, cx| s.bulk_queue(songs, false, cx)),
        button("bulk-playlist", "Add to Playlist", "list-music", any, |s, songs, cx| s.open_add_to_playlist(songs, cx)),
        button("bulk-favorite", "Favorite", "star", any, |s, songs, cx| s.bulk_favorite(songs, cx)),
    ];
    if can_remove {
        actions.push(button("bulk-remove", "Remove", "x", any, |s, songs, cx| s.bulk_remove_from_playlist(songs, cx)));
    }
    let clear = state.clone();
    actions.push(
        secondary_button("bulk-clear", "Clear", "", cx)
            .px(sz(10.0))
            .py(sz(6.0))
            .text_size(sz(12.0))
            .when(!any, |d| d.opacity(0.38).cursor_default())
            .when(any, move |d| d.on_click(move |_, _, cx| clear.update(cx, |s, cx| s.clear_selection(cx))))
            .into_any_element(),
    );
    let label: AnyElement = if any {
        div()
            .flex()
            .gap(sz(4.0))
            .child(div().text_color(tokens.text).font_weight(FontWeight::BOLD).font_features(theme::fonts::tabular_nums()).child(count.to_string()))
            .child("selected")
            .into_any_element()
    } else {
        div().child("Select songs").into_any_element()
    };
    Some(
        div()
            .flex()
            .when(narrow, |d| d.flex_col().items_stretch())
            .when(!narrow, |d| d.items_center().justify_between().flex_wrap())
            .gap(sz(12.0))
            .mb(sz(14.0))
            .py(sz(10.0))
            .px(sz(14.0))
            .border(sz(1.0))
            .border_color(tokens.line)
            .rounded(sz(12.0))
            .backdrop_blur(sz(18.0))
            .bg(tokens.dock)
            .shadow(vec![gpui::BoxShadow {
                color: theme::hex("#00000055"),
                offset: gpui::point(gpui::px(0.0), gpui::px(12.0)),
                blur_radius: gpui::px(32.0),
                spread_radius: gpui::px(0.0),
                inset: false,
            }])
            .child(div().text_size(sz(13.0)).text_color(tokens.secondary).child(label))
            .child(div().flex().items_center().flex_wrap().gap(sz(8.0)).children(actions))
            .into_any_element(),
    )
}
