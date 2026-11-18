#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

float4 hsla_to_rgba(Hsla hsla);
float3 srgb_to_linear(float3 color);
float3 linear_to_srgb(float3 color);
float4 srgb_to_oklab(float4 color);
float4 oklab_to_srgb(float4 color);
float4 to_device_position(float2 unit_vertex, Bounds_ScaledPixels bounds,
                          constant Size_DevicePixels *viewport_size);
float4 to_device_position_transformed(float2 unit_vertex, Bounds_ScaledPixels bounds,
                          TransformationMatrix transformation,
                          constant Size_DevicePixels *input_viewport_size);

float2 to_tile_position(float2 unit_vertex, AtlasTile tile,
                        constant Size_DevicePixels *atlas_size);
float4 distance_from_clip_rect(float2 unit_vertex, Bounds_ScaledPixels bounds,
                               Bounds_ScaledPixels clip_bounds);
float4 distance_from_clip_rect_transformed(float2 unit_vertex, Bounds_ScaledPixels bounds,
                               Bounds_ScaledPixels clip_bounds, TransformationMatrix transformation);
float corner_dash_velocity(float dv1, float dv2);
float dash_alpha(float t, float period, float length, float dash_velocity,
                 float antialias_threshold);
float quarter_ellipse_sdf(float2 point, float2 radii);
float pick_corner_radius(float2 center_to_point, Corners_ScaledPixels corner_radii);
float quad_sdf(float2 point, Bounds_ScaledPixels bounds,
               Corners_ScaledPixels corner_radii);
float quad_sdf_impl(float2 center_to_point, float corner_radius);
float gaussian(float x, float sigma);
float2 erf(float2 x);
float blur_along_x(float x, float y, float sigma, float corner,
                   float2 half_size);
float4 over(float4 below, float4 above);
float radians(float degrees);
float4 fill_color(Background background, float2 position, Bounds_ScaledPixels bounds,
  float4 solid_color, float4 color0, float4 color1);

struct GradientColor {
  float4 solid;
  float4 color0;
  float4 color1;
};
GradientColor prepare_fill_color(uint tag, uint color_space, Hsla solid, Hsla color0, Hsla color1);

struct QuadVertexOutput {
  uint quad_id [[flat]];
  float4 position [[position]];
  float4 border_color [[flat]];
  float4 background_solid [[flat]];
  float4 background_color0 [[flat]];
  float4 background_color1 [[flat]];
  float clip_distance [[clip_distance]][4];
};

struct QuadFragmentInput {
  uint quad_id [[flat]];
  float4 position [[position]];
  float4 border_color [[flat]];
  float4 background_solid [[flat]];
  float4 background_color0 [[flat]];
  float4 background_color1 [[flat]];
};

vertex QuadVertexOutput quad_vertex(uint unit_vertex_id [[vertex_id]],
                                    uint quad_id [[instance_id]],
                                    constant float2 *unit_vertices
                                    [[buffer(QuadInputIndex_Vertices)]],
                                    constant Quad *quads
                                    [[buffer(QuadInputIndex_Quads)]],
                                    constant Size_DevicePixels *viewport_size
                                    [[buffer(QuadInputIndex_ViewportSize)]]) {
  float2 unit_vertex = unit_vertices[unit_vertex_id];
  Quad quad = quads[quad_id];
  float4 device_position =
      to_device_position(unit_vertex, quad.bounds, viewport_size);
  float4 clip_distance = distance_from_clip_rect(unit_vertex, quad.bounds,
                                                 quad.content_mask.bounds);
  float4 border_color = hsla_to_rgba(quad.border_color);

  GradientColor gradient = prepare_fill_color(
    quad.background.tag,
    quad.background.color_space,
    quad.background.solid,
    quad.background.colors[0].color,
    quad.background.colors[1].color
  );

  return QuadVertexOutput{
      quad_id,
      device_position,
      border_color,
      gradient.solid,
      gradient.color0,
      gradient.color1,
      {clip_distance.x, clip_distance.y, clip_distance.z, clip_distance.w}};
}

