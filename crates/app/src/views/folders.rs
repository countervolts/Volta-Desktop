//! `FolderView` from app/folder-view.tsx: a Finder-like browser over the
//! folder tree (columns / list / icons), with search, a preview pane and a
//! path bar.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{AnyElement, App, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window, div, img, prelude::*};
use gpui_component::Sizable as _;
use gpui_component::input::{Input, InputState};
use volta_logic::folder_tree::{FolderNode, FolderTree, collect_folder_songs, folder_ancestors, search_folder_tree, song_folder_path};
use volta_logic::format::{duration, format_bytes, is_lossless};
use volta_logic::library::locale_compare;
use volta_logic::model::Song;

use crate::app_state::AppState;
use crate::components::album_grid::auto_fill_columns;
use crate::components::select::select;
use crate::components::ui::{IconButton, icon, icon_with, line};
use crate::theme::{self, Tokens, sz};
use crate::views::home::{empty_state, page_heading};
use crate::views::library::secondary_button;

const SORTS: &[(&str, &str)] = &[("name", "Name"), ("kind", "Kind"), ("size", "Size"), ("date", "Date Modified")];

#[derive(Clone)]
enum Entry {
    Folder(String),
    Song(Song),
}

fn kind_label(song: &Song) -> String {
    let ext = song.suffix.as_deref().map(str::to_uppercase).filter(|e| !e.is_empty());
    match (is_lossless(Some(song)), ext) {
        (_, Some(ext)) => format!("{ext} audio"),
        (true, None) => "Lossless audio".into(),
        (false, None) => "Audio".into(),
    }
}

fn modified_label(value: Option<f64>) -> String {
    match value {
        Some(ms) if ms > 0.0 => {
            let days = (ms / 86_400_000.0).floor() as i64;
            let (y, m, d) = civil_from_days(days);
            const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
            format!("{} {d}, {y}", MONTHS[(m - 1) as usize])
        }
        _ => "—".into(),
    }
}

/// Days since 1970-01-01 to (year, month, day).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `entriesFor` + `sortEntries`: folders first, then by the chosen key.
fn sorted_entries(tree: &FolderTree, node: &FolderNode, sort: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = node.folders.iter().cloned().map(Entry::Folder).chain(node.songs.iter().cloned().map(Entry::Song)).collect();
    let name = |e: &Entry| match e {
        Entry::Folder(p) => tree.get(p).map(|n| n.name.clone()).unwrap_or_default(),
        Entry::Song(s) => s.title.clone(),
    };
    let size = |e: &Entry| match e {
        Entry::Folder(p) => tree.get(p).map_or(0, |n| n.total_bytes),
        Entry::Song(s) => s.size.unwrap_or(0.0) as u64,
    };
    let date = |e: &Entry| match e {
        Entry::Folder(p) => tree.get(p).and_then(|n| n.modified).unwrap_or(0.0),
        Entry::Song(s) => s.local_modified.unwrap_or(0.0),
    };
    let kind = |e: &Entry| match e {
        Entry::Folder(_) => "Folder".to_string(),
        Entry::Song(s) => kind_label(s),
    };
    entries.sort_by(|a, b| {
        let folder = |e: &Entry| matches!(e, Entry::Folder(_));
        if folder(a) != folder(b) {
            return if folder(a) { std::cmp::Ordering::Less } else { std::cmp::Ordering::Greater };
        }
        let by_name = || locale_compare(&name(a), &name(b));
        match sort {
            "size" => size(b).cmp(&size(a)).then_with(by_name),
            "date" => date(b).partial_cmp(&date(a)).unwrap_or(std::cmp::Ordering::Equal).then_with(by_name),
            "kind" => locale_compare(&kind(a), &kind(b)).then_with(by_name),
            _ => by_name(),
        }
    });
    entries
}

fn play_list(state: &Entity<AppState>, songs: Vec<Song>, index: usize, cx: &mut App) {
    if songs.is_empty() {
        return;
    }
    let player = state.read(cx).player.clone();
    player.update(cx, |p, cx| p.play_queue(songs, index, cx));
}

