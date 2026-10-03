//! Security origin foundations (Phase 3 Wave A).
//!
//! Real tuple identity: scheme + host + port. The canonical [`Origin`] type lives in
//! `axiom-url` so the engine's document and resource pipeline uses the same type.

use axiom_url::Url;

pub use axiom_url::Origin;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityOrigin {
    pub origin: Origin,
    pub is_opaque: bool,
}

impl SecurityOrigin {
    pub fn from_url(url: &Url) -> Self {
        Self {
            origin: Origin::from_url(url),
            is_opaque: false,
        }
    }

    pub fn opaque() -> Self {
        Self {
            origin: Origin::opaque(),
            is_opaque: true,
        }
    }

    pub fn same_origin(&self, other: &SecurityOrigin) -> bool {
        if self.is_opaque || other.is_opaque {
            return false;
        }
        self.origin.is_same_origin(&other.origin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_origin_ignores_path() {
        let a = Origin::try_from_str("https://example.com/a").unwrap();
        let b = Origin::try_from_str("https://example.com/b").unwrap();
        assert!(a.is_same_origin(&b));
        let c = Origin::try_from_str("https://example.com:443/").unwrap();
        assert!(a.is_same_origin(&c));
        let d = Origin::try_from_str("http://example.com/").unwrap();
        assert!(!a.is_same_origin(&d));
        assert!(!Origin::opaque().is_same_origin(&Origin::opaque()));
    }
}
