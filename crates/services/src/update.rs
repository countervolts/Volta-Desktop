//! Asks GitHub for the newest Volta Desktop release. Only reads the public
//! releases API; nothing identifying is sent beyond the app version in the
//! User-Agent (GitHub requires one).

use std::time::Duration;

use volta_logic::update::{LATEST_RELEASE_API, ReleaseInfo, parse_release};

/// `Ok(None)` means no usable release exists yet (404, draft or prerelease).
pub fn latest_release() -> Result<Option<ReleaseInfo>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("Volta/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(LATEST_RELEASE_API)
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
    Ok(parse_release(&value))
}
