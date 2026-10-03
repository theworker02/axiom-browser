//! Policy hooks checked before a subresource request is created.
//!
//! [`ContentPolicy`] is the CSP integration point: it is asked once per subresource with
//! the matching fetch directive. There is no CSP header parser yet; the default policy
//! allows everything. Mixed content is decided centrally by [`mixed_content_decision`].

use axiom_net::{is_potentially_trustworthy, ResourceType};
use axiom_url::Url;

use crate::info::DocumentInfo;

/// CSP fetch directives the loader consults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PolicyDirective {
    ScriptSrc,
    StyleSrc,
    ImgSrc,
    FontSrc,
    ConnectSrc,
    DefaultSrc,
}

impl PolicyDirective {
    pub fn for_resource(kind: ResourceType) -> Self {
        match kind {
            ResourceType::Script => Self::ScriptSrc,
            ResourceType::Stylesheet => Self::StyleSrc,
            ResourceType::Image => Self::ImgSrc,
            ResourceType::Font => Self::FontSrc,
            ResourceType::Fetch | ResourceType::Xhr | ResourceType::WebSocket => Self::ConnectSrc,
            _ => Self::DefaultSrc,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ScriptSrc => "script-src",
            Self::StyleSrc => "style-src",
            Self::ImgSrc => "img-src",
            Self::FontSrc => "font-src",
            Self::ConnectSrc => "connect-src",
            Self::DefaultSrc => "default-src",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Block(String),
}

pub trait ContentPolicy: Send + Sync {
    fn name(&self) -> &str;
    fn check(
        &self,
        directive: PolicyDirective,
        url: &Url,
        document: &DocumentInfo,
    ) -> PolicyDecision;
}

/// The default: no content policy.
#[derive(Debug, Default, Clone, Copy)]
pub struct AllowAllPolicy;

impl ContentPolicy for AllowAllPolicy {
    fn name(&self) -> &str {
        "allow-all"
    }
    fn check(&self, _: PolicyDirective, _: &Url, _: &DocumentInfo) -> PolicyDecision {
        PolicyDecision::Allow
    }
}

/// Blocks the listed hosts for one directive (a stand-in for a parsed CSP source list;
/// used by tests and embedders).
#[derive(Debug, Clone)]
pub struct BlockHostsPolicy {
    pub directive: PolicyDirective,
    pub hosts: Vec<String>,
}

impl ContentPolicy for BlockHostsPolicy {
    fn name(&self) -> &str {
        "block-hosts"
    }
    fn check(&self, directive: PolicyDirective, url: &Url, _: &DocumentInfo) -> PolicyDecision {
        if directive == self.directive && self.hosts.iter().any(|h| h == &url.host) {
            PolicyDecision::Block(format!("{} blocks {}", directive.as_str(), url.host))
        } else {
            PolicyDecision::Allow
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixedContent {
    NotMixed,
    /// Images and media: loaded, but the document is flagged as mixed.
    Passive,
    /// Scripts, stylesheets, fonts, fetches: blocked.
    Blockable,
}

pub fn classify_mixed_content(
    document_secure: bool,
    url: &Url,
    kind: ResourceType,
) -> MixedContent {
    if !document_secure || is_potentially_trustworthy(url) {
        return MixedContent::NotMixed;
    }
    match kind {
        ResourceType::Image | ResourceType::Media => MixedContent::Passive,
        _ => MixedContent::Blockable,
    }
}

pub fn mixed_content_decision(
    document_secure: bool,
    url: &Url,
    kind: ResourceType,
) -> PolicyDecision {
    match classify_mixed_content(document_secure, url, kind) {
        MixedContent::Blockable => PolicyDecision::Block(format!(
            "mixed content: a secure document may not load {} over {}",
            kind.as_str(),
            url.scheme
        )),
        MixedContent::NotMixed | MixedContent::Passive => PolicyDecision::Allow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn mixed_content_blocks_active_and_flags_passive() {
        let http = url("http://example.test/a");
        assert_eq!(
            classify_mixed_content(true, &http, ResourceType::Script),
            MixedContent::Blockable
        );
        assert_eq!(
            classify_mixed_content(true, &http, ResourceType::Image),
            MixedContent::Passive
        );
        assert_eq!(
            classify_mixed_content(false, &http, ResourceType::Script),
            MixedContent::NotMixed
        );
        let loopback = url("http://127.0.0.1:8080/a.js");
        assert_eq!(
            classify_mixed_content(true, &loopback, ResourceType::Script),
            MixedContent::NotMixed
        );
        assert!(matches!(
            mixed_content_decision(true, &http, ResourceType::Font),
            PolicyDecision::Block(_)
        ));
    }

    #[test]
    fn directives_map_from_resource_types() {
        assert_eq!(
            PolicyDirective::for_resource(ResourceType::Script).as_str(),
            "script-src"
        );
        assert_eq!(
            PolicyDirective::for_resource(ResourceType::Font).as_str(),
            "font-src"
        );
        assert_eq!(
            PolicyDirective::for_resource(ResourceType::Fetch).as_str(),
            "connect-src"
        );
    }
}
