//! The Stats charts, drawn the way the web's recharts setup renders them:
//! a monotone area chart with an accent gradient, a bar chart with 3px top
//! corners (max 32px wide), and a donut with 2° padding. Axes: dashed
//! horizontal grid, integer Y ticks, X labels thinned like
//! `interval="preserveStartEnd"`.

use gpui::{
    AnyElement, Bounds, Hsla, IntoElement, ParentElement, PathBuilder, Pixels, Point, Styled, canvas, div, linear_color_stop,
    linear_gradient, point, px,
};

use crate::theme::{self, Tokens, sz};

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// How long a chart takes to draw itself in.
const ENTRANCE_MS: f32 = 850.0;

thread_local! {
    /// Per chart id: the data signature it last drew and when that drawing began.
    static ENTRANCES: RefCell<HashMap<String, (u64, Instant)>> = RefCell::new(HashMap::new());
}

/// Bumped whenever the stats page is opened, so its charts draw in again.
static VISIT: AtomicU64 = AtomicU64::new(0);

pub fn replay_entrances() {
    VISIT.fetch_add(1, Ordering::Relaxed);
}

fn signature(values: &[f64]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    VISIT.load(Ordering::Relaxed).hash(&mut hasher);
    for v in values {
        v.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

/// 0 → 1 over [`ENTRANCE_MS`], restarting when the chart's data changes or the
/// page is reopened. Asks for frames while moving; settled at 1 when the user
/// prefers reduced motion.
fn entrance(id: &str, values: &[f64], window: &gpui::Window, cx: &gpui::App) -> f32 {
    if cx.reduce_motion() {
        return 1.0;
    }
    let sig = signature(values);
    let t = ENTRANCES.with(|map| {
        let mut map = map.borrow_mut();
        let entry = map.entry(id.to_string()).or_insert_with(|| (sig, Instant::now()));
        if entry.0 != sig {
            *entry = (sig, Instant::now());
        }
        entry.1.elapsed().as_secs_f32() * 1000.0 / ENTRANCE_MS
    });
    if t < 1.0 {
        window.request_animation_frame();
    }
    t.min(1.0)
}

/// Fast start, gentle landing.
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Recharts-like integer ticks from 0: 5 ticks with a 1/2/5 × 10ⁿ step.
pub fn nice_ticks(max: f64) -> Vec<f64> {
    let rough = (max / 4.0).max(0.0);
    let mut step = 1.0;
    if rough > 1.0 {
        let magnitude = 10f64.powf(rough.log10().floor());
        step = [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * magnitude).find(|s| *s >= rough).unwrap_or(10.0 * magnitude).ceil();
    }
    (0..5).map(|i| i as f64 * step).collect()
}

/// Indices of X labels to show: first and last always, others when they fit.
fn visible_labels(labels: &[String], width: f32) -> Vec<usize> {
    if labels.is_empty() {
        return Vec::new();
    }
    let count = labels.len();
    let slot = width / count as f32;
    let widest = labels.iter().map(|l| l.chars().count()).max().unwrap_or(1) as f32 * 6.0 + 8.0;
    let every = ((widest / slot.max(1.0)).ceil() as usize).max(1);
    let mut picked: Vec<usize> = (0..count).step_by(every).collect();
    if !picked.contains(&(count - 1)) {
        if picked.last().is_some_and(|&last| count - 1 - last < every) && picked.len() > 1 {
            picked.pop();
        }
        picked.push(count - 1);
    }
    picked
}

/// d3 `curveMonotoneX` control points for a series.
fn monotone(points: &[Point<Pixels>]) -> Vec<(Point<Pixels>, Point<Pixels>, Point<Pixels>)> {
    let n = points.len();
    if n < 2 {
        return Vec::new();
    }
    let xs: Vec<f32> = points.iter().map(|p| f32::from(p.x)).collect();
    let ys: Vec<f32> = points.iter().map(|p| f32::from(p.y)).collect();
    let slopes: Vec<f32> = (0..n - 1).map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]).max(1e-6)).collect();
    let mut tangents = vec![0.0f32; n];
    tangents[0] = slopes[0];
    tangents[n - 1] = slopes[n - 2];
    for i in 1..n - 1 {
        let (a, b) = (slopes[i - 1], slopes[i]);
        tangents[i] = if a * b <= 0.0 {
            0.0
        } else {
            let (h0, h1) = (xs[i] - xs[i - 1], xs[i + 1] - xs[i]);
            let p = (a * h1 + b * h0) / (h0 + h1);
            let s = (a.abs().min(b.abs()) * 2.0).min(p.abs());
            s.copysign(p)
        };
    }
    (0..n - 1)
        .map(|i| {
            let dx = (xs[i + 1] - xs[i]) / 3.0;
            (
                point(px(xs[i] + dx), px(ys[i] + dx * tangents[i])),
                point(px(xs[i + 1] - dx), px(ys[i + 1] - dx * tangents[i + 1])),
                points[i + 1],
            )
        })
        .collect()
}

