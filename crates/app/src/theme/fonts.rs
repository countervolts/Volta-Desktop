//! Font resolution, matching what the web's CSS stack resolves to per OS:
//! `-apple-system, BlinkMacSystemFont, "Helvetica Neue", Arial, sans-serif`.
//!
//! - macOS: the system UI font (SF Pro), which gpui names `.SystemUIFont`.
//! - Windows: Arial (the first family in the stack that Windows ships).
//! - Linux: Chromium only accepts a fontconfig substitute when it is
//!   metric-compatible, so "Helvetica Neue" usually falls through to Arial,
//!   which fontconfig maps to Liberation Sans (or Arimo).
//!
//! Switching to a bundled font later only needs a change here.

use std::sync::Arc;

use gpui::{App, FontFeatures, SharedString};

pub fn ui_font_family(cx: &App) -> SharedString {
    if cfg!(target_os = "macos") {
        return ".SystemUIFont".into();
    }
    let installed = cx.text_system().all_font_names();
    let has = |name: &str| installed.iter().any(|family| family == name);
    let candidates: &[&str] = if cfg!(target_os = "windows") {
        &["Arial", "Segoe UI"]
    } else {
        &["Helvetica Neue", "Arial", "Liberation Sans", "Arimo", "Noto Sans", "DejaVu Sans"]
    };
    candidates
        .iter()
        .find(|name| has(name))
        .map(|name| SharedString::from(*name))
        .unwrap_or_else(|| ".SystemUIFont".into())
}

/// CSS `font-variant-numeric: tabular-nums`.
pub fn tabular_nums() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}