fn play_song_in(state: &Entity<AppState>, tree: &FolderTree, folder: &FolderNode, song: &Song, cx: &mut App) {
    let list = collect_folder_songs(tree, folder);
    let index = list.iter().position(|s| s.id == song.id).unwrap_or(0);
    play_list(state, list, index, cx);
}

pub fn render(state: &Entity<AppState>, search: &Entity<InputState>, content_width: f32, window: &mut Window, cx: &mut App) -> AnyElement {
    let tokens = theme::tokens(cx).clone();
    let viewport = theme::viewport_css_width(window);
    let (tree, loading, path, view, sort, back_ok, forward_ok) = {
        let s = state.read(cx);
        (
            s.folder_tree.clone(),
            s.page_loading,
            s.folder_path.clone(),
            s.folder_view,
            s.folder_sort,
            s.folder_history_index > 0,
            s.folder_history_index + 1 < s.folder_history.len(),
        )
    };
    let mut page = div().flex().flex_col().child(page_heading("Folders", cx));
    let Some(tree) = tree else {
        return page.into_any_element();
    };
    if !loading && !tree.has_paths {
        return page
            .child(empty_state(
                "No folder information",
                "This library does not expose file paths, so folders cannot be browsed. Songs, albums, and artists are still available.",
                cx,
            ))
            .into_any_element();
    }
    let selected = tree.get(&path).unwrap_or(tree.root()).clone();
    let query = search.read(cx).value().trim().to_string();

    // Toolbar.
    let (back, forward) = (state.clone(), state.clone());
    let switcher = div().flex().items_center().gap(sz(2.0)).p(sz(2.0)).rounded(sz(8.0)).bg(tokens.row).children(
        [("columns", "Columns", "columns3"), ("list", "List", "list"), ("icons", "Icons", "layout-grid")].into_iter().map(|(mode, label, glyph)| {
            let target = state.clone();
            let active = mode == view;
            div()
                .id(SharedString::from(format!("folder-view-{mode}")))
                .w(sz(28.0))
                .h(sz(24.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(sz(6.0))
                .cursor_pointer()
                .when(active, |d| d.bg(tokens.hover).text_color(tokens.text))
                .when(!active, |d| d.text_color(tokens.secondary).hover(|s| s.text_color(tokens.text)))
                .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(label).build(window, cx))
                .on_click(move |_, _, cx| {
                    target.update(cx, |s, cx| {
                        s.folder_view = mode;
                        s.storage.set("volta-folder-view", mode);
                        cx.notify();
                    })
                })
                .child(icon(glyph, 15.0))
        }),
    );
    let sort_target = state.clone();
    let narrow = viewport <= 640.0;
    let toolbar = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sz(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sz(2.0))
                .child(IconButton::new("folder-back", "Back", icon("arrow-left", 16.0)).disabled(!back_ok).on_click(move |_, _, cx| back.update(cx, |s, cx| s.folder_step(-1, cx))))
                .child(IconButton::new("folder-forward", "Forward", icon("arrow-right", 16.0)).disabled(!forward_ok).on_click(move |_, _, cx| forward.update(cx, |s, cx| s.folder_step(1, cx)))),
        )
        .child(switcher)
        .child(select(
            state,
            "folder-sort",
            sort,
            SORTS,
            Rc::new(move |v, _, cx| {
                sort_target.update(cx, |s, cx| {
                    s.folder_sort = v;
                    s.storage.set("volta-folder-sort", v);
                    cx.notify();
                })
            }),
            cx,
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sz(6.0))
                .when(!narrow, |d| d.ml_auto().w(sz(190.0)))
                .when(narrow, |d| d.w_full())
                .h(sz(27.0))
                .overflow_hidden()
                .px(sz(9.0))
                .rounded(sz(20.0))
                .bg(tokens.sidebar)
                .text_color(tokens.muted)
                .child(icon("search", 14.0))
                .child(div().flex_1().text_size(sz(11.0)).text_color(tokens.text).child(
                    Input::new(search).xsmall().appearance(false).bordered(false).focus_bordered(false).p_0(),
                )),
        );

    let body: AnyElement = if !query.is_empty() {
        search_view(state, &tree, &query, &tokens, cx)
    } else {
        let main_width = if viewport <= 900.0 { content_width } else { content_width - 232.0 - 14.0 };
        let main = match view {
            "list" => list_view(state, &tree, &selected, sort, narrow, &tokens, cx),
            "icons" => icons_view(state, &tree, &selected, sort, main_width, &tokens, cx),
            _ => columns_view(state, &tree, &selected, sort, narrow, &tokens, cx),
        };
        div()
            .flex()
            .gap(sz(14.0))
            .min_h_0()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h(sz(420.0))
                    .border(sz(1.0))
                    .border_color(tokens.line)
                    .rounded(sz(10.0))
                    .bg(tokens.surface)
                    .overflow_hidden()
                    .child(main),
            )
            .when(viewport > 900.0, |d| d.child(preview(state, &tree, &selected, &tokens, cx)))
            .into_any_element()
    };

    // Path bar.
    let chain: Vec<FolderNode> = folder_ancestors(&tree, &selected.path).into_iter().cloned().collect();
    let root_target = state.clone();
    let mut path_bar = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sz(2.0))
        .px(sz(10.0))
        .py(sz(6.0))
        .border(sz(1.0))
        .border_color(tokens.line)
        .rounded(sz(8.0))
        .bg(tokens.surface)
        .child(crumb("crumb-root", Some("hard-drive"), "Library", selected.path.is_empty(), &tokens).on_click(move |_, _, cx| {
            root_target.update(cx, |s, cx| s.folder_go(String::new(), cx))
        }));
    for (i, node) in chain.iter().skip(1).enumerate() {
        let target = state.clone();
        let node_path = node.path.clone();
        path_bar = path_bar
            .child(div().text_color(tokens.muted).child(icon("chevron-right", 12.0)))
            .child(crumb(&format!("crumb-{i}"), None, &node.name, node.path == selected.path, &tokens).on_click(move |_, _, cx| {
                let path = node_path.clone();
                target.update(cx, |s, cx| s.folder_go(path, cx))
            }));
    }

    page = page.child(div().flex().flex_col().gap(sz(12.0)).min_h_0().child(toolbar).child(body).child(path_bar));
    page.into_any_element()
}

