//! Identity types. None of them is ever derived from a URL.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DOCUMENT: AtomicU64 = AtomicU64::new(1);
static NEXT_RESOURCE: AtomicU64 = AtomicU64::new(1);
static NEXT_CONTEXT: AtomicU64 = AtomicU64::new(1);

/// One document instance. A reload or a navigation to the same URL creates a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentId(pub u64);

impl DocumentId {
    pub fn next() -> Self {
        Self(NEXT_DOCUMENT.fetch_add(1, Ordering::Relaxed))
    }
}

/// One discovered subresource of one document (unique across documents).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceId(pub u64);

impl ResourceId {
    pub fn next() -> Self {
        Self(NEXT_RESOURCE.fetch_add(1, Ordering::Relaxed))
    }
}

/// Identity of one navigation attempt within a browsing context. Only the newest
/// navigation of a context may commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NavigationId(pub u64);

/// One browsing context (a tab's top-level context, or a headless one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BrowsingContextId(pub u64);

impl BrowsingContextId {
    pub fn next() -> Self {
        Self(NEXT_CONTEXT.fetch_add(1, Ordering::Relaxed))
    }
}

macro_rules! display_id {
    ($($t:ty => $prefix:literal),*) => {$(
        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }
    )*};
}

display_id!(DocumentId => "doc-", ResourceId => "res-", NavigationId => "nav-", BrowsingContextId => "ctx-");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_display_with_a_prefix() {
        let (a, b) = (DocumentId::next(), DocumentId::next());
        assert_ne!(a, b);
        assert!(ResourceId::next() < ResourceId::next());
        assert_eq!(NavigationId(3).to_string(), "nav-3");
    }
}
