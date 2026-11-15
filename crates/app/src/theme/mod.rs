//! Design tokens from `src/theme.css` (`:root` and `:root[data-theme=…]`).
//!
//! Every CSS pixel goes through [`sz`], which returns rems. The window's rem
//! size is `16px × interface scale`, so changing the scale zooms the whole
//! UI uniformly, the same way the web's `#root { zoom }` does.

pub mod fonts;
pub mod glass;

use gpui::{App, Global, Hsla, Rems, Rgba, rems};
use volta_logic::storage::ThemePreference;

/// One CSS pixel at 100 % interface scale.
pub fn sz(css_px: f32) -> Rems {
    rems(css_px / 16.0)
}

/// `platformModifierLabel()` in shortcuts.ts: the label for the "mod" key
/// (which matches Ctrl or Meta).
pub fn mod_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        "Super"
    }
}

/// `--sidebar-width`: 218px, 194px at ≤1150px, 180px at ≤860px, and the
/// 224px drawer at ≤640px.
pub fn sidebar_width(viewport: f32) -> f32 {
    if viewport <= 640.0 {
        224.0
    } else if viewport <= 860.0 {
        180.0
    } else if viewport <= 1150.0 {
        194.0
    } else {
        218.0
    }
}

/// `.main-content` padding `(top, inline, bottom)` per media query.
pub fn main_padding(viewport: f32) -> (f32, f32, f32) {
    if viewport <= 640.0 {
        (10.0, 20.0, 113.0)
    } else if viewport <= 860.0 {
        (14.0, 21.0, 120.0)
    } else if viewport <= 1150.0 {
        (14.0, 26.0, 120.0)
    } else if viewport >= 1600.0 {
        (14.0, 50.0, 120.0)
    } else {
        (14.0, 37.0, 120.0)
    }
}

/// The window's CSS width, published by the shell each frame so element
/// helpers can apply media queries without a `Window`.
#[derive(Clone, Copy)]
pub struct Viewport(pub f32);
impl Global for Viewport {}

/// The width of the sidebar plus its margins, in CSS px (0 when it is a drawer), published
/// by the shell each frame. The sidebar floats over the page, so horizontal shelves reach
/// under it by this much instead of ending at its edge.
#[derive(Clone, Copy)]
pub struct SidebarInset(pub f32);
impl Global for SidebarInset {}

pub fn sidebar_inset(cx: &App) -> f32 {
    cx.try_global::<SidebarInset>().map(|v| v.0).unwrap_or(0.0)
}

pub fn viewport(cx: &App) -> f32 {
    cx.try_global::<Viewport>().map(|v| v.0).unwrap_or(1440.0)
}

/// The window width in CSS px, as media queries see it (the rem size carries
/// the interface scale, like `zoom` on `#root`).
pub fn viewport_css_width(window: &gpui::Window) -> f32 {
    f32::from(content_size(window).width) / (f32::from(window.rem_size()) / 16.0)
}

/// The app's drawable area: the window minus the client-side frame's shadow
/// margin and 1px border (Linux; gpui-component's `window_border`). Layout
/// must use this, not `viewport_size`, or pages are laid out wider than the
/// frame and clipped at the right edge.
pub fn content_size(window: &gpui::Window) -> gpui::Size<gpui::Pixels> {
    let mut size = window.viewport_size();
    if let gpui::Decorations::Client { tiling } = window.window_decorations() {
        let pad = gpui_component::window_paddings(window);
        let border = |tiled: bool| if tiled { gpui::px(0.0) } else { gpui::px(1.0) };
        size.width -= pad.left + pad.right + border(tiling.left) + border(tiling.right);
        size.height -= pad.top + pad.bottom + border(tiling.top) + border(tiling.bottom);
    }
    size
}