fragment float4 quad_fragment(QuadFragmentInput input [[stage_in]],
                              constant Quad *quads
                              [[buffer(QuadInputIndex_Quads)]]) {
  Quad quad = quads[input.quad_id];
  float4 background_color = fill_color(quad.background, input.position.xy, quad.bounds,
    input.background_solid, input.background_color0, input.background_color1);

  bool unrounded = quad.corner_radii.top_left == 0.0 &&
    quad.corner_radii.bottom_left == 0.0 &&
    quad.corner_radii.top_right == 0.0 &&
    quad.corner_radii.bottom_right == 0.0;

  // Fast path when the quad is not rounded and doesn't have any border
  if (quad.border_widths.top == 0.0 &&
      quad.border_widths.left == 0.0 &&
      quad.border_widths.right == 0.0 &&
      quad.border_widths.bottom == 0.0 &&
      unrounded) {
    return background_color;
  }

  float2 size = float2(quad.bounds.size.width, quad.bounds.size.height);
  float2 half_size = size / 2.0;
  float2 point = input.position.xy - float2(quad.bounds.origin.x, quad.bounds.origin.y);
  float2 center_to_point = point - half_size;

  // Signed distance field threshold for inclusion of pixels. 0.5 is the
  // minimum distance between the center of the pixel and the edge.
  const float antialias_threshold = 0.5;

  // Radius of the nearest corner
  float corner_radius = pick_corner_radius(center_to_point, quad.corner_radii);

  // Width of the nearest borders
  float2 border = float2(
    center_to_point.x < 0.0 ? quad.border_widths.left : quad.border_widths.right,
    center_to_point.y < 0.0 ? quad.border_widths.top : quad.border_widths.bottom
  );

  // 0-width borders are reduced so that `inner_sdf >= antialias_threshold`.
  // The purpose of this is to not draw antialiasing pixels in this case.
  float2 reduced_border = float2(
    border.x == 0.0 ? -antialias_threshold : border.x,
    border.y == 0.0 ? -antialias_threshold : border.y);

  // Vector from the corner of the quad bounds to the point, after mirroring
  // the point into the bottom right quadrant. Both components are <= 0.
  float2 corner_to_point = fabs(center_to_point) - half_size;

  // Vector from the point to the center of the rounded corner's circle, also
  // mirrored into bottom right quadrant.
  float2 corner_center_to_point = corner_to_point + corner_radius;

  // Whether the nearest point on the border is rounded
  bool is_near_rounded_corner =
    corner_center_to_point.x >= 0.0 &&
    corner_center_to_point.y >= 0.0;

  // Vector from straight border inner corner to point.
  //
  // 0-width borders are turned into width -1 so that inner_sdf is > 1.0 near
  // the border. Without this, antialiasing pixels would be drawn.
  float2 straight_border_inner_corner_to_point = corner_to_point + reduced_border;

  // Whether the point is beyond the inner edge of the straight border
  bool is_beyond_inner_straight_border =
    straight_border_inner_corner_to_point.x > 0.0 ||
    straight_border_inner_corner_to_point.y > 0.0;


  // Whether the point is far enough inside the quad, such that the pixels are
  // not affected by the straight border.
  bool is_within_inner_straight_border =
    straight_border_inner_corner_to_point.x < -antialias_threshold &&
    straight_border_inner_corner_to_point.y < -antialias_threshold;

  // Fast path for points that must be part of the background
  if (is_within_inner_straight_border && !is_near_rounded_corner) {
    return background_color;
  }

  // Signed distance of the point to the outside edge of the quad's border
  float outer_sdf = quad_sdf_impl(corner_center_to_point, corner_radius);

  // Approximate signed distance of the point to the inside edge of the quad's
  // border. It is negative outside this edge (within the border), and
  // positive inside.
  //
  // This is not always an accurate signed distance:
  // * The rounded portions with varying border width use an approximation of
  //   nearest-point-on-ellipse.
  // * When it is quickly known to be outside the edge, -1.0 is used.
  float inner_sdf = 0.0;
  if (corner_center_to_point.x <= 0.0 || corner_center_to_point.y <= 0.0) {
    // Fast paths for straight borders
    inner_sdf = -max(straight_border_inner_corner_to_point.x,
                     straight_border_inner_corner_to_point.y);
  } else if (is_beyond_inner_straight_border) {
    // Fast path for points that must be outside the inner edge
    inner_sdf = -1.0;
  } else if (reduced_border.x == reduced_border.y) {
    // Fast path for circular inner edge.
    inner_sdf = -(outer_sdf + reduced_border.x);
  } else {
    float2 ellipse_radii = max(float2(0.0), float2(corner_radius) - reduced_border);
    inner_sdf = quarter_ellipse_sdf(corner_center_to_point, ellipse_radii);
  }

  // Negative when inside the border
  float border_sdf = max(inner_sdf, outer_sdf);

  float4 color = background_color;
  if (border_sdf < antialias_threshold) {
    float4 border_color = input.border_color;

    // Dashed border logic when border_style == 1
    if (quad.border_style == 1) {
      // Position along the perimeter in "dash space", where each dash
      // period has length 1
      float t = 0.0;

      // Total number of dash periods, so that the dash spacing can be
      // adjusted to evenly divide it
      float max_t = 0.0;

      // Border width is proportional to dash size. This is the behavior
      // used by browsers, but also avoids dashes from different segments
      // overlapping when dash size is smaller than the border width.
      //
      // Dash pattern: (2 * border width) dash, (1 * border width) gap
      const float dash_length_per_width = 2.0;
      const float dash_gap_per_width = 1.0;
      const float dash_period_per_width = dash_length_per_width + dash_gap_per_width;

      // Since the dash size is determined by border width, the density of
      // dashes varies. Multiplying a pixel distance by this returns a
      // position in dash space - it has units (dash period / pixels). So
      // a dash velocity of (1 / 10) is 1 dash every 10 pixels.
      float dash_velocity = 0.0;

      // Dividing this by the border width gives the dash velocity
      const float dv_numerator = 1.0 / dash_period_per_width;

      if (unrounded) {
        // When corners aren't rounded, the dashes are separately laid
        // out on each straight line, rather than around the whole
        // perimeter. This way each line starts and ends with a dash.
        bool is_horizontal = corner_center_to_point.x < corner_center_to_point.y;

        // Choosing the right border width for dashed borders.
        // TODO: A better solution exists taking a look at the whole file.
        // this does not fix single dashed borders at the corners
        float2 dashed_border = float2(
        fmax(quad.border_widths.bottom, quad.border_widths.top),
        fmax(quad.border_widths.right, quad.border_widths.left));

        float border_width = is_horizontal ? dashed_border.x : dashed_border.y;
        dash_velocity = dv_numerator / border_width;
        t = is_horizontal ? point.x : point.y;
        t *= dash_velocity;
        max_t = is_horizontal ? size.x : size.y;
        max_t *= dash_velocity;
      } else {
        // When corners are rounded, the dashes are laid out clockwise
        // around the whole perimeter.

        float r_tr = quad.corner_radii.top_right;
        float r_br = quad.corner_radii.bottom_right;
        float r_bl = quad.corner_radii.bottom_left;
        float r_tl = quad.corner_radii.top_left;

        float w_t = quad.border_widths.top;
        float w_r = quad.border_widths.right;
        float w_b = quad.border_widths.bottom;
        float w_l = quad.border_widths.left;

        // Straight side dash velocities
        float dv_t = w_t <= 0.0 ? 0.0 : dv_numerator / w_t;
        float dv_r = w_r <= 0.0 ? 0.0 : dv_numerator / w_r;
        float dv_b = w_b <= 0.0 ? 0.0 : dv_numerator / w_b;
        float dv_l = w_l <= 0.0 ? 0.0 : dv_numerator / w_l;

        // Straight side lengths in dash space
        float s_t = (size.x - r_tl - r_tr) * dv_t;
        float s_r = (size.y - r_tr - r_br) * dv_r;
        float s_b = (size.x - r_br - r_bl) * dv_b;
        float s_l = (size.y - r_bl - r_tl) * dv_l;

        float corner_dash_velocity_tr = corner_dash_velocity(dv_t, dv_r);
        float corner_dash_velocity_br = corner_dash_velocity(dv_b, dv_r);
        float corner_dash_velocity_bl = corner_dash_velocity(dv_b, dv_l);
        float corner_dash_velocity_tl = corner_dash_velocity(dv_t, dv_l);

        // Corner lengths in dash space
        float c_tr = r_tr * (M_PI_F / 2.0) * corner_dash_velocity_tr;
        float c_br = r_br * (M_PI_F / 2.0) * corner_dash_velocity_br;
        float c_bl = r_bl * (M_PI_F / 2.0) * corner_dash_velocity_bl;
        float c_tl = r_tl * (M_PI_F / 2.0) * corner_dash_velocity_tl;

        // Cumulative dash space upto each segment
        float upto_tr = s_t;
        float upto_r = upto_tr + c_tr;
        float upto_br = upto_r + s_r;
        float upto_b = upto_br + c_br;
        float upto_bl = upto_b + s_b;
        float upto_l = upto_bl + c_bl;
        float upto_tl = upto_l + s_l;
        max_t = upto_tl + c_tl;

        if (is_near_rounded_corner) {
          float radians = atan2(corner_center_to_point.y, corner_center_to_point.x);
          float corner_t = radians * corner_radius;

          if (center_to_point.x >= 0.0) {
            if (center_to_point.y < 0.0) {
              dash_velocity = corner_dash_velocity_tr;
              // Subtracted because radians is pi/2 to 0 when
              // going clockwise around the top right corner,
              // since the y axis has been flipped
              t = upto_r - corner_t * dash_velocity;
            } else {
              dash_velocity = corner_dash_velocity_br;
              // Added because radians is 0 to pi/2 when going
              // clockwise around the bottom-right corner
              t = upto_br + corner_t * dash_velocity;
            }
          } else {
            if (center_to_point.y >= 0.0) {
              dash_velocity = corner_dash_velocity_bl;
              // Subtracted because radians is pi/1 to 0 when
              // going clockwise around the bottom-left corner,
              // since the x axis has been flipped
              t = upto_l - corner_t * dash_velocity;
            } else {
              dash_velocity = corner_dash_velocity_tl;
              // Added because radians is 0 to pi/2 when going
              // clockwise around the top-left corner, since both
              // axis were flipped
              t = upto_tl + corner_t * dash_velocity;
            }
          }
        } else {
          // Straight borders
          bool is_horizontal = corner_center_to_point.x < corner_center_to_point.y;
          if (is_horizontal) {
            if (center_to_point.y < 0.0) {
              dash_velocity = dv_t;
              t = (point.x - r_tl) * dash_velocity;
            } else {
              dash_velocity = dv_b;
              t = upto_bl - (point.x - r_bl) * dash_velocity;
            }
          } else {
            if (center_to_point.x < 0.0) {
              dash_velocity = dv_l;
              t = upto_tl - (point.y - r_tl) * dash_velocity;
            } else {
              dash_velocity = dv_r;
              t = upto_r + (point.y - r_tr) * dash_velocity;
            }
          }
        }
      }

      float dash_length = dash_length_per_width / dash_period_per_width;
      float desired_dash_gap = dash_gap_per_width / dash_period_per_width;

      // Straight borders should start and end with a dash, so max_t is
      // reduced to cause this.
      max_t -= unrounded ? dash_length : 0.0;
      if (max_t >= 1.0) {
        // Adjust dash gap to evenly divide max_t
        float dash_count = floor(max_t);
        float dash_period = max_t / dash_count;
        border_color.a *= dash_alpha(t, dash_period, dash_length, dash_velocity,
                                     antialias_threshold);
      } else if (unrounded) {
        // When there isn't enough space for the full gap between the
        // two start / end dashes of a straight border, reduce gap to
        // make them fit.
        float dash_gap = max_t - dash_length;
        if (dash_gap > 0.0) {
          float dash_period = dash_length + dash_gap;
          border_color.a *= dash_alpha(t, dash_period, dash_length, dash_velocity,
                                       antialias_threshold);
        }
      }
    }

    // Blend the border on top of the background and then linearly interpolate
    // between the two as we slide inside the background.
    float4 blended_border = over(background_color, border_color);
    color = mix(background_color, blended_border,
                saturate(antialias_threshold - inner_sdf));
  }

  return color * float4(1.0, 1.0, 1.0, saturate(antialias_threshold - outer_sdf));
}

// Returns the dash velocity of a corner given the dash velocity of the two
// sides, by returning the slower velocity (larger dashes).
//
// Since 0 is used for dash velocity when the border width is 0 (instead of
// +inf), this returns the other dash velocity in that case.
//
// An alternative to this might be to appropriately interpolate the dash
// velocity around the corner, but that seems overcomplicated.
float corner_dash_velocity(float dv1, float dv2) {
  if (dv1 == 0.0) {
    return dv2;
  } else if (dv2 == 0.0) {
    return dv1;
  } else {
    return min(dv1, dv2);
  }
}

// Returns alpha used to render antialiased dashes.
// `t` is within the dash when `fmod(t, period) < length`.
float dash_alpha(
    float t, float period, float length, float dash_velocity,
    float antialias_threshold) {
  float half_period = period / 2.0;
  float half_length = length / 2.0;
  // Value in [-half_period, half_period]
  // The dash is in [-half_length, half_length]
  float centered = fmod(t + half_period - half_length, period) - half_period;
  // Signed distance for the dash, negative values are inside the dash
  float signed_distance = abs(centered) - half_length;
  // Antialiased alpha based on the signed distance
  return saturate(antialias_threshold - signed_distance / dash_velocity);
}

// This approximates distance to the nearest point to a quarter ellipse in a way
// that is sufficient for anti-aliasing when the ellipse is not very eccentric.
// The components of `point` are expected to be positive.
//
// Negative on the outside and positive on the inside.
float quarter_ellipse_sdf(float2 point, float2 radii) {
  // Scale the space to treat the ellipse like a unit circle
  float2 circle_vec = point / radii;
  float unit_circle_sdf = length(circle_vec) - 1.0;
  // Approximate up-scaling of the length by using the average of the radii.
  //
  // TODO: A better solution would be to use the gradient of the implicit
  // function for an ellipse to approximate a scaling factor.
  return unit_circle_sdf * (radii.x + radii.y) * -0.5;
}

struct ShadowVertexOutput {
  float4 position [[position]];
  float4 color [[flat]];
  uint shadow_id [[flat]];
  float clip_distance [[clip_distance]][4];
};

struct ShadowFragmentInput {
  float4 position [[position]];
  float4 color [[flat]];
  uint shadow_id [[flat]];
};

vertex ShadowVertexOutput shadow_vertex(
    uint unit_vertex_id [[vertex_id]], uint shadow_id [[instance_id]],
    constant float2 *unit_vertices [[buffer(ShadowInputIndex_Vertices)]],
    constant Shadow *shadows [[buffer(ShadowInputIndex_Shadows)]],
    constant Size_DevicePixels *viewport_size
    [[buffer(ShadowInputIndex_ViewportSize)]]) {
  float2 unit_vertex = unit_vertices[unit_vertex_id];
  Shadow shadow = shadows[shadow_id];

  Bounds_ScaledPixels bounds;
  if (shadow.inset != 0u) {
    bounds = shadow.element_bounds;
  } else {
    // Leave room for the gaussian tail outside the shadow rect.
    float margin = 3. * shadow.blur_radius;
    bounds = shadow.bounds;
    bounds.origin.x -= margin;
    bounds.origin.y -= margin;
    bounds.size.width += 2. * margin;
    bounds.size.height += 2. * margin;
  }

  float4 device_position =
      to_device_position(unit_vertex, bounds, viewport_size);
  float4 clip_distance =
      distance_from_clip_rect(unit_vertex, bounds, shadow.content_mask.bounds);
  float4 color = hsla_to_rgba(shadow.color);

  return ShadowVertexOutput{
      device_position,
      color,
      shadow_id,
      {clip_distance.x, clip_distance.y, clip_distance.z, clip_distance.w}};
}

