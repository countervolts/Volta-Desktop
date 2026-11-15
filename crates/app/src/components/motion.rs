//! Motion primitives matching `theme.css` keyframes and easing curves.

use std::time::Duration;

use gpui::{Animation, AnimationExt, IntoElement, Transformation, percentage};

use crate::components::ui::icon;

/// `cubic-bezier(x1, y1, x2, y2)` as an easing function (Newton + bisection on x).
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32) -> impl Fn(f32) -> f32 + Clone {
    move |t: f32| {
        let t = t.clamp(0.0, 1.0);
        let bezier = |a: f32, b: f32, s: f32| {
            let u = 1.0 - s;
            3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
        };
        let derivative = |a: f32, b: f32, s: f32| {
            let u = 1.0 - s;
            3.0 * u * u * a + 6.0 * u * s * (b - a) + 3.0 * s * s * (1.0 - b)
        };
        let mut s = t;
        for _ in 0..8 {
            let x = bezier(x1, x2, s) - t;
            let d = derivative(x1, x2, s);
            if x.abs() < 1e-5 {
                return bezier(y1, y2, s);
            }
            if d.abs() < 1e-6 {
                break;
            }
            s = (s - x / d).clamp(0.0, 1.0);
        }
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        s = t;
        for _ in 0..30 {
            let x = bezier(x1, x2, s);
            if (x - t).abs() < 1e-5 {
                break;
            }
            if x < t { lo = s } else { hi = s }
            s = (lo + hi) / 2.0;
        }
        bezier(y1, y2, s)
    }
}

/// A CSS `transition` between an off (0) and on (1) state. Reading it gives
/// the eased value now and whether it is still moving (then request a frame).
pub struct Transition {
    from: f32,
    to: f32,
    started: std::time::Instant,
    duration: Duration,
    /// `cubic-bezier(x1, y1, x2, y2)`; CSS `ease` by default.
    curve: [f32; 4],
}

impl Transition {
    pub fn new(duration_ms: u64) -> Self {
        Self::new_at(duration_ms, false)
    }

    /// Starts settled in the given state (no animation on first show).
    pub fn new_at(duration_ms: u64, on: bool) -> Self {
        let at = if on { 1.0 } else { 0.0 };
        Self { from: at, to: at, started: std::time::Instant::now(), duration: Duration::from_millis(duration_ms), curve: [0.25, 0.1, 0.25, 1.0] }
    }

    pub fn with_curve(mut self, x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        self.curve = [x1, y1, x2, y2];
        self
    }

    /// Retargets; an interrupted transition reverses from where it is.
    pub fn set(&mut self, on: bool) {
        let to = if on { 1.0 } else { 0.0 };
        if to != self.to {
            self.from = self.linear();
            self.to = to;
            self.started = std::time::Instant::now();
        }
    }

    fn linear(&self) -> f32 {
        if reduced() {
            return self.to;
        }
        let span = (self.to - self.from).abs();
        if span == 0.0 {
            return self.to;
        }
        let total = self.duration.as_secs_f32() * span;
        let t = (self.started.elapsed().as_secs_f32() / total.max(1e-3)).min(1.0);
        self.from + (self.to - self.from) * t
    }

    /// `(eased value, still animating)`. Eased along the curve in the
    /// direction of travel, so reversing mirrors it like CSS.
    pub fn value(&self) -> (f32, bool) {
        let linear = self.linear();
        let [x1, y1, x2, y2] = self.curve;
        let eased = if self.to >= self.from {
            cubic_bezier(x1, y1, x2, y2)(linear)
        } else {
            1.0 - cubic_bezier(x1, y1, x2, y2)(1.0 - linear)
        };
        (eased, linear != self.to)
    }
}

/// CSS `ease` (kept for the easing tests).
#[cfg(test)]
pub fn ease() -> impl Fn(f32) -> f32 + Clone {
    cubic_bezier(0.25, 0.1, 0.25, 1.0)
}

