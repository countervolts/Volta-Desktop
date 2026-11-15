//! Modified-gpui renderer tests: blur effects drawn by the wgpu headless
//! renderer (the backend Linux uses). The Metal and DirectX paths have no
//! tests here; they can only be built on their platforms.
#![cfg(target_os = "linux")]

use gpui::{
    BorderStyle, Bounds, ContentMask, Corners, DevicePixels, Edges, Hsla, PlatformHeadlessRenderer, Point, Quad,
    ScaledPixels, Scene, Size,
};
use gpui_wgpu::WgpuHeadlessRenderer;

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

fn device_size(width: i32, height: i32) -> Size<DevicePixels> {
    Size { width: DevicePixels(width), height: DevicePixels(height) }
}

fn solid_quad(x: f32, y: f32, width: f32, height: f32, color: Hsla) -> Quad {
    let bounds = Bounds {
        origin: Point { x: x.into(), y: y.into() },
        size: Size { width: width.into(), height: height.into() },
    };
    Quad {
        order: 0,
        border_style: BorderStyle::Solid,
        bounds,
        content_mask: ContentMask { bounds },
        background: color.into(),
        border_color: color,
        corner_radii: Corners::default(),
        border_widths: Edges::default(),
    }
}

fn assert_pixel(image: &image::RgbaImage, x: u32, y: u32, expected: [u8; 4]) {
    let actual = image.get_pixel(x, y).0;
    for (a, e) in actual.iter().zip(expected) {
        assert!((*a as i32 - e as i32).abs() <= 8, "pixel ({x}, {y}) is {actual:?}, expected {expected:?}");
    }
}

/// Modified for Volta: a backdrop blur softens the edge it covers and leaves
/// pixels outside its bounds alone.
#[test]
fn headless_renderer_blurs_backdrop() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = Scene::default();
    scene.insert_primitive(solid_quad(0.0, 0.0, 32.0, 32.0, gpui::red()));
    scene.insert_primitive(solid_quad(32.0, 0.0, 32.0, 32.0, gpui::blue()));
    let bounds = Bounds {
        origin: Point { x: 16.0.into(), y: 0.0.into() },
        size: Size { width: 32.0.into(), height: 32.0.into() },
    };
    let blur = ScaledPixels(4.0);
    scene.insert_primitive(gpui::BlurEffect {
        order: 0,
        kind: gpui::BlurEffectKind::BackdropBlur,
        bounds,
        sample_bounds: bounds.dilate(gpui::blur_extent(blur)),
        content_mask: ContentMask { bounds: Bounds { origin: Point::default(), size: Size { width: 64.0.into(), height: 32.0.into() } } },
        corner_radii: Corners::default(),
        blur_radius: blur,
        saturation: 1.0,
        opacity: 1.0,
        color: Hsla::default(),
        offset: Point::default(),
        layer: 0,
        glass: gpui::GlassParams::default(),
    });
    scene.finish();

    let image = renderer.render_scene_to_image(&scene, device_size(64, 32))?;
    // Outside the effect: untouched.
    assert_pixel(&image, 4, 16, RED);
    assert_pixel(&image, 60, 16, BLUE);
    // On the edge: an even mix of both sides.
    let mid = image.get_pixel(32, 16).0;
    assert!(mid[0] > 70 && mid[0] < 190 && mid[2] > 70 && mid[2] < 190, "edge not blurred: {mid:?}");
    Ok(())
}

/// Modified for Volta: a filter layer is drawn blurred, and a drop-shadow
/// layer draws only its tinted coverage.
#[test]
fn headless_renderer_draws_filter_layers() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let full = ContentMask { bounds: Bounds { origin: Point::default(), size: Size { width: 64.0.into(), height: 32.0.into() } } };
    let mut scene = Scene::default();
    scene.push_filter(gpui::SceneFilter::Blur { radius: ScaledPixels(3.0) }, full.clone());
    scene.insert_primitive(solid_quad(8.0, 8.0, 16.0, 16.0, gpui::white()));
    scene.pop_filter();
    scene.push_filter(
        gpui::SceneFilter::DropShadow { offset: Point { x: ScaledPixels(4.0), y: ScaledPixels(0.0) }, blur: ScaledPixels(0.0), color: gpui::red() },
        full,
    );
    scene.insert_primitive(solid_quad(40.0, 8.0, 16.0, 16.0, gpui::white()));
    scene.pop_filter();
    scene.finish();
    assert_eq!(scene.effects.len(), 2);

    let image = renderer.render_scene_to_image(&scene, device_size(64, 32))?;
    // Blurred square: solid centre, soft edge, glow past the edge.
    assert_pixel(&image, 16, 16, WHITE);
    let edge = image.get_pixel(8, 16).0;
    assert!(edge[0] > 90 && edge[0] < 170, "edge not soft: {edge:?}");
    assert!(image.get_pixel(5, 16).0[0] > 10, "no glow past the edge");
    // Shadow: red, moved right by 4px; the white content itself is not drawn.
    assert_pixel(&image, 58, 16, RED);
    assert_pixel(&image, 46, 16, RED);
    assert_pixel(&image, 42, 16, BLACK);
    assert_pixel(&image, 41, 4, BLACK);
    Ok(())
}