fragment float4 shadow_fragment(ShadowFragmentInput input [[stage_in]],
                                constant Shadow *shadows
                                [[buffer(ShadowInputIndex_Shadows)]]) {
  Shadow shadow = shadows[input.shadow_id];

  float2 origin = float2(shadow.bounds.origin.x, shadow.bounds.origin.y);
  float2 size = float2(shadow.bounds.size.width, shadow.bounds.size.height);
  float2 half_size = size / 2.;
  float2 center = origin + half_size;
  float2 point = input.position.xy - center;
  float corner_radius;
  if (point.x < 0.) {
    if (point.y < 0.) {
      corner_radius = shadow.corner_radii.top_left;
    } else {
      corner_radius = shadow.corner_radii.bottom_left;
    }
  } else {
    if (point.y < 0.) {
      corner_radius = shadow.corner_radii.top_right;
    } else {
      corner_radius = shadow.corner_radii.bottom_right;
    }
  }

  float alpha;
  if (shadow.blur_radius == 0.) {
    float distance = quad_sdf(input.position.xy, shadow.bounds, shadow.corner_radii);
    alpha = saturate(0.5 - distance);
  } else {
    // The signal is only non-zero in a limited range, so don't waste samples
    float low = point.y - half_size.y;
    float high = point.y + half_size.y;
    float start = clamp(-3. * shadow.blur_radius, low, high);
    float end = clamp(3. * shadow.blur_radius, low, high);

    // Accumulate samples (we can get away with surprisingly few samples)
    float step = (end - start) / 4.;
    float y = start + step * 0.5;
    alpha = 0.;
    for (int i = 0; i < 4; i++) {
      alpha += blur_along_x(point.x, point.y - y, shadow.blur_radius,
                            corner_radius, half_size) *
               gaussian(y, shadow.blur_radius) * step;
      y += step;
    }
  }

  if (shadow.inset != 0u) {
    // The inset shadow is the complement of the (blurred) hole rect, clipped to the element.
    // `saturate(0.5 - d)` gives a 1-pixel antialiased edge: d <= -0.5 -> 1, d >= 0.5 -> 0.
    alpha = 1. - alpha;
    float element_distance = quad_sdf(input.position.xy, shadow.element_bounds,
                                      shadow.element_corner_radii);
    alpha *= saturate(0.5 - element_distance);
  }

  return input.color * float4(1., 1., 1., alpha);
}

struct UnderlineVertexOutput {
  float4 position [[position]];
  float4 color [[flat]];
  uint underline_id [[flat]];
  float clip_distance [[clip_distance]][4];
};

struct UnderlineFragmentInput {
  float4 position [[position]];
  float4 color [[flat]];
  uint underline_id [[flat]];
};

vertex UnderlineVertexOutput underline_vertex(
    uint unit_vertex_id [[vertex_id]], uint underline_id [[instance_id]],
    constant float2 *unit_vertices [[buffer(UnderlineInputIndex_Vertices)]],
    constant Underline *underlines [[buffer(UnderlineInputIndex_Underlines)]],
    constant Size_DevicePixels *viewport_size
    [[buffer(ShadowInputIndex_ViewportSize)]]) {
  float2 unit_vertex = unit_vertices[unit_vertex_id];
  Underline underline = underlines[underline_id];
  float4 device_position =
      to_device_position(unit_vertex, underline.bounds, viewport_size);
  float4 clip_distance = distance_from_clip_rect(unit_vertex, underline.bounds,
                                                 underline.content_mask.bounds);
  float4 color = hsla_to_rgba(underline.color);
  return UnderlineVertexOutput{
      device_position,
      color,
      underline_id,
      {clip_distance.x, clip_distance.y, clip_distance.z, clip_distance.w}};
}

fragment float4 underline_fragment(UnderlineFragmentInput input [[stage_in]],
                                   constant Underline *underlines
                                   [[buffer(UnderlineInputIndex_Underlines)]]) {
  const float WAVE_FREQUENCY = 2.0;
  const float WAVE_HEIGHT_RATIO = 0.8;

  Underline underline = underlines[input.underline_id];
  if (underline.wavy) {
    float half_thickness = underline.thickness * 0.5;
    float2 origin =
        float2(underline.bounds.origin.x, underline.bounds.origin.y);

    float2 st = ((input.position.xy - origin) / underline.bounds.size.height) -
                float2(0., 0.5);
    float frequency = (M_PI_F * WAVE_FREQUENCY * underline.thickness) / underline.bounds.size.height;
    float amplitude = (underline.thickness * WAVE_HEIGHT_RATIO) / underline.bounds.size.height;

    float sine = sin(st.x * frequency) * amplitude;
    float dSine = cos(st.x * frequency) * amplitude * frequency;
    float distance = (st.y - sine) / sqrt(1. + dSine * dSine);
    float distance_in_pixels = distance * underline.bounds.size.height;
    float distance_from_top_border = distance_in_pixels - half_thickness;
    float distance_from_bottom_border = distance_in_pixels + half_thickness;
    float alpha = saturate(
        0.5 - max(-distance_from_bottom_border, distance_from_top_border));
    return input.color * float4(1., 1., 1., alpha);
  } else {
    return input.color;
  }
}

struct MonochromeSpriteVertexOutput {
  float4 position [[position]];
  float2 tile_position;
  float4 color [[flat]];
  float4 clip_distance;
};

struct MonochromeSpriteFragmentInput {
  float4 position [[position]];
  float2 tile_position;
  float4 color [[flat]];
  float4 clip_distance;
};

vertex MonochromeSpriteVertexOutput monochrome_sprite_vertex(
    uint unit_vertex_id [[vertex_id]], uint sprite_id [[instance_id]],
    constant float2 *unit_vertices [[buffer(SpriteInputIndex_Vertices)]],
    constant MonochromeSprite *sprites [[buffer(SpriteInputIndex_Sprites)]],
    constant Size_DevicePixels *viewport_size
    [[buffer(SpriteInputIndex_ViewportSize)]],
    constant Size_DevicePixels *atlas_size
    [[buffer(SpriteInputIndex_AtlasTextureSize)]]) {
  float2 unit_vertex = unit_vertices[unit_vertex_id];
  MonochromeSprite sprite = sprites[sprite_id];
  float4 device_position =
      to_device_position_transformed(unit_vertex, sprite.bounds, sprite.transformation, viewport_size);
  float4 clip_distance = distance_from_clip_rect_transformed(unit_vertex, sprite.bounds,
                                                 sprite.content_mask.bounds, sprite.transformation);
  float2 tile_position = to_tile_position(unit_vertex, sprite.tile, atlas_size);
  float4 color = hsla_to_rgba(sprite.color);
  return MonochromeSpriteVertexOutput{
      device_position,
      tile_position,
      color,
      {clip_distance.x, clip_distance.y, clip_distance.z, clip_distance.w}};
}

fragment float4 monochrome_sprite_fragment(
    MonochromeSpriteFragmentInput input [[stage_in]],
    constant MonochromeSprite *sprites [[buffer(SpriteInputIndex_Sprites)]],
    texture2d<float> atlas_texture [[texture(SpriteInputIndex_AtlasTexture)]]) {
  if (any(input.clip_distance < float4(0.0))) {
    return float4(0.0);
  }

  constexpr sampler atlas_texture_sampler(mag_filter::linear,
                                          min_filter::linear);
  float4 sample =
      atlas_texture.sample(atlas_texture_sampler, input.tile_position);
  float4 color = input.color;
  color.a *= sample.a;
  return color;
}

struct PolychromeSpriteVertexOutput {
  float4 position [[position]];
  float2 tile_position;
  uint sprite_id [[flat]];
  float clip_distance [[clip_distance]][4];
};

struct PolychromeSpriteFragmentInput {
  float4 position [[position]];
  float2 tile_position;
  uint sprite_id [[flat]];
};

vertex PolychromeSpriteVertexOutput polychrome_sprite_vertex(
    uint unit_vertex_id [[vertex_id]], uint sprite_id [[instance_id]],
    constant float2 *unit_vertices [[buffer(SpriteInputIndex_Vertices)]],
    constant PolychromeSprite *sprites [[buffer(SpriteInputIndex_Sprites)]],
    constant Size_DevicePixels *viewport_size
    [[buffer(SpriteInputIndex_ViewportSize)]],
    constant Size_DevicePixels *atlas_size
    [[buffer(SpriteInputIndex_AtlasTextureSize)]]) {

  float2 unit_vertex = unit_vertices[unit_vertex_id];
  PolychromeSprite sprite = sprites[sprite_id];
  float4 device_position =
      to_device_position(unit_vertex, sprite.bounds, viewport_size);
  float4 clip_distance = distance_from_clip_rect(unit_vertex, sprite.bounds,
                                                 sprite.content_mask.bounds);
  float2 tile_position = to_tile_position(unit_vertex, sprite.tile, atlas_size);
  return PolychromeSpriteVertexOutput{
      device_position,
      tile_position,
      sprite_id,
      {clip_distance.x, clip_distance.y, clip_distance.z, clip_distance.w}};
}

