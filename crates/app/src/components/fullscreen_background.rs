//! `FullscreenBackground` from player-ui.tsx, composited on the CPU.
//!
//! The web scene: a bleed copy of the cover (inset −35 %) plus four squares
//! of it — 1.25W at (0.5W, 0.5H), 0.8W at (0.4W, 0.4H), 0.5W orbiting
//! (0.5W, 0.5W) of a W×W square and 0.25W orbiting (0.55W, 0.55W) — each
//! turning at its own period, all blurred by ≈104px, graded by one colour
//! matrix, then a 50 % black veil and a 5 % white lift. Every offset is in
//! units of the viewport WIDTH. The web ticks at 15 fps (native: every display
//! frame) and crossfades a new cover
//! in over 1.7 s.
//!
//! Here the scene is drawn with tiny-skia into a small buffer (the blur makes
//! the low resolution invisible), blurred with three box passes (≈ Gaussian),
//! graded, and shown stretched over the window.

use std::sync::Arc;
use std::time::Instant;

use gpui::{Context, Entity, IntoElement, ParentElement, Render, RenderImage, Styled, Task, Window, div, img, prelude::*};
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

use crate::app_state::AppState;
use crate::theme;

/// `FULLSCREEN_ARTWORK_FADE_MS`.
const FADE: f32 = 1.7;
/// Buffer pixels across the visible width.
const BUFFER_WIDTH: f32 = 96.0;
/// `blur(104px)`, as a standard deviation in CSS px.
const BLUR_SIGMA: f32 = 104.0;
/// The feColorMatrix grade (rows R, G, B; last column is the offset).
const GRADE: [[f32; 4]; 3] = [
    [3.162_853, -1.6652724, -0.1678308, -0.315],
    [-0.4946469, 1.9922276, -0.1678308, -0.315],
    [-0.4946469, -1.6652724, 3.489_669, -0.315],
];

/// Decodes cover bytes (raster or SVG) into a small square pixmap.
pub fn decode_cover(bytes: &[u8]) -> Option<Pixmap> {
    const SIDE: u32 = 128;
    if super::svg_raster::is_svg(bytes) {
        return super::svg_raster::render(bytes, SIDE);
    }
    let mut pixmap = Pixmap::new(SIDE, SIDE)?;
    let image = image::load_from_memory(bytes).ok()?;
    let small = image.resize_to_fill(SIDE, SIDE, image::imageops::FilterType::Triangle).to_rgba8();
    for (dst, src) in pixmap.pixels_mut().iter_mut().zip(small.pixels()) {
        let [r, g, b, a] = src.0;
        *dst = tiny_skia::ColorU8::from_rgba(r, g, b, a).premultiply();
    }
    Some(pixmap)
}

struct Scene {
    cover: Option<Arc<Pixmap>>,
    previous: Option<Arc<Pixmap>>,
    fade: f32,
    seconds: f32,
    width: f32,
    height: f32,
    canvas: [u8; 3],
}

fn turn(seconds: f32, period: f32, direction: f32) -> f32 {
    (seconds / period).fract() * 360.0 * direction
}

/// Draws `cover` as a `size`-wide square centred at (cx, cy), rotated.
fn draw_square(target: &mut Pixmap, cover: &Pixmap, cx: f32, cy: f32, size: f32, degrees: f32, opacity: f32) {
    let scale = size / cover.width() as f32;
    let transform = Transform::from_translate(cx, cy)
        .pre_rotate(degrees)
        .pre_scale(scale, scale)
        .pre_translate(-(cover.width() as f32) / 2.0, -(cover.height() as f32) / 2.0);
    let paint = PixmapPaint { opacity, quality: FilterQuality::Bilinear, ..Default::default() };
    target.draw_pixmap(0, 0, cover.as_ref(), &paint, transform, None);
}