const AXIS_WIDTH: f32 = 42.0;
const AXIS_HEIGHT: f32 = 22.0;

/// The shared frame: Y labels, X labels, and the plot drawn by `paint`.
fn frame(
    labels: Vec<String>,
    ticks: Vec<f64>,
    width: f32,
    height: f32,
    tokens: &Tokens,
    paint: impl Fn(Bounds<Pixels>, f64, &mut gpui::Window, &gpui::App) + 'static,
) -> AnyElement {
    let plot_w = (width - AXIS_WIDTH - 6.0).max(1.0);
    let plot_h = (height - AXIS_HEIGHT - 8.0).max(1.0);
    let top = *ticks.last().unwrap_or(&1.0);
    let line = tokens.line;
    let grid_ticks = ticks.clone();
    let shown = visible_labels(&labels, plot_w);
    let count = labels.len().max(1) as f32;
    let mut root = div().relative().w(sz(width)).h(sz(height)).text_size(sz(10.0)).text_color(tokens.muted);
    for tick in &ticks {
        let y = 8.0 + plot_h - (*tick / top.max(1e-9)) as f32 * plot_h;
        root = root.child(div().absolute().left_0().w(sz(AXIS_WIDTH - 6.0)).top(sz(y - 6.0)).flex().justify_end().child(format!("{tick}")));
    }
    for index in shown {
        let x = AXIS_WIDTH + plot_w * (index as f32 + 0.5) / count;
        root = root.child(div().absolute().top(sz(8.0 + plot_h + 6.0)).left(sz(x - 20.0)).w(sz(40.0)).flex().justify_center().child(labels[index].clone()));
    }
    let scale = |window: &gpui::Window| f32::from(window.rem_size()) / 16.0;
    root.child(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                let s = scale(window);
                let plot = Bounds {
                    origin: point(bounds.origin.x + px(AXIS_WIDTH * s), bounds.origin.y + px(8.0 * s)),
                    size: gpui::size(px(plot_w * s), px(plot_h * s)),
                };
                for tick in &grid_ticks {
                    let y = plot.origin.y + plot.size.height * (1.0 - (*tick / top.max(1e-9)) as f32);
                    let mut builder = PathBuilder::stroke(px(1.0)).dash_array(&[px(2.0), px(4.0)]);
                    builder.move_to(point(plot.origin.x, y));
                    builder.line_to(point(plot.origin.x + plot.size.width, y));
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, line);
                    }
                }
                // recharts' default axis lines (`stroke="#666"`).
                let axis = theme::hex("#666666");
                let mut axes = PathBuilder::stroke(px(1.0));
                axes.move_to(plot.origin);
                axes.line_to(point(plot.origin.x, plot.origin.y + plot.size.height));
                axes.line_to(point(plot.origin.x + plot.size.width, plot.origin.y + plot.size.height));
                if let Ok(path) = axes.build() {
                    window.paint_path(path, axis);
                }
                paint(plot, top, window, cx);
            },
        )
        .absolute()
        .inset_0(),
    )
    .into_any_element()
}

/// "Plays by …": monotone area with an accent gradient (0.48 → 0) and a 2px line.
pub fn area_chart(id: &'static str, labels: Vec<String>, values: Vec<f64>, width: f32, height: f32, tokens: &Tokens) -> AnyElement {
    let ticks = nice_ticks(values.iter().cloned().fold(0.0, f64::max));
    let accent = tokens.accent;
    frame(labels, ticks, width, height, tokens, move |plot, top, window, cx| {
        let n = values.len();
        if n == 0 {
            return;
        }
        // The line and its fill rise from the baseline together.
        let rise = ease_out(entrance(id, &values, window, cx));
        let x = |i: usize| plot.origin.x + plot.size.width * ((i as f32 + 0.5) / n as f32);
        let y = |v: f64| plot.origin.y + plot.size.height * (1.0 - (v / top.max(1e-9)) as f32 * rise);
        let points: Vec<Point<Pixels>> = values.iter().enumerate().map(|(i, v)| point(x(i), y(*v))).collect();
        let segments = monotone(&points);
        let bottom = plot.origin.y + plot.size.height;
        let mut fill = PathBuilder::fill();
        fill.move_to(point(points[0].x, bottom));
        fill.line_to(points[0]);
        for (a, b, to) in &segments {
            fill.cubic_bezier_to(*to, *a, *b);
        }
        fill.line_to(point(points[n - 1].x, bottom));
        fill.close();
        if let Ok(path) = fill.build() {
            window.paint_path(path, linear_gradient(180.0, linear_color_stop(accent.opacity(0.48), 0.05), linear_color_stop(accent.opacity(0.0), 0.95)));
        }
        let mut stroke = PathBuilder::stroke(px(2.0));
        stroke.move_to(points[0]);
        for (a, b, to) in &segments {
            stroke.cubic_bezier_to(*to, *a, *b);
        }
        if let Ok(path) = stroke.build() {
            window.paint_path(path, accent);
        }
    })
}

