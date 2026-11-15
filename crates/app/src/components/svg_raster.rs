//! SVG cover rasterizing with system fonts.
//!
//! gpui draws SVG images without system fonts, so a cover that names
//! `font-family="Georgia"` renders in a fallback face. Browsers resolve such
//! families through the platform (fontconfig on Linux: Georgia → Noto Serif).
//! This rasterizer loads the system font database and, for a named family the
//! database lacks, substitutes what `fc-match` resolves it to.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{self, fontdb};

fn database() -> Arc<fontdb::Database> {
    static DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        for (generic, set) in [
            ("serif", fontdb::Database::set_serif_family as fn(&mut fontdb::Database, String)),
            ("sans-serif", fontdb::Database::set_sans_serif_family),
            ("monospace", fontdb::Database::set_monospace_family),
        ] {
            if let Some(family) = fc_match(generic) {
                set(&mut db, family);
            }
        }
        Arc::new(db)
    })
    .clone()
}

/// The family fontconfig picks for `name` (Linux; `None` elsewhere).
fn fc_match(name: &str) -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let output = std::process::Command::new("fc-match").args(["-f", "%{family[0]}", name]).output().ok()?;
    let family = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!family.is_empty()).then_some(family)
}

fn resolve_family(db: &fontdb::Database, name: &str) -> String {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(found) = cache.lock().ok().and_then(|c| c.get(name).cloned()) {
        return found;
    }
    let generic = matches!(name, "serif" | "sans-serif" | "monospace" | "cursive" | "fantasy");
    let known = generic || db.faces().any(|face| face.families.iter().any(|(family, _)| family.eq_ignore_ascii_case(name)));
    let resolved = if known { name.to_string() } else { fc_match(name).unwrap_or_else(|| name.to_string()) };
    if let Ok(mut c) = cache.lock() {
        c.insert(name.to_string(), resolved.clone());
    }
    resolved
}

/// Rewrites `font-family="A, B"` attributes to families the database has.
fn substitute_families(svg: &str, db: &fontdb::Database) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(start) = rest.find("font-family=") {
        let (head, tail) = rest.split_at(start + "font-family=".len());
        out.push_str(head);
        let Some(quote) = tail.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            rest = tail;
            continue;
        };
        let Some(end) = tail[1..].find(quote) else {
            rest = tail;
            continue;
        };
        let families: Vec<String> = tail[1..1 + end]
            .split(',')
            .map(|f| f.trim().trim_matches(|c| c == '"' || c == '\''))
            .filter(|f| !f.is_empty())
            .map(|f| resolve_family(db, f))
            .collect();
        let inner = if quote == '"' { '\'' } else { '"' };
        out.push(quote);
        out.push_str(&families.iter().map(|f| if f.contains(' ') { format!("{inner}{f}{inner}") } else { f.clone() }).collect::<Vec<_>>().join(", "));
        out.push(quote);
        rest = &tail[end + 2..];
    }
    out.push_str(rest);
    out
}

pub fn is_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(256)]);
    head.contains("<svg") || head.trim_start().starts_with("<?xml")
}

/// Renders an SVG to a `side`×`side` pixmap (aspect-fit).
pub fn render(bytes: &[u8], side: u32) -> Option<Pixmap> {
    let db = database();
    let source = substitute_families(std::str::from_utf8(bytes).ok()?, &db);
    let options = usvg::Options { fontdb: db, ..Default::default() };
    let tree = usvg::Tree::from_str(&source, &options).ok()?;
    let mut pixmap = Pixmap::new(side, side)?;
    let size = tree.size();
    let scale = side as f32 / size.width().max(size.height());
    resvg::render(&tree, Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Some(pixmap)
}

/// PNG bytes for an SVG cover at `side` px.
pub fn render_png(bytes: &[u8], side: u32) -> Option<Vec<u8>> {
    render(bytes, side)?.encode_png().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_and_preserves_other_attributes() {
        let db = fontdb::Database::new();
        let out = substitute_families("<text font-family=\"serif\" x=\"1\">a</text>", &db);
        assert_eq!(out, "<text font-family=\"serif\" x=\"1\">a</text>");
    }
}
