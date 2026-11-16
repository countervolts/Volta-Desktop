//! `PerformanceOverlay` (diagnostics only, `volta-frame-monitor`).
//!
//! Frame delivery is measured exactly as the web does it (a 120-frame window
//! of frame gaps, refresh estimated from the most common gap, 500 ms report).
//! Browser-only metrics (TTFB, DOM nodes, JS heap, resource timing) are
//! replaced by their native counterparts: resident memory, decoded artwork,
//! data on disk, audio underruns, threads.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Context, FontWeight, IntoElement, ParentElement, Render, Styled, Window, div, prelude::*};
use parking_lot::Mutex;

use crate::app_state::AppState;
use crate::theme::{self, mix, sz, transparent};

const WINDOW_SIZE: usize = 120;
const REFRESH_MS: u128 = 500;
const SLOW_FACTOR: f32 = 1.5;
const SUSPEND_MS: f32 = 1000.0;
const FALLBACK_INTERVAL_MS: f32 = 1000.0 / 60.0;

/// Frame gaps of the main window, shared between whoever measures them (the
/// in-window overlay samples itself; the performance window is fed by
/// [`measure_frames`] on the main window) and whoever shows them.
#[derive(Default)]
struct Feed {
    samples: Vec<f32>,
    last: Option<Instant>,
    viewport: String,
    running: bool,
}

#[derive(Clone, Default)]
pub struct SharedFeed(Arc<Mutex<Feed>>);

impl SharedFeed {
    /// Records the gap since the previous frame of `window`.
    fn tick(&self, window: &Window) {
        let now = Instant::now();
        let mut feed = self.0.lock();
        if let Some(last) = feed.last {
            let delta = now.duration_since(last).as_secs_f32() * 1000.0;
            if delta > 0.0 && delta < SUSPEND_MS {
                if feed.samples.len() == WINDOW_SIZE {
                    feed.samples.remove(0);
                }
                feed.samples.push(delta);
            }
        }
        feed.last = Some(now);
        let size = crate::theme::content_size(window);
        feed.viewport = format!("{}×{} · {}x", f32::from(size.width).round(), f32::from(size.height).round(), window.scale_factor());
    }
}

/// Where the metrics are shown.
#[derive(Clone, Copy, PartialEq)]
enum Placement {
    /// Floating panel over the main window; samples that window itself.
    Overlay,
    /// Its own native window; samples come from [`measure_frames`].
    Window,
}

pub struct PerfOverlay {
    state: gpui::Entity<AppState>,
    placement: Placement,
    feed: SharedFeed,
    samples: Vec<f32>,
    started: Instant,
    reported: Instant,
    interval: f32,
    report: Report,
}

/// Measures `window`'s frame delivery into `feed` after every frame, for as
/// long as the performance window is open. Asking for the next frame keeps the
/// window presenting without redrawing anything itself.
pub fn measure_frames(feed: SharedFeed, window: &mut Window, cx: &mut gpui::App) {
    if !crate::tools::performance_window_open(cx) {
        feed.0.lock().running = false;
        return;
    }
    feed.tick(window);
    window.on_next_frame(move |window, cx| measure_frames(feed, window, cx));
}

impl SharedFeed {
    /// Starts [`measure_frames`] unless it is already running.
    pub fn start(&self, window: &mut Window, cx: &mut gpui::App) {
        let mut feed = self.0.lock();
        if !std::mem::replace(&mut feed.running, true) {
            feed.last = None;
            drop(feed);
            measure_frames(self.clone(), window, cx);
        }
    }
}

#[derive(Default, Clone)]
struct Report {
    fps: String,
    budget: String,
    low: String,
    refresh: String,
    average: String,
    p95: String,
    worst: String,
    bars: Vec<(f32, bool)>,
    baseline: f32,
    route: String,
    uptime: String,
    memory: String,
    artwork: String,
    storage: String,
    underruns: String,
    connection: String,
    device: String,
    viewport: String,
}

fn ms(value: f32, digits: usize) -> String {
    format!("{value:.digits$} ms")
}

fn bytes(value: u64) -> String {
    if value < 1024 * 1024 {
        format!("{} KB", (value as f64 / 1024.0).round())
    } else {
        format!("{:.1} MB", value as f64 / (1024.0 * 1024.0))
    }
}

