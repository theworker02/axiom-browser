//! Request cancellation.
//!
//! Tokens are hierarchical: cancelling a context token (e.g. a tab) cancels every child
//! request token. The transport awaits the token alongside connect / headers / body reads,
//! so cancellation interrupts in-flight I/O instead of waiting for the next poll.

use crate::error::NetworkError;

#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    inner: tokio_util::sync::CancellationToken,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }

    pub fn check(&self) -> Result<(), NetworkError> {
        if self.is_cancelled() {
            Err(NetworkError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// A token cancelled whenever `self` is cancelled (but not vice versa).
    pub fn child_token(&self) -> Self {
        Self {
            inner: self.inner.child_token(),
        }
    }

    pub(crate) async fn cancelled(&self) {
        self.inner.cancelled().await
    }
}