/// One pass of a horizontal + vertical box blur of radius `r` (RGB only).
fn box_blur(data: &mut [f32], w: usize, h: usize, r: usize) {
    let mut tmp = vec![0.0f32; data.len()];
    let norm = 1.0 / (2 * r + 1) as f32;
    let clampi = |v: isize, max: usize| v.clamp(0, max as isize - 1) as usize;
    for y in 0..h {
        for c in 0..3 {
            let mut acc = 0.0;
            for k in -(r as isize)..=(r as isize) {
                acc += data[(y * w + clampi(k, w)) * 3 + c];
            }
            for x in 0..w {
                tmp[(y * w + x) * 3 + c] = acc * norm;
                let add = clampi(x as isize + r as isize + 1, w);
                let sub = clampi(x as isize - r as isize, w);
                acc += data[(y * w + add) * 3 + c] - data[(y * w + sub) * 3 + c];
            }
        }
    }
    for x in 0..w {
        for c in 0..3 {
            let mut acc = 0.0;
            for k in -(r as isize)..=(r as isize) {
                acc += tmp[(clampi(k, h) * w + x) * 3 + c];
            }
            for y in 0..h {
                data[(y * w + x) * 3 + c] = acc * norm;
                let add = clampi(y as isize + r as isize + 1, h);
                let sub = clampi(y as isize - r as isize, h);
                acc += tmp[(add * w + x) * 3 + c] - tmp[(sub * w + x) * 3 + c];
            }
        }
    }
}

/// Box radii whose three passes approximate a Gaussian of `sigma`.
fn box_radii(sigma: f32) -> [usize; 3] {
    let n = 3.0;
    let ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let m = ((12.0 * sigma * sigma - n * (wl * wl) as f32 - 4.0 * n * wl as f32 - 3.0 * n) / (-4.0 * wl as f32 - 4.0)).round() as i32;
    let mut out = [0usize; 3];
    for (i, slot) in out.iter_mut().enumerate() {
        let width = if (i as i32) < m { wl } else { wu };
        *slot = ((width - 1) / 2).max(0) as usize;
    }
    out
}

