//! The root view: sign-in, or the library shell (sidebar, toolbar, page,
//! floating dock). Port of the `.desktop-app` / `.workspace` layout.

use gpui::{
    Context, Entity, FontWeight, IntoElement, ParentElement, Render, Styled, Window, div, prelude::*, relative,
};
use gpui_component::Sizable as _;
use gpui_component::input::{Input, InputEvent, InputState};

use crate::app_state::{AppState, Page};
use crate::components::window_chrome;
use crate::components::motion::spinner;
use crate::components::inspector;
use crate::components::playback_dock::PlaybackDock;
use crate::components::sidebar;
use crate::components::ui::{IconButton, icon};
use crate::theme::{self, sz};
use crate::views::connect::ConnectView;
use crate::views::{artist, collection, folders, home, library, search, settings, stats};

pub struct Shell {
    state: Entity<AppState>,
    connect: Option<Entity<ConnectView>>,
    dock: Entity<PlaybackDock>,
    filter: Entity<InputState>,
    filter_placeholder: String,
    search: Entity<InputState>,
    last_page: Option<Page>,
    lyrics: Entity<crate::components::lyrics_view::LyricsView>,
    full_player: Option<Entity<crate::views::full_player::FullPlayer>>,
    dialog_inputs: crate::views::dialogs::DialogInputs,
    last_dialog: Option<String>,
    focus: gpui::FocusHandle,
    folder_search: Entity<InputState>,
    main_scroll: gpui::ScrollHandle,
    /// The sidebar's own list has to scroll independently of the page.
    sidebar_scroll: gpui::ScrollHandle,
    /// `PerformanceOverlay`, created while the frame monitor is on.
    perf: Option<Entity<crate::components::perf_overlay::PerfOverlay>>,
    /// The message of a panic caught while building the page.
    crash: Option<String>,
}