/// `estimateInterval`: the most common gap, if it covers ≥10% of samples.
fn estimate_interval(samples: &[f32]) -> Option<(f32, f32)> {
    if samples.len() < 12 {
        return None;
    }
    let mut buckets: std::collections::BTreeMap<i64, (usize, f32)> = Default::default();
    for &gap in samples {
        let entry = buckets.entry((gap * 2.0).round() as i64).or_default();
        entry.0 += 1;
        entry.1 += gap;
    }
    let (_, (count, total)) = buckets.iter().max_by(|a, b| a.1.0.cmp(&b.1.0).then(b.0.cmp(a.0)))?;
    if (*count as f32) < samples.len() as f32 * 0.1 {
        return None;
    }
    let interval = total / *count as f32;
    let late = samples.iter().filter(|g| **g > interval * SLOW_FACTOR).count();
    Some((interval, 1.0 - late as f32 / samples.len() as f32))
}

/// Resident memory of this process, on every desktop OS.
#[cfg(target_os = "linux")]
fn resident_memory() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: `sysconf` has no preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    Some(pages * if page > 0 { page as u64 } else { 4096 })
}

#[cfg(target_os = "windows")]
fn resident_memory() -> Option<u64> {
    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut ProcessMemoryCounters, size: u32) -> i32;
    }
    // SAFETY: the struct matches PROCESS_MEMORY_COUNTERS and `cb` is its size.
    unsafe {
        let mut counters: ProcessMemoryCounters = std::mem::zeroed();
        counters.cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
        (K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) != 0).then_some(counters.working_set_size as u64)
    }
}