/// A glass surface over a red | blue backdrop with a thin green stripe inside its left bezel,
/// drawn like `Window::paint_glass` does. No veil, tint or rim light unless `configure` adds them.
fn render_glass(configure: impl FnOnce(&mut gpui::GlassParams)) -> anyhow::Result<image::RgbaImage> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = Scene::default();
    scene.insert_primitive(solid_quad(0.0, 0.0, 64.0, 64.0, gpui::red()));
    scene.insert_primitive(solid_quad(64.0, 0.0, 64.0, 64.0, gpui::blue()));
    scene.insert_primitive(solid_quad(40.0, 0.0, 3.0, 64.0, gpui::green()));
    let mut params = gpui::GlassParams {
        optics: [14.0, 14.0, 1.5, 24.0],
        blur: [4.0, 0.0, 0.0, 1.0],
        tone: [1.0, 1.0, 0.0, 0.0],
        light: [0.0, 0.0, 0.0, 0.0],
        shadow: [0.0, 0.0, 0.0, 0.0],
        // No ambient outline or bevel: the rim is only what the lights put there.
        finish: [0.0, 0.0, 3.0, 0.0],
        ..gpui::GlassParams::default()
    };
    configure(&mut params);
    let bounds = Bounds {
        origin: Point { x: 32.0.into(), y: 8.0.into() },
        size: Size { width: 64.0.into(), height: 48.0.into() },
    };
    let blur = ScaledPixels(params.blur[0]);
    let reach = gpui::blur_extent(blur).0.max(params.outset().ceil());
    scene.insert_primitive(gpui::BlurEffect {
        order: 0,
        kind: gpui::BlurEffectKind::Glass,
        bounds,
        sample_bounds: bounds.dilate(ScaledPixels(reach)),
        content_mask: ContentMask { bounds: Bounds { origin: Point::default(), size: Size { width: 128.0.into(), height: 64.0.into() } } },
        corner_radii: Corners::all(ScaledPixels(12.0)),
        blur_radius: blur,
        saturation: 1.0,
        opacity: 1.0,
        color: Hsla::default(),
        offset: Point::default(),
        layer: 0,
        glass: params,
    });
    scene.finish();
    renderer.render_scene_to_image(&scene, device_size(128, 64))
}

/// Modified for Volta: glass leaves everything outside it alone, frosts what is inside it
/// (the red | blue seam blends, the green stripe is smeared away), and bends the backdrop in
/// its bezel (a refracting glass differs from a flat one there).
#[test]
fn headless_renderer_draws_glass() -> anyhow::Result<()> {
    let flat = render_glass(|p| p.optics[0] = 0.0)?;
    // Outside the glass (and its zero-opacity shadow): the plain backdrop.
    assert_pixel(&flat, 4, 4, RED);
    assert_pixel(&flat, 124, 60, BLUE);
    assert_pixel(&flat, 41, 2, [0, 128, 0, 255]);
    // Deep inside, the interior is the blurred backdrop: pure colour away from the seam,
    // an even mix on it, and no trace of the sharp stripe.
    assert_pixel(&flat, 56, 32, RED);
    let seam = flat.get_pixel(64, 32).0;
    assert!(seam[0] > 70 && seam[0] < 190 && seam[2] > 70 && seam[2] < 190, "seam not blended: {seam:?}");
    assert!(flat.get_pixel(41, 32).0[1] < 120, "stripe survived the blur: {:?}", flat.get_pixel(41, 32));

    // The bezel bends the backdrop: with refraction the inner left rim samples further in.
    let bent = render_glass(|p| p.blur[1] = 0.0)?;
    let sharp_flat = render_glass(|p| {
        p.optics[0] = 0.0;
        p.blur[1] = 0.0;
    })?;
    let differs = (33..44).any(|x| {
        let a = bent.get_pixel(x, 32).0;
        let b = sharp_flat.get_pixel(x, 32).0;
        a.iter().zip(b).any(|(a, b)| (*a as i32 - b as i32).abs() > 25)
    });
    assert!(differs, "refraction changed nothing in the bezel");

    // Rim light brightens the outline, and unevenly: the two lights (up-right, down-left) leave
    // the top-left corner dark and light the bottom-left one fully.
    let lit = render_glass(|p| {
        p.optics[0] = 0.0;
        p.light = [0.785_398, -2.356_194, 0.9, 0.9];
    })?;
    let green = |image: &image::RgbaImage, x: u32, y: u32| image.get_pixel(x, y).0[1] as i32;
    assert!(green(&lit, 33, 32) > green(&flat, 33, 32) + 20, "no rim light on the left edge");
    // How much the lights add over the unlit glass near a corner (the frosted stripe is in
    // both renders, so it cancels).
    let added = |x: std::ops::Range<u32>, y: std::ops::Range<u32>| {
        let peak = |image: &image::RgbaImage| {
            y.clone().flat_map(|y| x.clone().map(move |x| (x, y))).map(|(x, y)| green(image, x, y)).max().unwrap_or(0)
        };
        peak(&lit) - peak(&flat)
    };
    let (lit_corner, dark_corner) = (added(35..40, 49..54), added(35..40, 10..15));
    assert!(lit_corner > dark_corner + 15, "rim light does not vary round the outline: +{lit_corner} vs +{dark_corner}");
    let shadowed = render_glass(|p| p.shadow = [0.0, 8.0, 0.6, 0.0])?;
    assert!(shadowed.get_pixel(29, 32).0[0] < flat.get_pixel(29, 32).0[0] - 20, "no shadow outside the glass");
    Ok(())
}
