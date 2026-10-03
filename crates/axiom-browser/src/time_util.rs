//! UTC millisecond timestamps for durable browser data.

use std::time::{SystemTime, UNIX_EPOCH};

/// Canonical persistent time: milliseconds since Unix epoch (UTC).
pub type TimestampMs = i64;

pub fn now_ms() -> TimestampMs {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn host_of_url(url: &str) -> String {
    if let Ok(u) = axiom_url::Url::parse(url) {
        return u.host;
    }
    // Fallback for non-http URLs
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_string()
}
