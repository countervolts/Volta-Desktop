//! Update checking: version comparison and GitHub release parsing.
//!
//! Native-only (the web app updates by reloading). Volta only *tells* the user
//! a newer release exists; it never downloads or installs anything.
//!
//! Two channels: **stable** follows tagged releases (`v0.2.0`); **nightly**
//! follows the CI builds of `main`: the newest successful run of the Release
//! workflow, whose artifacts are the nightly binaries. Nightlies have no
//! version number to compare, so a build is "outdated" when that run's commit
//! differs from its own and the run finished after it was built.

use serde_json::Value;

/// `GET` this for the newest published (non-draft, non-prerelease) release.
pub const LATEST_RELEASE_API: &str = "https://api.github.com/repos/countervolts/Volta-Desktop/releases/latest";
/// `GET` this for the newest successful builds of `main` (the nightlies).
pub const NIGHTLY_RUNS_API: &str =
    "https://api.github.com/repos/countervolts/Volta-Desktop/actions/workflows/release.yml/runs?branch=main&status=success&per_page=5";
/// Only links under this prefix are ever opened from an API response.
pub const RELEASES_URL_PREFIX: &str = "https://github.com/countervolts/Volta-Desktop/";
/// Automatic checks run at most this often.
pub const CHECK_INTERVAL_MS: u64 = 24 * 60 * 60 * 1000;

/// A parsed `major.minor.patch[-pre][+build]` version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// `false` sorts after `true`: `1.0.0` is newer than `1.0.0-beta`.
    release: bool,
}

impl Version {
    /// Accepts a leading `v`, build metadata and a pre-release suffix; missing
    /// minor/patch parts count as 0.
    pub fn parse(value: &str) -> Option<Version> {
        let value = value.trim().trim_start_matches(['v', 'V']);
        let value = value.split('+').next()?;
        let (numbers, pre) = match value.split_once('-') {
            Some((numbers, _)) => (numbers, true),
            None => (value, false),
        };
        let mut parts = numbers.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().map_or(Some(0), |p| p.parse().ok())?;
        let patch = parts.next().map_or(Some(0), |p| p.parse().ok())?;
        if parts.next().is_some() {
            return None;
        }
        Some(Version { major, minor, patch, release: !pre })
    }
}

/// `true` when `latest` is a newer version than `current`. Unparseable
/// versions are never "newer".
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (Version::parse(current), Version::parse(latest)) {
        (Some(current), Some(latest)) => latest > current,
        _ => false,
    }
}

