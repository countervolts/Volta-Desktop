// Modified for Volta: blur effects (CSS backdrop-filter, filter: blur(), text-shadow).
// Concatenated after `shaders.wgsl` and
// `shaders_storage.wgsl` into its own module; storage-buffer backends only.
//
// One effect runs as: (source) -> downsample into scratch A -> horizontal Gaussian
// A -> B -> vertical Gaussian B -> A -> composite A into the frame. The source,
// A and B textures are all frame-sized; each pass only touches the sub-rectangle
// the effect needs (viewport/scissor on the Rust side).

struct EffectParams {
    // Region of the source that is blurred, in frame pixels (whole pixels).
    region: Bounds,
    // Scratch texels covered by the downsampled region.
    scratch_size: vec2<f32>,
    // Size of the source and scratch textures (all frame-sized).
    texture_size: vec2<f32>,
    downsample: f32,
    // Gaussian standard deviation, in scratch texels.
    sigma: f32,
    // 0 = backdrop blur, 1 = layer blur, 2 = layer shadow, 3 = glass.
    kind: u32,
    saturation: f32,
    // Where the result is drawn, in frame pixels.
    bounds: Bounds,
    content_mask: Bounds,
    corner_radii: Corners,
    // Shadow tint, premultiplied.
    color: vec4<f32>,
    // Shadow offset: the drawn result is the source moved by this much.
    offset: vec2<f32>,
    opacity: f32,
    // Glass only: how far the quad extends beyond `bounds` (the shadow).
    outset: f32,
    glass: GlassParams,
}

// Liquid Glass material, in device pixels. Mirrors `gpui::GlassParams`.
struct GlassParams {
    tint: vec4<f32>,
    optics: vec4<f32>,
    blur: vec4<f32>,
    tone: vec4<f32>,
    light: vec4<f32>,
    rim: vec4<f32>,
    shadow: vec4<f32>,
    state: vec4<f32>,
    pointer: vec4<f32>,
    edge: vec4<f32>,
    finish: vec4<f32>,
}

@group(1) @binding(0) var<storage, read> b_effects: array<EffectParams>;

struct EffectPassVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) effect_id: u32,
}

// A triangle covering the viewport; the viewport is set to the pass's rectangle.
@vertex
fn vs_effect_pass(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> EffectPassVarying {
    let uv = vec2<f32>(f32((vertex_id << 1u) & 2u), f32(vertex_id & 2u));
    var out = EffectPassVarying();
    out.position = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.effect_id = instance_id;
    return out;
}

// Clears the scissored region of the source texture before a filter layer is drawn.
@fragment
fn fs_effect_clear(input: EffectPassVarying) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}

fn effect_sample(texel: vec2<f32>, lo: vec2<f32>, hi: vec2<f32>, texture_size: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(t_sprite, s_sprite, clamp(texel, lo, hi) / texture_size, 0.0);
}

// Box-filters `downsample`² source pixels into each scratch texel, using bilinear
// taps that each average a 2×2 block.
@fragment
fn fs_effect_downsample(input: EffectPassVarying) -> @location(0) vec4<f32> {
    let effect = b_effects[input.effect_id];
    let lo = effect.region.origin + vec2<f32>(0.5);
    let hi = effect.region.origin + effect.region.size - vec2<f32>(0.5);
    let d = effect.downsample;
    let cell = floor(input.position.xy);
    if (d <= 1.0) {
        return effect_sample(effect.region.origin + cell + vec2<f32>(0.5), lo, hi, effect.texture_size);
    }
    let base = effect.region.origin + cell * d;
    let taps = i32(d * 0.5);
    var sum = vec4<f32>(0.0);
    for (var y = 0; y < taps; y++) {
        for (var x = 0; x < taps; x++) {
            let texel = base + vec2<f32>(f32(x * 2 + 1), f32(y * 2 + 1));
            sum += effect_sample(texel, lo, hi, effect.texture_size);
        }
    }
    return sum / f32(taps * taps);
}

