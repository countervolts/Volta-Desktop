//! Asks GitHub for the newest Volta Desktop release. Only reads the public
//! releases API; nothing identifying is sent beyond the app version in the
//! User-Agent (GitHub requires one).

use std::time::Duration;

use volta_logic::update::{Channel, ReleaseInfo, parse_for_channel};

/// `Ok(None)` means no usable release exists yet (404, draft, or for the
/// stable channel a prerelease; no nightly has been published).
pub fn latest_release(channel: Channel) -> Result<Option<ReleaseInfo>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("Volta/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(channel.api_url())
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| e.to_string())?;
    if response.status().as_u16() == 404 {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!("GitHub returned {}", response.status()));
    }
    let value: serde_json::Value = response.json().map_err(|e| e.to_string())?;
    Ok(parse_for_channel(channel, &value))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Talks to the real GitHub API: `cargo test -p volta-services -- --ignored live_nightly`.
    #[test]
    #[ignore = "network"]
    fn live_nightly_check_finds_the_newest_main_build() {
        let release = latest_release(Channel::Nightly).expect("request succeeds").expect("a successful main build exists");
        assert_eq!(release.commit.as_deref().map(str::len), Some(40));
        assert!(release.url.contains("/actions/runs/"), "{}", release.url);
        println!("nightly: {} -> {}", release.version, release.url);
    }
}
