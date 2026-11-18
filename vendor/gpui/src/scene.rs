// Modified for Volta: blur effects (backdrop blur, filter layers, drop shadows).
// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AtlasTextureId, AtlasTile, Background, Bounds, ContentMask, Corners, Edges, Hsla, Pixels,
    Point, Radians, ScaledPixels, Size, bounds_tree::BoundsTree, point,
};
use std::{
    fmt::Debug,
    iter::Peekable,
    ops::{Add, Range, Sub},
    slice,
};

#[allow(non_camel_case_types, unused)]
#[expect(missing_docs)]
pub type PathVertex_ScaledPixels = PathVertex<ScaledPixels>;

#[expect(missing_docs)]
pub type DrawOrder = u32;

/// A boolean stored as a `u32` so that GPU-facing structs contain no
/// compiler-inserted padding bytes, which would be undefined behavior to
/// reinterpret as `&[u8]` when writing instance buffers. Guaranteed to be
/// `0` or `1` by construction; shaders read it as a `u32`/`uint`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct PaddedBool32(u32);

impl From<bool> for PaddedBool32 {
    fn from(value: bool) -> Self {
        PaddedBool32(value as u32)
    }
}

#[derive(Default)]
#[expect(missing_docs)]
pub struct Scene {
    pub(crate) paint_operations: Vec<PaintOperation>,
    primitive_bounds: BoundsTree<ScaledPixels>,
    layer_stack: Vec<DrawOrder>,
    pub shadows: Vec<Shadow>,
    pub quads: Vec<Quad>,
    pub paths: Vec<Path<ScaledPixels>>,
    pub underlines: Vec<Underline>,
    pub monochrome_sprites: Vec<MonochromeSprite>,
    pub subpixel_sprites: Vec<SubpixelSprite>,
    pub polychrome_sprites: Vec<PolychromeSprite>,
    pub surfaces: Vec<PaintSurface>,
    // Modified for Volta: blur effects and the offscreen sub-scenes they composite.
    pub effects: Vec<BlurEffect>,
    pub filter_layers: Vec<FilterLayer>,
    filter_stack: Vec<Option<ActiveFilter>>,
}

/// Modified for Volta: an offscreen sub-scene painted inside [`Scene::push_filter`].
/// The renderer draws it into a transparent texture the size of the frame, blurs
/// the region an [`BlurEffect`] samples, and composites the result at the effect's order.
#[derive(Default)]
pub struct FilterLayer {
    /// The primitives painted while the filter was active, in frame coordinates.
    pub scene: Scene,
}

/// Modified for Volta: a filter applied to everything painted inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SceneFilter {
    /// CSS `filter: blur(radius)`; `radius` is the Gaussian standard deviation.
    Blur {
        /// Standard deviation of the Gaussian.
        radius: ScaledPixels,
    },
    /// CSS `text-shadow` / `filter: drop-shadow()`: only the shadow is drawn, the
    /// caller paints the unfiltered content again on top.
    DropShadow {
        /// Shadow offset.
        offset: Point<ScaledPixels>,
        /// Standard deviation of the Gaussian.
        blur: ScaledPixels,
        /// Shadow color; its alpha multiplies the content's coverage.
        color: Hsla,
    },
}