fn effect_gaussian(input: EffectPassVarying, direction: vec2<f32>) -> vec4<f32> {
    let effect = b_effects[input.effect_id];
    let center = input.position.xy;
    let lo = vec2<f32>(0.5);
    let hi = effect.scratch_size - vec2<f32>(0.5);
    let sigma = effect.sigma;
    if (sigma < 0.05) {
        return effect_sample(center, lo, hi, effect.texture_size);
    }
    let radius = i32(ceil(sigma * 3.0));
    let denominator = 2.0 * sigma * sigma;
    var sum = vec4<f32>(0.0);
    var weight_sum = 0.0;
    for (var k = -radius; k <= radius; k++) {
        let offset = f32(k);
        let weight = exp(-(offset * offset) / denominator);
        sum += weight * effect_sample(center + direction * offset, lo, hi, effect.texture_size);
        weight_sum += weight;
    }
    return sum / weight_sum;
}

@fragment
fn fs_effect_blur_h(input: EffectPassVarying) -> @location(0) vec4<f32> {
    return effect_gaussian(input, vec2<f32>(1.0, 0.0));
}

@fragment
fn fs_effect_blur_v(input: EffectPassVarying) -> @location(0) vec4<f32> {
    return effect_gaussian(input, vec2<f32>(0.0, 1.0));
}

struct EffectCompositeVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) effect_id: u32,
    @location(1) clip_distances: vec4<f32>,
}

@vertex
fn vs_effect_composite(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> EffectCompositeVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let effect = b_effects[instance_id];
    var out = EffectCompositeVarying();
    out.position = to_device_position(unit_vertex, effect.bounds);
    out.effect_id = instance_id;
    out.clip_distances = distance_from_clip_rect(unit_vertex, effect.bounds, effect.content_mask);
    return out;
}

// Output is premultiplied; the pipeline blends with PREMULTIPLIED_ALPHA_BLENDING.
@fragment
fn fs_effect_composite(input: EffectCompositeVarying) -> @location(0) vec4<f32> {
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }
    let effect = b_effects[input.effect_id];
    let source = input.position.xy - effect.offset;
    let scratch = (source - effect.region.origin) / effect.downsample;
    let sample = effect_sample(scratch, vec2<f32>(0.5), effect.scratch_size - vec2<f32>(0.5), effect.texture_size);

    if (effect.kind == 0u) {
        // CSS saturate(): mix with the Rec. 709 luminance the filter spec uses.
        let luminance = dot(sample.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        let rgb = clamp(vec3<f32>(luminance) + (sample.rgb - vec3<f32>(luminance)) * effect.saturation,
            vec3<f32>(0.0), vec3<f32>(1.0));
        let distance = quad_sdf(input.position.xy, effect.bounds, effect.corner_radii);
        let coverage = saturate(0.5 - distance) * effect.opacity;
        let alpha = select(coverage, sample.a * coverage, globals.premultiplied_alpha != 0u);
        return vec4<f32>(rgb * coverage, alpha);
    }
    if (effect.kind == 1u) {
        return sample * effect.opacity;
    }
    return effect.color * sample.a * effect.opacity;
}


// --- Liquid Glass -------------------------------------------------------------------------
//
// Ported from the `liquidglass` research renderer. The backdrop was copied into the effect
// source texture (group 2, sharp) and blurred into scratch A (group 3) by the passes above,
// so the glass samples both: sharp, refracted taps at the rim, the frosted blur inside.
// The frame holds gamma-encoded values; the optics work in linear light.

@group(3) @binding(0) var t_glass_blur: texture_2d<f32>;

const GLASS_LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

// Interleaved gradient noise in about ±½ of an 8-bit step: hides the banding of the long,
// shallow gradients (shadow, frosted interior) without visible grain.
fn glass_dither(position: vec2<f32>) -> f32 {
    let noise = fract(52.9829189 * fract(dot(position, vec2<f32>(0.06711056, 0.00583715))));
    return (noise - 0.5) / 255.0;
}

// Signed distance (device px, negative inside) to a rounded rectangle whose corners are
// superellipses of exponent `n` (2 is a circular arc, larger approaches a continuous
// "squircle" corner). Exact on straight edges, approximate inside the corner.
fn glass_sdf(p: vec2<f32>, half_size: vec2<f32>, radii: Corners, n: f32) -> f32 {
    let r = min(pick_corner_radius(p, radii), min(half_size.x, half_size.y));
    let q = abs(p) - half_size + vec2<f32>(r);
    let outside_q = max(q, vec2<f32>(0.0));
    var outside = length(outside_q);
    if (r > 0.0 && n > 2.0) {
        outside = pow(pow(outside_q.x, n) + pow(outside_q.y, n), 1.0 / n);
    }
    return outside + min(max(q.x, q.y), 0.0) - r;
}