fragment float4 polychrome_sprite_fragment(
    PolychromeSpriteFragmentInput input [[stage_in]],
    constant PolychromeSprite *sprites [[buffer(SpriteInputIndex_Sprites)]],
    texture2d<float> atlas_texture [[texture(SpriteInputIndex_AtlasTexture)]]) {
  PolychromeSprite sprite = sprites[input.sprite_id];
  constexpr sampler atlas_texture_sampler(mag_filter::linear,
                                          min_filter::linear);
  float4 sample =
      atlas_texture.sample(atlas_texture_sampler, input.tile_position);
  float distance =
      quad_sdf(input.position.xy, sprite.bounds, sprite.corner_radii);

  float4 color = sample;
  if (sprite.grayscale) {
    float grayscale = 0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b;
    color.r = grayscale;
    color.g = grayscale;
    color.b = grayscale;
  }
  color.a *= sprite.opacity * saturate(0.5 - distance);
  return color;
}

struct PathRasterizationVertexOutput {
  float4 position [[position]];
  float2 st_position;
  uint vertex_id [[flat]];
  float clip_rect_distance [[clip_distance]][4];
};

struct PathRasterizationFragmentInput {
  float4 position [[position]];
  float2 st_position;
  uint vertex_id [[flat]];
};

vertex PathRasterizationVertexOutput path_rasterization_vertex(
  uint vertex_id [[vertex_id]],
  constant PathRasterizationVertex *vertices [[buffer(PathRasterizationInputIndex_Vertices)]],
  constant Size_DevicePixels *atlas_size [[buffer(PathRasterizationInputIndex_ViewportSize)]]
) {
  PathRasterizationVertex v = vertices[vertex_id];
  float2 vertex_position = float2(v.xy_position.x, v.xy_position.y);
  float4 position = float4(
    vertex_position * float2(2. / atlas_size->width, -2. / atlas_size->height) + float2(-1., 1.),
    0.,
    1.
  );
  return PathRasterizationVertexOutput{
      position,
      float2(v.st_position.x, v.st_position.y),
      vertex_id,
      {
        v.xy_position.x - v.bounds.origin.x,
        v.bounds.origin.x + v.bounds.size.width - v.xy_position.x,
        v.xy_position.y - v.bounds.origin.y,
        v.bounds.origin.y + v.bounds.size.height - v.xy_position.y
      }
  };
}

fragment float4 path_rasterization_fragment(
  PathRasterizationFragmentInput input [[stage_in]],
  constant PathRasterizationVertex *vertices [[buffer(PathRasterizationInputIndex_Vertices)]]
) {
  float2 dx = dfdx(input.st_position);
  float2 dy = dfdy(input.st_position);

  PathRasterizationVertex v = vertices[input.vertex_id];
  Background background = v.color;
  Bounds_ScaledPixels path_bounds = v.bounds;
  float alpha;
  if (length(float2(dx.x, dy.x)) < 0.001) {
    alpha = 1.0;
  } else {
    float2 gradient = float2(
      (2. * input.st_position.x) * dx.x - dx.y,
      (2. * input.st_position.x) * dy.x - dy.y
    );
    float f = (input.st_position.x * input.st_position.x) - input.st_position.y;
    float distance = f / length(gradient);
    alpha = saturate(0.5 - distance);
  }

  GradientColor gradient_color = prepare_fill_color(
    background.tag,
    background.color_space,
    background.solid,
    background.colors[0].color,
    background.colors[1].color
  );

  float4 color = fill_color(
    background,
    input.position.xy,
    path_bounds,
    gradient_color.solid,
    gradient_color.color0,
    gradient_color.color1
  );
  return float4(color.rgb * color.a * alpha, alpha * color.a);
}

struct PathSpriteVertexOutput {
  float4 position [[position]];
  float2 texture_coords;
};

vertex PathSpriteVertexOutput path_sprite_vertex(
  uint unit_vertex_id [[vertex_id]],
  uint sprite_id [[instance_id]],
  constant float2 *unit_vertices [[buffer(SpriteInputIndex_Vertices)]],
  constant PathSprite *sprites [[buffer(SpriteInputIndex_Sprites)]],
  constant Size_DevicePixels *viewport_size [[buffer(SpriteInputIndex_ViewportSize)]]
) {
  float2 unit_vertex = unit_vertices[unit_vertex_id];
  PathSprite sprite = sprites[sprite_id];
  // Don't apply content mask because it was already accounted for when
  // rasterizing the path.
  float4 device_position =
      to_device_position(unit_vertex, sprite.bounds, viewport_size);

  float2 screen_position = float2(sprite.bounds.origin.x, sprite.bounds.origin.y) + unit_vertex * float2(sprite.bounds.size.width, sprite.bounds.size.height);
  float2 texture_coords = screen_position / float2(viewport_size->width, viewport_size->height);

  return PathSpriteVertexOutput{
    device_position,
    texture_coords
  };
}

fragment float4 path_sprite_fragment(
  PathSpriteVertexOutput input [[stage_in]],
  texture2d<float> intermediate_texture [[texture(SpriteInputIndex_AtlasTexture)]]
) {
  constexpr sampler intermediate_texture_sampler(mag_filter::linear, min_filter::linear);
  return intermediate_texture.sample(intermediate_texture_sampler, input.texture_coords);
}

struct SurfaceVertexOutput {
  float4 position [[position]];
  float2 texture_position;
  float clip_distance [[clip_distance]][4];
};

struct SurfaceFragmentInput {
  float4 position [[position]];
  float2 texture_position;
};

vertex SurfaceVertexOutput surface_vertex(
    uint unit_vertex_id [[vertex_id]], uint surface_id [[instance_id]],
    constant float2 *unit_vertices [[buffer(SurfaceInputIndex_Vertices)]],
    constant SurfaceBounds *surfaces [[buffer(SurfaceInputIndex_Surfaces)]],
    constant Size_DevicePixels *viewport_size
    [[buffer(SurfaceInputIndex_ViewportSize)]],
    constant Size_DevicePixels *texture_size
    [[buffer(SurfaceInputIndex_TextureSize)]]) {
  float2 unit_vertex = unit_vertices[unit_vertex_id];
  SurfaceBounds surface = surfaces[surface_id];
  float4 device_position =
      to_device_position(unit_vertex, surface.bounds, viewport_size);
  float4 clip_distance = distance_from_clip_rect(unit_vertex, surface.bounds,
                                                 surface.content_mask.bounds);
  // We are going to copy the whole texture, so the texture position corresponds
  // to the current vertex of the unit triangle.
  float2 texture_position = unit_vertex;
  return SurfaceVertexOutput{
      device_position,
      texture_position,
      {clip_distance.x, clip_distance.y, clip_distance.z, clip_distance.w}};
}

fragment float4 surface_fragment(SurfaceFragmentInput input [[stage_in]],
                                 texture2d<float> y_texture
                                 [[texture(SurfaceInputIndex_YTexture)]],
                                 texture2d<float> cb_cr_texture
                                 [[texture(SurfaceInputIndex_CbCrTexture)]]) {
  constexpr sampler texture_sampler(mag_filter::linear, min_filter::linear);
  const float4x4 ycbcrToRGBTransform =
      float4x4(float4(+1.0000f, +1.0000f, +1.0000f, +0.0000f),
               float4(+0.0000f, -0.3441f, +1.7720f, +0.0000f),
               float4(+1.4020f, -0.7141f, +0.0000f, +0.0000f),
               float4(-0.7010f, +0.5291f, -0.8860f, +1.0000f));
  float4 ycbcr = float4(
      y_texture.sample(texture_sampler, input.texture_position).r,
      cb_cr_texture.sample(texture_sampler, input.texture_position).rg, 1.0);

  return ycbcrToRGBTransform * ycbcr;
}

float4 hsla_to_rgba(Hsla hsla) {
  float h = hsla.h * 6.0; // Now, it's an angle but scaled in [0, 6) range
  float s = hsla.s;
  float l = hsla.l;
  float a = hsla.a;

  float c = (1.0 - fabs(2.0 * l - 1.0)) * s;
  float x = c * (1.0 - fabs(fmod(h, 2.0) - 1.0));
  float m = l - c / 2.0;

  float r = 0.0;
  float g = 0.0;
  float b = 0.0;

  if (h >= 0.0 && h < 1.0) {
    r = c;
    g = x;
    b = 0.0;
  } else if (h >= 1.0 && h < 2.0) {
    r = x;
    g = c;
    b = 0.0;
  } else if (h >= 2.0 && h < 3.0) {
    r = 0.0;
    g = c;
    b = x;
  } else if (h >= 3.0 && h < 4.0) {
    r = 0.0;
    g = x;
    b = c;
  } else if (h >= 4.0 && h < 5.0) {
    r = x;
    g = 0.0;
    b = c;
  } else {
    r = c;
    g = 0.0;
    b = x;
  }

  float4 rgba;
  rgba.x = (r + m);
  rgba.y = (g + m);
  rgba.z = (b + m);
  rgba.w = a;
  return rgba;
}

float3 srgb_to_linear(float3 color) {
  return pow(color, float3(2.2));
}

float3 linear_to_srgb(float3 color) {
  return pow(color, float3(1.0 / 2.2));
}

// Converts a sRGB color to the Oklab color space.
// Reference: https://bottosson.github.io/posts/oklab/#converting-from-linear-srgb-to-oklab
float4 srgb_to_oklab(float4 color) {
  // Convert non-linear sRGB to linear sRGB
  color = float4(srgb_to_linear(color.rgb), color.a);

  float l = 0.4122214708 * color.r + 0.5363325363 * color.g + 0.0514459929 * color.b;
  float m = 0.2119034982 * color.r + 0.6806995451 * color.g + 0.1073969566 * color.b;
  float s = 0.0883024619 * color.r + 0.2817188376 * color.g + 0.6299787005 * color.b;

  float l_ = pow(l, 1.0/3.0);
  float m_ = pow(m, 1.0/3.0);
  float s_ = pow(s, 1.0/3.0);

  return float4(
   	0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
   	1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
   	0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
   	color.a
  );
}