impl SceneFilter {
    /// The Gaussian standard deviation this filter blurs with.
    pub fn blur_radius(&self) -> ScaledPixels {
        match self {
            SceneFilter::Blur { radius } => *radius,
            SceneFilter::DropShadow { blur, .. } => *blur,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ActiveFilter {
    layer: usize,
    filter: SceneFilter,
    content_mask: ContentMask<ScaledPixels>,
    content_bounds: Option<Bounds<ScaledPixels>>,
}

/// Modified for Volta: how far a Gaussian of standard deviation `sigma` reaches.
pub fn blur_extent(sigma: ScaledPixels) -> ScaledPixels {
    ScaledPixels((sigma.0.max(0.) * 3.).ceil())
}

/// Modified for Volta: what an [`BlurEffect`] draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlurEffectKind {
    /// Blur (and saturate) what is already drawn behind `bounds`, then draw it
    /// back clipped to `bounds` and its corner radii (CSS `backdrop-filter`).
    BackdropBlur,
    /// Draw filter layer `layer`, blurred (CSS `filter: blur()`).
    LayerBlur,
    /// Draw filter layer `layer`'s blurred coverage tinted with `color` and moved by
    /// `offset` (CSS `text-shadow`).
    LayerShadow,
    /// Liquid Glass: refract and blur what is already drawn behind `bounds` using
    /// `glass`, and draw its rim light and outside shadow. The backdrop is captured
    /// and blurred like [`Self::BackdropBlur`].
    Glass,
    /// The full-screen player background: rotating copies of a cover, blurred and
    /// colour-graded, drawn on the GPU from `cover` (Metal only for now; the other
    /// renderers treat it like [`Self::BackdropBlur`] and callers must not use it there).
    CoverBackground,
}

/// Modified for Volta: the scene a [`BlurEffectKind::CoverBackground`] effect draws.
/// Everything is positioned in units of the effect's width, so only `bounds` sets the scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoverScene {
    /// The current cover (atlas tile), if decoded yet.
    pub cover: Option<AtlasTile>,
    /// The cover fading out, while `fade` < 1.
    pub previous: Option<AtlasTile>,
    /// Opacity of `cover` over `previous`.
    pub fade: f32,
    /// Animation clock, in seconds.
    pub seconds: f32,
    /// Colour the scene starts from (sRGB, 0..1).
    pub canvas: [f32; 3],
}

/// Modified for Volta: optical parameters of a Liquid Glass surface, grouped in
/// `vec4`-sized blocks that the glass shader reads directly (`GlassParams` in
/// `shaders_effects.wgsl`; keep the two in sync). Lengths documented as "px" are
/// logical pixels when given to [`Window::paint_glass`](crate::Window::paint_glass),
/// which converts them to device pixels. Based on the `liquidglass` research project.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub struct GlassParams {
    /// Tint colour (straight sRGB `r, g, b`) and tint amount (`a`, 0 disables tinting).
    pub tint: [f32; 4],
    /// `[refraction amount (px), bezel width (px), refractive index, glass thickness (px)]`.
    pub optics: [f32; 4],
    /// `[blur sigma (px), blur at the rim relative to the interior (0..1), dispersion, saturation]`.
    pub blur: [f32; 4],
    /// `[brightness, contrast, veil, appearance]`. `veil` mixes towards white (appearance 0)
    /// or near-black (appearance 1); appearance below 0 follows the backdrop's luminance.
    pub tone: [f32; 4],
    /// `[light 0 angle (rad), light 1 angle (rad), light 0 strength, light 1 strength]`.
    pub light: [f32; 4],
    /// `[rim width (px), rim curvature, rim spread, rim vibrancy]`.
    pub rim: [f32; 4],
    /// `[shadow offset y (px), shadow blur (px), shadow opacity, shadow spread (px)]`.
    pub shadow: [f32; 4],
    /// `[corner exponent, pressed (0..1), appear (0..1, drives lensing), debug mode]`.
    pub state: [f32; 4],
    /// `[pointer x (px from bounds origin), pointer y (px), glow radius (px), glow intensity]`.
    pub pointer: [f32; 4],
    /// `[inner edge darkening, darkening width (px), outer contact shadow, rim dispersion (px)]`.
    pub edge: [f32; 4],
    /// `[ambient outline (0..1, rim light present on every side), bevel strength (soft
    /// light just inside the rim on lit sides), bevel width (px), unused]`.
    pub finish: [f32; 4],
}

impl GlassParams {
    /// Converts every length in logical pixels to device pixels.
    pub fn scaled(mut self, scale: f32) -> Self {
        self.optics[0] *= scale;
        self.optics[1] *= scale;
        self.optics[3] *= scale;
        self.blur[0] *= scale;
        self.rim[0] *= scale;
        self.shadow[0] *= scale;
        self.shadow[1] *= scale;
        self.shadow[3] *= scale;
        self.pointer[0] *= scale;
        self.pointer[1] *= scale;
        self.pointer[2] *= scale;
        self.edge[1] *= scale;
        self.edge[3] *= scale;
        self.finish[2] *= scale;
        self
    }

    /// How far the glass paints beyond its bounds, in the units of this struct.
    pub fn outset(&self) -> f32 {
        let shadow_extent =
            self.shadow[3].max(0.0) + self.shadow[1].max(0.0) * 1.5 + self.shadow[0].abs();
        let shadow_extent = if self.shadow[2] > 0.0 { shadow_extent } else { 0.0 };
        let contact = if self.edge[2] > 0.0 { 6.0 } else { 0.0 };
        shadow_extent.max(contact).max(self.optics[1] * 0.5)
    }
}

impl Default for GlassParams {
    fn default() -> Self {
        Self {
            tint: [1.0, 1.0, 1.0, 0.0],
            optics: [14.0, 14.0, 1.5, 24.0],
            blur: [6.0, 0.5, 0.0, 1.0],
            tone: [1.0, 1.0, 0.12, 0.0],
            light: [0.785_398_2, -2.356_194_5, 0.9, 0.9],
            rim: [1.5, 1.7, 1.59, 1.0],
            shadow: [8.0, 40.0, 0.25, 0.0],
            state: [4.0, 0.0, 1.0, 0.0],
            pointer: [0.0, 0.0, 0.0, 0.0],
            edge: [0.0, 8.0, 0.0, 0.0],
            finish: [0.25, 0.25, 3.0, 0.0],
        }
    }
}

