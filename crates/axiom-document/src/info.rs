//! Who a document belongs to.

use axiom_url::Origin;

use crate::ids::{BrowsingContextId, DocumentId, NavigationId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    /// Fetched over HTTP(S) through the network service.
    Network,
    /// Read from a local file (`file:` or a path).
    LocalFile,
    /// Trusted, locally generated HTML (`axiom://` pages, error and download pages, test
    /// HTML). Never loads web resources.
    Internal,
}

impl DocumentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::LocalFile => "file",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DocumentInfo {
    pub id: DocumentId,
    /// The navigation that created the document (`None` for internal HTML).
    pub navigation: Option<NavigationId>,
    pub browsing_context: BrowsingContextId,
    pub tab: Option<u64>,
    pub profile: Option<String>,
    pub origin: Origin,
    /// Final document URL (after redirects).
    pub url: String,
    pub kind: DocumentKind,
}

impl DocumentInfo {
    /// Delivered over HTTPS (for mixed-content decisions).
    pub fn is_secure(&self) -> bool {
        self.kind == DocumentKind::Network && self.origin.scheme == "https"
    }
}