/// `.spin { animation: spin 1s linear infinite }` on `LoaderCircle`.
pub fn spinner(size: f32) -> gpui::AnyElement {
    if reduced() {
        return icon("loader-circle", size).into_any_element();
    }
    icon("loader-circle", size).with_animation(
        "spin",
        Animation::new(Duration::from_secs(1)).repeat(),
        |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bezier_endpoints_and_ease_midpoint() {
        let ease = ease();
        assert!(ease(0.0).abs() < 1e-4);
        assert!((ease(1.0) - 1.0).abs() < 1e-4);
        // CSS `ease` at 50 % time is ~80.2 % progress.
        assert!((ease(0.5) - 0.8024).abs() < 2e-3);
    }
}

/// `prefers-reduced-motion: reduce`, read once from the OS (GNOME
/// `enable-animations`, KDE `AnimationDurationFactor`, macOS
/// `reduceMotion`, Windows `MinAnimate`). `VOLTA_REDUCED_MOTION=1|0`
/// overrides it; parity screenshots reduce motion, as the web shots do.
pub fn reduced() -> bool {
    static REDUCED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *REDUCED.get_or_init(|| {
        if let Some(value) = std::env::var_os("VOLTA_REDUCED_MOTION") {
            return value == "1";
        }
        if std::env::var_os("VOLTA_SHOT").is_some() {
            return true;
        }
        os_reduced_motion().unwrap_or(false)
    })
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn os_reduced_motion() -> Option<bool> {
    if cfg!(target_os = "macos") {
        return command_output("defaults", &["read", "com.apple.universalaccess", "reduceMotion"]).map(|v| v == "1");
    }
    if cfg!(target_os = "windows") {
        let out = command_output("reg", &["query", r"HKCU\Control Panel\Desktop\WindowMetrics", "/v", "MinAnimate"])?;
        return Some(out.split_whitespace().last() == Some("0"));
    }
    if let Some(value) = command_output("gsettings", &["get", "org.gnome.desktop.interface", "enable-animations"]) {
        if value == "false" {
            return Some(true);
        }
    }
    for tool in ["kreadconfig6", "kreadconfig5"] {
        if let Some(value) = command_output(tool, &["--file", "kdeglobals", "--group", "KDE", "--key", "AnimationDurationFactor"]) {
            return Some(value.parse::<f32>().is_ok_and(|f| f == 0.0));
        }
    }
    Some(false)
}

/// CSS `ease-in-out`.
pub fn ease_in_out() -> impl Fn(f32) -> f32 + Clone {
    cubic_bezier(0.42, 0.0, 0.58, 1.0)
}

/// One `.playing-indicator i`: `bounce-bar 0.9s ease-in-out infinite
/// alternate` from `base` (fraction of the indicator height) to 25%, started
/// `delay` seconds early (negative CSS delay).
pub fn bounce_bar(id: impl Into<gpui::ElementId>, base: f32, delay: f32, color: gpui::Hsla) -> gpui::AnyElement {
    use gpui::{Styled as _, div};
    let bar = div().w(crate::theme::sz(3.0)).h(gpui::relative(base)).rounded(crate::theme::sz(2.0)).bg(color);
    if reduced() {
        return bar.into_any_element();
    }
    let ease = ease_in_out();
    bar.with_animation(id, Animation::new(Duration::from_millis(1800)).repeat(), move |el, delta| {
        let t = (delta * 1.8 + delay).rem_euclid(1.8);
        let forward = if t <= 0.9 { t / 0.9 } else { (1.8 - t) / 0.9 };
        el.h(gpui::relative(base + (0.25 - base) * ease(forward)))
    })
    .into_any_element()
}

/// One `.mixing-bars i`: `mixing-bar 900ms ease-in-out infinite`, scaleY
/// 0.5 → 1 → 0.5 of `height` px, delayed by `delay` seconds.
pub fn mixing_bar(id: impl Into<gpui::ElementId>, height: f32, delay: f32, color: gpui::Hsla) -> gpui::AnyElement {
    use gpui::{Styled as _, div};
    let bar = |scale: f32| div().w(crate::theme::sz(2.0)).h(crate::theme::sz(height * scale)).rounded(crate::theme::sz(1.0)).bg(color);
    if reduced() {
        return bar(1.0).into_any_element();
    }
    let ease = ease_in_out();
    bar(0.5)
        .with_animation(id, Animation::new(Duration::from_millis(900)).repeat(), move |el, delta| {
            // Before its delay the bar holds its unanimated size.
            let t = (delta * 0.9 - delay).rem_euclid(0.9) / 0.9;
            let half = if t <= 0.5 { t / 0.5 } else { (1.0 - t) / 0.5 };
            el.h(crate::theme::sz(height * (0.5 + 0.5 * ease(half))))
        })
        .into_any_element()
}

/// `MixingIndicator`: three bars and "MIXING" on an accent tint.
pub fn mixing_indicator(id: &str, large: bool, tokens: &crate::theme::Tokens) -> gpui::AnyElement {
    use gpui::{ParentElement as _, Styled as _, div};
    let color = tokens.accent;
    div()
        .flex()
        .items_center()
        .gap(crate::theme::sz(5.0))
        .mt(crate::theme::sz(if large { 8.0 } else { 2.0 }))
        .pl(crate::theme::sz(4.0))
        .pr(crate::theme::sz(6.0))
        .py(crate::theme::sz(1.0))
        .rounded_full()
        .bg(crate::theme::mix(color, 16.0, crate::theme::transparent()))
        .text_color(color)
        .text_size(crate::theme::sz(if large { 11.0 } else { 9.0 }))
        .line_height(gpui::relative(1.0))
        .child(
            div()
                .flex()
                .items_end()
                .gap(crate::theme::sz(1.5))
                .h(crate::theme::sz(8.0))
                .child(mixing_bar(gpui::SharedString::from(format!("{id}-1")), 4.0, 0.0, color))
                .child(mixing_bar(gpui::SharedString::from(format!("{id}-2")), 7.0, 0.14, color))
                .child(mixing_bar(gpui::SharedString::from(format!("{id}-3")), 5.0, 0.28, color)),
        )
        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).letter_spacing(crate::theme::sz(if large { 11.0 * 0.04 } else { 0.0 })).child("MIXING"))
        .into_any_element()
}