/// `#rrggbb` / `#rrggbbaa` / `#rgb` / `#rgba` to a gpui colour.
pub fn hex(value: &str) -> Hsla {
    let digits = value.trim_start_matches('#');
    let expand = |s: &str| -> String { s.chars().flat_map(|c| [c, c]).collect() };
    let full = match digits.len() {
        3 => expand(digits) + "ff",
        4 => expand(digits),
        6 => format!("{digits}ff"),
        8 => digits.to_string(),
        _ => "ff00ffff".to_string(),
    };
    let value = u32::from_str_radix(&full, 16).unwrap_or(0xff00ffff);
    Rgba {
        r: ((value >> 24) & 0xff) as f32 / 255.0,
        g: ((value >> 16) & 0xff) as f32 / 255.0,
        b: ((value >> 8) & 0xff) as f32 / 255.0,
        a: (value & 0xff) as f32 / 255.0,
    }
    .into()
}

/// CSS `color-mix(in srgb, a pct%, b)`, with premultiplied-alpha interpolation
/// as the spec requires (so mixing with `transparent` keeps the hue).
pub fn mix(a: Hsla, pct: f32, b: Hsla) -> Hsla {
    let p = (pct / 100.0).clamp(0.0, 1.0);
    let a = Rgba::from(a);
    let b = Rgba::from(b);
    let alpha = a.a * p + b.a * (1.0 - p);
    if alpha <= 0.0 {
        return gpui::transparent_black();
    }
    let channel = |x: f32, y: f32| (x * a.a * p + y * b.a * (1.0 - p)) / alpha;
    Rgba {
        r: channel(a.r, b.r),
        g: channel(a.g, b.g),
        b: channel(a.b, b.b),
        a: alpha,
    }
    .into()
}

pub fn transparent() -> Hsla {
    gpui::transparent_black()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
    HighContrast,
}

#[derive(Debug, Clone)]
pub struct Tokens {
    pub appearance: Appearance,
    pub canvas: Hsla,
    pub sidebar: Hsla,
    pub surface: Hsla,
    pub hover: Hsla,
    pub row: Hsla,
    pub text: Hsla,
    pub secondary: Hsla,
    pub muted: Hsla,
    pub line: Hsla,
    pub accent: Hsla,
    pub selection: Hsla,
    pub dock: Hsla,
}

impl Tokens {
    pub fn dark() -> Self {
        Self {
            appearance: Appearance::Dark,
            canvas: hex("#202020"),
            sidebar: hex("#292929"),
            surface: hex("#303030"),
            hover: hex("#383838"),
            row: hex("#ffffff03"),
            text: hex("#f5f5f7"),
            secondary: hex("#aaa9ae"),
            muted: hex("#85858b"),
            line: hex("#ffffff12"),
            accent: hex("#fa586a"),
            selection: hex("#fa586a21"),
            dock: hex("#343434f5"),
        }
    }

    pub fn light() -> Self {
        Self {
            appearance: Appearance::Light,
            canvas: hex("#fff"),
            sidebar: hex("#f5f5f7"),
            surface: hex("#eeeef0"),
            hover: hex("#e6e6e9"),
            row: hex("#00000002"),
            text: hex("#252527"),
            secondary: hex("#757579"),
            muted: hex("#88888d"),
            line: hex("#00000013"),
            accent: hex("#e52443"),
            selection: hex("#e5244313"),
            dock: hex("#fafafaf5"),
        }
    }

    pub fn high_contrast() -> Self {
        Self {
            appearance: Appearance::HighContrast,
            canvas: hex("#000"),
            sidebar: hex("#050505"),
            surface: hex("#111"),
            hover: hex("#242424"),
            row: hex("#ffffff0d"),
            text: hex("#fff"),
            secondary: hex("#f4f4f4"),
            muted: hex("#d0d0d0"),
            line: hex("#ffffff45"),
            accent: hex("#ff6475"),
            selection: hex("#ff64753d"),
            dock: hex("#0a0a0af7"),
        }
    }

    pub fn is_light(&self) -> bool {
        self.appearance == Appearance::Light
    }
}

/// `.primary-button` fills are fixed, not themed.
pub fn primary_fill() -> Hsla {
    hex("#e73650")
}
pub fn primary_fill_hover() -> Hsla {
    hex("#fa4059")
}

/// Global theme state: the user's preference, what the OS reports, and scale.
pub struct ThemeState {
    pub preference: ThemePreference,
    pub system_light: bool,
    /// Interface scale in percent (70–150).
    pub scale: f32,
    pub tokens: Tokens,
    pub font_family: gpui::SharedString,
}

