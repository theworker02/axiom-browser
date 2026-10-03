use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Service-wide counters. Every value here is counted at the point it happens.
#[derive(Debug, Default)]
pub struct NetworkMetrics {
    pub requests: AtomicU64,
    /// Hops actually sent to the transport (redirect hops and revalidations included).
    pub network_requests: AtomicU64,
    pub bytes_transferred: AtomicU64,
    pub bytes_decoded: AtomicU64,
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,
    pub cache_revalidations: AtomicU64,
    pub cancelled: AtomicU64,
    pub failed: AtomicU64,
    pub blocked: AtomicU64,
    /// Largest body buffered for the cache at once.
    pub peak_cache_buffer_bytes: Arc<AtomicU64>,
}

impl NetworkMetrics {
    pub fn snapshot(&self) -> MetricsSnapshot {
        let l = |a: &AtomicU64| a.load(Ordering::Relaxed);
        MetricsSnapshot {
            requests: l(&self.requests),
            network_requests: l(&self.network_requests),
            bytes_transferred: l(&self.bytes_transferred),
            bytes_decoded: l(&self.bytes_decoded),
            cache_hits: l(&self.cache_hits),
            cache_misses: l(&self.cache_misses),
            cache_revalidations: l(&self.cache_revalidations),
            cancelled: l(&self.cancelled),
            failed: l(&self.failed),
            blocked: l(&self.blocked),
            peak_cache_buffer_bytes: l(&self.peak_cache_buffer_bytes),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MetricsSnapshot {
    pub requests: u64,
    pub network_requests: u64,
    pub bytes_transferred: u64,
    pub bytes_decoded: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_revalidations: u64,
    pub cancelled: u64,
    pub failed: u64,
    pub blocked: u64,
    pub peak_cache_buffer_bytes: u64,
}

/// Connection pool facts.
///
/// `connections_opened` is counted by a connector layer inside the transport (one per new
/// TCP connection). Reuse and live-connection counts are not observable through the
/// transport and are therefore `None` rather than estimated.
#[derive(Debug, Clone, Default)]
pub struct PoolStats {
    pub connections_opened: Option<u64>,
    pub connections_reused: Option<u64>,
    pub active_connections: Option<u64>,
}
