//! Update checking: version comparison and GitHub release parsing.
//!
//! Native-only (the web app updates by reloading). Volta only *tells* the user
//! a newer release exists; it never downloads or installs anything.

use serde_json::Value;

/// `GET` this for the newest published (non-draft, non-prerelease) release.
pub const LATEST_RELEASE_API: &str = "https://api.github.com/repos/countervolts/Volta-Desktop/releases/latest";
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// The tag as published, e.g. `v0.2.0`.
    pub tag: String,
    /// The bare version, e.g. `0.2.0`.
    pub version: String,
    /// The release page.
    pub url: String,
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
    Some(ReleaseInfo { tag, version, url })
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
}
