//! `LibrarySidebar` (`src/app/library-sidebar.tsx`) with the floating card
//! style (`volta-floating-sidebar`, on by default) and the icon rail.

use gpui::{
    AnimationExt, AnyElement, App, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window, div,
    img, prelude::*,
};
use gpui_component::tooltip::Tooltip;

use crate::components::window_chrome;
use crate::app_state::{AppState, Page, Route};
use crate::components::ui::{IconButton, icon_with};
use crate::theme::{self, Tokens, mix, sz, transparent};

pub const RAIL_WIDTH: f32 = 68.0;

struct NavItem {
    page: Page,
    icon: &'static str,
    label: &'static str,
}

const PRIMARY: [NavItem; 2] = [
    NavItem { page: Page::Home, icon: "house", label: "Home" },
    NavItem { page: Page::Recent, icon: "grid2-x2", label: "New in Your Library" },
];
const LIBRARY: [NavItem; 6] = [
    NavItem { page: Page::Albums, icon: "disc3", label: "Albums" },
    NavItem { page: Page::Artists, icon: "mic2", label: "Artists" },
    NavItem { page: Page::Songs, icon: "music2", label: "Songs" },
    NavItem { page: Page::Folders, icon: "folder", label: "Folders" },
    NavItem { page: Page::Favorites, icon: "star", label: "Favorites" },
    NavItem { page: Page::Stats, icon: "bar-chart3", label: "Stats" },
];

thread_local! {
    /// Label opacity during the collapse animation (`opacity 0.16s ease`).
    static LABELS: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
}

fn labels() -> f32 {
    LABELS.with(std::cell::Cell::get)
}

/// The sidebar's width at `rail` (0 = full, 1 = icon rail) of the collapse.
pub fn animated_width(viewport: f32, rail: f32) -> f32 {
    let full = theme::sidebar_width(viewport);
    full + (RAIL_WIDTH - full) * rail
}

fn heading(text: &'static str, collapsed: bool, tokens: &Tokens) -> AnyElement {
    if collapsed {
        // Section headings become thin dividers on the rail.
        return div().h(sz(1.0)).mt(sz(15.0)).mb(sz(9.0)).mx(sz(6.0)).bg(tokens.line).into_any_element();
    }
    div()
        .mt(sz(24.0))
        .mb(sz(6.0))
        .mx(sz(9.0))
        .text_size(sz(10.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tokens.muted)
        .whitespace_nowrap()
        .opacity(labels())
        .child(text)
        .into_any_element()
}

/// One `nav > button` row (or 36px rail square when collapsed).
fn row(
    id: impl Into<gpui::ElementId>,
    leading: AnyElement,
    label: SharedString,
    selected: bool,
    collapsed: bool,
    tokens: &Tokens,
) -> gpui::Stateful<gpui::Div> {
    let tooltip_label = label.clone();
    let base = div()
        .id(id)
        .flex()
        .items_center()
        .rounded(sz(if collapsed { 9.0 } else { 6.0 }))
        .text_size(sz(12.0))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.bg(tokens.hover))
        .when(selected, |r| r.bg(tokens.selection))
        .child(leading);
    if collapsed {
        base.size(sz(36.0))
            .mx_auto()
            .justify_center()
            .tooltip(move |window, cx| Tooltip::new(tooltip_label.clone()).build(window, cx))
    } else {
        base.w_full()
            .min_h(sz(32.0))
            .gap(sz(9.0))
            .px(sz(9.0))
            .py(sz(5.0))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .opacity(labels())
                    .when(selected, |l| l.font_weight(FontWeight::MEDIUM))
                    .child(label),
            )
    }
}

fn nav_icon(name: &str, tokens: &Tokens) -> AnyElement {
    div().text_color(tokens.accent).child(icon_with(name, 17.0, Some(1.8), false)).into_any_element()
}

