//! Cheap counters the app's performance log reads and resets every few
//! seconds: how many server requests ran, how long they took, how many failed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static REQUESTS: AtomicU64 = AtomicU64::new(0);
static REQUEST_NANOS: AtomicU64 = AtomicU64::new(0);
static REQUEST_MAX_NANOS: AtomicU64 = AtomicU64::new(0);
static SLOW_REQUESTS: AtomicU64 = AtomicU64::new(0);
static FAILED_REQUESTS: AtomicU64 = AtomicU64::new(0);

/// A request slower than this is logged on its own.
pub const SLOW_REQUEST: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RequestStats {
    pub count: u64,
    pub average_ms: f64,
    pub max_ms: f64,
    pub slow: u64,
    pub failed: u64,
}

pub fn record_request(endpoint: &str, elapsed: Duration, ok: bool) {
    let nanos = elapsed.as_nanos() as u64;
    REQUESTS.fetch_add(1, Ordering::Relaxed);
    REQUEST_NANOS.fetch_add(nanos, Ordering::Relaxed);
    REQUEST_MAX_NANOS.fetch_max(nanos, Ordering::Relaxed);
    if !ok {
        FAILED_REQUESTS.fetch_add(1, Ordering::Relaxed);
    }
    if elapsed >= SLOW_REQUEST {
        SLOW_REQUESTS.fetch_add(1, Ordering::Relaxed);
        log::warn!(target: "perf", "slow request {endpoint}: {:.0} ms{}", elapsed.as_secs_f64() * 1000.0, if ok { "" } else { " (failed)" });
    }
}

/// Returns what accumulated since the last call and starts over.
pub fn take_request_stats() -> RequestStats {
    let count = REQUESTS.swap(0, Ordering::Relaxed);
    let nanos = REQUEST_NANOS.swap(0, Ordering::Relaxed);
    RequestStats {
        count,
        average_ms: if count > 0 { nanos as f64 / count as f64 / 1e6 } else { 0.0 },
        max_ms: REQUEST_MAX_NANOS.swap(0, Ordering::Relaxed) as f64 / 1e6,
        slow: SLOW_REQUESTS.swap(0, Ordering::Relaxed),
        failed: FAILED_REQUESTS.swap(0, Ordering::Relaxed),
    }
}