// Outward unit normal of the outline by central differences of the SDF. Corners are
// rounded to at least `min_radius` first: a box's distance field flips its gradient along
// the diagonals, which would show as a seam in the refraction.
fn glass_normal(p: vec2<f32>, half_size: vec2<f32>, radii: Corners, n: f32, min_radius: f32) -> vec2<f32> {
    let e = 0.75;
    let soft = Corners(
        max(radii.top_left, min_radius),
        max(radii.top_right, min_radius),
        max(radii.bottom_right, min_radius),
        max(radii.bottom_left, min_radius),
    );
    let gradient = vec2<f32>(
        glass_sdf(p + vec2<f32>(e, 0.0), half_size, soft, n) - glass_sdf(p - vec2<f32>(e, 0.0), half_size, soft, n),
        glass_sdf(p + vec2<f32>(0.0, e), half_size, soft, n) - glass_sdf(p - vec2<f32>(0.0, e), half_size, soft, n),
    );
    let len = length(gradient);
    return select(vec2<f32>(0.0, -1.0), gradient / len, len > 1e-6);
}

// The sharp backdrop (linear light) at a frame position; only the copied region is valid.
fn glass_sharp(effect: EffectParams, position: vec2<f32>) -> vec3<f32> {
    let lo = effect.region.origin + vec2<f32>(0.5);
    let hi = effect.region.origin + effect.region.size - vec2<f32>(0.5);
    let uv = clamp(position, lo, hi) / effect.texture_size;
    return srgb_to_linear(textureSampleLevel(t_sprite, s_sprite, uv, 0.0).rgb);
}

// The blurred backdrop (linear light) at a frame position.
fn glass_blurred(effect: EffectParams, position: vec2<f32>) -> vec3<f32> {
    let scratch = (position - effect.region.origin) / effect.downsample;
    let uv = clamp(scratch, vec2<f32>(0.5), effect.scratch_size - vec2<f32>(0.5)) / effect.texture_size;
    return srgb_to_linear(textureSampleLevel(t_glass_blur, s_sprite, uv, 0.0).rgb);
}

// Height profile slope of the bezel, u = 0 at the outer edge and 1 where the glass is flat.
// A convex squircle: it leaves the edge nearly vertical and flattens gradually, so the
// bending is concentrated in a thin rim.
fn glass_bezel_slope(u: f32) -> f32 {
    let a = 1.0 - u;
    let a3 = a * a * a;
    let f = max(1.0 - a3 * a, 1e-4);
    return a3 / pow(f, 0.75);
}

struct GlassVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) effect_id: u32,
    @location(1) clip_distances: vec4<f32>,
}

@vertex
fn vs_glass(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> GlassVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let effect = b_effects[instance_id];
    var expanded: Bounds;
    expanded.origin = effect.bounds.origin - vec2<f32>(effect.outset);
    expanded.size = effect.bounds.size + vec2<f32>(effect.outset * 2.0);
    var out = GlassVarying();
    out.position = to_device_position(unit_vertex, expanded);
    out.effect_id = instance_id;
    out.clip_distances = distance_from_clip_rect(unit_vertex, expanded, effect.content_mask);
    return out;
}