// Converts an Oklab color to the sRGB color space.
float4 oklab_to_srgb(float4 color) {
  float l_ = color.r + 0.3963377774 * color.g + 0.2158037573 * color.b;
  float m_ = color.r - 0.1055613458 * color.g - 0.0638541728 * color.b;
  float s_ = color.r - 0.0894841775 * color.g - 1.2914855480 * color.b;

  float l = l_ * l_ * l_;
  float m = m_ * m_ * m_;
  float s = s_ * s_ * s_;

  float3 linear_rgb = float3(
   	4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
   	-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
   	-0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s
  );

  // Convert linear sRGB to non-linear sRGB
  return float4(linear_to_srgb(linear_rgb), color.a);
}

float4 to_device_position(float2 unit_vertex, Bounds_ScaledPixels bounds,
                          constant Size_DevicePixels *input_viewport_size) {
  float2 position =
      unit_vertex * float2(bounds.size.width, bounds.size.height) +
      float2(bounds.origin.x, bounds.origin.y);
  float2 viewport_size = float2((float)input_viewport_size->width,
                                (float)input_viewport_size->height);
  float2 device_position =
      position / viewport_size * float2(2., -2.) + float2(-1., 1.);
  return float4(device_position, 0., 1.);
}

float4 to_device_position_transformed(float2 unit_vertex, Bounds_ScaledPixels bounds,
                          TransformationMatrix transformation,
                          constant Size_DevicePixels *input_viewport_size) {
  float2 position =
      unit_vertex * float2(bounds.size.width, bounds.size.height) +
      float2(bounds.origin.x, bounds.origin.y);

  // Apply the transformation matrix to the position via matrix multiplication.
  float2 transformed_position = float2(0, 0);
  transformed_position[0] = position[0] * transformation.rotation_scale[0][0] + position[1] * transformation.rotation_scale[0][1];
  transformed_position[1] = position[0] * transformation.rotation_scale[1][0] + position[1] * transformation.rotation_scale[1][1];

  // Add in the translation component of the transformation matrix.
  transformed_position[0] += transformation.translation[0];
  transformed_position[1] += transformation.translation[1];

  float2 viewport_size = float2((float)input_viewport_size->width,
                                (float)input_viewport_size->height);
  float2 device_position =
      transformed_position / viewport_size * float2(2., -2.) + float2(-1., 1.);
  return float4(device_position, 0., 1.);
}


float2 to_tile_position(float2 unit_vertex, AtlasTile tile,
                        constant Size_DevicePixels *atlas_size) {
  float2 tile_origin = float2(tile.bounds.origin.x, tile.bounds.origin.y);
  float2 tile_size = float2(tile.bounds.size.width, tile.bounds.size.height);
  return (tile_origin + unit_vertex * tile_size) /
         float2((float)atlas_size->width, (float)atlas_size->height);
}

// Selects corner radius based on quadrant.
float pick_corner_radius(float2 center_to_point, Corners_ScaledPixels corner_radii) {
  if (center_to_point.x < 0.) {
    if (center_to_point.y < 0.) {
      return corner_radii.top_left;
    } else {
      return corner_radii.bottom_left;
    }
  } else {
    if (center_to_point.y < 0.) {
      return corner_radii.top_right;
    } else {
      return corner_radii.bottom_right;
    }
  }
}

// Signed distance of the point to the quad's border - positive outside the
// border, and negative inside.
float quad_sdf(float2 point, Bounds_ScaledPixels bounds,
               Corners_ScaledPixels corner_radii) {
    float2 half_size = float2(bounds.size.width, bounds.size.height) / 2.0;
    float2 center = float2(bounds.origin.x, bounds.origin.y) + half_size;
    float2 center_to_point = point - center;
    float corner_radius = pick_corner_radius(center_to_point, corner_radii);
    float2 corner_to_point = fabs(center_to_point) - half_size;
    float2 corner_center_to_point = corner_to_point + corner_radius;
    return quad_sdf_impl(corner_center_to_point, corner_radius);
}

// Implementation of quad signed distance field
float quad_sdf_impl(float2 corner_center_to_point, float corner_radius) {
    if (corner_radius == 0.0) {
        // Fast path for unrounded corners
        return max(corner_center_to_point.x, corner_center_to_point.y);
    } else {
        // Signed distance of the point from a quad that is inset by corner_radius
        // It is negative inside this quad, and positive outside
        float signed_distance_to_inset_quad =
            // 0 inside the inset quad, and positive outside
            length(max(float2(0.0), corner_center_to_point)) +
            // 0 outside the inset quad, and negative inside
            min(0.0, max(corner_center_to_point.x, corner_center_to_point.y));

        return signed_distance_to_inset_quad - corner_radius;
    }
}

// A standard gaussian function, used for weighting samples
float gaussian(float x, float sigma) {
  return exp(-(x * x) / (2. * sigma * sigma)) / (sqrt(2. * M_PI_F) * sigma);
}

// This approximates the error function, needed for the gaussian integral
float2 erf(float2 x) {
  float2 s = sign(x);
  float2 a = abs(x);
  float2 r1 = 1. + (0.278393 + (0.230389 + (0.000972 + 0.078108 * a) * a) * a) * a;
  float2 r2 = r1 * r1;
  return s - s / (r2 * r2);
}

float blur_along_x(float x, float y, float sigma, float corner,
                   float2 half_size) {
  float delta = min(half_size.y - corner - abs(y), 0.);
  float curved =
      half_size.x - corner + sqrt(max(0., corner * corner - delta * delta));
  float2 integral =
      0.5 + 0.5 * erf((x + float2(-curved, curved)) * (sqrt(0.5) / sigma));
  return integral.y - integral.x;
}

float4 distance_from_clip_rect(float2 unit_vertex, Bounds_ScaledPixels bounds,
                               Bounds_ScaledPixels clip_bounds) {
  float2 position =
      unit_vertex * float2(bounds.size.width, bounds.size.height) +
      float2(bounds.origin.x, bounds.origin.y);
  return float4(position.x - clip_bounds.origin.x,
                clip_bounds.origin.x + clip_bounds.size.width - position.x,
                position.y - clip_bounds.origin.y,
                clip_bounds.origin.y + clip_bounds.size.height - position.y);
}

float4 distance_from_clip_rect_transformed(float2 unit_vertex, Bounds_ScaledPixels bounds,
                               Bounds_ScaledPixels clip_bounds, TransformationMatrix transformation) {
  float2 position =
      unit_vertex * float2(bounds.size.width, bounds.size.height) +
      float2(bounds.origin.x, bounds.origin.y);
  float2 transformed_position = float2(0, 0);
  transformed_position[0] = position[0] * transformation.rotation_scale[0][0] + position[1] * transformation.rotation_scale[0][1];
  transformed_position[1] = position[0] * transformation.rotation_scale[1][0] + position[1] * transformation.rotation_scale[1][1];
  transformed_position[0] += transformation.translation[0];
  transformed_position[1] += transformation.translation[1];

  return float4(transformed_position.x - clip_bounds.origin.x,
                clip_bounds.origin.x + clip_bounds.size.width - transformed_position.x,
                transformed_position.y - clip_bounds.origin.y,
                clip_bounds.origin.y + clip_bounds.size.height - transformed_position.y);
}

float4 over(float4 below, float4 above) {
  float4 result;
  float alpha = above.a + below.a * (1.0 - above.a);
  result.rgb =
      (above.rgb * above.a + below.rgb * below.a * (1.0 - above.a)) / alpha;
  result.a = alpha;
  return result;
}

GradientColor prepare_fill_color(uint tag, uint color_space, Hsla solid,
                                     Hsla color0, Hsla color1) {
  GradientColor out;
  if (tag == 0 || tag == 2 || tag == 3) {
    out.solid = hsla_to_rgba(solid);
  } else if (tag == 1) {
    out.color0 = hsla_to_rgba(color0);
    out.color1 = hsla_to_rgba(color1);

    // Prepare color space in vertex for avoid conversion
    // in fragment shader for performance reasons
    if (color_space == 1) {
      // Oklab
      out.color0 = srgb_to_oklab(out.color0);
      out.color1 = srgb_to_oklab(out.color1);
    }
  }

  return out;
}

float2x2 rotate2d(float angle) {
    float s = sin(angle);
    float c = cos(angle);
    return float2x2(c, -s, s, c);
}

