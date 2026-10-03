//! `fetch(…, { keepalive: true })` requests.
//!
//! Keepalive requests run on their own [`ResourceLoader`]: the same profile scheduler and
//! network service as every other request, but a separate cancellation context, so
//! navigating away (which cancels the document's loads) does not cancel them. When a
//! tab closes, its [`KeepaliveLoads`] are handed to the owner, which keeps polling them
//! until they finish.

use std::collections::HashMap;
use std::sync::Arc;

use axiom_loader::{LoaderEvent, RequestId, ResourceLoader, ResourceRequest};
use axiom_net::RequestScheduler;
use axiom_trace::TraceTimeline;

/// Fetch "keepalive" quota: total request-body bytes of in-flight keepalive requests.
pub const KEEPALIVE_QUOTA: u64 = 64 * 1024;

pub struct KeepaliveLoads {
    loader: ResourceLoader,
    /// Request-body length of every in-flight request (counted against the quota).
    in_flight: HashMap<RequestId, u64>,
}

impl KeepaliveLoads {
    pub fn new(scheduler: Arc<RequestScheduler>, trace: Arc<TraceTimeline>) -> Self {
        Self {
            loader: ResourceLoader::shared(scheduler, trace),
            in_flight: HashMap::new(),
        }
    }

    pub fn scheduler(&self) -> &Arc<RequestScheduler> {
        self.loader.scheduler()
    }

    pub fn start(&mut self, request: ResourceRequest, body_len: u64) -> RequestId {
        let id = self.loader.start(request);
        self.in_flight.insert(id, body_len);
        id
    }

    pub fn cancel(&mut self, id: RequestId) {
        if self.in_flight.contains_key(&id) {
            self.loader.cancel(id);
        }
    }

    pub fn contains(&self, id: RequestId) -> bool {
        self.in_flight.contains_key(&id)
    }

    /// Body bytes of in-flight requests.
    pub fn in_flight_bytes(&self) -> u64 {
        self.in_flight.values().sum()
    }

    pub fn len(&self) -> usize {
        self.in_flight.len()
    }

    pub fn is_empty(&self) -> bool {
        self.in_flight.is_empty()
    }

    /// Drain loader events; finished requests leave the quota.
    pub fn poll(&mut self) -> Vec<LoaderEvent> {
        let events = self.loader.poll();
        for ev in &events {
            if ev.is_terminal() {
                self.in_flight.remove(&ev.id());
            }
        }
        events
    }
}

/// Keepalive loads whose browsing context is gone (closed tabs). Poll regularly: undrained
/// events would otherwise hold shared scheduler workers.
#[derive(Default)]
pub struct DetachedKeepalive {
    loads: Vec<KeepaliveLoads>,
}

impl DetachedKeepalive {
    pub fn adopt(&mut self, loads: Vec<KeepaliveLoads>) {
        self.loads
            .extend(loads.into_iter().filter(|l| !l.is_empty()));
    }

    pub fn poll(&mut self) {
        for loads in &mut self.loads {
            for ev in loads.poll() {
                if let LoaderEvent::Failed { url, error, .. } = ev {
                    log::warn!("keepalive fetch {url} failed: {error}");
                }
            }
        }
        self.loads.retain(|l| !l.is_empty());
    }

    /// Requests still in flight.
    pub fn len(&self) -> usize {
        self.loads.iter().map(KeepaliveLoads::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