fn crumb(id: &str, glyph: Option<&str>, label: &str, current: bool, tokens: &Tokens) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id.to_string()))
        .flex()
        .items_center()
        .gap(sz(5.0))
        .px(sz(7.0))
        .py(sz(3.0))
        .rounded(sz(5.0))
        .text_size(sz(11.0))
        .cursor_pointer()
        .when(current, |d| d.text_color(tokens.text).font_weight(FontWeight::MEDIUM))
        .when(!current, |d| d.text_color(tokens.secondary))
        .hover(|s| s.bg(tokens.hover).text_color(tokens.text))
        .when_some(glyph, |d, g| d.child(icon(g, 13.0)))
        .child(label.to_string())
}

fn song_artwork(state: &Entity<AppState>, song: &Song, size: f32, radius: f32, request: u32, tokens: &Tokens, cx: &mut App) -> AnyElement {
    let cover = state.update(cx, |s, cx| s.cover(song.cover_art.as_deref(), request, cx));
    div()
        .size(sz(size))
        .flex_none()
        .rounded(sz(radius))
        .bg(tokens.hover)
        .when_some(cover, |d, image| d.child(img(image).size_full().rounded(sz(radius))))
        .into_any_element()
}

fn columns_view(state: &Entity<AppState>, tree: &Arc<FolderTree>, selected: &FolderNode, sort: &str, narrow: bool, tokens: &Tokens, cx: &mut App) -> AnyElement {
    let chain: Vec<FolderNode> = folder_ancestors(tree, &selected.path).into_iter().cloned().collect();
    let column_width = if narrow { 200.0 } else { 232.0 };
    let mut columns = div().id("finder-columns").flex().h_full().min_h(sz(420.0)).overflow_x_scroll();
    for (ci, node) in chain.iter().enumerate() {
        let next = chain.get(ci + 1).map(|n| n.path.clone());
        let entries = sorted_entries(tree, node, sort);
        let mut column = div()
            .id(SharedString::from(format!("finder-column-{ci}")))
            .flex_none()
            .w(sz(column_width))
            .p(sz(6.0))
            .overflow_y_scroll()
            .when(ci + 1 < chain.len(), |d| d.border_r(sz(1.0)).border_color(tokens.line));
        if entries.is_empty() {
            column = column.child(div().m(sz(10.0)).text_size(sz(11.0)).text_color(tokens.muted).child("Empty"));
        }
        for (ei, entry) in entries.into_iter().enumerate() {
            let id = SharedString::from(format!("finder-{ci}-{ei}"));
            match entry {
                Entry::Folder(path) => {
                    let Some(child) = tree.get(&path) else { continue };
                    let active = next.as_deref() == Some(path.as_str());
                    let (open, play) = (state.clone(), state.clone());
                    let (open_path, play_tree, play_node) = (path.clone(), tree.clone(), child.clone());
                    column = column.child(
                        div()
                            .id(id)
                            .flex()
                            .items_center()
                            .gap(sz(8.0))
                            .w_full()
                            .px(sz(7.0))
                            .py(sz(5.0))
                            .rounded(sz(6.0))
                            .text_size(sz(12.0))
                            .cursor_pointer()
                            .when(active, |d| d.bg(tokens.selection).text_color(tokens.accent))
                            .when(!active, |d| d.text_color(tokens.text).hover(|s| s.bg(tokens.hover)))
                            .on_click(move |event, _, cx| {
                                if event.click_count() >= 2 {
                                    play_list(&play, collect_folder_songs(&play_tree, &play_node), 0, cx);
                                } else {
                                    let path = open_path.clone();
                                    open.update(cx, |s, cx| s.folder_go(path, cx));
                                }
                            })
                            .child(div().flex_none().text_color(tokens.accent).child(icon(if active { "folder-open" } else { "folder" }, 16.0)))
                            .child(line(child.name.clone()).flex_1())
                            .child(div().flex_none().text_size(sz(10.0)).text_color(tokens.muted).child(child.total_songs.to_string()))
                            .child(div().flex_none().text_color(tokens.muted).child(icon("chevron-right", 13.0))),
                    );
                }
                Entry::Song(song) => {
                    let (play, play_tree, folder, s) = (state.clone(), tree.clone(), node.clone(), song.clone());
                    let art = song_artwork(state, &song, 22.0, 4.0, 80, tokens, cx);
                    column = column.child(
                        div()
                            .id(id)
                            .flex()
                            .items_center()
                            .gap(sz(8.0))
                            .w_full()
                            .px(sz(7.0))
                            .py(sz(5.0))
                            .rounded(sz(6.0))
                            .text_size(sz(12.0))
                            .text_color(tokens.text)
                            .cursor_pointer()
                            .hover(|s| s.bg(tokens.hover))
                            .on_click(move |_, _, cx| play_song_in(&play, &play_tree, &folder, &s, cx))
                            .child(art)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(line(song.title.clone()).font_weight(FontWeight::MEDIUM))
                                    .child(line(song.artist.clone().unwrap_or_else(|| "Unknown artist".into())).text_size(sz(10.0)).text_color(tokens.muted)),
                            )
                            .child(div().flex_none().text_size(sz(10.0)).text_color(tokens.muted).font_features(theme::fonts::tabular_nums()).child(duration(song.duration))),
                    );
                }
            }
        }
        columns = columns.child(column);
    }
    columns.into_any_element()
}