float4 fill_color(Background background,
                      float2 position,
                      Bounds_ScaledPixels bounds,
                      float4 solid_color, float4 color0, float4 color1) {
  float4 color;

  switch (background.tag) {
    case 0:
      color = solid_color;
      break;
    case 1: {
      // -90 degrees to match the CSS gradient angle.
      float gradient_angle = background.gradient_angle_or_pattern_height;
      float radians = (fmod(gradient_angle, 360.0) - 90.0) * (M_PI_F / 180.0);
      float2 direction = float2(cos(radians), sin(radians));

      // Expand the short side to be the same as the long side
      if (bounds.size.width > bounds.size.height) {
          direction.y *= bounds.size.height / bounds.size.width;
      } else {
          direction.x *=  bounds.size.width / bounds.size.height;
      }

      // Get the t value for the linear gradient with the color stop percentages.
      float2 half_size = float2(bounds.size.width, bounds.size.height) / 2.;
      float2 center = float2(bounds.origin.x, bounds.origin.y) + half_size;
      float2 center_to_point = position - center;
      float t = dot(center_to_point, direction) / length(direction);
      // Check the direction to determine whether to use x or y
      if (abs(direction.x) > abs(direction.y)) {
          t = (t + half_size.x) / bounds.size.width;
      } else {
          t = (t + half_size.y) / bounds.size.height;
      }

      // Adjust t based on the stop percentages
      t = (t - background.colors[0].percentage)
        / (background.colors[1].percentage
        - background.colors[0].percentage);
      t = clamp(t, 0.0, 1.0);

      switch (background.color_space) {
        case 0:
          color = mix(color0, color1, t);
          break;
        case 1: {
          float4 oklab_color = mix(color0, color1, t);
          color = oklab_to_srgb(oklab_color);
          break;
        }
      }

      // Dither to reduce banding in gradients (especially dark/alpha).
      // Triangular-distributed noise breaks up 8-bit quantization steps.
      // ±2/255 for RGB (enough for dark-on-dark compositing),
      // ±3/255 for alpha (needs more because alpha × dark color = tiny steps).
      {
        float2 seed = position * 0.6180339887; // golden ratio spread
        float r1 = fract(sin(dot(seed, float2(12.9898, 78.233))) * 43758.5453);
        float r2 = fract(sin(dot(seed, float2(39.3460, 11.135))) * 24634.6345);
        float tri = r1 + r2 - 1.0; // triangular PDF, range [-1, +1]
        color.rgb += tri * 2.0 / 255.0;
        color.a   += tri * 3.0 / 255.0;
      }

      break;
    }
    case 2: {
        float gradient_angle_or_pattern_height = background.gradient_angle_or_pattern_height;
        float pattern_width = (gradient_angle_or_pattern_height / 65535.0f) / 255.0f;
        float pattern_interval = fmod(gradient_angle_or_pattern_height, 65535.0f) / 255.0f;
        float pattern_height = pattern_width + pattern_interval;
        float stripe_angle = M_PI_F / 4.0;
        float pattern_period = pattern_height * sin(stripe_angle);
        float2x2 rotation = rotate2d(stripe_angle);
        float2 relative_position = position - float2(bounds.origin.x, bounds.origin.y);
        float2 rotated_point = rotation * relative_position;
        float pattern = fmod(rotated_point.x, pattern_period);
        float distance = min(pattern, pattern_period - pattern) - pattern_period * (pattern_width / pattern_height) /  2.0f;
        color = solid_color;
        color.a *= saturate(0.5 - distance);
        break;
    }
    case 3: {
        // checkerboard
        float size = background.gradient_angle_or_pattern_height;
        float2 relative_position = position - float2(bounds.origin.x, bounds.origin.y);

        float x_index = floor(relative_position.x / size);
        float y_index = floor(relative_position.y / size);
        float should_be_colored = fmod(x_index + y_index, 2.0);

        color = solid_color;
        color.a *= saturate(should_be_colored);
        break;
    }
  }

  return color;
}

// Modified for Volta: blur effects (CSS backdrop-filter, filter: blur(),
// text-shadow). A port of gpui-pre-wgpu's shaders_effects.wgsl.
// UNTESTED: written on Linux, never compiled.
//
// One effect runs as: (source) -> downsample into scratch A -> horizontal
// Gaussian A -> B -> vertical Gaussian B -> A -> composite A into the frame.

// Must match `VoltaEffectParams` in metal_renderer.rs (304 bytes).
struct VoltaEffectParams {
  float4 region;       // blurred source region, frame px: x, y, width, height
  float2 scratch_size; // scratch texels covered by the downsampled region
  float2 texture_size; // size of the source and scratch textures
  float downsample;
  float sigma;         // Gaussian standard deviation, in scratch texels
  uint kind;           // 0 backdrop blur, 1 layer blur, 2 layer shadow, 3 glass
  float saturation;
  float4 bounds;       // where the result is drawn, frame px
  float4 content_mask;
  float4 corner_radii; // top-left, top-right, bottom-right, bottom-left
  float4 color;        // shadow tint, premultiplied
  float2 offset;       // shadow offset
  float opacity;
  float outset;        // glass only: how far the quad extends beyond bounds
  // Glass only: `GlassParams` as eleven float4s, in field order: tint, optics, blur, tone,
  // light, rim, shadow, state, pointer, edge, finish.
  float4 glass[11];
};

struct VoltaEffectPassOutput {
  float4 position [[position]];
  uint effect_id [[flat]];
};

vertex VoltaEffectPassOutput volta_effect_pass_vertex(uint vertex_id [[vertex_id]],
                                                      uint instance_id [[instance_id]]) {
  float2 uv = float2(float((vertex_id << 1) & 2u), float(vertex_id & 2u));
  VoltaEffectPassOutput out;
  out.position = float4(uv * float2(2., -2.) + float2(-1., 1.), 0., 1.);
  out.effect_id = instance_id;
  return out;
}

fragment float4 volta_effect_clear_fragment(VoltaEffectPassOutput input [[stage_in]]) {
  return float4(0.);
}

float4 volta_effect_sample(texture2d<float> source, float2 texel, float2 lo, float2 hi,
                           float2 texture_size) {
  constexpr sampler effect_sampler(mag_filter::linear, min_filter::linear,
                                   address::clamp_to_edge);
  return source.sample(effect_sampler, clamp(texel, lo, hi) / texture_size, level(0));
}

fragment float4 volta_effect_downsample_fragment(VoltaEffectPassOutput input [[stage_in]],
                                                 constant VoltaEffectParams *effects [[buffer(0)]],
                                                 texture2d<float> source [[texture(0)]]) {
  VoltaEffectParams effect = effects[input.effect_id];
  float2 origin = effect.region.xy;
  float2 lo = origin + 0.5;
  float2 hi = origin + effect.region.zw - 0.5;
  float d = effect.downsample;
  float2 cell = floor(input.position.xy);
  if (d <= 1.) {
    return volta_effect_sample(source, origin + cell + 0.5, lo, hi, effect.texture_size);
  }
  float2 base = origin + cell * d;
  int taps = int(d * 0.5);
  float4 sum = float4(0.);
  for (int y = 0; y < taps; y++) {
    for (int x = 0; x < taps; x++) {
      float2 texel = base + float2(float(x * 2 + 1), float(y * 2 + 1));
      sum += volta_effect_sample(source, texel, lo, hi, effect.texture_size);
    }
  }
  return sum / float(taps * taps);
}

float4 volta_effect_gaussian(VoltaEffectPassOutput input, constant VoltaEffectParams *effects,
                             texture2d<float> source, float2 direction) {
  VoltaEffectParams effect = effects[input.effect_id];
  float2 center = input.position.xy;
  float2 lo = float2(0.5);
  float2 hi = effect.scratch_size - 0.5;
  float sigma = effect.sigma;
  if (sigma < 0.05) {
    return volta_effect_sample(source, center, lo, hi, effect.texture_size);
  }
  int radius = int(ceil(sigma * 3.));
  float denominator = 2. * sigma * sigma;
  float4 sum = float4(0.);
  float weight_sum = 0.;
  for (int k = -radius; k <= radius; k++) {
    float offset = float(k);
    float weight = exp(-(offset * offset) / denominator);
    sum += weight * volta_effect_sample(source, center + direction * offset, lo, hi,
                                        effect.texture_size);
    weight_sum += weight;
  }
  return sum / weight_sum;
}

fragment float4 volta_effect_blur_h_fragment(VoltaEffectPassOutput input [[stage_in]],
                                             constant VoltaEffectParams *effects [[buffer(0)]],
                                             texture2d<float> source [[texture(0)]]) {
  return volta_effect_gaussian(input, effects, source, float2(1., 0.));
}

fragment float4 volta_effect_blur_v_fragment(VoltaEffectPassOutput input [[stage_in]],
                                             constant VoltaEffectParams *effects [[buffer(0)]],
                                             texture2d<float> source [[texture(0)]]) {
  return volta_effect_gaussian(input, effects, source, float2(0., 1.));
}

vertex VoltaEffectPassOutput volta_effect_composite_vertex(uint vertex_id [[vertex_id]],
                                                           uint instance_id [[instance_id]],
                                                           constant VoltaEffectParams *effects [[buffer(0)]],
                                                           constant float2 *viewport_size [[buffer(1)]]) {
  VoltaEffectParams effect = effects[instance_id];
  float2 unit_vertex = float2(float(vertex_id & 1u), float((vertex_id >> 1) & 1u));
  float2 position = effect.bounds.xy + unit_vertex * effect.bounds.zw;
  VoltaEffectPassOutput out;
  out.position = float4(position / *viewport_size * float2(2., -2.) + float2(-1., 1.), 0., 1.);
  out.effect_id = instance_id;
  return out;
}

float volta_rounded_rect_sdf(float2 point, float4 bounds, float4 radii) {
  float2 half_size = bounds.zw * 0.5;
  float2 center_to_point = point - (bounds.xy + half_size);
  float radius = center_to_point.x < 0.
                     ? (center_to_point.y < 0. ? radii.x : radii.w)
                     : (center_to_point.y < 0. ? radii.y : radii.z);
  float2 q = abs(center_to_point) - half_size + radius;
  return length(max(q, 0.)) + min(max(q.x, q.y), 0.) - radius;
}