/// Whether an automatic check is due, given the stored last-check time.
pub fn check_due(last_check_ms: &str, now_ms: u64) -> bool {
    match last_check_ms.trim().parse::<u64>() {
        // A stored time in the future (clock moved back) must not suppress checks.
        Ok(last) => last > now_ms || now_ms - last >= CHECK_INTERVAL_MS,
        Err(_) => true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Channel {
    #[default]
    Stable,
    Nightly,
}

impl Channel {
    pub fn from_stored(value: &str) -> Self {
        if value == "nightly" { Channel::Nightly } else { Channel::Stable }
    }

    pub fn id(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Nightly => "nightly",
        }
    }

    pub fn api_url(self) -> &'static str {
        match self {
            Channel::Stable => LATEST_RELEASE_API,
            Channel::Nightly => NIGHTLY_RUNS_API,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub channel: Channel,
    /// The tag as published, e.g. `v0.2.0` or `nightly`.
    pub tag: String,
    /// The bare version, e.g. `0.2.0`; `nightly <sha7>` for nightlies.
    pub version: String,
    /// The release page, or for a nightly the build run (its Artifacts list the downloads).
    pub url: String,
    /// The commit a nightly was built from.
    pub commit: Option<String>,
    /// Unix seconds the release was published.
    pub published_unix: Option<i64>,
    pub assets: Vec<ReleaseAsset>,
}

impl ReleaseInfo {
    /// The download for this platform (`macos-aarch64`, `windows-x86_64`, …):
    /// the asset whose name contains it, and nothing else.
    pub fn asset_for(&self, platform: &str) -> Option<&ReleaseAsset> {
        self.assets.iter().find(|a| a.name.contains(platform) && !a.name.ends_with(".sha256") && !a.name.ends_with(".txt"))
    }
}

/// The `os-arch` pair used in release asset names.
pub fn platform_name() -> String {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
    format!("{os}-{arch}")
}

/// Whether `release` is an update for a build with these facts.
pub fn update_available(current_version: &str, current_commit: &str, current_build_unix: i64, release: &ReleaseInfo) -> bool {
    match release.channel {
        Channel::Stable => is_newer(current_version, &release.version),
        Channel::Nightly => {
            let Some(commit) = release.commit.as_deref() else { return false };
            // A build that doesn't know its commit can't tell, so it never nags.
            if current_commit.is_empty() || current_commit == "unknown" || commit == current_commit {
                return false;
            }
            release.published_unix.is_some_and(|published| published > current_build_unix)
        }
    }
}

fn parse_assets(value: &Value) -> Vec<ReleaseAsset> {
    value
        .get("assets")
        .and_then(Value::as_array)
        .map(|assets| {
            assets
                .iter()
                .filter_map(|a| {
                    let name = a.get("name")?.as_str()?.to_string();
                    let url = a.get("browser_download_url")?.as_str()?.to_string();
                    url.starts_with(RELEASES_URL_PREFIX).then_some(ReleaseAsset { name, url })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn published_unix(value: &Value) -> Option<i64> {
    let at = value.get("published_at").or_else(|| value.get("created_at"))?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(at).ok().map(|d| d.timestamp())
}

/// Reads the workflow-runs response: the newest successful run is the
/// current nightly. Its artifacts (one per OS) are on the run's page; they
/// need a GitHub sign-in to download, so Volta only links there.
pub fn parse_nightly(value: &Value) -> Option<ReleaseInfo> {
    let run = value.get("workflow_runs")?.as_array()?.iter().find(|run| {
        run.get("conclusion").and_then(Value::as_str) == Some("success") && run.get("head_branch").and_then(Value::as_str) == Some("main")
    })?;
    let url = run.get("html_url")?.as_str()?.to_string();
    if !url.starts_with(RELEASES_URL_PREFIX) {
        return None;
    }
    let commit = run
        .get("head_sha")?
        .as_str()?
        .trim()
        .to_lowercase();
    if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    // When the build finished: a binary built before this is outdated.
    let finished = run.get("updated_at").or_else(|| run.get("created_at")).and_then(Value::as_str);
    Some(ReleaseInfo {
        channel: Channel::Nightly,
        tag: "nightly".into(),
        version: format!("nightly {}", &commit[..7]),
        url,
        commit: Some(commit),
        published_unix: finished.and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok()).map(|d| d.timestamp()),
        assets: Vec::new(),
    })
}

/// Reads the response for `channel`.
pub fn parse_for_channel(channel: Channel, value: &Value) -> Option<ReleaseInfo> {
    match channel {
        Channel::Stable => parse_release(value),
        Channel::Nightly => parse_nightly(value),
    }
}

/// Reads a `releases/latest` response. Drafts, prereleases and links that
/// point anywhere but this project's GitHub pages are rejected.
pub fn parse_release(value: &Value) -> Option<ReleaseInfo> {
    if value.get("draft").and_then(Value::as_bool) == Some(true) || value.get("prerelease").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let tag = value.get("tag_name")?.as_str()?.trim().to_string();
    Version::parse(&tag)?;
    let url = value.get("html_url")?.as_str()?.to_string();
    if !url.starts_with(RELEASES_URL_PREFIX) {
        return None;
    }
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    Some(ReleaseInfo { channel: Channel::Stable, tag, version, url, commit: None, published_unix: published_unix(value), assets: parse_assets(value) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_versions() {
        let v = Version::parse("v1.2.3").unwrap();
        assert_eq!((v.major, v.minor, v.patch), (1, 2, 3));
        assert_eq!(Version::parse("2"), Version::parse("2.0.0"));
        assert_eq!(Version::parse("1.2.3+build5"), Version::parse("1.2.3"));
        assert!(Version::parse("1.2.3.4").is_none());
        assert!(Version::parse("latest").is_none());
        assert!(Version::parse("").is_none());
    }

    #[test]
    fn compares_numerically_not_textually() {
        assert!(is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("0.9.0", "0.10.0"));
        assert!(is_newer("1.9.9", "v2.0.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.2.0", "0.1.9"));
        assert!(!is_newer("0.1.0", "garbage"));
    }

    #[test]
    fn prerelease_sorts_before_release() {
        assert!(is_newer("1.0.0-beta.1", "1.0.0"));
        assert!(!is_newer("1.0.0", "1.0.0-beta.1"));
    }

    #[test]
    fn check_interval() {
        let day = CHECK_INTERVAL_MS;
        assert!(check_due("", 5));
        assert!(check_due("nonsense", 5));
        assert!(!check_due("1000", 1000 + day - 1));
        assert!(check_due("1000", 1000 + day));
        // A clock that moved backwards must not wedge the check forever.
        assert!(check_due("5000", 1000));
    }

    #[test]
    fn parses_release_json() {
        let ok = json!({"tag_name": "v0.2.0", "html_url": "https://github.com/countervolts/Volta-Desktop/releases/tag/v0.2.0"});
        let release = parse_release(&ok).unwrap();
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.tag, "v0.2.0");

        let draft = json!({"tag_name": "v0.3.0", "draft": true, "html_url": "https://github.com/countervolts/Volta-Desktop/releases/tag/v0.3.0"});
        assert!(parse_release(&draft).is_none());
        let pre = json!({"tag_name": "v0.3.0", "prerelease": true, "html_url": "https://github.com/countervolts/Volta-Desktop/releases/tag/v0.3.0"});
        assert!(parse_release(&pre).is_none());
        let foreign = json!({"tag_name": "v0.3.0", "html_url": "https://evil.example/download"});
        assert!(parse_release(&foreign).is_none());
        let bad_tag = json!({"tag_name": "nightly", "html_url": "https://github.com/countervolts/Volta-Desktop/releases/tag/nightly"});
        assert!(parse_release(&bad_tag).is_none());
    }

    fn runs_json(commit: &str, finished: &str) -> Value {
        json!({
            "workflow_runs": [
                {"conclusion": "failure", "head_branch": "main", "head_sha": "0000000000000000000000000000000000000000",
                 "html_url": "https://github.com/countervolts/Volta-Desktop/actions/runs/1", "updated_at": "2026-11-18T00:00:00Z"},
                {"conclusion": "success", "head_branch": "dev", "head_sha": "1111111111111111111111111111111111111111",
                 "html_url": "https://github.com/countervolts/Volta-Desktop/actions/runs/2", "updated_at": "2026-11-18T00:00:00Z"},
                {"conclusion": "success", "head_branch": "main", "head_sha": commit,
                 "html_url": "https://github.com/countervolts/Volta-Desktop/actions/runs/3", "updated_at": finished}
            ]
        })
    }

    #[test]
    fn nightly_is_the_newest_successful_run_of_main() {
        let sha = "139e0807a1b2c3d4e5f60718293a4b5c6d7e8f90";
        let release = parse_nightly(&runs_json(sha, "2026-11-17T17:50:20Z")).unwrap();
        assert_eq!(release.commit.as_deref(), Some(sha));
        assert_eq!(release.version, "nightly 139e080");
        assert!(release.url.ends_with("/actions/runs/3"), "failed runs and other branches are skipped");
        assert!(parse_nightly(&json!({"workflow_runs": []})).is_none());
        let foreign = json!({"workflow_runs": [{"conclusion": "success", "head_branch": "main", "head_sha": sha, "html_url": "https://evil.example/run"}]});
        assert!(parse_nightly(&foreign).is_none());
        let bad_sha = json!({"workflow_runs": [{"conclusion": "success", "head_branch": "main", "head_sha": "abc", "html_url": "https://github.com/countervolts/Volta-Desktop/actions/runs/9"}]});
        assert!(parse_nightly(&bad_sha).is_none());
    }

    #[test]
    fn nightly_updates_need_a_different_commit_published_after_the_build() {
        let new = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let release = parse_nightly(&runs_json(new, "2026-11-17T18:00:00Z")).unwrap();
        let published = release.published_unix.unwrap();
        assert!(update_available("0.1.0", "bbbb", published - 60, &release));
        assert!(!update_available("0.1.0", new, published - 60, &release), "same commit");
        assert!(!update_available("0.1.0", "bbbb", published + 60, &release), "built after the nightly");
        assert!(!update_available("0.1.0", "unknown", 0, &release));
    }

    #[test]
    fn channel_round_trips() {
        assert_eq!(Channel::from_stored("nightly"), Channel::Nightly);
        assert_eq!(Channel::from_stored(""), Channel::Stable);
        assert_eq!(Channel::from_stored("whatever"), Channel::Stable);
    }
}