fn list_view(state: &Entity<AppState>, tree: &Arc<FolderTree>, selected: &FolderNode, sort: &str, narrow: bool, tokens: &Tokens, _cx: &mut App) -> AnyElement {
    let cols = |name: AnyElement, date: String, size: String, kind: String| {
        div()
            .flex()
            .items_center()
            .gap(sz(10.0))
            .px(sz(8.0))
            .py(sz(6.0))
            .text_size(sz(11.0))
            .child(div().flex_grow(if narrow { 1.0 } else { 2.4 }).flex_basis(gpui::relative(0.0)).min_w_0().child(name))
            .child(div().flex_grow(if narrow { 0.8 } else { 1.0 }).flex_basis(gpui::relative(0.0)).min_w_0().child(date))
            .when(!narrow, |d| {
                d.child(div().flex_grow(0.7).flex_basis(gpui::relative(0.0)).min_w_0().child(size))
                    .child(div().flex_grow(1.0).flex_basis(gpui::relative(0.0)).min_w_0().child(kind))
            })
    };
    let mut list = div().p(sz(6.0)).child(
        cols(div().child("Name").into_any_element(), "Date Modified".into(), "Size".into(), "Kind".into())
            .text_color(tokens.muted)
            .font_weight(FontWeight::MEDIUM)
            .border_b(sz(1.0))
            .border_color(tokens.line),
    );
    let entries = sorted_entries(tree, selected, sort);
    if entries.is_empty() {
        list = list.child(div().m(sz(10.0)).text_size(sz(11.0)).text_color(tokens.muted).child("This folder is empty."));
    }
    for (i, entry) in entries.into_iter().enumerate() {
        let id = SharedString::from(format!("finder-list-{i}"));
        let row = match entry {
            Entry::Folder(path) => {
                let Some(node) = tree.get(&path) else { continue };
                let (open, play, play_tree, play_node) = (state.clone(), state.clone(), tree.clone(), node.clone());
                let name = div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap(sz(8.0))
                    .text_size(sz(12.0))
                    .text_color(tokens.text)
                    .cursor_pointer()
                    .on_click(move |event, _, cx| {
                        if event.click_count() >= 2 {
                            play_list(&play, collect_folder_songs(&play_tree, &play_node), 0, cx);
                        } else {
                            let path = path.clone();
                            open.update(cx, |s, cx| s.folder_go(path, cx));
                        }
                    })
                    .child(div().flex_none().text_color(tokens.accent).child(icon("folder", 15.0)))
                    .child(line(node.name.clone()))
                    .into_any_element();
                cols(name, modified_label(node.modified), format_bytes(node.total_bytes), format!("Folder — {} songs", node.total_songs))
            }
            Entry::Song(song) => {
                let (play, play_tree, folder, s) = (state.clone(), tree.clone(), selected.clone(), song.clone());
                let name = div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap(sz(8.0))
                    .text_size(sz(12.0))
                    .text_color(tokens.text)
                    .cursor_pointer()
                    .on_click(move |_, _, cx| play_song_in(&play, &play_tree, &folder, &s, cx))
                    .child(div().flex_none().text_color(tokens.accent).child(icon("music2", 15.0)))
                    .child(line(song.title.clone()))
                    .into_any_element();
                cols(name, modified_label(song.local_modified), format_bytes(song.size.unwrap_or(0.0) as u64), kind_label(&song))
            }
        };
        list = list.child(row.rounded(sz(6.0)).text_color(tokens.secondary).hover(|s| s.bg(tokens.hover)));
    }
    div().id("finder-list").size_full().overflow_y_scroll().child(list).into_any_element()
}