fn render_scene(scene: &Scene) -> Option<Arc<RenderImage>> {
    let s = BUFFER_WIDTH / scene.width.max(1.0);
    let (vw, vh) = (BUFFER_WIDTH, (scene.height * s).max(1.0));
    // The mesh bleeds 35 % past every edge so the blur never samples outside.
    let (mx, my) = (vw * 0.35, vh * 0.35);
    let (bw, bh) = ((vw * 1.7).ceil() as u32, (vh * 1.7).ceil() as u32);
    let mut pixmap = Pixmap::new(bw, bh)?;
    let [cr, cg, cb] = scene.canvas;
    pixmap.fill(tiny_skia::Color::from_rgba8(cr, cg, cb, 255));

    let t = scene.seconds;
    let draw_all = |pixmap: &mut Pixmap, cover: &Pixmap, opacity: f32| {
        // `.fullscreen-cover`: inset −35 %, object-fit cover.
        let side = bw.max(bh) as f32;
        draw_square(pixmap, cover, bw as f32 / 2.0, bh as f32 / 2.0, side, 0.0, opacity);
        let (w, h) = (vw, vh);
        draw_square(pixmap, cover, mx + 0.5 * w, my + 0.5 * h, 1.25 * w, turn(t, 69.813, 1.0), opacity);
        draw_square(pixmap, cover, mx + 0.4 * w, my + 0.4 * h, 0.8 * w, turn(t, 26.18, -1.0), opacity);
        // Orbits: W×W squares centred vertically, turning about a pivot.
        let orbit_top = my + h / 2.0 - w / 2.0;
        let orbit = |pivot: (f32, f32), point: (f32, f32), degrees: f32| {
            let (sin, cos) = degrees.to_radians().sin_cos();
            let (dx, dy) = (point.0 - pivot.0, point.1 - pivot.1);
            (mx + pivot.0 + dx * cos - dy * sin, orbit_top + pivot.1 + dx * sin + dy * cos)
        };
        let (cx, cy) = orbit((0.5 * w, 0.5 * w), (0.75 * w, 0.5 * w), turn(t, 46.542, -1.0));
        draw_square(pixmap, cover, cx, cy, 0.5 * w, turn(t, 34.907, 1.0), opacity);
        let (dx, dy) = orbit((0.55 * w, 0.55 * w), (0.8 * w, 0.55 * w), turn(t, 69.813, 1.0));
        draw_square(pixmap, cover, dx, dy, 0.25 * w, turn(t, 52.36, -1.0), opacity);
    };
    if let Some(previous) = scene.previous.as_ref().filter(|_| scene.fade < 1.0) {
        draw_all(&mut pixmap, previous, 1.0);
    }
    if let Some(cover) = scene.cover.as_ref() {
        draw_all(&mut pixmap, cover, scene.fade.clamp(0.0, 1.0));
    }

    // Blur.
    let (w, h) = (bw as usize, bh as usize);
    let mut data: Vec<f32> = Vec::with_capacity(w * h * 3);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        data.extend([c.red() as f32 / 255.0, c.green() as f32 / 255.0, c.blue() as f32 / 255.0]);
    }
    for r in box_radii(BLUR_SIGMA * s) {
        if r > 0 {
            box_blur(&mut data, w, h, r);
        }
    }

    // Crop the visible region, grade, veil (50 % black) and lift (5 % white).
    let (ow, oh) = (vw.round() as u32, vh.round() as u32);
    let mut out = image::RgbaImage::new(ow.max(1), oh.max(1));
    for y in 0..oh.max(1) {
        for x in 0..ow.max(1) {
            let sx = (x as f32 + mx).min(bw as f32 - 1.0) as usize;
            let sy = (y as f32 + my).min(bh as f32 - 1.0) as usize;
            let i = (sy * w + sx) * 3;
            let rgb = [data[i], data[i + 1], data[i + 2]];
            let mut pixel = [0u8; 4];
            for (c, row) in GRADE.iter().enumerate() {
                let v = (row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2] + row[3]).clamp(0.0, 1.0);
                let v = v * 0.5;
                let v = v * 0.95 + 0.05;
                pixel[c] = (v * 255.0).round() as u8;
            }
            // gpui expects BGRA.
            out.put_pixel(x, y, image::Rgba([pixel[2], pixel[1], pixel[0], 255]));
        }
    }
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(out)])))
}

pub struct FullscreenBackground {
    state: Entity<AppState>,
    frame: Option<Arc<RenderImage>>,
    /// The frame shown before `frame`; freed from the sprite atlas once
    /// `frame` has been drawn in its place. Each 15 fps frame is a new
    /// image, so without this the atlas grows for as long as the player is open.
    retired: Option<Arc<RenderImage>>,
    cover_id: Option<String>,
    cover: Option<Arc<Pixmap>>,
    previous: Option<Arc<Pixmap>>,
    fade_started: Option<Instant>,
    started: Instant,
    size: (f32, f32),
    rendering: bool,
    _load: Option<Task<()>>,
}