/// Modified for Volta: a blur effect primitive. See [`BlurEffectKind`].
#[derive(Clone, Copy, Debug)]
pub struct BlurEffect {
    #[expect(missing_docs)]
    pub order: DrawOrder,
    #[expect(missing_docs)]
    pub kind: BlurEffectKind,
    /// Where the result is drawn. For layer effects this is `sample_bounds`
    /// moved by `offset`.
    pub bounds: Bounds<ScaledPixels>,
    /// The region of the source that is blurred: the drawn area grown by the
    /// kernel's reach, so edge pixels see their neighbours.
    pub sample_bounds: Bounds<ScaledPixels>,
    #[expect(missing_docs)]
    pub content_mask: ContentMask<ScaledPixels>,
    /// Backdrop only: the element's rounded corners.
    pub corner_radii: Corners<ScaledPixels>,
    /// Gaussian standard deviation.
    pub blur_radius: ScaledPixels,
    /// Backdrop only: CSS `saturate()` amount (1 = unchanged).
    pub saturation: f32,
    /// Multiplies the result's alpha.
    pub opacity: f32,
    /// Shadow only: tint.
    pub color: Hsla,
    /// Shadow only: offset of the drawn result from the sampled region.
    pub offset: Point<ScaledPixels>,
    /// Layer effects: index into [`Scene::filter_layers`].
    pub layer: usize,
    /// Glass only: the material, in device pixels (see [`GlassParams::scaled`]).
    pub glass: GlassParams,
    /// Cover background only.
    pub cover: Option<CoverScene>,
}

/// Modified for Volta: where a renderer runs one [`BlurEffect`]'s passes.
/// Shared by the wgpu, Metal and DirectX renderers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlurEffectPlan {
    /// x, y, width, height of the blurred source region, in whole frame pixels.
    pub region: [u32; 4],
    /// Width and height of the downsampled region in the scratch textures.
    pub scratch: [u32; 2],
    /// How many source pixels make one scratch texel per axis (a power of two).
    pub downsample: u32,
    /// The Gaussian standard deviation in scratch texels.
    pub sigma: f32,
}

/// Modified for Volta: the largest blur, in downsampled texels, before the
/// source is downsampled further; keeps the Gaussian at most 37 taps per axis.
pub const MAX_BLUR_SIGMA_TEXELS: f32 = 6.0;
/// Modified for Volta: the largest downsampling factor.
pub const MAX_BLUR_DOWNSAMPLE: u32 = 16;

