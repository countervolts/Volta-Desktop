//! Volta desktop player.

mod app_icon;
mod app_state;
mod assets;
mod build_info;
mod components;
mod console;
mod covers;
mod automix_analysis;
mod infinite;
mod keyboard;
mod local;
mod logs;
mod media_controls;
mod paging;
mod perf_log;
mod player;
mod recommend;
#[cfg(feature = "shots")]
mod shot;
mod theme;
mod tools;
mod views;

use gpui::{App, AppContext, Bounds, Entity, TitlebarOptions, Window, WindowAppearance, WindowBounds, WindowOptions, px, size};
use gpui_platform::application;
use volta_logic::storage::{INTERFACE_SCALE_KEY, THEME_KEY, ThemePreference, read_interface_scale};
use volta_services::Storage;

use crate::app_state::{AppState, GlobalStorage};
use crate::theme::ThemeState;
use crate::views::shell::Shell;

fn is_light(appearance: WindowAppearance) -> bool {
    matches!(appearance, WindowAppearance::Light | WindowAppearance::VibrantLight)
}

/// App-wide globals: component library, theme tokens, fonts, storage.
pub(crate) fn init_globals(storage: &Storage, cx: &mut App) {
    gpui_component::init(cx);
    let preference = ThemePreference::from_stored(&storage.get(THEME_KEY));
    let scale = read_interface_scale(&storage.get(INTERFACE_SCALE_KEY)) as f32;
    let system_light = is_light(cx.window_appearance());
    let accent = theme::Accent::from_stored(&storage.get(volta_logic::storage::ACCENT_KEY));
    cx.set_global(ThemeState {
        preference,
        system_light,
        scale,
        tokens: ThemeState::resolve(preference, system_light, accent),
        font_family: theme::fonts::ui_font_family(cx),
        accent,
    });
    cx.set_global(GlobalStorage(storage.clone()));
    theme::apply(cx);
}

/// The window's root view: app state, shell, and gpui-component's `Root`.
pub(crate) fn build_root(storage: &Storage, window: &mut Window, cx: &mut App) -> Entity<gpui_component::Root> {
    let state = cx.new(|cx| AppState::new(storage.clone(), cx));
    // Offscreen screenshots have no OS session to publish to.
    if std::env::var_os("VOLTA_SHOT").is_none() {
        media_controls::start(&state, window, cx);
    }
    let shell = cx.new(|cx| Shell::new(state, window, cx));
    cx.new(|cx| gpui_component::Root::new(shell, window, cx))
}

fn main() {
    console::init_logger();
    let storage = Storage::open_default();
    if let Some(path) = storage.path() {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        log::info!("storage: {} ({} KB)", path.display(), size / 1024);
    }
    logs::mark("storage opened");

    #[cfg(feature = "shots")]
    if let Some(out) = std::env::var_os("VOLTA_SHOT") {
        if let Err(error) = shot::run(storage, out.into()) {
            eprintln!("shot failed: {error:#}");
            std::process::exit(1);
        }
        return;
    }

    app_icon::register_desktop_entry();
    application().with_assets(assets::Assets).run(move |cx: &mut App| {
        logs::mark("app running");
        init_globals(&storage, cx);
        logs::mark("globals ready");

        // Developer option: show the log console at launch. Opened first so
        // the main window ends up in front.
        if storage.get(volta_logic::storage::CONSOLE_AT_STARTUP_KEY) == "true" {
            tools::open_console(cx);
        }

        // Default 1440×900, like the canonical screenshot size; the web's
        // `min-width: 320px` is the minimum.
        let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(320.0), px(480.0))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Volta".into()),
                        // Windows: hide the native title bar; the app draws its own
                        // controls and drag area (components/window_chrome.rs).
                        // macOS: same, but the OS traffic lights stay.
                        appears_transparent: cfg!(any(target_os = "windows", target_os = "macos")),
                        traffic_light_position: cfg!(target_os = "macos").then(|| gpui::point(px(14.0), px(14.0))),
                        ..Default::default()
                    }),
                    app_id: Some(app_icon::APP_ID.into()),
                    icon: app_icon::window_icon(),
                    // Lets the rounded corners and frame shadow show the
                    // desktop behind them (Linux client-side decorations).
                    window_background: if cfg!(any(target_os = "linux", target_os = "freebsd")) {
                        gpui::WindowBackgroundAppearance::Transparent
                    } else {
                        Default::default()
                    },
                    // Linux: no compositor title bar; the app draws its own
                    // frame (components/window_chrome.rs). Windows hides the
                    // native title bar above and keeps the OS resize frame.
                    // macOS keeps its native title bar.
                    window_decorations: cfg!(any(target_os = "linux", target_os = "freebsd"))
                        .then_some(gpui::WindowDecorations::Client),
                    // gpui caps unfocused windows at ~30 fps by default; only
                    // keep that when the user opted in (Settings → Diagnostics).
                    // Read at window creation, so changes apply on next launch.
                    inactive_frame_interval: (storage.get(volta_logic::storage::BACKGROUND_FPS_LIMIT_KEY) == "true")
                        .then(|| std::time::Duration::from_micros(33_333)),
                    ..Default::default()
                },
                |window, cx| {
                    // "Match system" follows the OS appearance live.
                    window
                        .observe_window_appearance(|window, cx| {
                            let light = is_light(window.appearance());
                            let changed = {
                                let state = cx.global_mut::<ThemeState>();
                                let changed = state.system_light != light;
                                state.system_light = light;
                                state.refresh();
                                changed
                            };
                            if changed {
                                theme::apply(cx);
                            }
                        })
                        .detach();
                    let root = build_root(&storage, window, cx);
                    // `volta-warn-before-leave`: confirm closing while music plays.
                    window.on_window_should_close(cx, components::window_chrome::should_close);
                    root
                },
            )
            .expect("open window");
        logs::mark("window opened");
        {
            let displays: Vec<String> = cx
                .displays()
                .iter()
                .map(|d| {
                    let b = d.bounds();
                    format!("{}x{} at ({}, {})", f32::from(b.size.width), f32::from(b.size.height), f32::from(b.origin.x), f32::from(b.origin.y))
                })
                .collect();
            let (scale, size, appearance) = window
                .update(cx, |_, window, _| (window.scale_factor(), window.viewport_size(), window.appearance()))
                .unwrap_or((1.0, Default::default(), WindowAppearance::Dark));
            log::info!(
                "window: {}x{} at {scale}x scale, system appearance {appearance:?}; displays: {}",
                f32::from(size.width),
                f32::from(size.height),
                displays.join("; ")
            );
        }
        cx.on_app_quit(|_| async {
            log::info!("Volta is quitting");
            logs::flush();
        })
        .detach();
        // Tool windows (performance, console) belong to the main window.
        let main_id = window.window_id();
        tools::register_main(window.into(), cx);
        cx.on_window_closed(move |cx, id| {
            if id == main_id {
                tools::close_all(cx);
            }
        })
        .detach();
        cx.activate(true);
    });
}
