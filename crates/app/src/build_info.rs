//! What this binary was built from; shown in Settings → About and used by
//! the nightly update check. Filled in by `build.rs`.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GIT_SHA: &str = env!("VOLTA_GIT_SHA");
pub const TARGET: &str = env!("VOLTA_TARGET");
pub const PROFILE: &str = env!("VOLTA_PROFILE");
pub const RUSTC: &str = env!("VOLTA_RUSTC");
/// "stable", "nightly" or "dev".
pub const CHANNEL: &str = env!("VOLTA_CHANNEL");

pub fn dirty() -> bool {
    env!("VOLTA_GIT_DIRTY") == "true"
}

pub fn build_unix() -> i64 {
    env!("VOLTA_BUILD_UNIX").parse().unwrap_or(0)
}

pub fn short_sha() -> &'static str {
    GIT_SHA.get(..7).unwrap_or(GIT_SHA)
}

/// `139e080`, or `139e080 (modified)` when built from a dirty tree.
pub fn commit_label() -> String {
    if dirty() { format!("{} (modified)", short_sha()) } else { short_sha().to_string() }
}

pub fn built_label() -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(build_unix(), 0).single().map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_else(|| "unknown".into())
}

/// "Volta 0.1.0 (nightly, 139e080)".
pub fn summary() -> String {
    format!("Volta {VERSION} ({CHANNEL}, {})", commit_label())
}

/// Library versions the binary was built against: `(name, version)`, for the
/// libraries that exist on this OS.
pub fn libraries() -> Vec<(&'static str, &'static str)> {
    const DEPS: &str = env!("VOLTA_DEPS");
    let skip: &[&str] = if cfg!(target_os = "macos") {
        &["gpui-pre-windows", "gpui-pre-wgpu", "wgpu", "zbus"]
    } else if cfg!(target_os = "windows") {
        &["gpui-pre-apple", "gpui-pre-wgpu", "wgpu", "zbus"]
    } else {
        &["gpui-pre-apple", "gpui-pre-windows"]
    };
    DEPS.split(';')
        .filter_map(|entry| entry.split_once('='))
        .filter(|(name, _)| !skip.contains(name))
        .map(|(name, version)| (if name == "gpui-pre" { "gpui" } else { name }, version))
        .collect()
}