fn icons_view(state: &Entity<AppState>, tree: &Arc<FolderTree>, selected: &FolderNode, sort: &str, width: f32, tokens: &Tokens, cx: &mut App) -> AnyElement {
    let entries = sorted_entries(tree, selected, sort);
    let columns = auto_fill_columns(width - 32.0, 112.0, 16.0);
    let mut grid = div().grid().grid_cols(columns as u16).gap(sz(16.0)).p(sz(16.0));
    for (i, entry) in entries.iter().enumerate() {
        let id = SharedString::from(format!("finder-icon-{i}"));
        let tile = div()
            .id(id)
            .flex()
            .flex_col()
            .items_center()
            .gap(sz(6.0))
            .px(sz(6.0))
            .py(sz(10.0))
            .rounded(sz(8.0))
            .text_size(sz(11.0))
            .text_center()
            .text_color(tokens.text)
            .cursor_pointer()
            .hover(|s| s.bg(tokens.hover));
        let element: AnyElement = match entry {
            Entry::Folder(path) => {
                let Some(node) = tree.get(path) else { continue };
                let (open, path) = (state.clone(), path.clone());
                tile.on_click(move |_, _, cx| {
                    let path = path.clone();
                    open.update(cx, |s, cx| s.folder_go(path, cx))
                })
                .child(div().text_color(tokens.accent).child(icon_with("folder", 54.0, Some(1.1), false)))
                .child(line(node.name.clone()).w_full())
                .child(div().text_size(sz(10.0)).text_color(tokens.muted).child(format!("{} songs", node.total_songs)))
                    .into_any_element()
            }
            Entry::Song(song) => {
                let (play, play_tree, folder, s) = (state.clone(), tree.clone(), selected.clone(), song.clone());
                let cover = state.update(cx, |st, cx| st.cover(song.cover_art.as_deref(), 200, cx));
                tile.on_click(move |_, _, cx| play_song_in(&play, &play_tree, &folder, &s, cx))
                    .child(
                        div()
                            .w_full()
                            .relative()
                            .rounded(sz(6.0))
                            .bg(tokens.hover)
                            .child(div().w_full().pb(gpui::relative(1.0)).when_some(cover, |d, image| {
                                d.child(img(image).absolute().top_0().left_0().size_full().rounded(sz(6.0)))
                            })),
                    )
                    .child(line(song.title.clone()).w_full())
                    .child(div().text_size(sz(10.0)).text_color(tokens.muted).child(song.artist.clone().unwrap_or_else(|| "Unknown artist".into())))
                    .into_any_element()
            }
        };
        grid = grid.child(element);
    }
    if entries.is_empty() {
        return div().m(sz(10.0)).text_size(sz(11.0)).text_color(tokens.muted).child("This folder is empty.").into_any_element();
    }
    div().id("finder-icons").size_full().overflow_y_scroll().child(grid).into_any_element()
}