pub fn render(state: &Entity<AppState>, _window: &mut Window, cx: &mut App) -> impl IntoElement {
    let tokens = theme::tokens(cx).clone();
    // The rail layout switches in once the labels have faded (collapsing) or
    // the full layout at once (expanding); width and padding glide between.
    let motion = state.read(cx).sidebar_motion();
    let collapsed = motion.rail_layout;
    let toggled_collapsed = state.read(cx).sidebar_collapsed;
    LABELS.with(|l| l.set(motion.labels));
    let floating = state.read(cx).floating_sidebar;
    let route = state.read(cx).route().clone();
    let playlists = state.read(cx).playlists.clone();
    let username = state.read(cx).account_label();

    let navigate = |page: Page| {
        let state = state.clone();
        move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
            state.update(cx, |state, cx| state.navigate(Route::page(page), cx));
        }
    };

    let collapse_button = {
        let state = state.clone();
        IconButton::new(
            "sidebar-collapse",
            if toggled_collapsed { "Expand navigation" } else { "Collapse navigation" },
            icon_with(if toggled_collapsed { "panel-left-open" } else { "panel-left-close" }, 17.0, None, false),
        )
        .size(26.0)
        .when(collapsed, |b| b.color(tokens.muted))
        .tooltip(format!("{} navigation ({} B)", if toggled_collapsed { "Expand" } else { "Collapse" }, crate::theme::mod_label()))
        .on_click(move |_, _, cx| state.update(cx, |state, cx| state.toggle_sidebar(cx)))
    };

    let brand = if collapsed {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(sz(4.0))
            .mb(sz(12.0))
            .pb(sz(12.0))
            .border_b(sz(1.0))
            .border_color(tokens.line)
            .child(img("images/volta-bolt.svg").size(sz(23.0)).flex_none())
            .child(collapse_button)
    } else {
        div()
            .flex()
            .items_center()
            .gap(sz(8.0))
            .mx(sz(9.0))
            .mb(sz(25.0))
            .h(sz(25.0))
            .text_size(sz(22.0))
            .font_weight(FontWeight::BOLD)
            .letter_spacing(sz(-0.8))
            .whitespace_nowrap()
            .child(img("images/volta-bolt.svg").size(sz(23.0)).flex_none())
            .child(div().opacity(labels()).child("Volta"))
            .child(div().ml_auto().child(collapse_button))
    };

    let search_selected = matches!(route.page, Page::Search | Page::Genre);
    let search = {
        let row = row(
            "nav-search",
            div().text_color(tokens.accent).child(icon_with("search", 17.0, None, false)).into_any_element(),
            "Search".into(),
            search_selected,
            collapsed,
            &tokens,
        )
        .mb(sz(4.0))
        // `.sidebar-search { color: var(--muted) }`; selected uses the accent.
        .text_color(if search_selected { tokens.accent } else { tokens.muted })
        .on_click(navigate(Page::Search));
        if collapsed {
            row
        } else {
            row.child(
                div()
                    .ml_auto()
                    .flex_none()
                    .text_size(sz(10.0))
                    .opacity(0.6 * labels())
                    .child(format!("{} K", crate::theme::mod_label())),
            )
        }
    };

    let items = |list: &'static [NavItem]| -> Vec<AnyElement> {
        list.iter()
            .map(|item| {
                row(
                    SharedString::from(format!("nav-{}", item.label)),
                    nav_icon(item.icon, &tokens),
                    item.label.into(),
                    route.page == item.page,
                    collapsed,
                    &tokens,
                )
                .on_click(navigate(item.page))
                .into_any_element()
            })
            .collect()
    };

    let playlist_rows: Vec<AnyElement> = playlists
        .iter()
        .enumerate()
        .map(|(index, playlist)| {
            let selected = route.page == Page::Playlist && route.id.as_deref() == Some(playlist.id.as_str());
            let cover = state.update(cx, |state, cx| state.cover(playlist.cover_art.as_deref(), 60, cx));
            let artwork = div()
                .size(sz(21.0))
                .flex_none()
                .rounded(sz(3.0))
                .overflow_hidden()
                .bg(tokens.surface)
                .when_some(cover, |a, image| a.child(img(image).size_full().rounded(sz(3.0))))
                .into_any_element();
            let target = Route {
                page: Page::Playlist,
                id: Some(playlist.id.clone()),
                title: Some(playlist.name.clone()),
            };
            let state = state.clone();
            row(("playlist", index), artwork, playlist.name.clone().into(), selected, collapsed, &tokens)
                .on_click(move |_, _, cx| {
                    let target = target.clone();
                    state.update(cx, |state, cx| state.navigate(target, cx));
                })
                .into_any_element()
        })
        .collect();

    let nav = div()
        .id("sidebar-nav")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        // A little air between rows so two highlighted rows never fuse into one block.
        .gap(sz(3.0))
        .children(items(&PRIMARY))
        .child(heading("Library", collapsed, &tokens))
        .children(items(&LIBRARY))
        .child(heading("Playlists", collapsed, &tokens))
        .child(
            row(
                "nav-playlists",
                nav_icon("list-music", &tokens),
                "All Playlists".into(),
                route.page == Page::Playlists,
                collapsed,
                &tokens,
            )
            .on_click(navigate(Page::Playlists)),
        )
        .children(playlist_rows)
        .children(pinned_section(state, &route, collapsed, &tokens, cx));

    let link = |id: &'static str, text: &'static str, url: &'static str, github: bool| {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(sz(5.0))
            .mx(sz(7.0))
            .text_size(sz(11.0))
            .text_color(tokens.muted)
            .cursor_pointer()
            .hover(|s| s.text_color(tokens.text).underline())
            .on_click(move |_, _, cx| cx.open_url(url))
            .whitespace_nowrap()
            .opacity(labels())
            .when(github, |l| l.child(icon_with("github", 13.0, None, false)))
            .child(text)
            .child(div().text_color(tokens.secondary).child("↗"))
    };

    let settings = row(
        "nav-settings",
        nav_icon("settings2", &tokens),
        "Settings".into(),
        route.page == Page::Settings,
        collapsed,
        &tokens,
    )
    .mt(sz(12.0))
    .text_color(tokens.text)
    .on_click(navigate(Page::Settings))
    .when(!collapsed, |r| {
        r.child(div().ml_auto().opacity(labels()).text_color(tokens.muted).child(icon_with("chevron-right", 15.0, None, false)))
    });

    let account = div()
        .flex()
        .items_center()
        .gap(sz(9.0))
        .mt(sz(14.0))
        .pt(sz(10.0))
        .px(sz(7.0))
        .pb(sz(2.0))
        .border_t(sz(1.0))
        .border_color(tokens.line)
        .text_size(sz(12.0))
        .when(collapsed, |a| a.justify_center().px_0())
        .child(icon_with("circle-user-round", 26.0, None, false))
        .when(!collapsed, |a| {
            a.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .opacity(labels())
                    .child(div().whitespace_nowrap().text_ellipsis().child(username.clone()))
                    .child(div().mt(sz(2.0)).text_size(sz(10.0)).text_color(tokens.muted).child(if state.read(cx).is_local() { "This device" } else { "Navidrome" })),
            )
        });

    let width = animated_width(theme::viewport_css_width(_window), motion.rail);
    div()
        .id("library-sidebar")
        // The sidebar floats over the page: clicks and hovers on it must not
        // reach whatever is underneath.
        .occlude()
        .flex_none()
        .w(sz(width))
        .flex()
        .flex_col()
        .min_h_0()
        .pt(sz(17.0 + window_chrome::traffic_light_inset()))
        .px(sz(12.0 - 3.0 * motion.rail))
        .pb(sz(9.0))
        .overflow_hidden()
        .when(floating, |s| {
            // The floating card is Liquid Glass (rim light and shadow included); themes
            // without it (high contrast) keep the opaque card. The 1px border stays so
            // the layout does not move.
            let glass = theme::glass::sidebar(&tokens);
            s.m(sz(8.0))
                .border(sz(1.0))
                .rounded(sz(20.0))
                .map(|s| match glass {
                    Some(glass) => s.border_color(transparent()).glass(glass),
                    None => s.bg(tokens.sidebar).border_color(mix(tokens.text, 20.0, transparent())).shadow(vec![
                        gpui::BoxShadow {
                            color: theme::hex("#00000018"),
                            offset: gpui::point(gpui::px(0.0), gpui::px(6.0)),
                            blur_radius: gpui::px(20.0),
                            spread_radius: gpui::px(0.0),
                            inset: false,
                        },
                    ]),
                })
        })
        .when(!floating, |s| s.bg(tokens.sidebar))
        .when(!floating, |s| s.border_r(sz(1.0)).border_color(tokens.line))
        .child(brand)
        .child(search)
        .child(nav)
        .when(!collapsed, |s| {
            s.child(div().mt(sz(14.0)).child(link(
                "source-link",
                "View source code",
                "https://github.com/countervolts/Volta-Desktop",
                true,
            )))
            .child(div().mt(sz(7.0)).child(link("discord-link", "Join the Discord", "https://discord.gg/h5Hybq9SJs", false)))
        })
        .child(settings)
        .child(account)
}


