//! Client-side window frame (Linux).
//!
//! The web app runs inside a browser, so it has no title bar of its own. On
//! Linux the native window asks for client-side decorations and draws none:
//! the toolbar's empty space (and a strip at the top of full-screen views)
//! moves the window, double-click maximizes, right-click opens the window
//! menu, a 5px band along the edges resizes, and small minimize / maximize /
//! close buttons sit at the toolbar's end. When the compositor insists on
//! server-side decorations (`Decorations::Server`) none of this is drawn.
//!
//! Windows hides the native title bar too (`appears_transparent`) and uses the
//! same layout, but moving, resizing and the three buttons are done by the OS
//! through window-control hit regions (`WindowControlArea`), so no resize bands
//! are drawn. macOS keeps its native title bar.

use std::cell::Cell;

use gpui::{
    AnyElement, App, CursorStyle, Decorations, InteractiveElement, IntoElement, MouseButton, ParentElement, ResizeEdge,
    Styled, Window, WindowControlArea, div, prelude::*, px,
};

use crate::components::ui::{IconButton, icon};
use crate::theme::sz;

/// Resize band along the window edges, in logical pixels.
const RESIZE_BAND: f32 = 5.0;

thread_local! {
    /// "Leave" was chosen in the close prompt; the next close request passes.
    static CLOSE_CONFIRMED: Cell<bool> = const { Cell::new(false) };
}

/// Rounds `element`'s corners to match the window's (gpui-component's
/// `window_border` frame, radius [`WINDOW_RADIUS`]) where it touches them.
/// gpui clips to rectangles, so every layer that paints a background into a
/// window corner (the shell, full-screen views, scrims) needs this.
pub fn window_corners<E: Styled>(element: E, cx: &App) -> E {
    let radii = cx.try_global::<WindowCorners>().map(|c| c.0).unwrap_or_default();
    element.rounded_tl(radii.top_left).rounded_tr(radii.top_right).rounded_bl(radii.bottom_left).rounded_br(radii.bottom_right)
}

/// The window's corner radii this frame (square when maximized or tiled),
/// recorded by the shell so views without a `Window` can match them.
#[derive(Default)]
pub struct WindowCorners(pub gpui::Corners<gpui::Pixels>);
impl gpui::Global for WindowCorners {}

pub fn record_corners(window: &Window, cx: &mut App) {
    cx.set_global(WindowCorners(gpui_component::window_corner_radii(window, cx)));
}

/// Corner radius of the client-decorated window (Linux), like KDE/GNOME.
pub const WINDOW_RADIUS: f32 = 10.0;

/// The window has no native title bar, so the app provides the drag area and
/// the window controls (Linux client-side decorations, or Windows).
pub fn client_side(window: &Window) -> bool {
    cfg!(target_os = "windows")
        || (cfg!(any(target_os = "linux", target_os = "freebsd")) && matches!(window.window_decorations(), Decorations::Client { .. }))
}

/// macOS hides the native title bar too (`appears_transparent`) but keeps the
/// OS traffic lights and resize frame, so only a drag area is added.
pub const MACOS: bool = cfg!(target_os = "macos");

/// Extra top space (logical px) that keeps content clear of the traffic lights.
pub fn traffic_light_inset() -> f32 {
    if MACOS { 24.0 } else { 0.0 }
}

/// Whether the app provides the drag area (client-side frame, or macOS).
pub fn custom_titlebar(window: &Window) -> bool {
    MACOS || client_side(window)
}

/// `volta-warn-before-leave`: closing while music plays asks first. Returns
/// whether the window may close now; otherwise a prompt is shown and the
/// window closes itself if "Leave" is chosen.
pub fn should_close(window: &mut Window, cx: &mut App) -> bool {
    let playing = cx.try_global::<crate::app_state::GlobalPlaying>().is_some_and(|p| p.0);
    let warn = cx
        .try_global::<crate::app_state::GlobalStorage>()
        .is_none_or(|storage| storage.0.get(volta_logic::storage::WARN_BEFORE_LEAVE_KEY) != "false");
    if CLOSE_CONFIRMED.with(Cell::get) || !playing || !warn {
        return true;
    }
    let answer = window.prompt(
        gpui::PromptLevel::Warning,
        "Leave Volta?",
        Some("Music is still playing. Closing Volta stops it."),
        &["Leave", "Stay"],
        cx,
    );
    window
        .spawn(cx, async move |cx| {
            if answer.await == Ok(0) {
                CLOSE_CONFIRMED.with(|c| c.set(true));
                let _ = cx.update(|window, _| window.remove_window());
            }
        })
        .detach();
    false
}

/// The close button: same confirmation as a compositor close.
fn request_close(window: &mut Window, cx: &mut App) {
    if should_close(window, cx) {
        window.remove_window();
    }
}

/// Makes `element` move the window: drag to move, double-click to maximize or
/// restore, right-click for the window menu.
pub fn drag_area<E: InteractiveElement + IntoElement>(element: E) -> E {
    // Windows: the OS moves the window and handles double-click / right-click
    // for any region reported as the caption.
    #[cfg(target_os = "windows")]
    return element.window_control_area(WindowControlArea::Drag);
    #[cfg(not(target_os = "windows"))]
    element
        .on_mouse_down(MouseButton::Left, |event, window, cx| {
            if event.click_count >= 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
            cx.stop_propagation();
        })
        .on_mouse_down(MouseButton::Right, |event, window, cx| {
            window.show_window_menu(event.position);
            cx.stop_propagation();
        })
}

