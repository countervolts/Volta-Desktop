//! Offscreen screenshots (feature `shots`).
//!
//! Renders the real window tree in gpui's `HeadlessAppContext` with the wgpu
//! headless renderer and writes the frame to a PNG, so parity screenshots do
//! not depend on a compositor (a nested KWin cannot read gpui's Vulkan
//! buffers, which captured as black).
//!
//! ```text
//! VOLTA_DATA_DIR=<scratch> VOLTA_SHOT=out.png [VOLTA_SHOT_SIZE=1440x900] \
//!   [VOLTA_SHOT_MS=4000] cargo run -p volta --features shots
//! ```
//!
//! `VOLTA_SHOT_MS` is simulated time; real time also passes in small sleeps
//! between steps so network and audio threads can make progress.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use gpui::{HeadlessAppContext, px, size};
use volta_services::Storage;

use crate::assets::Assets;

thread_local! {
    /// The app state, for `VOLTA_SHOT_SEEK`.
    pub static STATE: std::cell::RefCell<Option<gpui::WeakEntity<crate::app_state::AppState>>> = const { std::cell::RefCell::new(None) };
}

/// `VOLTA_SHOT_ROUTE=<page>[:<id>]` opens that route after sign-in.
pub fn initial_route() -> Option<crate::app_state::Route> {
    let value = std::env::var("VOLTA_SHOT_ROUTE").ok()?;
    let (page, id) = match value.split_once(':') {
        Some((page, id)) => (page.to_string(), Some(id.to_string())),
        None => (value, None),
    };
    let page = crate::app_state::Page::from_name(&page)?;
    Some(crate::app_state::Route { page, id, title: None })
}

fn parse_size() -> (f32, f32) {
    std::env::var("VOLTA_SHOT_SIZE")
        .ok()
        .and_then(|value| {
            let (w, h) = value.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((1440.0, 900.0))
}

pub fn run(storage: Storage, out: PathBuf) -> Result<()> {
    let (width, height) = parse_size();
    let total_ms: u64 = std::env::var("VOLTA_SHOT_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(4000);

    let text_system = Arc::new(gpui_wgpu::CosmicTextSystem::new("IBM Plex Sans"));
    let mut cx = HeadlessAppContext::with_platform(text_system, Arc::new(Assets), || {
        gpui_platform::current_headless_renderer()
    });
    cx.allow_parking();

    cx.update(|cx| crate::init_globals(&storage, cx));
    let handle = cx
        .open_window(size(px(width), px(height)), |window, cx| crate::build_root(&storage, window, cx))
        .context("open headless window")?;

    const STEP: u64 = 50;
    for _ in 0..total_ms / STEP {
        cx.advance_clock(Duration::from_millis(STEP));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }

    // `VOLTA_SHOT_SEEK=<seconds>`: pause at that position before capturing
    // (the web shots do the same), so time-dependent views line up.
    if let Some(seconds) = std::env::var("VOLTA_SHOT_SEEK").ok().and_then(|v| v.parse::<f64>().ok()) {
        cx.update(|cx| {
            if let Some(state) = STATE.with(|s| s.borrow().clone()).and_then(|w| w.upgrade()) {
                let player = state.read(cx).player.clone();
                player.update(cx, |p, cx| {
                    // `VOLTA_PLAYING=1` keeps playing (the lyric blur shows).
                    if std::env::var_os("VOLTA_PLAYING").is_none() {
                        p.pause(cx);
                    }
                    p.seek(seconds, cx);
                });
            }
        });
        for _ in 0..30 {
            cx.advance_clock(Duration::from_millis(STEP));
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let any = handle.into();
    // `VOLTA_SHOT_WHEEL=x,y,dx,dy,count[,precise]`: send `count` wheel notches of
    // (dx, dy) lines (or pixels, when `precise`) at (x, y) before capturing.
    if let Some(spec) = std::env::var("VOLTA_SHOT_WHEEL").ok() {
        let v: Vec<f32> = spec.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if let [x, y, dx, dy, count, ..] = v[..] {
            let precise = v.get(5).is_some_and(|p| *p > 0.0);
            for _ in 0..count as usize {
                cx.update_window(any, |_, window, cx| {
                    let delta = if precise {
                        gpui::ScrollDelta::Pixels(gpui::point(px(dx), px(dy)))
                    } else {
                        gpui::ScrollDelta::Lines(gpui::point(dx, dy))
                    };
                    let event = gpui::ScrollEvent {
                        position: gpui::point(px(x), px(y)),
                        delta,
                        modifiers: Default::default(),
                        touch_phase: gpui::TouchPhase::Moved,
                        source: gpui::ScrollSource::Pointer,
                    };
                    window.dispatch_event(gpui::InputEvent::to_platform_input(event), cx);
                })?;
                for _ in 0..3 {
                    cx.advance_clock(Duration::from_millis(STEP));
                    cx.run_until_parked();
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            for _ in 0..30 {
                cx.advance_clock(Duration::from_millis(STEP));
                cx.run_until_parked();
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    // `VOLTA_PROFILE=1`: CPU time of a full redraw (build, layout, paint)
    // of the final state, to catch pages that blow the 16 ms frame budget.
    if std::env::var_os("VOLTA_PROFILE").is_some() {
        for _ in 0..5 {
            cx.update_window(any, |_, window, cx| {
                let t = std::time::Instant::now();
                window.refresh();
                window.draw(cx).clear(cx);
                eprintln!("PROFILE draw {:?}", t.elapsed());
            })?;
        }
    }
    cx.update_window(any, |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    })?;
    let mut image = cx.capture_screenshot(any)?;
    // The headless renderer draws at the test display's scale factor; save
    // at CSS-pixel size so native shots line up 1:1 with the web shots.
    let (w, h) = (width as u32, height as u32);
    if image.width() != w || image.height() != h {
        image = image::imageops::resize(&image, w, h, image::imageops::FilterType::Triangle);
    }
    image.save(&out).with_context(|| format!("write {}", out.display()))?;
    println!("ok {} {}x{}", out.display(), image.width(), image.height());
    Ok(())
}