#[cfg(target_os = "macos")]
fn resident_memory() -> Option<u64> {
    /// `mach_task_basic_info`.
    #[repr(C)]
    struct TaskBasicInfo {
        virtual_size: u64,
        resident_size: u64,
        resident_size_max: u64,
        user_time: [i32; 2],
        system_time: [i32; 2],
        policy: i32,
        suspend_count: i32,
    }
    const MACH_TASK_BASIC_INFO: u32 = 20;
    unsafe extern "C" {
        fn mach_task_self() -> u32;
        fn task_info(task: u32, flavor: u32, info: *mut TaskBasicInfo, count: *mut u32) -> i32;
    }
    // SAFETY: `info` is a `mach_task_basic_info` and `count` is its size in 32-bit words.
    unsafe {
        let mut info: TaskBasicInfo = std::mem::zeroed();
        let mut count = (std::mem::size_of::<TaskBasicInfo>() / 4) as u32;
        (task_info(mach_task_self(), MACH_TASK_BASIC_INFO, &mut info, &mut count) == 0).then_some(info.resident_size)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn resident_memory() -> Option<u64> {
    None
}

fn dir_size(path: &std::path::Path) -> u64 {
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| match e.metadata() {
                    Ok(m) if m.is_dir() => dir_size(&e.path()),
                    Ok(m) => m.len(),
                    Err(_) => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

impl PerfOverlay {
    fn with(state: gpui::Entity<AppState>, placement: Placement, feed: SharedFeed) -> Self {
        let now = Instant::now();
        Self {
            state,
            placement,
            feed,
            samples: Vec::with_capacity(WINDOW_SIZE),
            started: now,
            reported: now,
            interval: FALLBACK_INTERVAL_MS,
            report: Report { fps: "—".into(), refresh: "measuring…".into(), ..Default::default() },
        }
    }

    /// The floating panel inside the main window.
    pub fn new(state: gpui::Entity<AppState>) -> Self {
        Self::with(state, Placement::Overlay, SharedFeed::default())
    }

    /// The contents of the performance window, fed by [`measure_frames`].
    pub fn windowed(state: gpui::Entity<AppState>, feed: SharedFeed, cx: &mut Context<Self>) -> Self {
        // Nothing animates here, so refresh on a timer instead of every frame.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(REFRESH_MS as u64)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self::with(state, Placement::Window, feed)
    }

    fn sample(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.placement == Placement::Overlay {
            self.feed.tick(window);
        }
        let now = Instant::now();
        if now.duration_since(self.reported).as_millis() >= REFRESH_MS {
            self.reported = now;
            self.build_report(cx);
        }
    }

    fn build_report(&mut self, cx: &mut Context<Self>) {
        {
            let feed = self.feed.0.lock();
            self.samples.clone_from(&feed.samples);
            self.report.viewport.clone_from(&feed.viewport);
        }
        let estimate = estimate_interval(&self.samples);
        if let Some((interval, _)) = estimate {
            self.interval = interval;
        }
        let r = &mut self.report;
        if !self.samples.is_empty() {
            let filled = self.samples.len();
            let average = self.samples.iter().sum::<f32>() / filled as f32;
            let worst = self.samples.iter().cloned().fold(0.0, f32::max);
            let mut ranked = self.samples.clone();
            ranked.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let low = ranked[filled - ((filled as f32 * 0.05).round() as usize).max(1)];
            r.fps = format!("{} fps", (1000.0 / average).round());
            r.average = ms(average, 1);
            r.p95 = ms(ranked[(filled as f32 * 0.95).floor().min(filled as f32 - 1.0) as usize], 1);
            r.worst = ms(worst, 0);
            r.low = format!("{} fps", (1000.0 / low.max(0.01)).round());
            r.budget = format!("{}% on time", (estimate.map_or(0.0, |e| e.1) * 100.0).round());
            r.refresh = estimate.map_or("measuring…".into(), |e| format!("{} Hz", (1000.0 / e.0).round()));
            let scale = worst.max(self.interval * 2.0);
            r.bars = self.samples.iter().map(|s| ((s / scale).min(1.0), *s > self.interval * SLOW_FACTOR)).collect();
            r.baseline = (self.interval / scale).min(1.0);
        }
        let s = self.state.read(cx);
        let route = s.route();
        r.route = format!("{}{}", route.page.name(), route.id.as_deref().map(|id| format!(" · {id}")).unwrap_or_default());
        r.uptime = ms(self.started.elapsed().as_millis() as f32, 0);
        r.memory = resident_memory().map(bytes).unwrap_or_else(|| "not reported".into());
        r.artwork = format!("{} images decoded", s.covers.len());
        r.storage = volta_services::storage::data_dir().map(|d| bytes(dir_size(&d))).unwrap_or_else(|| "not reported".into());
        r.underruns = s.player.read(cx).underruns().to_string();
        r.connection = match s.connection {
            crate::components::connection_banner::Connection::Online => "online",
            crate::components::connection_banner::Connection::Offline => "offline",
            crate::components::connection_banner::Connection::Reconnecting => "reconnecting",
        }
        .into();
        r.device = format!("{} threads", std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));
    }
}

impl Render for PerfOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sample(window, cx);
        // `requestAnimationFrame` loop: keep sampling frames while shown.
        if self.placement == Placement::Overlay {
            window.request_animation_frame();
        }
        let overlay = self.placement == Placement::Overlay;
        let tokens = theme::tokens(cx).clone();
        let mono = theme::fonts::mono_font_family(cx);
        let r = self.report.clone();
        let metric = |label: &str, value: String| {
            div()
                .flex()
                .items_baseline()
                .justify_between()
                .gap(sz(14.0))
                .min_w_0()
                .child(div().whitespace_nowrap().text_color(tokens.muted).child(label.to_string()))
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(sz(9.0))
                        .font_family(mono.clone())
                        .text_color(tokens.text)
                        .child(value),
                )
        };
        let key = |label: &str, value: String| {
            div()
                .flex()
                .flex_col()
                .gap(sz(2.0))
                .min_w_0()
                .child(div().text_size(sz(9.0)).text_color(tokens.muted).child(label.to_string()))
                .child(div().text_size(sz(12.0)).font_weight(FontWeight::BOLD).child(value))
        };
        let pair = |a: gpui::Div, b: gpui::Div| div().flex().gap(sz(16.0)).child(a.flex_1().flex_basis(sz(0.0))).child(b.flex_1().flex_basis(sz(0.0)));
        let section = |title: &str| {
            div()
                .flex()
                .flex_col()
                .gap(sz(6.0))
                .py(sz(11.0))
                .border_b(sz(1.0))
                .border_color(tokens.line)
                .child(div().text_size(sz(10.0)).font_weight(FontWeight::BOLD).letter_spacing(sz(10.0 * 0.08)).text_color(tokens.secondary).child(title.to_uppercase()))
        };
        let chart = div()
            .relative()
            .h(sz(60.0))
            .w_full()
            .overflow_hidden()
            .border(sz(1.0))
            .border_color(tokens.line)
            .rounded(sz(10.0))
            .bg(mix(tokens.canvas, 62.0, transparent()))
            .flex()
            .items_end()
            .gap(sz(0.5))
            .children(r.bars.iter().map(|(height, slow)| {
                div().flex_1().h(gpui::relative(height.max(1.0 / 60.0))).bg(theme::hex(if *slow { "#ff7b72" } else { "#7ee787" }))
            }))
            .child(div().absolute().left_0().right_0().bottom(gpui::relative(r.baseline)).h(sz(1.0)).bg(theme::hex("#ffffff40")));
        div()
            .flex()
            .flex_col()
            .overflow_hidden()
            .map(|d| {
                if overlay {
                    d.absolute()
                        .top(sz(54.0))
                        .right(sz(12.0))
                        .bottom(sz(54.0))
                        .w(sz(348.0))
                        .border(sz(1.0))
                        .border_color(mix(tokens.text, 20.0, transparent()))
                        .rounded(sz(20.0))
                        .bg(mix(tokens.sidebar, 97.0, tokens.canvas))
                        .shadow(vec![gpui::BoxShadow {
                            color: theme::hex("#0000002b"),
                            offset: gpui::point(gpui::px(0.0), gpui::px(12.0)),
                            blur_radius: gpui::px(34.0),
                            spread_radius: gpui::px(0.0),
                            inset: false,
                        }])
                } else {
                    d.size_full().bg(tokens.canvas)
                }
            })
            .text_color(tokens.text)
            .text_size(sz(11.0))
            .line_height(gpui::relative(1.4))
            .font_features(theme::fonts::tabular_nums())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .min_h(sz(58.0))
                    .px(sz(18.0))
                    .border_b(sz(1.0))
                    .border_color(tokens.line)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sz(10.0))
                            .child(div().size(sz(9.0)).rounded_full().bg(theme::hex("#4cd47a")).border(sz(4.0)).border_color(theme::hex("#4cd47a1f")))
                            .child(
                                div()
                                    .child(div().text_size(sz(14.0)).font_weight(FontWeight::BOLD).child("Performance inspector"))
                                    .child(div().text_size(sz(10.0)).text_color(tokens.muted).child("Diagnostics")),
                            ),
                    )
                    .child(div().max_w(sz(110.0)).text_size(sz(10.0)).text_color(tokens.muted).child(if overlay { "live · 500 ms sample" } else { "main window · 500 ms" })),
            )
            .child(
                div()
                    .id("perf-body")
                    .overflow_y_scroll()
                    .px(sz(18.0))
                    .pt(sz(3.0))
                    .pb(sz(12.0))
                    .child(
                        section("Frame delivery")
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(sz(8.0))
                                    .py(sz(8.0))
                                    .px(sz(12.0))
                                    .rounded(sz(12.0))
                                    .bg(tokens.row)
                                    .child(pair(key("FPS", r.fps.clone()), key("On budget", r.budget.clone())))
                                    .child(pair(key("5% low", r.low.clone()), key("Refresh", r.refresh.clone()))),
                            )
                            .child(chart)
                            .child(pair(metric("Frame average", r.average.clone()), metric("Frame p95", r.p95.clone())))
                            .child(pair(metric("Worst frame", r.worst.clone()), div())),
                    )
                    .child(section("App").child(metric("Route", r.route.clone())).child(metric("Session uptime", r.uptime.clone())))
                    .child(
                        section("Runtime")
                            .child(metric("Resident memory", r.memory.clone()))
                            .child(metric("Artwork", r.artwork.clone()))
                            .child(metric("Data on disk", r.storage.clone()))
                            .child(metric("Audio underruns", r.underruns.clone())),
                    )
                    .child(section("Network").child(metric("Server", r.connection.clone())))
                    .child(
                        section("Environment")
                            .border_b_0()
                            .child(metric("Device hint", r.device.clone()))
                            .child(metric("Viewport", r.viewport.clone())),
                    ),
            )
    }
}
