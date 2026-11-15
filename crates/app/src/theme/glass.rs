//! Liquid Glass presets for the sidebar and the playback dock.
//!
//! The material itself (SDF lens, rim light, frosted interior, outside shadow) lives in the
//! patched gpui renderer (`Styled::glass`); this module only
//! chooses how it looks. Appearance follows the theme rather than the backdrop, because the
//! text drawn over the glass is themed, not adaptive.

use gpui::GlassParams;

use super::{Appearance, Tokens};

/// The one glass material every surface uses (Apple's is a single material too): same frost,
/// tone, tint, lensing and rim light everywhere. Surfaces differ only in their shape and in
/// how big a shadow that shape casts, see [`sidebar`] and [`dock`].
fn material(tokens: &Tokens) -> Option<GlassParams> {
    let dark = match tokens.appearance {
        Appearance::Dark => true,
        Appearance::Light => false,
        // High contrast keeps its opaque, outlined surfaces (Apple's Increase Contrast
        // does the same to glass).
        Appearance::HighContrast => return None,
    };
    let mut glass = GlassParams::default();
    // A slightly squircular rounded rectangle; capsules and circles stay true semicircles.
    glass.state[0] = 3.0;
    // Appearance follows the theme, not the backdrop: the text on top is themed. Dark glass
    // veils towards a dark grey (0.88 mixes the veil colour to ~#2c2c2c), not black, so the
    // surface stays a touch above the canvas as the opaque ones were.
    glass.tone[3] = if dark { 0.88 } else { 0.0 };
    // Frosted and veiled enough that labels stay legible over any cover scrolling underneath.
    glass.blur = [16.0, 0.6, 0.0, 1.15];
    glass.tone[2] = if dark { 0.55 } else { 0.70 };
    glass.optics = [10.0, 14.0, 1.5, 24.0];
    // Two opposed lights at +45° / -135° (the research's reverse-engineered angles): a thin
    // hairline, brightest at the top-right and bottom-left, over a faint outline all round.
    glass.light = [45f32.to_radians(), (-135f32).to_radians(), 0.3, 0.3];
    glass.rim = [1.2, 2.0, 3.0, 1.0];
    glass.edge = [if dark { 0.0 } else { 0.12 }, 6.0, 0.12, 0.1];
    glass.finish = [0.07, 0.3, 3.5, 0.0];
    glass.shadow[2] = if dark { 0.35 } else { 0.16 };
    Some(glass)
}

/// The floating library sidebar (a tall 20px-radius card).
pub fn sidebar(tokens: &Tokens) -> Option<GlassParams> {
    let mut glass = material(tokens)?;
    glass.shadow[0] = 6.0;
    glass.shadow[1] = 20.0;
    Some(glass)
}

/// The playback dock: a capsule, or a 16px-radius card in the narrow layout. The same
/// material as the sidebar; a smaller shape just casts a slightly tighter shadow.
pub fn dock(tokens: &Tokens, narrow: bool) -> Option<GlassParams> {
    let mut glass = material(tokens)?;
    glass.shadow[0] = if narrow { 6.0 } else { 8.0 };
    glass.shadow[1] = if narrow { 20.0 } else { 24.0 };
    Some(glass)
}