/// `renderPinRows` and the Pinned / Pinned Albums / Artists / Playlists headings.
fn pinned_section(
    state: &Entity<AppState>,
    route: &crate::app_state::Route,
    collapsed: bool,
    tokens: &Tokens,
    cx: &mut App,
) -> Vec<AnyElement> {
    use volta_logic::storage::{Pin, PinKind, pin_key};
    let (pins, separate, added) = {
        let s = state.read(cx);
        (s.pins.clone(), s.separate_pins, s.pin_added.clone())
    };
    if pins.is_empty() {
        return Vec::new();
    }
    let rows_for = |items: Vec<Pin>, cx: &mut App| -> Vec<AnyElement> {
        items
            .into_iter()
            .map(|pin| {
                let kind_page = match pin.kind {
                    PinKind::Album => Page::Album,
                    PinKind::Artist => Page::Artist,
                    PinKind::Playlist => Page::Playlist,
                };
                let selected = route.page == kind_page && route.id.as_deref() == Some(pin.id.as_str());
                let cover = state.update(cx, |s, cx| s.cover(pin.cover_art.as_deref(), 60, cx));
                let fallback = match pin.kind {
                    PinKind::Album => "disc3",
                    PinKind::Artist => "mic2",
                    PinKind::Playlist => "list-music",
                };
                let leading = match cover {
                    Some(image) => div()
                        .size(sz(21.0))
                        .flex_none()
                        .rounded(sz(if pin.kind == PinKind::Artist { 11.0 } else { 3.0 }))
                        .bg(tokens.surface)
                        .child(img(image).size_full().rounded(sz(if pin.kind == PinKind::Artist { 11.0 } else { 3.0 })))
                        .into_any_element(),
                    None => nav_icon(fallback, tokens),
                };
                let key = pin_key(&pin);
                let group = SharedString::from(format!("pin-{key}"));
                let (open_state, unpin_state) = (state.clone(), state.clone());
                let target = crate::app_state::Route { page: kind_page, id: Some(pin.id.clone()), title: Some(pin.name.clone()) };
                let unpin = pin.clone();
                let open = row(SharedString::from(format!("pin-open-{key}")), leading, pin.name.clone().into(), selected, collapsed, tokens)
                    .flex_1()
                    .min_w_0()
                    .on_click(move |_, _, cx| {
                        let target = target.clone();
                        open_state.update(cx, |s, cx| s.navigate(target, cx));
                    });
                let mut item = div()
                    .id(SharedString::from(format!("pin-{key}")))
                    .group(group.clone())
                    .flex()
                    .items_center()
                    .gap(sz(2.0))
                    .child(open);
                if !collapsed {
                    item = item.child(
                        div().flex_none().opacity(0.0).group_hover(group, |s| s.opacity(1.0)).child(
                            crate::components::ui::IconButton::new(
                                SharedString::from(format!("unpin-{key}")),
                                format!("Unpin {}", pin.name),
                                icon_with("x", 13.0, None, false),
                            )
                            .size(22.0)
                            .on_click(move |_, _, cx| {
                                let pin = unpin.clone();
                                unpin_state.update(cx, |s, cx| s.toggle_pin(pin, cx));
                            }),
                        ),
                    );
                }
                // `pin-added-pop` (520 ms): the scale is not drawable, the fade is.
                match added.as_ref().filter(|(k, at)| *k == key && at.elapsed() < std::time::Duration::from_millis(520) && !crate::components::motion::reduced()) {
                    Some(_) => item
                        .with_animation(
                            SharedString::from(format!("pin-pop-{key}")),
                            gpui::Animation::new(std::time::Duration::from_millis(520))
                                .with_easing(crate::components::motion::cubic_bezier(0.2, 0.8, 0.2, 1.0)),
                            |el, t| el.opacity(0.65 + 0.35 * (t / 0.68).min(1.0)),
                        )
                        .into_any_element(),
                    None => item.into_any_element(),
                }
            })
            .collect()
    };
    let mut out = Vec::new();
    if separate {
        for (kind, title) in [(PinKind::Album, "Pinned Albums"), (PinKind::Artist, "Pinned Artists"), (PinKind::Playlist, "Pinned Playlists")] {
            let items: Vec<Pin> = pins.iter().filter(|p| p.kind == kind).cloned().collect();
            if !items.is_empty() {
                out.push(heading(title, collapsed, tokens));
                out.extend(rows_for(items, cx));
            }
        }
    } else {
        out.push(heading("Pinned", collapsed, tokens));
        out.extend(rows_for(pins, cx));
    }
    out
}