impl Global for ThemeState {}

impl ThemeState {
    pub fn resolve(preference: ThemePreference, system_light: bool) -> Tokens {
        match preference {
            ThemePreference::Dark => Tokens::dark(),
            ThemePreference::Light => Tokens::light(),
            ThemePreference::HighContrast => Tokens::high_contrast(),
            ThemePreference::System if system_light => Tokens::light(),
            ThemePreference::System => Tokens::dark(),
        }
    }
}

pub fn tokens(cx: &App) -> &Tokens {
    &cx.global::<ThemeState>().tokens
}

/// Builds elements with `tokens` swapped in, like a CSS block that redefines
/// the custom properties for its subtree (`.fullscreen-dialog`).
pub fn with_tokens<R>(cx: &mut App, tokens: Tokens, build: impl FnOnce(&mut App) -> R) -> R {
    let saved = std::mem::replace(&mut cx.global_mut::<ThemeState>().tokens, tokens);
    let result = build(cx);
    cx.global_mut::<ThemeState>().tokens = saved;
    result
}

/// `.fullscreen-dialog` overrides.
pub fn fullscreen_tokens(base: &Tokens) -> Tokens {
    Tokens {
        text: hex("#ffffff"),
        secondary: hex("#ffffffac"),
        muted: hex("#ffffff5c"),
        line: hex("#ffffff2b"),
        hover: hex("#ffffff18"),
        accent: hex("#ffffff"),
        ..base.clone()
    }
}

/// Pushes our tokens into gpui-component's theme so its widgets (inputs,
/// tooltips, menus) match, and sets the rem size used for interface scale.
/// CSS `line-height: normal`. theme.css never sets a base line-height, and
/// for Arial / Liberation Sans `normal` is about 1.15; gpui defaults to 1.618.
pub const NORMAL_LINE_HEIGHT: f32 = 1.15;

pub fn apply(cx: &mut App) {
    let (tokens, scale, family) = {
        let state = cx.global::<ThemeState>();
        (state.tokens.clone(), state.scale, state.font_family.clone())
    };
    let mode = if tokens.is_light() {
        gpui_component::ThemeMode::Light
    } else {
        gpui_component::ThemeMode::Dark
    };
    gpui_component::Theme::change(mode, None, cx);
    let theme = gpui_component::Theme::global_mut(cx);
    theme.font_size = gpui::px(16.0 * scale / 100.0);
    theme.font_family = family;
    theme.tokens.background = tokens.canvas.into();
    // Rounded client-side window frame (gpui-component's window_border);
    // only applies to client-decorated (Linux) windows.
    theme.window_radius = gpui::px(crate::components::window_chrome::WINDOW_RADIUS);
    let colors = &mut theme.colors;
    colors.foreground = tokens.text;
    colors.background = tokens.canvas;
    colors.window_border = tokens.line;
    colors.caret = tokens.text;
    colors.selection = mix(tokens.accent, 35.0, transparent());
    colors.ring = tokens.accent;
    colors.muted = tokens.surface;
    colors.muted_foreground = tokens.muted;
    colors.popover = tokens.surface;
    colors.popover_foreground = tokens.text;
    colors.border = tokens.line;
    colors.input = tokens.line;
    colors.accent = tokens.hover;
    colors.accent_foreground = tokens.text;
    colors.scrollbar = transparent();
    colors.scrollbar_thumb = mix(tokens.text, 22.0, transparent());
    colors.scrollbar_thumb_hover = mix(tokens.text, 35.0, transparent());
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_mix_with_transparent_keeps_hue() {
        let accent = hex("#fa586a");
        let mixed = Rgba::from(mix(accent, 16.0, transparent()));
        let source = Rgba::from(accent);
        assert!((mixed.a - 0.16).abs() < 1e-4);
        assert!((mixed.r - source.r).abs() < 1e-4);
    }

    #[test]
    fn hex_parses_short_and_alpha_forms() {
        assert_eq!(Rgba::from(hex("#fff")).a, 1.0);
        assert!((Rgba::from(hex("#ffffff12")).a - 0x12 as f32 / 255.0).abs() < 1e-6);
    }
}