impl FullscreenBackground {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            state,
            frame: None,
            retired: None,
            cover_id: None,
            cover: None,
            previous: None,
            fade_started: None,
            started: Instant::now(),
            size: (1440.0, 900.0),
            rendering: false,
            _load: None,
        }
    }

    fn load_cover(&mut self, cover_id: Option<String>, cx: &mut Context<Self>) {
        self.cover_id = cover_id.clone();
        let (Some(id), Some(client)) = (cover_id, self.state.read(cx).client_handle()) else { return };
        let task = cx.background_spawn(async move { client.cover_bytes(&id, Some(900)).ok().and_then(|b| decode_cover(&b)) });
        // The new cover waits until it has decoded, then fades in over the old.
        self._load = Some(cx.spawn(async move |this, cx| {
            if let Some(pixmap) = task.await {
                let _ = this.update(cx, |view, _| {
                    view.previous = view.cover.take();
                    view.cover = Some(Arc::new(pixmap));
                    view.fade_started = Some(Instant::now());
                });
            }
        }));
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let cover_id = self.state.read(cx).player.read(cx).current_song().and_then(|s| s.cover_art.clone());
        if cover_id != self.cover_id {
            self.load_cover(cover_id, cx);
        }
        if self.rendering {
            return;
        }
        let fade = match self.fade_started {
            Some(start) if self.previous.is_some() => start.elapsed().as_secs_f32() / FADE,
            _ => 1.0,
        };
        let canvas = theme::tokens(cx).canvas;
        let rgba = gpui::Rgba::from(canvas);
        let scene = Scene {
            cover: self.cover.clone(),
            previous: self.previous.clone(),
            fade,
            // `FULLSCREEN_REDUCED_MOTION_SCALE`.
            seconds: self.started.elapsed().as_secs_f32() * if crate::components::motion::reduced() { 0.2 } else { 1.0 },
            width: self.size.0,
            height: self.size.1,
            canvas: [(rgba.r * 255.0) as u8, (rgba.g * 255.0) as u8, (rgba.b * 255.0) as u8],
        };
        self.rendering = true;
        let task = cx.background_spawn(async move { render_scene(&scene) });
        cx.spawn(async move |this, cx| {
            let frame = task.await;
            // No notify: the per-frame request in `render` shows it.
            let _ = this.update(cx, |view, cx| {
                view.rendering = false;
                if let Some(frame) = frame {
                    if let Some(stale) = std::mem::replace(&mut view.retired, view.frame.replace(frame)) {
                        cx.drop_image(stale, None);
                    }
                }
            });
        })
        .detach();
    }
}

impl Render for FullscreenBackground {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = crate::theme::content_size(window);
        let scale = f32::from(window.rem_size()) / 16.0;
        self.size = (f32::from(viewport.width) / scale, f32::from(viewport.height) / scale);
        // Paced by the display, not a timer: every frame starts the next
        // scene (≈2 ms on a background thread) unless one is in flight, and
        // shows the newest finished one. The web renders at 15 fps; native
        // runs at the refresh rate.
        self.tick(cx);
        window.request_animation_frame();
        div()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .bg(gpui::rgb(0x242426))
            .map(|d| crate::components::window_chrome::window_corners(d, cx))
            // Stretched, not `contain`: the 96px-wide frame's rounded height
            // never matches the window's aspect exactly, and contain would
            // letterbox it with dark bars at the sides.
            .when_some(self.frame.clone(), |d, frame| {
                d.child(crate::components::window_chrome::window_corners(img(frame).absolute().inset_0().size_full().object_fit(gpui::ObjectFit::Fill), cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_radii_cover_the_sigma() {
        // Three boxes of these radii have variance ≈ sigma².
        let sigma = 6.9;
        let variance: f32 = box_radii(sigma).iter().map(|r| {
            let w = (2 * r + 1) as f32;
            (w * w - 1.0) / 12.0
        }).sum();
        assert!((variance.sqrt() - sigma).abs() < 0.8, "{variance}");
    }

    #[test]
    fn scene_renders_a_frame() {
        let mut cover = Pixmap::new(8, 8).unwrap();
        cover.fill(tiny_skia::Color::from_rgba8(30, 80, 160, 255));
        let scene = Scene {
            cover: Some(Arc::new(cover)),
            previous: None,
            fade: 1.0,
            seconds: 3.0,
            width: 1440.0,
            height: 900.0,
            canvas: [32, 32, 32],
        };
        let frame = render_scene(&scene).unwrap();
        assert_eq!(frame.size(0).width.0, 96);

    }
}