/// "Listening time": bars at most 32px wide with 3px top corners.
pub fn bar_chart(id: &'static str, labels: Vec<String>, values: Vec<f64>, width: f32, height: f32, tokens: &Tokens) -> AnyElement {
    let ticks = nice_ticks(values.iter().cloned().fold(0.0, f64::max));
    let accent = tokens.accent;
    frame(labels, ticks, width, height, tokens, move |plot, top, window, cx| {
        let n = values.len().max(1);
        let progress = entrance(id, &values, window, cx);
        // Bars grow left to right: each starts a little after the one before it.
        const STAGGER: f32 = 0.45;
        let band = plot.size.width / n as f32;
        // recharts: bar size is 90 % of the band (10 % gap), capped at 32px.
        let s = f32::from(window.rem_size()) / 16.0;
        let bar = (band * 0.8).min(px(32.0 * s));
        for (i, value) in values.iter().enumerate() {
            if *value <= 0.0 {
                continue;
            }
            let local = ((progress - STAGGER * i as f32 / n as f32) / (1.0 - STAGGER)).clamp(0.0, 1.0);
            let h = plot.size.height * (*value / top.max(1e-9)) as f32 * ease_out(local);
            if h < px(0.5) {
                continue;
            }
            let left = plot.origin.x + band * i as f32 + (band - bar) / 2.0;
            let bottom = plot.origin.y + plot.size.height;
            let r = px(3.0 * s).min(h).min(bar / 2.0);
            let mut path = PathBuilder::fill();
            path.move_to(point(left, bottom));
            path.line_to(point(left, bottom - h + r));
            path.curve_to(point(left + r, bottom - h), point(left, bottom - h));
            path.line_to(point(left + bar - r, bottom - h));
            path.curve_to(point(left + bar, bottom - h + r), point(left + bar, bottom - h));
            path.line_to(point(left + bar, bottom));
            path.close();
            if let Ok(path) = path.build() {
                window.paint_path(path, accent);
            }
        }
    })
}

/// "Genre breakdown": donut, inner 54 %, outer 78 %, 2° padding.
pub fn donut(id: String, values: Vec<f64>, colors: Vec<Hsla>, size: f32) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let total: f64 = values.iter().sum();
            if total <= 0.0 {
                return;
            }
            // The ring sweeps round to its full size.
            let sweep_in = ease_out(entrance(&id, &values, window, cx));
            let center = bounds.center();
            let radius = f32::from(bounds.size.width.min(bounds.size.height)) / 2.0;
            let (outer, inner) = (radius * 0.78, radius * 0.54);
            let base_padding = if values.len() > 1 { 2f32.to_radians() } else { 0.0 };
            let padding = base_padding * sweep_in;
            let available = (std::f32::consts::TAU - base_padding * values.len() as f32) * sweep_in;
            // recharts starts at 0° (3 o'clock) and runs counter-clockwise.
            let mut angle = 0.0f32;
            for (i, value) in values.iter().enumerate() {
                let sweep = available * (*value / total) as f32;
                let (start, end) = (angle, angle + sweep);
                let at = |r: f32, a: f32| point(center.x + px(r * a.cos()), center.y - px(r * a.sin()));
                let mut path = PathBuilder::fill();
                let steps = ((sweep / 0.05).ceil() as usize).max(2);
                path.move_to(at(outer, start));
                for k in 1..=steps {
                    path.line_to(at(outer, start + sweep * k as f32 / steps as f32));
                }
                for k in (0..=steps).rev() {
                    path.line_to(at(inner, start + sweep * k as f32 / steps as f32));
                }
                path.close();
                if let Ok(path) = path.build() {
                    window.paint_path(path, colors[i % colors.len()]);
                }
                angle = end + padding;
            }
        },
    )
    .size(sz(size))
    .into_any_element()
}

/// `GRAPH_COLORS`.
pub fn graph_colors() -> Vec<Hsla> {
    let mut colors = vec![theme::accent_on_dark()];
    colors.extend(["#d9899d", "#b79bdf", "#79b6c9", "#d7b36d", "#86b987", "#dd8d66", "#8ea8d8"].iter().map(|c| theme::hex(c)));
    colors
}

#[cfg(test)]
mod tests {
    #[test]
    fn ticks_are_integer_and_cover_the_max() {
        assert_eq!(super::nice_ticks(0.0), vec![0.0, 1.0, 2.0, 3.0, 4.0]);
        assert_eq!(super::nice_ticks(3.0), vec![0.0, 1.0, 2.0, 3.0, 4.0]);
        assert_eq!(super::nice_ticks(9.0), vec![0.0, 5.0, 10.0, 15.0, 20.0]);
    }
}