/// Modified for Volta: plans `effect` for a `frame_width`×`frame_height` frame;
/// `None` when nothing of it is on screen.
pub fn plan_blur_effect(effect: &BlurEffect, frame_width: u32, frame_height: u32) -> Option<BlurEffectPlan> {
    let sample = effect.sample_bounds;
    let x0 = sample.origin.x.0.floor().max(0.);
    let y0 = sample.origin.y.0.floor().max(0.);
    let x1 = (sample.origin.x.0 + sample.size.width.0).ceil().min(frame_width as f32);
    let y1 = (sample.origin.y.0 + sample.size.height.0).ceil().min(frame_height as f32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let sigma = effect.blur_radius.0.max(0.);
    let mut downsample = 1u32;
    while sigma / downsample as f32 > MAX_BLUR_SIGMA_TEXELS && downsample < MAX_BLUR_DOWNSAMPLE {
        downsample *= 2;
    }
    let (width, height) = ((x1 - x0) as u32, (y1 - y0) as u32);
    Some(BlurEffectPlan {
        region: [x0 as u32, y0 as u32, width, height],
        scratch: [width.div_ceil(downsample), height.div_ceil(downsample)],
        downsample,
        sigma: sigma / downsample as f32,
    })
}

impl From<BlurEffect> for Primitive {
    fn from(effect: BlurEffect) -> Self {
        Primitive::BlurEffect(effect)
    }
}

#[expect(missing_docs)]
impl Scene {
    pub fn clear(&mut self) {
        self.paint_operations.clear();
        self.primitive_bounds.clear();
        self.layer_stack.clear();
        self.paths.clear();
        self.shadows.clear();
        self.quads.clear();
        self.underlines.clear();
        self.monochrome_sprites.clear();
        self.subpixel_sprites.clear();
        self.polychrome_sprites.clear();
        self.surfaces.clear();
        self.effects.clear();
        self.filter_layers.clear();
        self.filter_stack.clear();
    }

    pub fn len(&self) -> usize {
        self.paint_operations.len()
    }

    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        // Modified for Volta: inside a filter the layer belongs to the sub-scene.
        match self.filter_stack.first() {
            Some(Some(active)) => self.filter_layers[active.layer].scene.push_layer_local(bounds),
            _ => self.push_layer_local(bounds),
        }
        self.paint_operations
            .push(PaintOperation::StartLayer(bounds));
    }

    fn push_layer_local(&mut self, bounds: Bounds<ScaledPixels>) {
        let order = self.primitive_bounds.insert(bounds);
        self.layer_stack.push(order);
    }

    pub fn pop_layer(&mut self) {
        match self.filter_stack.first() {
            Some(Some(active)) => {
                self.filter_layers[active.layer].scene.layer_stack.pop();
            }
            _ => {
                self.layer_stack.pop();
            }
        }
        self.paint_operations.push(PaintOperation::EndLayer);
    }

    /// Modified for Volta: start a filter. Everything inserted until the matching
    /// [`Self::pop_filter`] is drawn into an offscreen sub-scene, and one [`BlurEffect`]
    /// compositing it is inserted here when the filter ends. Filters nested inside
    /// a filter are flattened into the outer one (their content is unfiltered).
    pub fn push_filter(&mut self, filter: SceneFilter, content_mask: ContentMask<ScaledPixels>) {
        self.paint_operations.push(PaintOperation::StartFilter {
            filter,
            content_mask: content_mask.clone(),
        });
        if !self.filter_stack.is_empty() {
            self.filter_stack.push(None);
            return;
        }
        let layer = self.filter_layers.len();
        self.filter_layers.push(FilterLayer::default());
        self.filter_stack.push(Some(ActiveFilter {
            layer,
            filter,
            content_mask,
            content_bounds: None,
        }));
    }

    /// Modified for Volta: end the filter started by [`Self::push_filter`].
    pub fn pop_filter(&mut self) {
        self.paint_operations.push(PaintOperation::EndFilter);
        let Some(Some(active)) = self.filter_stack.pop() else {
            return;
        };
        let Some(content_bounds) = active.content_bounds else {
            return;
        };
        let blur_radius = active.filter.blur_radius();
        let sample_bounds = content_bounds.dilate(blur_extent(blur_radius));
        let (kind, offset, color) = match active.filter {
            SceneFilter::Blur { .. } => (BlurEffectKind::LayerBlur, Point::default(), Hsla::default()),
            SceneFilter::DropShadow { offset, color, .. } => (BlurEffectKind::LayerShadow, offset, color),
        };
        let bounds = Bounds {
            origin: sample_bounds.origin + offset,
            size: sample_bounds.size,
        };
        let clipped_bounds = bounds.intersect(&active.content_mask.bounds);
        if clipped_bounds.is_empty() {
            return;
        }
        let order = self.primitive_bounds.insert(clipped_bounds);
        self.effects.push(BlurEffect {
            order,
            kind,
            bounds,
            sample_bounds,
            content_mask: active.content_mask,
            corner_radii: Corners::default(),
            blur_radius,
            saturation: 1.,
            opacity: 1.,
            color,
            offset,
            layer: active.layer,
            glass: GlassParams::default(),
            cover: None,
        });
    }

    pub fn insert_primitive(&mut self, primitive: impl Into<Primitive>) {
        let primitive = primitive.into();
        // Modified for Volta: route into the active filter's sub-scene.
        let inserted = match self.filter_stack.first_mut() {
            Some(Some(active)) => {
                let bounds = self.filter_layers[active.layer]
                    .scene
                    .insert_primitive_local(primitive.clone());
                if let Some(bounds) = bounds {
                    active.content_bounds = Some(match active.content_bounds {
                        Some(existing) => existing.union(&bounds),
                        None => bounds,
                    });
                }
                bounds.is_some()
            }
            _ => self.insert_primitive_local(primitive.clone()).is_some(),
        };
        if inserted {
            self.paint_operations
                .push(PaintOperation::Primitive(primitive));
        }
    }

    /// Inserts into this scene's own primitive lists; returns the clipped bounds
    /// when the primitive is visible.
    fn insert_primitive_local(&mut self, mut primitive: Primitive) -> Option<Bounds<ScaledPixels>> {
        let clipped_bounds = primitive
            .bounds()
            .intersect(&primitive.content_mask().bounds);

        if clipped_bounds.is_empty() {
            return None;
        }

        // Modified for Volta: an effect samples past its bounds, so it orders
        // after everything under its whole sample area, and never shares a
        // layer's order (layers assume their contents don't overlap).
        if let Primitive::BlurEffect(effect) = &mut primitive {
            let reach = match effect.kind {
                // A glass surface also paints its shadow beyond the blur's reach.
                BlurEffectKind::Glass => ScaledPixels(
                    (effect.sample_bounds.size.width.0 - effect.bounds.size.width.0)
                        .max(effect.sample_bounds.size.height.0 - effect.bounds.size.height.0)
                        .max(0.)
                        / 2.,
                ),
                _ => blur_extent(effect.blur_radius),
            };
            let sample = effect.sample_bounds.intersect(&effect.content_mask.bounds.dilate(reach));
            effect.order = self.primitive_bounds.insert(sample);
            self.effects.push(*effect);
            return Some(clipped_bounds);
        }

        let order = self
            .layer_stack
            .last()
            .copied()
            .unwrap_or_else(|| self.primitive_bounds.insert(clipped_bounds));
        match &mut primitive {
            Primitive::Shadow(shadow) => {
                shadow.order = order;
                self.shadows.push(*shadow);
            }
            Primitive::Quad(quad) => {
                quad.order = order;
                self.quads.push(*quad);
            }
            Primitive::Path(path) => {
                path.order = order;
                path.id = PathId(self.paths.len());
                self.paths.push(path.clone());
            }
            Primitive::Underline(underline) => {
                underline.order = order;
                self.underlines.push(*underline);
            }
            Primitive::MonochromeSprite(sprite) => {
                sprite.order = order;
                self.monochrome_sprites.push(*sprite);
            }
            Primitive::SubpixelSprite(sprite) => {
                sprite.order = order;
                self.subpixel_sprites.push(*sprite);
            }
            Primitive::PolychromeSprite(sprite) => {
                sprite.order = order;
                self.polychrome_sprites.push(*sprite);
            }
            Primitive::Surface(surface) => {
                surface.order = order;
                self.surfaces.push(surface.clone());
            }
            Primitive::BlurEffect(_) => unreachable!(),
        }
        Some(clipped_bounds)
    }

    pub fn replay(&mut self, range: Range<usize>, prev_scene: &Scene) {
        for operation in &prev_scene.paint_operations[range] {
            match operation {
                PaintOperation::Primitive(primitive) => self.insert_primitive(primitive.clone()),
                PaintOperation::StartLayer(bounds) => self.push_layer(*bounds),
                PaintOperation::EndLayer => self.pop_layer(),
                PaintOperation::StartFilter {
                    filter,
                    content_mask,
                } => self.push_filter(*filter, content_mask.clone()),
                PaintOperation::EndFilter => self.pop_filter(),
            }
        }
    }

    pub fn finish(&mut self) {
        self.shadows.sort_by_key(|shadow| shadow.order);
        self.quads.sort_by_key(|quad| quad.order);
        self.paths.sort_by_key(|path| path.order);
        self.underlines.sort_by_key(|underline| underline.order);
        self.monochrome_sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.texture_id.index));
        self.subpixel_sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.texture_id.index));
        self.polychrome_sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.texture_id.index));
        self.surfaces.sort_by_key(|surface| surface.order);
        self.effects.sort_by_key(|effect| effect.order);
        for layer in &mut self.filter_layers {
            layer.scene.finish();
        }
    }

    #[cfg_attr(
        all(
            any(target_os = "linux", target_os = "freebsd"),
            not(any(feature = "x11", feature = "wayland"))
        ),
        allow(dead_code)
    )]
    pub fn batches(&self) -> impl Iterator<Item = PrimitiveBatch> + '_ {
        BatchIterator {
            shadows_start: 0,
            shadows_iter: self.shadows.iter().peekable(),
            quads_start: 0,
            quads_iter: self.quads.iter().peekable(),
            paths_start: 0,
            paths_iter: self.paths.iter().peekable(),
            underlines_start: 0,
            underlines_iter: self.underlines.iter().peekable(),
            monochrome_sprites_start: 0,
            monochrome_sprites_iter: self.monochrome_sprites.iter().peekable(),
            subpixel_sprites_start: 0,
            subpixel_sprites_iter: self.subpixel_sprites.iter().peekable(),
            polychrome_sprites_start: 0,
            polychrome_sprites_iter: self.polychrome_sprites.iter().peekable(),
            surfaces_start: 0,
            surfaces_iter: self.surfaces.iter().peekable(),
            effects_start: 0,
            effects_iter: self.effects.iter().peekable(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Default)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveKind {
    Shadow,
    #[default]
    Quad,
    Path,
    Underline,
    MonochromeSprite,
    SubpixelSprite,
    PolychromeSprite,
    Surface,
    BlurEffect,
}

pub(crate) enum PaintOperation {
    Primitive(Primitive),
    StartLayer(Bounds<ScaledPixels>),
    EndLayer,
    // Modified for Volta: recorded so view-cache replay rebuilds filter layers.
    StartFilter {
        filter: SceneFilter,
        content_mask: ContentMask<ScaledPixels>,
    },
    EndFilter,
}

#[derive(Clone)]
#[expect(missing_docs)]
pub enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Path(Path<ScaledPixels>),
    Underline(Underline),
    MonochromeSprite(MonochromeSprite),
    SubpixelSprite(SubpixelSprite),
    PolychromeSprite(PolychromeSprite),
    Surface(PaintSurface),
    BlurEffect(BlurEffect),
}