fn preview(state: &Entity<AppState>, tree: &Arc<FolderTree>, selected: &FolderNode, tokens: &Tokens, cx: &mut App) -> AnyElement {
    let meta = |label: &str, value: String| {
        div()
            .flex()
            .justify_between()
            .gap(sz(12.0))
            .pb(sz(5.0))
            .border_b(sz(1.0))
            .border_color(tokens.line)
            .text_size(sz(11.0))
            .child(div().text_color(tokens.muted).child(label.to_string()))
            .child(div().text_color(tokens.text).child(value))
    };
    let (play, play_tree, play_node) = (state.clone(), tree.clone(), selected.clone());
    let preview_songs: Vec<Song> = selected.songs.iter().take(6).cloned().collect();
    let more = selected.songs.len().saturating_sub(preview_songs.len());
    div()
        .w(sz(232.0))
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .gap(sz(6.0))
        .px(sz(16.0))
        .py(sz(20.0))
        .border(sz(1.0))
        .border_color(tokens.line)
        .rounded(sz(10.0))
        .bg(tokens.surface)
        .text_center()
        .child(div().text_color(tokens.accent).child(icon_with(if selected.path.is_empty() { "hard-drive" } else { "folder-open" }, 62.0, Some(1.0), false)))
        .child(div().mt(sz(4.0)).text_size(sz(14.0)).font_weight(FontWeight::SEMIBOLD).child(if selected.path.is_empty() { "Library".to_string() } else { selected.name.clone() }))
        .child(div().text_size(sz(11.0)).text_color(tokens.muted).child(format!("Folder — {} songs", selected.total_songs)))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sz(6.0))
                .w_full()
                .my(sz(12.0))
                .child(meta("Size", format_bytes(selected.total_bytes)))
                .child(meta("Subfolders", selected.folders.len().to_string()))
                .child(meta("Modified", modified_label(selected.modified))),
        )
        .child(
            secondary_button("play-folder", "Play folder", "play", cx)
                .when(selected.total_songs == 0, |d| d.opacity(0.5))
                .on_click(move |_, _, cx| play_list(&play, collect_folder_songs(&play_tree, &play_node), 0, cx)),
        )
        .when(!preview_songs.is_empty(), |d| {
            let mut songs = div()
                .flex()
                .flex_col()
                .gap(sz(3.0))
                .w_full()
                .mt(sz(14.0))
                .text_left()
                .child(div().mb(sz(4.0)).text_size(sz(10.0)).font_weight(FontWeight::SEMIBOLD).letter_spacing(sz(0.4)).text_color(tokens.muted).child("IN THIS FOLDER"));
            for (i, song) in preview_songs.iter().enumerate() {
                let (play, play_tree, folder, s) = (state.clone(), tree.clone(), selected.clone(), song.clone());
                songs = songs.child(
                    div()
                        .id(("finder-preview-song", i))
                        .flex()
                        .items_center()
                        .gap(sz(6.0))
                        .px(sz(6.0))
                        .py(sz(4.0))
                        .rounded(sz(5.0))
                        .text_size(sz(11.0))
                        .text_color(tokens.secondary)
                        .cursor_pointer()
                        .hover(|s| s.bg(tokens.hover).text_color(tokens.text))
                        .on_click(move |_, _, cx| play_song_in(&play, &play_tree, &folder, &s, cx))
                        .child(div().flex_none().child(icon("music2", 12.0)))
                        .child(line(song.title.clone())),
                );
            }
            if more > 0 {
                songs = songs.child(div().text_size(sz(10.0)).text_color(tokens.muted).child(format!("+{more} more")));
            }
            d.child(songs)
        })
        .into_any_element()
}

