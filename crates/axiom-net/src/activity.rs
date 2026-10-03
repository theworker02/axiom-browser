//! Network activity stream: the foundation for DevTools-style inspection.
//!
//! Subscribers get bounded channels. A subscriber that falls behind loses events (counted
//! in [`ActivityHub::dropped`]); it never slows the network down. Headers are redacted
//! before they are published.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use parking_lot::Mutex;

use crate::cache::CacheState;
use crate::error::NetworkError;
use crate::headers::HeaderMap;
use crate::id::NetworkRequestId;
use crate::protocol::HttpProtocol;

#[derive(Debug, Clone)]
pub enum NetworkActivity {
    RequestStarted {
        id: NetworkRequestId,
        context_id: Option<u64>,
        method: String,
        url: String,
        resource_type: &'static str,
        initiator: String,
    },
    /// Final request headers for one hop (redacted).
    RequestHeadersReady {
        id: NetworkRequestId,
        url: String,
        headers: HeaderMap,
    },
    /// Served from the cache (`Hit` or `Stale`), or confirmed by a 304 (`Revalidated`).
    CacheHit {
        id: NetworkRequestId,
        state: CacheState,
    },
    CacheMiss {
        id: NetworkRequestId,
    },
    Redirected {
        id: NetworkRequestId,
        status: u16,
        from: String,
        to: String,
        cross_origin: bool,
    },
    /// The transport delivered response headers for a hop.
    ResponseStarted {
        id: NetworkRequestId,
        status: u16,
        protocol: HttpProtocol,
    },
    /// Final response headers handed to the consumer (redacted).
    ResponseHeadersReady {
        id: NetworkRequestId,
        status: u16,
        protocol: HttpProtocol,
        cache_state: CacheState,
        headers: HeaderMap,
    },
    DataReceived {
        id: NetworkRequestId,
        bytes: u64,
    },
    RequestCompleted {
        id: NetworkRequestId,
        transferred_bytes: u64,
        decoded_bytes: u64,
        total_ms: f64,
    },
    RequestFailed {
        id: NetworkRequestId,
        error: NetworkError,
    },
    RequestCanceled {
        id: NetworkRequestId,
    },
}

impl NetworkActivity {
    pub fn id(&self) -> NetworkRequestId {
        match self {
            Self::RequestStarted { id, .. }
            | Self::RequestHeadersReady { id, .. }
            | Self::CacheHit { id, .. }
            | Self::CacheMiss { id }
            | Self::Redirected { id, .. }
            | Self::ResponseStarted { id, .. }
            | Self::ResponseHeadersReady { id, .. }
            | Self::DataReceived { id, .. }
            | Self::RequestCompleted { id, .. }
            | Self::RequestFailed { id, .. }
            | Self::RequestCanceled { id } => *id,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::RequestStarted { .. } => "RequestStarted",
            Self::RequestHeadersReady { .. } => "RequestHeadersReady",
            Self::CacheHit { .. } => "CacheHit",
            Self::CacheMiss { .. } => "CacheMiss",
            Self::Redirected { .. } => "Redirected",
            Self::ResponseStarted { .. } => "ResponseStarted",
            Self::ResponseHeadersReady { .. } => "ResponseHeadersReady",
            Self::DataReceived { .. } => "DataReceived",
            Self::RequestCompleted { .. } => "RequestCompleted",
            Self::RequestFailed { .. } => "RequestFailed",
            Self::RequestCanceled { .. } => "RequestCanceled",
        }
    }
}

#[derive(Default)]
pub struct ActivityHub {
    subscribers: Mutex<Vec<Sender<NetworkActivity>>>,
    count: AtomicUsize,
    dropped: AtomicU64,
}

impl ActivityHub {
    /// A new subscriber whose channel holds at most `capacity` undelivered events.
    pub fn subscribe(&self, capacity: usize) -> Receiver<NetworkActivity> {
        let (tx, rx) = bounded(capacity.max(1));
        let mut subs = self.subscribers.lock();
        subs.push(tx);
        self.count.store(subs.len(), Ordering::Release);
        rx
    }

    /// Whether anyone is listening; lets hot paths skip building events.
    pub fn is_observed(&self) -> bool {
        self.count.load(Ordering::Acquire) > 0
    }

    /// Events not delivered because a subscriber's channel was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn emit(&self, event: impl FnOnce() -> NetworkActivity) {
        if !self.is_observed() {
            return;
        }
        let event = event();
        let mut subs = self.subscribers.lock();
        subs.retain(|tx| match tx.try_send(event.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        });
        self.count.store(subs.len(), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_subscribers_drop_and_closed_ones_are_removed() {
        let hub = ActivityHub::default();
        assert!(!hub.is_observed());
        let id = NetworkRequestId::new();
        let rx = hub.subscribe(1);
        let closed = hub.subscribe(4);
        drop(closed);
        hub.emit(|| NetworkActivity::CacheMiss { id });
        hub.emit(|| NetworkActivity::RequestCanceled { id });
        assert_eq!(hub.dropped(), 1);
        assert!(matches!(
            rx.try_recv(),
            Ok(NetworkActivity::CacheMiss { .. })
        ));
        assert_eq!(hub.subscribers.lock().len(), 1);
    }
}
