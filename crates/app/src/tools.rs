//! Developer tool windows (performance, console): ordinary native windows, so
//! they behave the same on Linux, macOS and Windows. Each opens at most once;
//! asking again brings the existing window to the front.

use gpui::{AnyWindowHandle, App, AppContext, Bounds, Entity, Global, Render, TitlebarOptions, Window, WindowBounds, WindowOptions, px, size};

use crate::components::automix_preview::AutomixPreview;
use crate::components::console_window::ConsoleView;
use crate::components::perf_overlay::{PerfOverlay, SharedFeed};
use crate::app_state::AppState;

#[derive(Default)]
struct ToolWindows {
    performance: Option<AnyWindowHandle>,
    console: Option<AnyWindowHandle>,
    automix: Option<AnyWindowHandle>,
    /// The player window, whose frames the performance window reports.
    main: Option<AnyWindowHandle>,
    /// Frame samples of the main window, filled while the performance window is open.
    feed: SharedFeed,
}
impl Global for ToolWindows {}

fn is_open(handle: Option<AnyWindowHandle>, cx: &App) -> bool {
    handle.is_some_and(|handle| cx.windows().contains(&handle))
}

/// Whether the performance window exists (the main window measures its frames only then).
pub fn performance_window_open(cx: &App) -> bool {
    is_open(cx.try_global::<ToolWindows>().and_then(|t| t.performance), cx)
}

fn performance_feed(cx: &mut App) -> SharedFeed {
    cx.default_global::<ToolWindows>().feed.clone()
}

/// Records the player window; closing it closes the tool windows too.
pub fn register_main(handle: AnyWindowHandle, cx: &mut App) {
    cx.default_global::<ToolWindows>().main = Some(handle);
}

/// Opens the window held in `slot`, or focuses it if it is already open.
fn open<V: Render + 'static>(
    cx: &mut App,
    slot: fn(&mut ToolWindows) -> &mut Option<AnyWindowHandle>,
    title: &'static str,
    (width, height): (f32, f32),
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> bool {
    let existing = *slot(cx.default_global::<ToolWindows>());
    if let Some(handle) = existing.filter(|_| is_open(existing, cx)) {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
        return false;
    }
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(320.0), px(240.0))),
            titlebar: Some(TitlebarOptions { title: Some(title.into()), ..Default::default() }),
            app_id: Some(crate::app_icon::APP_ID.into()),
            icon: crate::app_icon::window_icon(),
            ..Default::default()
        },
        |window, cx| {
            let view = build(window, cx);
            cx.new(|cx| gpui_component::Root::new(view, window, cx))
        },
    );
    match opened {
        Ok(handle) => {
            *slot(cx.default_global::<ToolWindows>()) = Some(handle.into());
            true
        }
        Err(error) => {
            log::error!("could not open the {title} window: {error:#}");
            false
        }
    }
}

/// Live frame, runtime and network metrics in their own window.
pub fn open_performance(state: &Entity<AppState>, cx: &mut App) {
    let feed = performance_feed(cx);
    let target = state.clone();
    let opened = open(cx, |t| &mut t.performance, "Volta Performance", (420.0, 760.0), move |_, cx| {
        cx.new(|cx| PerfOverlay::windowed(target, feed, cx))
    });
    if opened {
        // The main window starts reporting its frames (after the current event).
        let main = cx.default_global::<ToolWindows>().main;
        let feed = performance_feed(cx);
        cx.defer(move |cx| {
            if let Some(main) = main {
                let _ = main.update(cx, |_, window, cx| feed.start(window, cx));
            }
        });
    }
}

/// The log console.
pub fn open_console(cx: &mut App) {
    open(cx, |t| &mut t.console, "Volta Console", (820.0, 520.0), |_, cx| cx.new(ConsoleView::new));
}

/// Two random library songs and the AutoMix join between them, playable.
pub fn open_automix_preview(state: &Entity<AppState>, cx: &mut App) {
    let target = state.clone();
    open(cx, |t| &mut t.automix, "Volta AutoMix Preview", (460.0, 720.0), move |_, cx| {
        cx.new(|cx| AutomixPreview::new(target, cx))
    });
}

/// Closes every tool window (they belong to the main window).
pub fn close_all(cx: &mut App) {
    for handle in cx.windows() {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}