// Output is premultiplied; the pipeline blends One, OneMinusSourceAlpha.
fragment float4 volta_effect_composite_fragment(VoltaEffectPassOutput input [[stage_in]],
                                                constant VoltaEffectParams *effects [[buffer(0)]],
                                                texture2d<float> source [[texture(0)]]) {
  VoltaEffectParams effect = effects[input.effect_id];
  float2 p = input.position.xy;
  float4 mask = effect.content_mask;
  if (p.x < mask.x || p.y < mask.y || p.x > mask.x + mask.z || p.y > mask.y + mask.w) {
    return float4(0.);
  }
  float2 scratch = (p - effect.offset - effect.region.xy) / effect.downsample;
  float4 blurred = volta_effect_sample(source, scratch, float2(0.5), effect.scratch_size - 0.5,
                                      effect.texture_size);
  if (effect.kind == 0u) {
    float luminance = dot(blurred.rgb, float3(0.2126, 0.7152, 0.0722));
    float3 rgb = clamp(float3(luminance) + (blurred.rgb - float3(luminance)) * effect.saturation,
                       0., 1.);
    float sdf = volta_rounded_rect_sdf(p, effect.bounds, effect.corner_radii);
    float coverage = saturate(0.5 - sdf) * effect.opacity;
    return float4(rgb * coverage, coverage);
  }
  if (effect.kind == 1u) {
    return blurred * effect.opacity;
  }
  return effect.color * blurred.a * effect.opacity;
}

// Liquid Glass: a port of `fs_glass` (gpui-pre-wgpu shaders_effects.wgsl, and the HLSL port in
// gpui-pre-windows). The backdrop copy (texture 0, sharp) and its blur (texture 1) are both
// sampled: refracted sharp taps at the rim, frosted blur inside. The frame holds
// gamma-encoded values; the optics work in linear light.

constant float3 GLASS_LUMA = float3(0.2126, 0.7152, 0.0722);

float glass_dither(float2 position) {
  float noise = fract(52.9829189 * fract(dot(position, float2(0.06711056, 0.00583715))));
  return (noise - 0.5) / 255.0;
}

float glass_pick_radius(float2 p, float4 radii) {
  // radii: top-left, top-right, bottom-right, bottom-left.
  if (p.x < 0.) {
    return p.y < 0. ? radii.x : radii.w;
  }
  return p.y < 0. ? radii.y : radii.z;
}

float glass_sdf(float2 p, float2 half_size, float4 radii, float n) {
  float r = min(glass_pick_radius(p, radii), min(half_size.x, half_size.y));
  float2 q = abs(p) - half_size + r;
  float2 outside_q = max(q, 0.0);
  float outside = length(outside_q);
  if (r > 0.0 && n > 2.0) {
    outside = pow(pow(outside_q.x, n) + pow(outside_q.y, n), 1.0 / n);
  }
  return outside + min(max(q.x, q.y), 0.0) - r;
}

float2 glass_normal(float2 p, float2 half_size, float4 radii, float n, float min_radius) {
  float e = 0.75;
  float4 soft = max(radii, float4(min_radius));
  float2 gradient = float2(
      glass_sdf(p + float2(e, 0.0), half_size, soft, n) - glass_sdf(p - float2(e, 0.0), half_size, soft, n),
      glass_sdf(p + float2(0.0, e), half_size, soft, n) - glass_sdf(p - float2(0.0, e), half_size, soft, n));
  float len = length(gradient);
  return len > 1e-6 ? gradient / len : float2(0.0, -1.0);
}

float3 glass_sharp(texture2d<float> sharp, constant VoltaEffectParams &effect, float2 position) {
  constexpr sampler s(mag_filter::linear, min_filter::linear, address::clamp_to_edge);
  float2 lo = effect.region.xy + 0.5;
  float2 hi = effect.region.xy + effect.region.zw - 0.5;
  float2 uv = clamp(position, lo, hi) / effect.texture_size;
  return srgb_to_linear(sharp.sample(s, uv, level(0)).rgb);
}

float3 glass_blurred(texture2d<float> blurred, constant VoltaEffectParams &effect, float2 position) {
  constexpr sampler s(mag_filter::linear, min_filter::linear, address::clamp_to_edge);
  float2 scratch = (position - effect.region.xy) / effect.downsample;
  float2 uv = clamp(scratch, float2(0.5), effect.scratch_size - 0.5) / effect.texture_size;
  return srgb_to_linear(blurred.sample(s, uv, level(0)).rgb);
}

float glass_bezel_slope(float u) {
  float a = 1.0 - u;
  float a3 = a * a * a;
  float f = max(1.0 - a3 * a, 1e-4);
  return a3 / pow(f, 0.75);
}

vertex VoltaEffectPassOutput volta_effect_glass_vertex(uint vertex_id [[vertex_id]],
                                                       uint instance_id [[instance_id]],
                                                       constant VoltaEffectParams *effects [[buffer(0)]],
                                                       constant float2 *viewport_size [[buffer(1)]]) {
  VoltaEffectParams effect = effects[instance_id];
  float2 unit_vertex = float2(float(vertex_id & 1u), float((vertex_id >> 1) & 1u));
  float2 origin = effect.bounds.xy - effect.outset;
  float2 size = effect.bounds.zw + effect.outset * 2.0;
  float2 position = origin + unit_vertex * size;
  VoltaEffectPassOutput out;
  out.position = float4(position / *viewport_size * float2(2., -2.) + float2(-1., 1.), 0., 1.);
  out.effect_id = instance_id;
  return out;
}

// Output is premultiplied; the pipeline blends One, OneMinusSourceAlpha.
fragment float4 volta_effect_glass_fragment(VoltaEffectPassOutput input [[stage_in]],
                                            constant VoltaEffectParams *effects [[buffer(0)]],
                                            texture2d<float> sharp [[texture(0)]],
                                            texture2d<float> blurred_tex [[texture(1)]]) {
  constant VoltaEffectParams &effect = effects[input.effect_id];
  float2 frag = input.position.xy;
  float4 mask = effect.content_mask;
  if (frag.x < mask.x || frag.y < mask.y || frag.x > mask.x + mask.z || frag.y > mask.y + mask.w) {
    return float4(0.0);
  }
  float4 g_tint = effect.glass[0];
  float4 g_optics = effect.glass[1];
  float4 g_blur = effect.glass[2];
  float4 g_tone = effect.glass[3];
  float4 g_light = effect.glass[4];
  float4 g_rim = effect.glass[5];
  float4 g_shadow = effect.glass[6];
  float4 g_state = effect.glass[7];
  float4 g_pointer = effect.glass[8];
  float4 g_edge = effect.glass[9];
  float4 g_finish = effect.glass[10];

  float4 radii = effect.corner_radii;
  float2 half_size = effect.bounds.zw * 0.5;
  float2 mid_point = effect.bounds.xy + half_size;
  float2 p = frag - mid_point;
  float min_half = min(half_size.x, half_size.y);
  float largest_radius = max(max(radii.x, radii.y), max(radii.z, radii.w));
  float corner_exponent = mix(max(g_state.x, 2.0), 2.0,
                              smoothstep(0.8, 0.98, largest_radius / max(min_half, 1.0)));
  float sdf = glass_sdf(p, half_size, radii, corner_exponent);
  float appear = clamp(g_state.z, 0.0, 1.0) * effect.opacity;
  float coverage = clamp(0.5 - sdf, 0.0, 1.0);

  float shadow_alpha = 0.0;
  if (g_shadow.z > 0.0) {
    float sigma = max(g_shadow.y * 0.5, 0.5);
    float shadow_sdf = glass_sdf(p - float2(0.0, g_shadow.x), half_size + g_shadow.w, radii, corner_exponent);
    float falloff = 0.5 * (1.0 - tanh(1.2028 * shadow_sdf / (sigma * 1.41421356)));
    float size_factor = clamp(min_half * 2.0 / (sigma * 3.0), 0.2, 1.0);
    shadow_alpha = g_shadow.z * falloff * size_factor * (1.0 - coverage) * appear;
  }
  if (g_edge.z > 0.0) {
    shadow_alpha += g_edge.z * exp(-max(sdf, 0.0) / 1.4) * (1.0 - coverage) * appear;
  }
  if (sdf > 0.5) {
    return float4(0.0, 0.0, 0.0, shadow_alpha > 0.0 ? max(shadow_alpha + glass_dither(frag), 0.0) : 0.0);
  }

  float depth = max(-sdf, 0.0);
  float bezel = clamp(g_optics.y, 1.0, max(min_half * 0.98, 1.0));
  float2 normal = glass_normal(p, half_size, radii, corner_exponent, bezel);

  float refraction_amount = g_optics.x * appear;
  float ior = max(g_optics.z, 1.02);
  float2 refract_offset = float2(0.0);
  if (depth < bezel && refraction_amount != 0.0) {
    float u = clamp(depth / bezel, 0.0, 1.0);
    float slope = glass_bezel_slope(u) * g_optics.w / bezel;
    float3 surface_normal = normalize(float3(normal * slope, 1.0));
    float3 refracted = refract(float3(0.0, 0.0, -1.0), surface_normal, 1.0 / ior);
    float tan_slope = length(refracted.xy) / max(-refracted.z, 1e-3);
    float sine_limit = 1.0 / ior;
    float tangent_limit = sine_limit / sqrt(1.0 - sine_limit * sine_limit);
    refract_offset = -normal * (clamp(tan_slope / tangent_limit, 0.0, 1.0) * refraction_amount);
  }

  float2 sample_position = frag + refract_offset;
  float dispersion = g_blur.z;
  float3 color = glass_sharp(sharp, effect, sample_position);
  if (dispersion != 0.0) {
    color = float3(glass_sharp(sharp, effect, frag + refract_offset * (1.0 - dispersion)).r,
                   color.g,
                   glass_sharp(sharp, effect, frag + refract_offset * (1.0 + dispersion)).b);
  }
  float blur_radius = g_blur.x * appear;
  if (blur_radius > 0.5) {
    float3 blurred = glass_blurred(blurred_tex, effect, sample_position);
    float blur_mix = mix(g_blur.y, 1.0, smoothstep(0.0, bezel * 1.5, depth));
    color = mix(color, blurred, blur_mix);
  }

  float appearance = g_tone.w;
  if (appearance < 0.0) {
    float region_luma = dot(glass_blurred(blurred_tex, effect, mid_point), GLASS_LUMA);
    float region_gamma_luma = linear_to_srgb(float3(region_luma)).x;
    appearance = 1.0 - smoothstep(0.28, 0.62, region_gamma_luma);
  }

  float3 encoded = linear_to_srgb(max(color, 0.0));
  float3 untoned = encoded;
  encoded = (encoded - 0.5) * g_tone.y + 0.5;
  encoded = encoded * g_tone.x;
  float3 veil_color = mix(float3(1.0), float3(0.06), appearance);
  encoded = mix(encoded, veil_color, g_tone.z);
  float encoded_luma = dot(encoded, GLASS_LUMA);
  encoded = mix(float3(encoded_luma), encoded, g_blur.w);
  encoded = clamp(encoded, 0.0, 1.0);

  if (g_tint.a > 0.0) {
    float brightness = clamp(dot(encoded, GLASS_LUMA), 0.0, 1.0);
    float3 tinted = g_tint.rgb * mix(0.45, 1.25, brightness);
    encoded = mix(encoded, clamp(tinted, 0.0, 1.0), clamp(g_tint.a, 0.0, 1.0) * 0.9);
  }
  if (g_edge.x > 0.0) {
    float band = exp(-depth / max(g_edge.y, 0.5)) * g_edge.x * (1.0 - appearance);
    encoded = encoded * (1.0 - clamp(band, 0.0, 0.9));
  }
  encoded = mix(untoned, encoded, appear);

  float2 light_0 = float2(cos(g_light.x), -sin(g_light.x));
  float2 light_1 = float2(cos(g_light.y), -sin(g_light.y));
  float facing_0 = pow(max(dot(normal, light_0), 0.0), g_rim.z);
  float facing_1 = pow(max(dot(normal, light_1), 0.0), g_rim.z);
  float lit_amount = clamp(facing_0 * g_light.z + facing_1 * g_light.w, 0.0, 1.5);
  float rim_width = max(g_rim.x, 0.25);
  float3 channel_shift = float3(-1.0, 0.0, 1.0) * g_edge.w;
  float3 rim_profile = exp(-pow(max(depth + channel_shift, 0.0) / rim_width, float3(g_rim.y)));
  float bevel = exp(-depth / max(g_finish.z, 0.5)) * g_finish.y * lit_amount;
  float3 rim_energy = clamp((g_finish.x + lit_amount) * rim_profile + bevel, 0.0, 1.0) * appear;
  float3 face_hue = encoded / max(max(encoded.r, max(encoded.g, encoded.b)), 0.08);
  float3 rim_light = mix(float3(1.0), face_hue, 0.55);
  encoded = 1.0 - (1.0 - encoded) * (1.0 - rim_energy * rim_light);

  if (g_pointer.z > 0.0 && g_pointer.w > 0.0) {
    float pointer_distance = length(frag - (effect.bounds.xy + g_pointer.xy));
    float glow_radius = g_pointer.z * 0.5;
    encoded += g_pointer.w * 0.2 * exp(-(pointer_distance * pointer_distance) / (2.0 * glow_radius * glow_radius));
  }

  float3 output_encoded = clamp(encoded, 0.0, 1.0) + glass_dither(frag);
  return float4(output_encoded * coverage, coverage + shadow_alpha);
}

