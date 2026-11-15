//! `resolveShareUrl`: Apple Music links resolve through the public iTunes
//! Search API (title and artist only); Spotify uses its search URL.

use std::time::Duration;

use volta_logic::share_links::{ShareResolution, ShareTarget, itunes_entity, provider_search_url, query_for, resolve_from_itunes};
use volta_logic::storage::ShareProvider;

pub fn resolve_share_url(provider: ShareProvider, target: &ShareTarget) -> ShareResolution {
    let fallback = ShareResolution { url: provider_search_url(provider, target), exact: false };
    if provider == ShareProvider::Spotify {
        return fallback;
    }
    let Ok(mut url) = url::Url::parse("https://itunes.apple.com/search") else { return fallback };
    url.query_pairs_mut()
        .append_pair("term", &query_for(target))
        .append_pair("country", "US")
        .append_pair("media", "music")
        .append_pair("entity", itunes_entity(target.kind))
        .append_pair("limit", "8");
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("Volta/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()
        .and_then(|client| client.get(url).send().ok())
        .filter(|r| r.status().is_success());
    match response.and_then(|r| r.json::<serde_json::Value>().ok()) {
        Some(payload) => resolve_from_itunes(target, &payload),
        None => fallback,
    }
}
