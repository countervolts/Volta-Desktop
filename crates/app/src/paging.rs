//! `loadMore` + `InfiniteScrollSentinel`: further pages of albums (60),
//! songs (100) and search results; artists, favorite songs and local songs grow the
//! rendered slice (`renderLimit`) instead.

use gpui::{AnyElement, App, AppContext, Context, Entity, IntoElement, ParentElement, Styled, div};
use volta_logic::model::{AlbumRecord, Song};
use volta_services::SearchResults;

use crate::app_state::{AppState, Page};
use crate::components::motion::spinner;
use crate::components::notifications::NoticeKind;
use crate::theme::{self, sz};

#[derive(Default)]
pub struct Paging {
    pub has_more: bool,
    pub loading: bool,
    /// `renderLimit`: rows of artists / favorite songs shown so far.
    pub render_limit: usize,
}

enum More {
    Albums(Vec<AlbumRecord>),
    Songs(Vec<Song>),
    Search(SearchResults),
}

impl AppState {
    /// `pageHasMore` after a fresh load.
    pub fn reset_paging(&mut self) {
        let page = &self.page;
        self.paging.render_limit = 60;
        self.paging.loading = false;
        self.paging.has_more = match self.route().page {
            Page::Albums | Page::Recent | Page::Played => page.albums.len() == 60,
            Page::Songs => !self.is_local() && page.songs.len() == 100,
            Page::Search => page.artists.len() == 30 || page.albums.len() == 50 || page.songs.len() == 100,
            _ => false,
        };
    }

    /// `canLoadMore`: server pages, or client rows not yet rendered.
    pub fn can_load_more(&self) -> bool {
        let limit = self.paging.render_limit;
        self.paging.has_more
            || (self.route().page == Page::Artists && limit < self.page.artists.len())
            || (self.route().page == Page::Favorites && self.favorites_tab == "songs" && limit < self.page.songs.len())
            || (self.local_songs_sliced() && limit < self.page.songs.len())
    }

    /// Local mode loads the whole library at once, so the Songs page grows
    /// its rendered slice instead of fetching pages (`renderedSongs`).
    pub fn local_songs_sliced(&self) -> bool {
        self.route().page == Page::Songs && self.is_local()
    }

    /// `loadMore`.
    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.paging.loading || !self.can_load_more() {
            return;
        }
        let Some(client) = self.client() else { return };
        let page = self.route().page;
        match page {
            Page::Artists => {
                self.paging.render_limit += 60;
                cx.notify();
                return;
            }
            Page::Favorites | Page::Songs if page == Page::Favorites || self.is_local() => {
                self.paging.render_limit += 100;
                cx.notify();
                return;
            }
            _ => {}
        }
        if !self.paging.has_more {
            return;
        }
        self.paging.loading = true;
        cx.notify();
        let generation = self.page_generation;
        let (albums, songs, artists) = (self.page.albums.len(), self.page.songs.len(), self.page.artists.len());
        let query = self.search_debounced.clone();
        let sort = self.album_sort.to_string();
        let task = cx.background_spawn(async move {
            Ok::<_, volta_services::NavidromeError>(match page {
                Page::Search => More::Search(client.search_page(&query, artists, albums, songs)?),
                Page::Songs => More::Songs(client.songs(songs as u32, 100)?),
                _ => {
                    let kind = match page {
                        Page::Recent => "newest".to_string(),
                        Page::Played => "recent".to_string(),
                        _ => sort,
                    };
                    More::Albums(client.albums(&kind, 60, albums as u32)?)
                }
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| {
                if state.page_generation != generation {
                    return;
                }
                state.paging.loading = false;
                match result {
                    Ok(More::Albums(more)) => {
                        state.paging.has_more = more.len() == 60;
                        std::rc::Rc::make_mut(&mut state.page).albums.extend(more);
                    }
                    Ok(More::Songs(more)) => {
                        state.paging.has_more = more.len() == 100;
                        std::rc::Rc::make_mut(&mut state.page).songs.extend(more);
                    }
                    Ok(More::Search(more)) => {
                        state.paging.has_more = more.artist.len() == 30 || more.album.len() == 50 || more.song.len() == 100;
                        let page = std::rc::Rc::make_mut(&mut state.page);
                        page.artists.extend(more.artist);
                        page.albums.extend(more.album);
                        page.songs.extend(more.song);
                    }
                    Err(_) => state.notify("Could not load more. Try again.", NoticeKind::Info, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// `.infinite-scroll-sentinel`: a spinner row while the next page loads.
pub fn sentinel(state: &Entity<AppState>, cx: &App) -> Option<AnyElement> {
    let s = state.read(cx);
    if !s.can_load_more() {
        return None;
    }
    let tokens = theme::tokens(cx);
    Some(
        div()
            .flex()
            .items_center()
            .justify_center()
            .min_h(sz(52.0))
            .pt(sz(12.0))
            .pb(sz(24.0))
            .text_color(tokens.muted)
            .when(s.paging.loading, |d| d.child(spinner(22.0)))
            .into_any_element(),
    )
}

use gpui::prelude::FluentBuilder as _;