// Full-screen player background (`BlurEffectKind::CoverBackground`), the GPU version of
// the CPU compositor that used to live in the app (fullscreen_background.rs): rotating copies of a
// cover blurred by ~104 css px, colour-graded, veiled. One effect runs as
//   scene pass  -> scratch A  (this evaluates every square analytically per texel)
//   gaussian    -> A -> B -> A (the shared passes above)
//   composite   -> frame      (grade + veil, rounded corners)
// Parameters are carried in `VoltaEffectParams`:
//   region      virtual scene origin (frame px, bleeds 35 % past the bounds) and size
//   bounds      where the background is drawn (the viewport)
//   glass[0]    seconds, fade, -, -
//   glass[1]    canvas rgb
//   glass[2]/[3] atlas rect (x, y, w, h in texels) of the cover / previous cover; w = 0: none

float2 cover_rotate(float2 v, float degrees) {
  float angle = degrees * 0.017453292519943295;
  float s = sin(angle);
  float c = cos(angle);
  return float2(c * v.x - s * v.y, s * v.x + c * v.y);
}

float cover_turn(float seconds, float period, float direction) {
  return fract(seconds / period) * 360.0 * direction;
}

// Composites one cover over `c` (premultiplied source-over) wherever the rotated square
// centred at `centre`, `size` wide, covers `pos`.
float3 cover_square(float3 c, texture2d<float> tex, float4 rect, float opacity, float2 pos,
                    float2 centre, float size, float degrees) {
  constexpr sampler tile_sampler(mag_filter::linear, min_filter::linear, address::clamp_to_edge);
  float2 local = cover_rotate(pos - centre, -degrees) / size + 0.5;
  if (local.x < 0.0 || local.y < 0.0 || local.x > 1.0 || local.y > 1.0) {
    return c;
  }
  float2 texel = clamp(rect.xy + local * rect.zw, rect.xy + 0.5, rect.xy + rect.zw - 0.5);
  float4 src = tex.sample(tile_sampler, texel / float2(tex.get_width(), tex.get_height()), level(0));
  return src.rgb * opacity + c * (1.0 - src.a * opacity);
}

float3 cover_layer(float3 c, texture2d<float> tex, float4 rect, float opacity, float2 pos,
                   float2 view, float2 bleed, float2 buffer, float t) {
  if (rect.z <= 0.0 || opacity <= 0.0) {
    return c;
  }
  float w = view.x;
  float h = view.y;
  // `.fullscreen-cover`: inset -35 %, object-fit cover.
  c = cover_square(c, tex, rect, opacity, pos, buffer * 0.5, max(buffer.x, buffer.y), 0.0);
  c = cover_square(c, tex, rect, opacity, pos, bleed + float2(0.5 * w, 0.5 * h), 1.25 * w, cover_turn(t, 69.813, 1.0));
  c = cover_square(c, tex, rect, opacity, pos, bleed + float2(0.4 * w, 0.4 * h), 0.8 * w, cover_turn(t, 26.18, -1.0));
  // Orbits: W x W squares centred vertically, turning about a pivot.
  float orbit_top = bleed.y + h / 2.0 - w / 2.0;
  float2 pivot_a = float2(0.5 * w, 0.5 * w);
  float2 point_a = pivot_a + cover_rotate(float2(0.25 * w, 0.0), cover_turn(t, 46.542, -1.0));
  c = cover_square(c, tex, rect, opacity, pos, float2(bleed.x + point_a.x, orbit_top + point_a.y), 0.5 * w,
                   cover_turn(t, 34.907, 1.0));
  float2 pivot_b = float2(0.55 * w, 0.55 * w);
  float2 point_b = pivot_b + cover_rotate(float2(0.25 * w, 0.0), cover_turn(t, 69.813, 1.0));
  c = cover_square(c, tex, rect, opacity, pos, float2(bleed.x + point_b.x, orbit_top + point_b.y), 0.25 * w,
                   cover_turn(t, 52.36, -1.0));
  return c;
}

fragment float4 volta_cover_scene_fragment(VoltaEffectPassOutput input [[stage_in]],
                                           constant VoltaEffectParams *effects [[buffer(0)]],
                                           texture2d<float> cover_tex [[texture(0)]],
                                           texture2d<float> previous_tex [[texture(1)]]) {
  constant VoltaEffectParams &effect = effects[input.effect_id];
  // Scene position in frame px relative to the virtual origin; the texel centre of the
  // downsampled grid.
  float2 pos = input.position.xy * effect.downsample;
  float2 view = effect.bounds.zw;
  float2 bleed = view * 0.35;
  float2 buffer = effect.region.zw;
  float seconds = effect.glass[0].x;
  float fade = clamp(effect.glass[0].y, 0.0, 1.0);
  float3 c = effect.glass[1].rgb;
  if (fade < 1.0) {
    c = cover_layer(c, previous_tex, effect.glass[3], 1.0, pos, view, bleed, buffer, seconds);
  }
  c = cover_layer(c, cover_tex, effect.glass[2], fade, pos, view, bleed, buffer, seconds);
  return float4(c, 1.0);
}

fragment float4 volta_cover_composite_fragment(VoltaEffectPassOutput input [[stage_in]],
                                               constant VoltaEffectParams *effects [[buffer(0)]],
                                               texture2d<float> source [[texture(0)]]) {
  VoltaEffectParams effect = effects[input.effect_id];
  float2 p = input.position.xy;
  float4 mask = effect.content_mask;
  if (p.x < mask.x || p.y < mask.y || p.x > mask.x + mask.z || p.y > mask.y + mask.w) {
    return float4(0.);
  }
  float2 scratch = (p - effect.region.xy) / effect.downsample;
  float3 rgb = volta_effect_sample(source, scratch, float2(0.5), effect.scratch_size - 0.5,
                                   effect.texture_size).rgb;
  // The feColorMatrix grade, then a 50 % black veil and a 5 % white lift.
  float3 graded = clamp(float3(
      dot(rgb, float3(3.162853, -1.6652724, -0.1678308)) - 0.315,
      dot(rgb, float3(-0.4946469, 1.9922276, -0.1678308)) - 0.315,
      dot(rgb, float3(-0.4946469, -1.6652724, 3.489669)) - 0.315), 0., 1.);
  graded = graded * 0.5 * 0.95 + 0.05;
  graded += (fract(52.9829189 * fract(dot(p, float2(0.06711056, 0.00583715)))) - 0.5) / 255.0;
  float sdf = volta_rounded_rect_sdf(p, effect.bounds, effect.corner_radii);
  float coverage = saturate(0.5 - sdf) * effect.opacity;
  return float4(saturate(graded) * coverage, coverage);
}
