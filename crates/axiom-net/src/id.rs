use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NetworkRequestId(pub u64);

static NEXT: AtomicU64 = AtomicU64::new(1);

impl NetworkRequestId {
    pub fn new() -> Self {
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for NetworkRequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for NetworkRequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