/// Minimize / maximize / close, sized like the toolbar's icon buttons.
pub fn controls(window: &Window) -> AnyElement {
    let maximized = window.is_maximized();
    // Windows: the OS handles presses on these regions (Min / Max / Close
    // hit-test results), so they are plain hover targets with no click handler.
    if cfg!(target_os = "windows") {
        let button = |id: &'static str, area: WindowControlArea, glyph: AnyElement, close: bool| {
            div()
                .id(id)
                .window_control_area(area)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(sz(30.0))
                .rounded(sz(8.0))
                .cursor(CursorStyle::Arrow)
                .hover(move |s| s.bg(if close { gpui::hsla(0.0, 0.75, 0.5, 0.9) } else { gpui::hsla(0.0, 0.0, 0.5, 0.18) }))
                .child(glyph)
        };
        return div()
            .flex()
            .flex_none()
            .items_center()
            .gap(sz(2.0))
            .child(button("window-minimize", WindowControlArea::Min, icon("minus", 15.0).into_any_element(), false))
            .child(button(
                "window-maximize",
                WindowControlArea::Max,
                icon(if maximized { "copy" } else { "square" }, 13.0).into_any_element(),
                false,
            ))
            .child(button("window-close", WindowControlArea::Close, icon("x", 16.0).into_any_element(), true))
            .into_any_element();
    }
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(sz(2.0))
        .child(
            IconButton::new("window-minimize", "Minimize", icon("minus", 15.0))
                .on_click(|_, window, _| window.minimize_window()),
        )
        .child(
            IconButton::new(
                "window-maximize",
                if maximized { "Restore" } else { "Maximize" },
                icon(if maximized { "copy" } else { "square" }, 13.0),
            )
            .on_click(|_, window, _| window.zoom_window()),
        )
        .child(IconButton::new("window-close", "Close", icon("x", 16.0)).on_click(|_, window, cx| request_close(window, cx)))
        .into_any_element()
}

/// Wraps the window's content with the resize bands, and, for views without
/// the toolbar (`floating_controls`), a drag strip and the window controls in
/// the top-right corner.
pub fn frame(content: AnyElement, floating_controls: bool, window: &Window) -> AnyElement {
    if MACOS {
        if !floating_controls {
            return content;
        }
        return div()
            .relative()
            .size_full()
            .child(content)
            .child(drag_area(div().id("window-drag-strip").absolute().top_0().left_0().right_0().h(sz(20.0))))
            .into_any_element();
    }
    if !client_side(window) {
        return content;
    }
    // Windows: the OS provides the resize borders, so only the controls are added.
    if cfg!(target_os = "windows") {
        if !floating_controls {
            return content;
        }
        return div()
            .relative()
            .size_full()
            .child(content)
            .child(drag_area(div().id("window-drag-strip").absolute().top_0().left_0().right(sz(110.0)).h(sz(20.0))))
            .child(div().absolute().top(sz(10.0)).right(sz(12.0)).child(controls(window)))
            .into_any_element();
    }
    let tiling = match window.window_decorations() {
        Decorations::Client { tiling } => tiling,
        Decorations::Server => Default::default(),
    };
    let resizable = !window.is_maximized() && !window.is_fullscreen();
    let band = px(RESIZE_BAND);
    let edge = |id: &'static str, edge: ResizeEdge, cursor: CursorStyle| {
        div()
            .id(id)
            .absolute()
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.start_window_resize(edge);
                cx.stop_propagation();
            })
    };
    div()
        .relative()
        .size_full()
        .child(content)
        .when(floating_controls, |d| {
            d.child(drag_area(div().id("window-drag-strip").absolute().top_0().left_0().right_0().h(sz(20.0))))
                .child(div().absolute().top(sz(10.0)).right(sz(12.0)).child(controls(window)))
        })
        .when(resizable, |d| {
            d.when(!tiling.top, |d| d.child(edge("resize-t", ResizeEdge::Top, CursorStyle::ResizeUpDown).top_0().left(band).right(band).h(band)))
                .when(!tiling.bottom, |d| {
                    d.child(edge("resize-b", ResizeEdge::Bottom, CursorStyle::ResizeUpDown).bottom_0().left(band).right(band).h(band))
                })
                .when(!tiling.left, |d| d.child(edge("resize-l", ResizeEdge::Left, CursorStyle::ResizeLeftRight).left_0().top(band).bottom(band).w(band)))
                .when(!tiling.right, |d| {
                    d.child(edge("resize-r", ResizeEdge::Right, CursorStyle::ResizeLeftRight).right_0().top(band).bottom(band).w(band))
                })
                .when(!(tiling.top || tiling.left), |d| {
                    d.child(edge("resize-tl", ResizeEdge::TopLeft, CursorStyle::ResizeUpLeftDownRight).top_0().left_0().size(band))
                })
                .when(!(tiling.top || tiling.right), |d| {
                    d.child(edge("resize-tr", ResizeEdge::TopRight, CursorStyle::ResizeUpRightDownLeft).top_0().right_0().size(band))
                })
                .when(!(tiling.bottom || tiling.left), |d| {
                    d.child(edge("resize-bl", ResizeEdge::BottomLeft, CursorStyle::ResizeUpRightDownLeft).bottom_0().left_0().size(band))
                })
                .when(!(tiling.bottom || tiling.right), |d| {
                    d.child(edge("resize-br", ResizeEdge::BottomRight, CursorStyle::ResizeUpLeftDownRight).bottom_0().right_0().size(band))
                })
        })
        .into_any_element()
}
