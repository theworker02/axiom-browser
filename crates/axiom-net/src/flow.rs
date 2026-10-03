//! Per-request flow control between a scheduler worker and a slow consumer.
//!
//! The worker may have at most `window` delivered-but-unconsumed body bytes outstanding;
//! past that it stops reading from the connection (TCP backpressure reaches the server)
//! until the consumer [`release`](FlowControl::release)s what it has processed.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};

use crate::cancel::CancellationToken;

#[derive(Clone)]
pub struct FlowControl {
    inner: Arc<Inner>,
}

struct Inner {
    outstanding: Mutex<u64>,
    wake: Condvar,
    window: u64,
}

impl FlowControl {
    pub fn new(window: u64) -> Self {
        Self {
            inner: Arc::new(Inner {
                outstanding: Mutex::new(0),
                wake: Condvar::new(),
                window: window.max(1),
            }),
        }
    }

    pub fn window(&self) -> u64 {
        self.inner.window
    }

    /// Delivered bytes the consumer has not released yet.
    pub fn outstanding(&self) -> u64 {
        *self.inner.outstanding.lock()
    }

    /// The consumer processed `bytes`.
    pub fn release(&self, bytes: u64) {
        let mut o = self.inner.outstanding.lock();
        *o = o.saturating_sub(bytes);
        self.inner.wake.notify_all();
    }

    pub(crate) fn delivered(&self, bytes: u64) {
        *self.inner.outstanding.lock() += bytes;
    }

    /// Block until the window has room. `false` if the request was cancelled meanwhile
    /// (or `stop` says the worker must give up).
    pub(crate) fn wait_for_room(
        &self,
        cancel: &CancellationToken,
        stop: impl Fn() -> bool,
    ) -> bool {
        let mut o = self.inner.outstanding.lock();
        while *o >= self.inner.window {
            if cancel.is_cancelled() || stop() {
                return false;
            }
            self.inner.wake.wait_for(&mut o, Duration::from_millis(25));
        }
        true
    }
}

impl std::fmt::Debug for FlowControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowControl")
            .field("window", &self.inner.window)
            .field("outstanding", &self.outstanding())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_until_released_or_cancelled() {
        let flow = FlowControl::new(10);
        let cancel = CancellationToken::new();
        flow.delivered(10);
        let f2 = flow.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            f2.release(4);
        });
        assert!(flow.wait_for_room(&cancel, || false));
        assert_eq!(flow.outstanding(), 6);
        t.join().unwrap();

        flow.delivered(10);
        let c2 = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            c2.cancel();
        });
        assert!(!flow.wait_for_room(&cancel, || false));
    }
}