#[expect(missing_docs)]
impl Primitive {
    pub fn bounds(&self) -> &Bounds<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.bounds,
            Primitive::Quad(quad) => &quad.bounds,
            Primitive::Path(path) => &path.bounds,
            Primitive::Underline(underline) => &underline.bounds,
            Primitive::MonochromeSprite(sprite) => &sprite.bounds,
            Primitive::SubpixelSprite(sprite) => &sprite.bounds,
            Primitive::PolychromeSprite(sprite) => &sprite.bounds,
            Primitive::Surface(surface) => &surface.bounds,
            Primitive::BlurEffect(effect) => &effect.bounds,
        }
    }

    pub fn content_mask(&self) -> &ContentMask<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.content_mask,
            Primitive::Quad(quad) => &quad.content_mask,
            Primitive::Path(path) => &path.content_mask,
            Primitive::Underline(underline) => &underline.content_mask,
            Primitive::MonochromeSprite(sprite) => &sprite.content_mask,
            Primitive::SubpixelSprite(sprite) => &sprite.content_mask,
            Primitive::PolychromeSprite(sprite) => &sprite.content_mask,
            Primitive::Surface(surface) => &surface.content_mask,
            Primitive::BlurEffect(effect) => &effect.content_mask,
        }
    }
}

#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
struct BatchIterator<'a> {
    shadows_start: usize,
    shadows_iter: Peekable<slice::Iter<'a, Shadow>>,
    quads_start: usize,
    quads_iter: Peekable<slice::Iter<'a, Quad>>,
    paths_start: usize,
    paths_iter: Peekable<slice::Iter<'a, Path<ScaledPixels>>>,
    underlines_start: usize,
    underlines_iter: Peekable<slice::Iter<'a, Underline>>,
    monochrome_sprites_start: usize,
    monochrome_sprites_iter: Peekable<slice::Iter<'a, MonochromeSprite>>,
    subpixel_sprites_start: usize,
    subpixel_sprites_iter: Peekable<slice::Iter<'a, SubpixelSprite>>,
    polychrome_sprites_start: usize,
    polychrome_sprites_iter: Peekable<slice::Iter<'a, PolychromeSprite>>,
    surfaces_start: usize,
    surfaces_iter: Peekable<slice::Iter<'a, PaintSurface>>,
    effects_start: usize,
    effects_iter: Peekable<slice::Iter<'a, BlurEffect>>,
}