// Output is premultiplied; the pipeline blends with PREMULTIPLIED_ALPHA_BLENDING.
@fragment
fn fs_glass(in: GlassVarying) -> @location(0) vec4<f32> {
    if (any(in.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }
    let effect = b_effects[in.effect_id];
    let params = effect.glass;
    let half_size = effect.bounds.size * 0.5;
    let center = effect.bounds.origin + half_size;
    let frag = in.position.xy;
    let p = frag - center;
    let min_half = min(half_size.x, half_size.y);
    // A corner radius that fills the shape (capsule, circle) is a true semicircle; the
    // continuous-corner exponent only applies to genuinely rounded rectangles.
    let largest_radius = max(
        max(effect.corner_radii.top_left, effect.corner_radii.top_right),
        max(effect.corner_radii.bottom_right, effect.corner_radii.bottom_left),
    );
    let corner_exponent = mix(
        max(params.state.x, 2.0),
        2.0,
        smoothstep(0.8, 0.98, largest_radius / max(min_half, 1.0)),
    );
    let sdf = glass_sdf(p, half_size, effect.corner_radii, corner_exponent);
    let appear = clamp(params.state.z, 0.0, 1.0) * effect.opacity;
    let coverage = clamp(0.5 - sdf, 0.0, 1.0);

    // Shadow: painted only outside the shape, since the glass itself is see-through.
    var shadow_alpha = 0.0;
    if (params.shadow.z > 0.0) {
        let sigma = max(params.shadow.y * 0.5, 0.5);
        let shadow_sdf = glass_sdf(
            p - vec2<f32>(0.0, params.shadow.x),
            half_size + vec2<f32>(params.shadow.w),
            effect.corner_radii,
            corner_exponent,
        );
        let falloff = 0.5 * (1.0 - tanh(1.2028 * shadow_sdf / (sigma * 1.41421356)));
        // A blur wider than the shape would otherwise make the shadow as dark as a big one.
        let size_factor = clamp(min_half * 2.0 / (sigma * 3.0), 0.2, 1.0);
        shadow_alpha = params.shadow.z * falloff * size_factor * (1.0 - coverage) * appear;
    }
    // A tight, faint line where the glass meets what is behind it.
    if (params.edge.z > 0.0) {
        shadow_alpha += params.edge.z * exp(-max(sdf, 0.0) / 1.4) * (1.0 - coverage) * appear;
    }
    if (sdf > 0.5) {
        return vec4<f32>(0.0, 0.0, 0.0, select(0.0, max(shadow_alpha + glass_dither(frag), 0.0), shadow_alpha > 0.0));
    }

    let depth = max(-sdf, 0.0);
    let bezel = clamp(params.optics.y, 1.0, max(min_half * 0.98, 1.0));
    let normal = glass_normal(p, half_size, effect.corner_radii, corner_exponent, bezel);

    // Refraction: a ray entering the sloped bezel is bent by Snell's law; the displacement
    // of where it lands on the backdrop is the sampling offset.
    let refraction_amount = params.optics.x * appear;
    let ior = max(params.optics.z, 1.02);
    var offset = vec2<f32>(0.0);
    if (depth < bezel && refraction_amount != 0.0) {
        let u = clamp(depth / bezel, 0.0, 1.0);
        let slope = glass_bezel_slope(u) * params.optics.w / bezel;
        let surface_normal = normalize(vec3<f32>(normal * slope, 1.0));
        let refracted = refract(vec3<f32>(0.0, 0.0, -1.0), surface_normal, 1.0 / ior);
        let tangent = length(refracted.xy) / max(-refracted.z, 1e-3);
        // The steepest possible slope bends the ray to asin(1 / ior).
        let sine_limit = 1.0 / ior;
        let tangent_limit = sine_limit / sqrt(1.0 - sine_limit * sine_limit);
        offset = -normal * (clamp(tangent / tangent_limit, 0.0, 1.0) * refraction_amount);
    }

    let sample_position = frag + offset;
    let dispersion = params.blur.z;
    var color = glass_sharp(effect, sample_position);
    if (dispersion != 0.0) {
        color = vec3<f32>(
            glass_sharp(effect, frag + offset * (1.0 - dispersion)).r,
            color.g,
            glass_sharp(effect, frag + offset * (1.0 + dispersion)).b,
        );
    }
    let blur_radius = params.blur.x * appear;
    if (blur_radius > 0.5) {
        let blurred = glass_blurred(effect, sample_position);
        // The rim keeps more of the sharp, refracted backdrop than the interior does, so the
        // ends and edges of the glass stay see-through while the middle is frosted.
        let blur_mix = mix(params.blur.y, 1.0, smoothstep(0.0, bezel * 1.5, depth));
        color = mix(color, blurred, blur_mix);
    }

    // Local backdrop luminance decides whether glass with automatic appearance reads as
    // light or dark.
    var appearance = params.tone.w;
    if (appearance < 0.0) {
        let region_gamma_luma = linear_to_srgb(vec3<f32>(dot(glass_blurred(effect, center), GLASS_LUMA))).x;
        appearance = 1.0 - smoothstep(0.28, 0.62, region_gamma_luma);
    }

    // Tone is applied to encoded values, where brightness and contrast behave intuitively.
    var encoded = linear_to_srgb(max(color, vec3<f32>(0.0)));
    let untoned = encoded;
    encoded = (encoded - vec3<f32>(0.5)) * params.tone.y + vec3<f32>(0.5);
    encoded = encoded * params.tone.x;
    let veil_color = mix(vec3<f32>(1.0), vec3<f32>(0.06), appearance);
    encoded = mix(encoded, veil_color, params.tone.z);
    let encoded_luma = dot(encoded, GLASS_LUMA);
    encoded = mix(vec3<f32>(encoded_luma), encoded, params.blur.w);
    encoded = clamp(encoded, vec3<f32>(0.0), vec3<f32>(1.0));

    // Tint is a tone ramp keyed to the brightness underneath, not a flat overlay.
    if (params.tint.a > 0.0) {
        let brightness = clamp(dot(encoded, GLASS_LUMA), 0.0, 1.0);
        let tinted = params.tint.rgb * mix(0.45, 1.25, brightness);
        encoded = mix(encoded, clamp(tinted, vec3<f32>(0.0), vec3<f32>(1.0)), clamp(params.tint.a, 0.0, 1.0) * 0.9);
    }
    // Darkening just inside the rim, fading inward.
    if (params.edge.x > 0.0) {
        let band = exp(-depth / max(params.edge.y, 0.5)) * params.edge.x * (1.0 - appearance);
        encoded = encoded * (1.0 - clamp(band, 0.0, 0.9));
    }
    // At appear = 0 the material is the plain backdrop; lensing, tone and shading arrive together.
    encoded = mix(untoned, encoded, appear);

    // Edge light, after Apple's two-light model: a thin specular hairline whose strength
    // follows how much the outline faces each of two opposed lights, over a faint ambient
    // outline that runs all the way round, plus a short soft bevel just inside it on the lit
    // sides. It is screened over the face in encoded space (a highlight lightens towards
    // white; adding it in linear light reads as grey fog on dark glass).
    let light_0 = vec2<f32>(cos(params.light.x), -sin(params.light.x));
    let light_1 = vec2<f32>(cos(params.light.y), -sin(params.light.y));
    let facing_0 = pow(max(dot(normal, light_0), 0.0), params.rim.z);
    let facing_1 = pow(max(dot(normal, light_1), 0.0), params.rim.z);
    let lit = clamp(facing_0 * params.light.z + facing_1 * params.light.w, 0.0, 1.5);
    let rim_width = max(params.rim.x, 0.25);
    // The hairline's profile is offset per colour channel, so wherever it is lit its inner
    // edge fringes into colour (seen in Apple's footage on a plain grey backdrop).
    let channel_shift = vec3<f32>(-1.0, 0.0, 1.0) * params.edge.w;
    let rim_profile = exp(-pow(max(vec3<f32>(depth) + channel_shift, vec3<f32>(0.0)) / rim_width, vec3<f32>(params.rim.y)));
    let bevel = exp(-depth / max(params.finish.z, 0.5)) * params.finish.y * lit;
    let rim_energy = clamp((params.finish.x + lit) * rim_profile + vec3<f32>(bevel), vec3<f32>(0.0), vec3<f32>(1.0)) * appear;
    // Apple's rim is the backdrop's own colour lifted, not white: over a teal face it is a pale
    // teal. The face colour, normalised to its brightest channel, tints the light.
    let face_hue = encoded / max(max(encoded.r, max(encoded.g, encoded.b)), 0.08);
    let rim_light = mix(vec3<f32>(1.0), face_hue, 0.55);
    encoded = vec3<f32>(1.0) - (vec3<f32>(1.0) - encoded) * (vec3<f32>(1.0) - rim_energy * rim_light);

    // Light that starts under the pointer and spreads across the surface.
    if (params.pointer.z > 0.0 && params.pointer.w > 0.0) {
        let pointer_distance = length(frag - (effect.bounds.origin + params.pointer.xy));
        let radius = params.pointer.z * 0.5;
        encoded += vec3<f32>(params.pointer.w * 0.2 * exp(-(pointer_distance * pointer_distance) / (2.0 * radius * radius)));
    }

    let output_encoded = clamp(encoded, vec3<f32>(0.0), vec3<f32>(1.0)) + vec3<f32>(glass_dither(frag));
    return vec4<f32>(output_encoded * coverage, coverage + shadow_alpha);
}