impl Shell {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let dock = cx.new(|cx| PlaybackDock::new(state.clone(), cx));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        cx.subscribe(&filter, |this, input, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                let value = input.read(cx).value().to_string();
                this.state.update(cx, |state, cx| {
                    state.filter = value;
                    cx.notify();
                });
            }
        })
        .detach();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        cx.subscribe(&search, |this, input, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                let value = input.read(cx).value().to_string();
                this.state.update(cx, |state, cx| state.set_search_query(value, cx));
            }
            InputEvent::PressEnter { .. } | InputEvent::Blur => {
                this.state.update(cx, |state, cx| state.remember_search(cx));
            }
            _ => {}
        })
        .detach();
        let lyrics = cx.new(|cx| crate::components::lyrics_view::LyricsView::new(state.clone(), cx));
        let folder_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        cx.observe(&folder_search, |_, _, cx| cx.notify()).detach();
        let dialog_inputs = crate::views::dialogs::DialogInputs {
            name: cx.new(|cx| InputState::new(window, cx)),
            comment: cx.new(|cx| InputState::new(window, cx)),
            shortcut_query: cx.new(|cx| InputState::new(window, cx).placeholder("Search shortcuts")),
            metadata: (0..crate::views::dialogs::METADATA_FIELDS).map(|_| cx.new(|cx| InputState::new(window, cx))).collect(),
        };
        cx.observe(&dialog_inputs.shortcut_query, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            connect: None,
            dock,
            filter,
            filter_placeholder: "Filter".into(),
            search,
            last_page: None,
            lyrics,
            full_player: None,
            dialog_inputs,
            last_dialog: None,
            focus: cx.focus_handle(),
            folder_search,
            main_scroll: {
                let handle = gpui::ScrollHandle::new();
                cx.set_global(crate::components::virtual_rows::MainScroll(handle.clone()));
                handle
            },
            sidebar_scroll: gpui::ScrollHandle::new(),
            crash: None,
            perf: None,
        }
    }

    /// `ErrorBoundary` around the page content.
    fn guarded_page(&mut self, content_width: f32, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.crash.is_none() {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.page(content_width, window, cx)));
            match result {
                Ok(page) => return page,
                Err(payload) => {
                    let message = crate::views::crash::message(payload.as_ref());
                    log::error!("Volta crashed while rendering: {message}");
                    self.crash = Some(message);
                }
            }
        }
        let shell = cx.entity().downgrade();
        let retry = shell.clone();
        let state = self.state.clone();
        crate::views::crash::render(
            self.crash.as_deref().unwrap_or_default(),
            move |_, cx| {
                state.update(cx, |s, cx| s.load_page(cx));
                let _ = shell.update(cx, |s, cx| {
                    s.crash = None;
                    cx.notify();
                });
            },
            move |_, cx| {
                let _ = retry.update(cx, |s, cx| {
                    s.crash = None;
                    cx.notify();
                });
            },
            cx,
        )
    }

    /// Window width in CSS px (the rem size carries the interface scale).
    fn css_width(window: &Window) -> f32 {
        theme::viewport_css_width(window)
    }

    fn toolbar(&self, viewport: f32, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx).clone();
        let state = self.state.read(cx);
        let route = state.route().clone();
        let can_go_back = state.history.len() > 1;
        let title = if matches!(route.page, Page::Settings | Page::Stats) || route.page.is_detail() {
            route
                .title
                .clone()
                .or_else(|| state.page.album.as_ref().map(|a| volta_logic::format::album_name(a).to_string()))
                .or_else(|| state.page.playlist.as_ref().map(|p| p.name.clone()))
                .or_else(|| state.page.artist.as_ref().map(|a| a.name.clone()))
                .unwrap_or_else(|| route.page.title().to_string())
        } else {
            "Your Library".to_string()
        };
        let show_filter = !matches!(route.page, Page::Settings | Page::Folders | Page::Stats);
        let search_page = matches!(route.page, Page::Search | Page::Genre);
        let search_focused = gpui::Focusable::focus_handle(self.search.read(cx), cx).is_focused(window);
        let back_state = self.state.clone();
        let placeholder = if route.page.is_detail() {
            format!("Find in {}", if route.page == Page::Playlist { "Playlist" } else { "Album" })
        } else {
            "Filter".to_string()
        };
        let refresh_state = self.state.clone();
        let narrow = viewport <= 640.0;
        let toggle_state = self.state.clone();
        // Client-side window frame (Linux): the bar's empty space moves the
        // window and the window controls end it.
        let client_frame = window_chrome::client_side(window);
        let drag_region = window_chrome::custom_titlebar(window);
        div()
            .h(sz(53.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap(sz(15.0))
            .min_w_0()
            .px(sz(if narrow { 9.0 } else if viewport <= 860.0 { 15.0 } else { 26.0 }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sz(if narrow { 3.0 } else { 9.0 }))
                    .min_w_0()
                    // "Toggle navigation" only shows in the compact layout.
                    .when(narrow, |d| {
                        d.child(IconButton::new("toggle-navigation", "Toggle navigation", icon("panel-left", 18.0)).on_click(
                            move |_, _, cx| {
                                toggle_state.update(cx, |s, cx| {
                                    s.mobile_sidebar = !s.mobile_sidebar;
                                    cx.notify();
                                })
                            },
                        ))
                    })
                    .child(
                        IconButton::new("back", "Back", icon("chevron-left", 22.0))
                            .disabled(!can_go_back)
                            .on_click(move |_, _, cx| back_state.update(cx, |s, cx| s.back(cx))),
                    )
                    .when(!search_page, |d| d.child(
                        window_chrome::drag_area(div())
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(sz(if narrow { 11.0 } else { 12.0 }))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(tokens.secondary)
                            .child(title),
                    )),
            )
            .when(drag_region, |d| d.child(window_chrome::drag_area(div().id("toolbar-drag")).flex_1().h_full()))
            // `.search-toolbar`: a centered search field and the scope chip.
            .when(search_page, |bar| {
                let refresh = self.state.clone();
                bar.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(sz(if narrow { 3.0 } else { 9.0 }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(sz(8.0))
                                .w_full()
                                .max_w(sz(850.0))
                                .mx_auto()
                                // `padding: 7px 12px` around a 16px line, plus the border.
                                .h(sz(34.0))
                                .overflow_hidden()
                                .px(sz(12.0))
                                .border(sz(1.0))
                                .border_color(if search_focused { tokens.accent } else { tokens.line })
                                .rounded(sz(22.0))
                                .text_color(tokens.secondary)
                                .child(icon("search", 16.0))
                                .child(
                                    Input::new(&self.search)
                                        .xsmall()
                                        .appearance(false)
                                        .bordered(false)
                                        .focus_bordered(false)
                                        .flex_1()
                                        .p_0()
                                        .text_size(sz(16.0))
                                        .text_color(tokens.text),
                                ),
                        )
                        .when(viewport > 860.0, |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .px(sz(12.0))
                                    .py(sz(5.0))
                                    .rounded(sz(20.0))
                                    .bg(tokens.surface)
                                    .text_size(sz(11.0))
                                    .child("Your Library"),
                            )
                        })
                        .child(
                            IconButton::new("refresh-search", "Refresh view", icon("refresh-cw", 16.0))
                                .on_click(move |_, _, cx| refresh.update(cx, |s, cx| s.refresh(cx))),
                        ),
                )
            })
            .when(show_filter && !search_page, |bar| {
                let _ = &placeholder;
                bar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sz(if narrow { 3.0 } else { 9.0 }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(sz(6.0))
                                .w(sz(if narrow { 122.0 } else if viewport <= 860.0 { 128.0 } else { 175.0 }))
                                .h(sz(27.0))
                                .overflow_hidden()
                                .px(sz(8.0))
                                .py(sz(5.0))
                                .rounded(sz(20.0))
                                .bg(tokens.sidebar)
                                .text_color(tokens.muted)
                                .child(icon("search", 14.0))
                                .child(
                                    Input::new(&self.filter)
                                        .xsmall()
                                        .appearance(false)
                                        .bordered(false)
                                        .focus_bordered(false)
                                        .flex_1()
                                        .p_0()
                                        .h(sz(17.0))
                                        .text_size(sz(11.0))
                                        .text_color(tokens.text),
                                ),
                        )
                        .child(
                            IconButton::new("refresh", "Refresh view", icon("refresh-cw", 16.0))
                                .on_click(move |_, _, cx| refresh_state.update(cx, |s, cx| s.refresh(cx))),
                        ),
                )
            })
            .when(client_frame, |d| d.child(window_chrome::controls(window)))
    }

    fn page(&self, content_width: f32, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let route = self.state.read(cx).route().clone();
        match route.page {
            Page::Home => home::render(&self.state, content_width, window, cx).into_any_element(),
            Page::Artist => artist::render(&self.state, content_width, window, cx),
            Page::Search | Page::Genre => search::render(&self.state, content_width, window, cx),
            Page::Settings => settings::render(&self.state, content_width, window, cx),
            Page::Stats => stats::render(&self.state, content_width, window, cx),
            Page::Folders => folders::render(&self.state, &self.folder_search, content_width, window, cx),
            Page::Album | Page::Playlist => collection::render(&self.state, content_width, window, cx).into_any_element(),
            Page::Albums | Page::Recent | Page::Played | Page::Artists | Page::Songs | Page::Favorites | Page::Playlists => {
                library::render(&self.state, content_width, window, cx)
            }
        }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _build_time = crate::logs::RenderTimer::start();
        crate::logs::mark("first frame");
        window_chrome::record_corners(window, cx);
        #[cfg(feature = "shots")]
        if let Some(y) = std::env::var("VOLTA_SHOT_SCROLL").ok().and_then(|v| v.parse::<f32>().ok()) {
            if f32::from(self.main_scroll.max_offset().y) > 0.0 && self.main_scroll.offset().y == gpui::px(0.0) {
                self.main_scroll.set_offset(gpui::point(gpui::px(0.0), gpui::px(-y)));
            }
        }
        // `InfiniteScrollSentinel`: within 480px of the end (or a page shorter
        // than the viewport), ask for the next page.
        if self.state.read(cx).can_load_more() && !self.state.read(cx).paging.loading {
            let remaining = f32::from(self.main_scroll.max_offset().y) + f32::from(self.main_scroll.offset().y);
            if remaining < f32::from(window.rem_size()) * 30.0 {
                let state = self.state.clone();
                cx.defer(move |cx| state.update(cx, |s, cx| s.load_more(cx)));
            }
        }
        let tokens = theme::tokens(cx).clone();
        let (auth_ready, signed_in, connecting) = {
            let state = self.state.read(cx);
            (state.auth_ready, state.session.is_some(), state.connecting)
        };
        if !auth_ready {
            // `.session-loading-content`
            let loading = div()
                .size_full()
                .line_height(relative(theme::NORMAL_LINE_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .gap(sz(10.0))
                .bg(tokens.canvas)
                .map(|d| window_chrome::window_corners(d, cx))
                .text_size(sz(13.0))
                .text_color(tokens.secondary)
                .child(spinner(17.0))
                .child(if connecting { "Restoring your session…" } else { "Loading…" })
                .into_any_element();
            return window_chrome::frame(loading, true, window);
        }
        if !signed_in {
            let state = self.state.clone();
            let connect = self
                .connect
                .get_or_insert_with(|| cx.new(|cx| ConnectView::new(state, window, cx)))
                .clone();
            let content = div().size_full().line_height(relative(theme::NORMAL_LINE_HEIGHT)).child(connect).into_any_element();
            return window_chrome::frame(content, true, window);
        }
        self.connect = None;
        // The filter placeholder names the detail page being searched.
        let route = self.state.read(cx).route().clone();
        let placeholder = if route.page.is_detail() {
            format!("Find in {}", if route.page == Page::Playlist { "Playlist" } else { "Album" })
        } else {
            "Filter".to_string()
        };
        // Mirror the search text when it changes outside the field (history
        // items, restored queries), and focus the field when Search opens
        // (`autoFocus` on the web).
        let query = self.state.read(cx).search_query.clone();
        if self.search.read(cx).value().as_ref() != query.as_str() {
            self.search.update(cx, |input, cx| input.set_value(query, window, cx));
        }
        if route.page == Page::Search && self.last_page != Some(Page::Search) {
            let handle = gpui::Focusable::focus_handle(self.search.read(cx), cx);
            window.focus(&handle, cx);
        }
        self.last_page = Some(route.page);
        // Shortcut follow-ups: focus a field, or scroll the page to the top.
        if let Some(target) = self.state.update(cx, |s, _| s.focus_request.take()) {
            let input = match target {
                crate::app_state::FocusTarget::Search => &self.search,
                crate::app_state::FocusTarget::Filter => &self.filter,
            };
            let handle = gpui::Focusable::focus_handle(input.read(cx), cx);
            window.focus(&handle, cx);
        }
        if self.state.update(cx, |s, _| std::mem::take(&mut s.scroll_top_request)) {
            self.main_scroll.set_offset(gpui::point(gpui::px(0.0), gpui::px(0.0)));
        }
        if self.state.update(cx, |s, _| std::mem::take(&mut s.folder_clear_search)) {
            self.folder_search.update(cx, |input, cx| input.set_value("", window, cx));
        }
        // Keys go to the shell when no field has focus.
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
        // Fill the playlist form when a draft opens (`playlistDraft` values).
        let dialog_key = match &self.state.read(cx).dialog {
            Some(crate::app_state::Dialog::PlaylistDraft { playlist, .. }) => {
                Some(format!("draft:{}", playlist.as_ref().map_or("", |p| p.id.as_str())))
            }
            Some(crate::app_state::Dialog::SongDetails(song)) if self.state.read(cx).local.editing => Some(format!("edit-song:{}", song.id)),
            Some(crate::app_state::Dialog::AlbumDetails(album)) if self.state.read(cx).local.editing => Some(format!("edit-album:{}", album.id)),
            _ => None,
        };
        if dialog_key != self.last_dialog {
            let values = match self.state.read(cx).dialog.clone() {
                Some(crate::app_state::Dialog::SongDetails(song)) if dialog_key.is_some() => Some(crate::views::dialogs::song_metadata_values(&song)),
                Some(crate::app_state::Dialog::AlbumDetails(album)) if dialog_key.is_some() => Some(crate::views::dialogs::album_metadata_values(&album)),
                _ => None,
            };
            if let Some(values) = values {
                for (input, value) in self.dialog_inputs.metadata.iter().zip(values) {
                    input.update(cx, |input, cx| input.set_value(value, window, cx));
                }
            }
            if !matches!(self.state.read(cx).dialog, Some(crate::app_state::Dialog::SongDetails(_) | crate::app_state::Dialog::AlbumDetails(_))) {
                self.state.update(cx, |s, _| s.local.editing = false);
            }
            if let Some(crate::app_state::Dialog::PlaylistDraft { playlist, .. }) = self.state.read(cx).dialog.clone() {
                let name = playlist.as_ref().map(|p| p.name.clone()).unwrap_or_default();
                let comment = playlist.as_ref().and_then(|p| p.comment.clone()).unwrap_or_default();
                self.dialog_inputs.name.update(cx, |input, cx| input.set_value(name, window, cx));
                self.dialog_inputs.comment.update(cx, |input, cx| input.set_value(comment, window, cx));
                let handle = gpui::Focusable::focus_handle(self.dialog_inputs.name.read(cx), cx);
                window.focus(&handle, cx);
            }
            self.last_dialog = dialog_key;
        }
        if self.filter_placeholder != placeholder {
            self.filter_placeholder = placeholder.clone();
            self.filter.update(cx, |input, cx| input.set_placeholder(placeholder, window, cx));
        }

        let collapsed = self.state.read(cx).sidebar_collapsed;
        let floating = self.state.read(cx).floating_sidebar;
        let viewport = Self::css_width(window);
        cx.set_global(theme::Viewport(viewport));
        let narrow = viewport <= 640.0;
        let mobile_open = self.state.read(cx).mobile_sidebar && narrow;
        // ≤640px the sidebar is a drawer outside the layout.
        let motion = self.state.read(cx).sidebar_motion();
        if motion.animating {
            window.request_animation_frame();
        }
        let _ = collapsed;
        let sidebar_outer = if narrow {
            0.0
        } else {
            crate::components::sidebar::animated_width(viewport, motion.rail) + if floating { 16.0 } else { 0.0 }
        };
        let (pad_top, pad_x, pad_bottom) = theme::main_padding(viewport);
        // `.artist-page-content { padding-top: 0 }`.
        let pad_top = if route.page == Page::Artist { 0.0 } else { pad_top };
        let panel = self.state.read(cx).panel;
        // The inspector is a grid column above 860px and an overlay below.
        let inspector_column = panel.is_some() && viewport > 860.0;
        let inspector_space = if inspector_column { inspector::inspector_width(viewport) } else { 0.0 };
        let content_width = (viewport - sidebar_outer - inspector_space - 2.0 * pad_x).max(0.0);
        let dock_inset = if narrow {
            10.0
        } else if viewport <= 860.0 {
            13.0
        } else if inspector_column && viewport <= 1150.0 {
            14.0
        } else {
            25.0
        };

        let workspace = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(self.toolbar(viewport, window, cx))
            .child({
                // `.bulk-bar { position: sticky; top: 0 }`.
                let scroll_top = f32::from(self.main_scroll.bounds().origin.y) + f32::from(window.rem_size()) * pad_top / 16.0;
                let pinned = crate::views::selection::pinned_bulk_bar(&self.state, scroll_top, pad_top, cx);
                div().relative().flex_1().min_h_0().min_w_0().flex().flex_col().child(
                // The scroll area reaches left under the floating sidebar (and pads itself
                // back), so horizontal shelves can scroll all the way to the window edge.
                div()
                    .id("main-content")
                    .track_scroll(&self.main_scroll)
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .ml(sz(-sidebar_outer))
                    .pt(sz(pad_top))
                    .pl(sz(pad_x + sidebar_outer))
                    .pr(sz(pad_x))
                    .pb(sz(pad_bottom))
                    // Re-check the sentinel distance as the page scrolls.
                    .on_scroll(cx.listener(|_, _, _, cx| cx.notify()))
                    .child(self.guarded_page(content_width, window, cx))
                    .children(crate::paging::sentinel(&self.state, cx)),
                )
                .when_some(pinned, |d, bar| d.child(div().absolute().top_0().left(sz(pad_x)).right(sz(pad_x)).child(bar)))
            })
            .child(
                div()
                    .absolute()
                    .bottom(sz(if narrow { 11.0 } else { 17.0 }))
                    .left(sz(dock_inset))
                    .right(sz(dock_inset))
                    .flex()
                    .justify_center()
                    .child(self.dock.clone()),
            )
            .when_some(panel.filter(|_| !inspector_column), |d, panel| {
                d.child(inspector::render(&self.state, &self.lyrics, panel, viewport, cx))
            });

        let key_state = self.state.clone();
        let text_fields: Vec<gpui::FocusHandle> = [
            &self.filter,
            &self.search,
            &self.dialog_inputs.name,
            &self.dialog_inputs.comment,
            &self.dialog_inputs.shortcut_query,
            &self.folder_search,
        ]
            .iter()
            .map(|input| gpui::Focusable::focus_handle(input.read(cx), cx))
            .collect();
        let full_player_open = self.state.read(cx).full_player_open
            && self.state.read(cx).player.read(cx).current_song().is_some();
        let shell = div()
            .id("shell")
            .track_focus(&self.focus)
            .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                let typing = text_fields.iter().any(|handle| handle.is_focused(window));
                if crate::keyboard::handle(&key_state, &event.keystroke, typing, window, cx) {
                    cx.stop_propagation();
                }
            })
            .size_full()
            .line_height(relative(theme::NORMAL_LINE_HEIGHT))
            .flex()
            .overflow_hidden()
            .bg(tokens.canvas)
            .map(|d| window_chrome::window_corners(d, cx))
            .text_color(tokens.text)
            // The sidebar takes its width in the row but is painted after the page, over it:
            // content that scrolls under it shows through the glass.
            .when(!narrow, |d| d.child(div().flex_none().w(sz(sidebar_outer))))
            .child(workspace)
            .when(!narrow, |d| {
                d.child(div().absolute().top_0().bottom_0().left_0().flex().child(sidebar::render(&self.state, &self.sidebar_scroll, window, cx)))
            })
            .when_some(panel.filter(|_| inspector_column), |d, panel| d.child(inspector::render(&self.state, &self.lyrics, panel, viewport, cx)))
            .children({
                let open = self.state.read(cx).full_player_open
                    && self.state.read(cx).player.read(cx).current_song().is_some();
                if !open {
                    self.full_player = None;
                }
                open.then(|| {
                    let state = self.state.clone();
                    self.full_player
                        .get_or_insert_with(|| cx.new(|cx| crate::views::full_player::FullPlayer::new(state, cx)))
                        .clone()
                })
            })
            .children({
                // The toggle lives in Settings → Performance, in every build.
                let on = self.state.read(cx).storage.get(volta_logic::storage::FRAME_MONITOR_KEY) == "true";
                if !on {
                    self.perf = None;
                }
                on.then(|| {
                    let state = self.state.clone();
                    let perf = self.perf.get_or_insert_with(|| cx.new(|_| crate::components::perf_overlay::PerfOverlay::new(state))).clone();
                    gpui::deferred(perf).with_priority(1).into_any_element()
                })
            })
            .children(crate::components::connection_banner::render(&self.state, cx))
            .children(crate::components::update_banner::render(&self.state, cx))
            .children(crate::components::notifications::render(&self.state, cx))
            .children(crate::components::scale_preview::render(&self.state, cx))
            .children(crate::views::dialogs::render(&self.state, &self.dialog_inputs, cx))
            .children(crate::components::menu::render(&self.state, cx))
            .when(mobile_open, |d| {
                let close = self.state.clone();
                d.child(
                    // `.sidebar-scrim`.
                    div()
                        .id("sidebar-scrim")
                        .absolute()
                        .inset_0()
                        .bg(theme::hex("#0008"))
                        .map(|d| window_chrome::window_corners(d, cx))
                        .on_click(move |_, _, cx| {
                            close.update(cx, |s, cx| {
                                s.mobile_sidebar = false;
                                cx.notify();
                            })
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .flex()
                        .child(sidebar::render(&self.state, &self.sidebar_scroll, window, cx)),
                )
            })
            .into_any_element();
        // The full player covers the toolbar, so its window controls float.
        window_chrome::frame(shell, full_player_open, window)
    }
}