impl<'a> Iterator for BatchIterator<'a> {
    type Item = PrimitiveBatch;

    fn next(&mut self) -> Option<Self::Item> {
        let mut orders_and_kinds = [
            (
                self.shadows_iter.peek().map(|s| s.order),
                PrimitiveKind::Shadow,
            ),
            (self.quads_iter.peek().map(|q| q.order), PrimitiveKind::Quad),
            (self.paths_iter.peek().map(|q| q.order), PrimitiveKind::Path),
            (
                self.underlines_iter.peek().map(|u| u.order),
                PrimitiveKind::Underline,
            ),
            (
                self.monochrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::MonochromeSprite,
            ),
            (
                self.subpixel_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::SubpixelSprite,
            ),
            (
                self.polychrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::PolychromeSprite,
            ),
            (
                self.surfaces_iter.peek().map(|s| s.order),
                PrimitiveKind::Surface,
            ),
            (
                self.effects_iter.peek().map(|e| e.order),
                PrimitiveKind::BlurEffect,
            ),
        ];
        orders_and_kinds.sort_by_key(|(order, kind)| (order.unwrap_or(u32::MAX), *kind));

        let first = orders_and_kinds[0];
        let second = orders_and_kinds[1];
        let (batch_kind, max_order_and_kind) = if first.0.is_some() {
            (first.1, (second.0.unwrap_or(u32::MAX), second.1))
        } else {
            return None;
        };

        match batch_kind {
            PrimitiveKind::Shadow => {
                let shadows_start = self.shadows_start;
                let mut shadows_end = shadows_start + 1;
                self.shadows_iter.next();
                while self
                    .shadows_iter
                    .next_if(|shadow| (shadow.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    shadows_end += 1;
                }
                self.shadows_start = shadows_end;
                Some(PrimitiveBatch::Shadows(shadows_start..shadows_end))
            }
            PrimitiveKind::Quad => {
                let quads_start = self.quads_start;
                let mut quads_end = quads_start + 1;
                self.quads_iter.next();
                while self
                    .quads_iter
                    .next_if(|quad| (quad.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    quads_end += 1;
                }
                self.quads_start = quads_end;
                Some(PrimitiveBatch::Quads(quads_start..quads_end))
            }
            PrimitiveKind::Path => {
                let paths_start = self.paths_start;
                let mut paths_end = paths_start + 1;
                self.paths_iter.next();
                while self
                    .paths_iter
                    .next_if(|path| (path.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    paths_end += 1;
                }
                self.paths_start = paths_end;
                Some(PrimitiveBatch::Paths(paths_start..paths_end))
            }
            PrimitiveKind::Underline => {
                let underlines_start = self.underlines_start;
                let mut underlines_end = underlines_start + 1;
                self.underlines_iter.next();
                while self
                    .underlines_iter
                    .next_if(|underline| (underline.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    underlines_end += 1;
                }
                self.underlines_start = underlines_end;
                Some(PrimitiveBatch::Underlines(underlines_start..underlines_end))
            }
            PrimitiveKind::MonochromeSprite => {
                let texture_id = self.monochrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.monochrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.monochrome_sprites_iter.next();
                while self
                    .monochrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.monochrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::SubpixelSprite => {
                let texture_id = self.subpixel_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.subpixel_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.subpixel_sprites_iter.next();
                while self
                    .subpixel_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.subpixel_sprites_start = sprites_end;
                Some(PrimitiveBatch::SubpixelSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::PolychromeSprite => {
                let texture_id = self.polychrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.polychrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.polychrome_sprites_iter.next();
                while self
                    .polychrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.polychrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::Surface => {
                let surfaces_start = self.surfaces_start;
                let mut surfaces_end = surfaces_start + 1;
                self.surfaces_iter.next();
                while self
                    .surfaces_iter
                    .next_if(|surface| (surface.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    surfaces_end += 1;
                }
                self.surfaces_start = surfaces_end;
                Some(PrimitiveBatch::Surfaces(surfaces_start..surfaces_end))
            }
            PrimitiveKind::BlurEffect => {
                let effects_start = self.effects_start;
                let mut effects_end = effects_start + 1;
                self.effects_iter.next();
                while self
                    .effects_iter
                    .next_if(|effect| (effect.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    effects_end += 1;
                }
                self.effects_start = effects_end;
                Some(PrimitiveBatch::Effects(effects_start..effects_end))
            }
        }
    }
}

#[derive(Debug)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
#[allow(missing_docs)]
pub enum PrimitiveBatch {
    Shadows(Range<usize>),
    Quads(Range<usize>),
    Paths(Range<usize>),
    Underlines(Range<usize>),
    MonochromeSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    SubpixelSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    PolychromeSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    Surfaces(Range<usize>),
    /// Modified for Volta: blur effects; the renderer handles them one at a time.
    Effects(Range<usize>),
}

impl PrimitiveBatch {
    #[expect(missing_docs)]
    pub fn label(&self) -> String {
        match self {
            Self::Shadows(range) => format!("shadows ({})", range.len()),
            Self::Quads(range) => format!("quads ({})", range.len()),
            Self::Paths(range) => format!("paths ({})", range.len()),
            Self::Underlines(range) => format!("underlines ({})", range.len()),
            Self::MonochromeSprites { texture_id, range } => {
                format!(
                    "monochrome sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::SubpixelSprites { texture_id, range } => {
                format!(
                    "subpixel sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::PolychromeSprites { texture_id, range } => {
                format!(
                    "polychrome sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::Surfaces(range) => format!("surfaces ({})", range.len()),
            Self::Effects(range) => format!("effects ({})", range.len()),
        }
    }
}

#[derive(Default, Debug, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Quad {
    pub order: DrawOrder,
    pub border_style: BorderStyle,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub background: Background,
    pub border_color: Hsla,
    pub corner_radii: Corners<ScaledPixels>,
    pub border_widths: Edges<ScaledPixels>,
}

impl From<Quad> for Primitive {
    fn from(quad: Quad) -> Self {
        Primitive::Quad(quad)
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Underline {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub thickness: ScaledPixels,
    pub wavy: PaddedBool32,
}

impl From<Underline> for Primitive {
    fn from(underline: Underline) -> Self {
        Primitive::Underline(underline)
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Shadow {
    pub order: DrawOrder,
    pub blur_radius: ScaledPixels,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub element_bounds: Bounds<ScaledPixels>,
    pub element_corner_radii: Corners<ScaledPixels>,
    /// 0 = drop shadow (rendered outside the element), 1 = inset shadow (rendered inside).
    pub inset: u32,
    pub pad: u32, // align to 8 bytes
}

impl From<Shadow> for Primitive {
    fn from(shadow: Shadow) -> Self {
        Primitive::Shadow(shadow)
    }
}

/// The style of a border.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub enum BorderStyle {
    /// A solid border.
    #[default]
    Solid = 0,
    /// A dashed border.
    Dashed = 1,
}

/// A data type representing a 2 dimensional transformation that can be applied to an element.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct TransformationMatrix {
    /// 2x2 matrix containing rotation and scale,
    /// stored row-major
    pub rotation_scale: [[f32; 2]; 2],
    /// translation vector
    pub translation: [f32; 2],
}

impl Eq for TransformationMatrix {}

impl TransformationMatrix {
    /// The unit matrix, has no effect.
    pub fn unit() -> Self {
        Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [0.0, 0.0],
        }
    }

    /// Move the origin by a given point
    pub fn translate(self, point: Point<ScaledPixels>) -> Self {
        self.compose(Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [point.x.0, point.y.0],
        })
    }

    /// Clockwise rotation in radians around the origin
    pub fn rotate(self, angle: Radians) -> Self {
        self.compose(Self {
            rotation_scale: [
                [angle.0.cos(), -angle.0.sin()],
                [angle.0.sin(), angle.0.cos()],
            ],
            translation: [0.0, 0.0],
        })
    }

    /// Scale around the origin
    pub fn scale(self, size: Size<f32>) -> Self {
        self.compose(Self {
            rotation_scale: [[size.width, 0.0], [0.0, size.height]],
            translation: [0.0, 0.0],
        })
    }

    /// Perform matrix multiplication with another transformation
    /// to produce a new transformation that is the result of
    /// applying both transformations: first, `other`, then `self`.
    #[inline]
    pub fn compose(self, other: TransformationMatrix) -> TransformationMatrix {
        if other == Self::unit() {
            return self;
        }
        // Perform matrix multiplication
        TransformationMatrix {
            rotation_scale: [
                [
                    self.rotation_scale[0][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][0],
                    self.rotation_scale[0][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][1],
                ],
                [
                    self.rotation_scale[1][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][0],
                    self.rotation_scale[1][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][1],
                ],
            ],
            translation: [
                self.translation[0]
                    + self.rotation_scale[0][0] * other.translation[0]
                    + self.rotation_scale[0][1] * other.translation[1],
                self.translation[1]
                    + self.rotation_scale[1][0] * other.translation[0]
                    + self.rotation_scale[1][1] * other.translation[1],
            ],
        }
    }

    /// Apply transformation to a point, mainly useful for debugging
    pub fn apply(&self, point: Point<Pixels>) -> Point<Pixels> {
        let input = [point.x.0, point.y.0];
        let mut output = self.translation;
        for (i, output_cell) in output.iter_mut().enumerate() {
            for (k, input_cell) in input.iter().enumerate() {
                *output_cell += self.rotation_scale[i][k] * *input_cell;
            }
        }
        Point::new(output[0].into(), output[1].into())
    }
}

impl Default for TransformationMatrix {
    fn default() -> Self {
        Self::unit()
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct MonochromeSprite {
    pub order: DrawOrder,
    pub pad: u32,
    pub tile: AtlasTile,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub transformation: TransformationMatrix,
}

impl From<MonochromeSprite> for Primitive {
    fn from(sprite: MonochromeSprite) -> Self {
        Primitive::MonochromeSprite(sprite)
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct SubpixelSprite {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub tile: AtlasTile,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub transformation: TransformationMatrix,
}

impl From<SubpixelSprite> for Primitive {
    fn from(sprite: SubpixelSprite) -> Self {
        Primitive::SubpixelSprite(sprite)
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct PolychromeSprite {
    pub order: DrawOrder,
    pub pad: u32,
    pub tile: AtlasTile,
    pub grayscale: PaddedBool32,
    pub opacity: f32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
}

impl From<PolychromeSprite> for Primitive {
    fn from(sprite: PolychromeSprite) -> Self {
        Primitive::PolychromeSprite(sprite)
    }
}

#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub struct PaintSurface {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub image_buffer: objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>,
}

impl From<PaintSurface> for Primitive {
    fn from(surface: PaintSurface) -> Self {
        Primitive::Surface(surface)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[expect(missing_docs)]
pub struct PathId(pub usize);

/// A line made up of a series of vertices and control points.
#[derive(Clone, Debug)]
#[expect(missing_docs)]
pub struct Path<P: Clone + Debug + Default + PartialEq> {
    pub id: PathId,
    pub order: DrawOrder,
    pub bounds: Bounds<P>,
    pub content_mask: ContentMask<P>,
    pub vertices: Vec<PathVertex<P>>,
    pub color: Background,
    start: Point<P>,
    current: Point<P>,
    contour_count: usize,
}

impl Path<Pixels> {
    /// Create a new path with the given starting point.
    pub fn new(start: Point<Pixels>) -> Self {
        Self {
            id: PathId(0),
            order: DrawOrder::default(),
            vertices: Vec::new(),
            start,
            current: start,
            bounds: Bounds {
                origin: start,
                size: Default::default(),
            },
            content_mask: Default::default(),
            color: Default::default(),
            contour_count: 0,
        }
    }

    /// Scale this path by the given factor.
    pub fn scale(&self, factor: f32) -> Path<ScaledPixels> {
        Path {
            id: self.id,
            order: self.order,
            bounds: self.bounds.scale(factor),
            content_mask: self.content_mask.scale(factor),
            vertices: self
                .vertices
                .iter()
                .map(|vertex| vertex.scale(factor))
                .collect(),
            start: self.start.map(|start| start.scale(factor)),
            current: self.current.scale(factor),
            contour_count: self.contour_count,
            color: self.color,
        }
    }

    /// Move the start, current point to the given point.
    pub fn move_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        self.start = to;
        self.current = to;
    }

    /// Draw a straight line from the current point to the given point.
    pub fn line_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }
        self.current = to;
    }

    /// Draw a curve from the current point to the given point, using the given control point.
    pub fn curve_to(&mut self, to: Point<Pixels>, ctrl: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }

        self.push_triangle(
            (self.current, ctrl, to),
            (point(0., 0.), point(0.5, 0.), point(1., 1.)),
        );
        self.current = to;
    }

    /// Push a triangle to the Path.
    pub fn push_triangle(
        &mut self,
        xy: (Point<Pixels>, Point<Pixels>, Point<Pixels>),
        st: (Point<f32>, Point<f32>, Point<f32>),
    ) {
        self.bounds = self
            .bounds
            .union(&Bounds {
                origin: xy.0,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.1,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.2,
                size: Default::default(),
            });

        self.vertices.push(PathVertex {
            xy_position: xy.0,
            st_position: st.0,
            content_mask: Default::default(),
        });
        self.vertices.push(PathVertex {
            xy_position: xy.1,
            st_position: st.1,
            content_mask: Default::default(),
        });
        self.vertices.push(PathVertex {
            xy_position: xy.2,
            st_position: st.2,
            content_mask: Default::default(),
        });
    }
}

impl<T> Path<T>
where
    T: Clone + Debug + Default + PartialEq + PartialOrd + Add<T, Output = T> + Sub<Output = T>,
{
    #[allow(unused)]
    #[expect(missing_docs)]
    pub fn clipped_bounds(&self) -> Bounds<T> {
        self.bounds.intersect(&self.content_mask.bounds)
    }
}

impl From<Path<ScaledPixels>> for Primitive {
    fn from(path: Path<ScaledPixels>) -> Self {
        Primitive::Path(path)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct PathVertex<P: Clone + Debug + Default + PartialEq> {
    pub xy_position: Point<P>,
    pub st_position: Point<f32>,
    pub content_mask: ContentMask<P>,
}

#[expect(missing_docs)]
impl PathVertex<Pixels> {
    pub fn scale(&self, factor: f32) -> PathVertex<ScaledPixels> {
        PathVertex {
            xy_position: self.xy_position.scale(factor),
            st_position: self.st_position,
            content_mask: self.content_mask.scale(factor),
        }
    }
}
