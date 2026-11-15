//! `.genre-tile .artwork { opacity: .75; mix-blend-mode: luminosity }`:
//! the cover's luminance with the genre colour's hue and saturation (W3C
//! compositing `SetLum(backdrop, Lum(source))`), composited at 75% over the
//! colour, rendered once per genre on the CPU (gpui has no blend modes).

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    if n < 0.0 {
        out = out.map(|v| l + (v - l) * l / (l - n));
    }
    if x > 1.0 {
        out = out.map(|v| l + (v - l) * (1.0 - l) / (x - l));
    }
    out
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color(c.map(|v| v + d))
}

/// A `width`×`height` BGRA image (object-fit: cover) for the tile background.
pub fn luminosity_tile(cover: image::RgbaImage, backdrop: [f32; 3], width: u32, height: u32) -> Option<gpui::RenderImage> {
    let image = image::DynamicImage::ImageRgba8(cover);
    let fitted = image.resize_to_fill(width, height, image::imageops::FilterType::Triangle).to_rgba8();
    let mut out = image::RgbaImage::new(width, height);
    for (src, dst) in fitted.pixels().zip(out.pixels_mut()) {
        let [r, g, b, a] = src.0;
        let source = [r, g, b].map(|v| f32::from(v) / 255.0);
        let alpha = f32::from(a) / 255.0 * 0.75;
        let blended = set_lum(backdrop, lum(source));
        let px = [0, 1, 2].map(|i| ((backdrop[i] * (1.0 - alpha) + blended[i] * alpha).clamp(0.0, 1.0) * 255.0).round() as u8);
        // gpui sprites are BGRA.
        dst.0 = [px[2], px[1], px[0], 255];
    }
    Some(gpui::RenderImage::new(vec![image::Frame::new(out)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_backdrop_hue_with_source_luminance() {
        let red = [0.8, 0.2, 0.2];
        let white = set_lum(red, 1.0);
        assert!(white.iter().all(|v| (*v - 1.0).abs() < 1e-4));
        let dark = set_lum(red, 0.1);
        assert!((lum(dark) - 0.1).abs() < 1e-4 && dark[0] > dark[1]);
    }
}