fn search_view(state: &Entity<AppState>, tree: &Arc<FolderTree>, query: &str, tokens: &Tokens, cx: &mut App) -> AnyElement {
    let results = search_folder_tree(tree, query, 200);
    if results.folders.is_empty() && results.songs.is_empty() {
        return empty_state("No matches", &format!("Nothing in your folders matches “{query}”."), cx).into_any_element();
    }
    let result_row = |id: SharedString, glyph: &str, name: String, path: String, meta: String| {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(sz(10.0))
            .px(sz(9.0))
            .py(sz(7.0))
            .rounded(sz(6.0))
            .text_size(sz(12.0))
            .text_color(tokens.text)
            .cursor_pointer()
            .hover(|s| s.bg(tokens.hover))
            .child(div().w(sz(18.0)).flex_none().text_color(tokens.accent).child(icon(glyph, 15.0)))
            .child(line(name).flex_grow(1.4).flex_basis(gpui::relative(0.0)))
            .child(line(path).flex_grow(1.6).flex_basis(gpui::relative(0.0)).text_size(sz(11.0)).text_color(tokens.muted))
            .child(line(meta).flex_grow(0.8).flex_basis(gpui::relative(0.0)).text_size(sz(11.0)).text_color(tokens.muted))
    };
    let heading = |text: &str| div().mb(sz(8.0)).text_size(sz(11.0)).font_weight(FontWeight::SEMIBOLD).letter_spacing(sz(0.4)).text_color(tokens.muted).child(text.to_uppercase());
    let mut out = div().flex().flex_col().gap(sz(20.0));
    if !results.folders.is_empty() {
        let mut group = div().flex().flex_col().gap(sz(2.0));
        for (i, node) in results.folders.iter().enumerate() {
            let (open, path) = (state.clone(), node.path.clone());
            group = group.child(
                result_row(SharedString::from(format!("folder-result-{i}")), "folder", node.name.clone(), node.path.clone(), format!("{} songs", node.total_songs))
                    .on_click(move |_, _, cx| {
                        let path = path.clone();
                        open.update(cx, |s, cx| {
                            s.folder_clear_search = true;
                            s.folder_go(path, cx)
                        })
                    }),
            );
        }
        out = out.child(div().child(heading("Folders")).child(group));
    }
    if !results.songs.is_empty() {
        let mut group = div().flex().flex_col().gap(sz(2.0));
        for (i, (song, folder)) in results.songs.iter().enumerate() {
            let (play, play_tree, folder_node, s) = (state.clone(), tree.clone(), (*folder).clone(), (*song).clone());
            let folder_path = song_folder_path(song);
            group = group.child(
                result_row(
                    SharedString::from(format!("song-result-{i}")),
                    "music2",
                    song.title.clone(),
                    if folder_path.is_empty() { "Library".into() } else { folder_path },
                    song.artist.clone().unwrap_or_else(|| "Unknown artist".into()),
                )
                .on_click(move |_, _, cx| play_song_in(&play, &play_tree, &folder_node, &s, cx)),
            );
        }
        out = out.child(div().child(heading("Songs")).child(group));
    }
    out.into_any_element()
}
