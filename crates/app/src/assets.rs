//! gpui `AssetSource` over the embedded `assets/` tree.
//!
//! Icon paths may carry lucide props as a fragment, because the web passes
//! `strokeWidth` and `fill` per use:
//! `icons/play.svg#fill` (fill="currentColor"), `icons/house.svg#sw=1.8`.
//! gpui caches rasterized SVGs by path, so each variant is cached on its own.

use std::borrow::Cow;

use anyhow::Result;
use gpui::{AssetSource, SharedString};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_assets.rs"));
}

pub struct Assets;

fn find(path: &str) -> Option<&'static [u8]> {
    embedded::EMBEDDED
        .iter()
        .find(|(name, _)| *name == path)
        .map(|(_, bytes)| *bytes)
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let (file, fragment) = path.split_once('#').unwrap_or((path, ""));
        let Some(bytes) = find(file) else {
            // gpui-component looks up its own icon names; those are absent.
            return Ok(None);
        };
        if fragment.is_empty() {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        let mut svg = String::from_utf8_lossy(bytes).into_owned();
        for prop in fragment.split('&') {
            if prop == "fill" {
                svg = svg.replacen("fill=\"none\"", "fill=\"currentColor\"", 1);
            } else if let Some(width) = prop.strip_prefix("sw=") {
                svg = svg.replacen("stroke-width=\"2\"", &format!("stroke-width=\"{width}\""), 1);
            }
        }
        Ok(Some(Cow::Owned(svg.into_bytes())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(embedded::EMBEDDED
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}
