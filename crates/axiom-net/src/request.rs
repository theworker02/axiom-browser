use crate::body::RequestBody;
use crate::cache::CacheMode;
use crate::flow::FlowControl;
use crate::headers::HeaderMap;
use crate::id::NetworkRequestId;
use crate::priority::RequestPriority;
use crate::redirect::RedirectMode;
use crate::referrer::ReferrerPolicy;
use crate::resource_type::ResourceType;
use axiom_url::Url;
use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
    Options,
    /// Any other valid token (case-preserved, as HTTP methods are case-sensitive).
    Other(String),
}

impl HttpMethod {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Options => "OPTIONS",
            Self::Other(m) => m,
        }
    }

    /// Parse a method token. Standard methods are normalized case-insensitively (as Fetch
    /// does); anything else must be a valid RFC 9110 token.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let m = match s.to_ascii_uppercase().as_str() {
            "GET" => Self::Get,
            "HEAD" => Self::Head,
            "POST" => Self::Post,
            "PUT" => Self::Put,
            "PATCH" => Self::Patch,
            "DELETE" => Self::Delete,
            "OPTIONS" => Self::Options,
            _ => {
                let valid = !s.is_empty()
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b));
                if !valid {
                    return None;
                }
                Self::Other(s.to_string())
            }
        };
        Some(m)
    }

    pub fn is_safe(&self) -> bool {
        matches!(self, Self::Get | Self::Head | Self::Options)
    }

    pub fn is_idempotent(&self) -> bool {
        matches!(
            self,
            Self::Get | Self::Head | Self::Put | Self::Delete | Self::Options
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialsMode {
    Omit,
    SameOrigin,
    Include,
}

/// Fetch request mode. `SameOrigin` requests never follow a redirect to another origin.
/// `Cors` requests get a CORS check on every hop whose URL is cross-origin to the
/// requester, and a preflight first when they need one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestMode {
    Navigate,
    SameOrigin,
    NoCors,
    Cors,
}

impl RequestMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Navigate => "navigate",
            Self::SameOrigin => "same-origin",
            Self::NoCors => "no-cors",
            Self::Cors => "cors",
        }
    }
}

/// A requester-supplied check of every redirect target (the requesting document's
/// Content Security Policy). An `Err` stops the request with
/// [`NetworkError::Blocked`](crate::NetworkError::Blocked) before the hop is sent.
#[derive(Clone)]
pub struct RedirectCheck(Arc<RedirectCheckFn>);

type RedirectCheckFn = dyn Fn(&Url) -> Result<(), String> + Send + Sync;

impl RedirectCheck {
    pub fn new(check: impl Fn(&Url) -> Result<(), String> + Send + Sync + 'static) -> Self {
        Self(Arc::new(check))
    }

    pub fn check(&self, url: &Url) -> Result<(), String> {
        (self.0)(url)
    }
}

impl fmt::Debug for RedirectCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RedirectCheck")
    }
}

#[derive(Debug, Clone)]
pub struct NetworkRequest {
    pub id: NetworkRequestId,
    pub url: Url,
    pub method: HttpMethod,
    pub headers: HeaderMap,
    pub body: RequestBody,
    pub resource_type: ResourceType,
    pub priority: RequestPriority,
    pub redirect_mode: RedirectMode,
    pub mode: RequestMode,
    pub cache_mode: CacheMode,
    pub credentials_mode: CredentialsMode,
    /// Referrer source URL (policy is applied by the service on every hop).
    pub referrer: Option<String>,
    pub referrer_policy: ReferrerPolicy,
    /// The requesting document's origin URL (`None` for browser-initiated navigations).
    pub origin: Option<Url>,
    /// Top-level document URL, used for SameSite cookie decisions. For top-level
    /// navigations this is updated to follow redirects.
    pub top_level_url: Option<Url>,
    pub initiator: String,
    pub context_id: Option<u64>,
    /// Hard cap on the decoded body delivered by the scheduler.
    pub max_body_bytes: Option<u64>,
    /// Filled in by the scheduler before dispatch.
    pub queued_ms: Option<f64>,
    /// Whole-request deadline (connect + headers + body).
    pub request_timeout_ms: Option<u64>,
    /// Consumer-driven backpressure for the response body (see [`FlowControl`]).
    pub flow: Option<FlowControl>,
    /// Fetch "unsafe-request flag" (script `fetch()`): a cross-origin `Cors` request
    /// with a non-safelisted method or header is preflighted.
    pub unsafe_request: bool,
    /// Fetch "use-CORS-preflight flag": preflight a cross-origin `Cors` request even if
    /// its method and headers are safelisted (streamed request bodies).
    pub use_cors_preflight: bool,
    /// Runs on every redirect target before it is fetched.
    pub redirect_check: Option<RedirectCheck>,
}

impl NetworkRequest {
    pub fn new(method: HttpMethod, url: Url, resource_type: ResourceType) -> Self {
        Self {
            id: NetworkRequestId::new(),
            url,
            method,
            headers: HeaderMap::new(),
            body: RequestBody::Empty,
            resource_type,
            priority: resource_type.default_priority(),
            redirect_mode: RedirectMode::Follow,
            mode: if resource_type == ResourceType::Document {
                RequestMode::Navigate
            } else {
                RequestMode::NoCors
            },
            cache_mode: CacheMode::Default,
            credentials_mode: CredentialsMode::Include,
            referrer: None,
            referrer_policy: ReferrerPolicy::StrictOriginWhenCrossOrigin,
            origin: None,
            top_level_url: None,
            initiator: String::new(),
            context_id: None,
            max_body_bytes: None,
            queued_ms: None,
            request_timeout_ms: Some(60_000),
            flow: None,
            unsafe_request: false,
            use_cors_preflight: false,
            redirect_check: None,
        }
    }

    pub fn get(url: Url, resource_type: ResourceType) -> Self {
        Self::new(HttpMethod::Get, url, resource_type)
    }

    pub fn is_top_level_navigation(&self) -> bool {
        self.resource_type == ResourceType::Document
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_tokens() {
        assert_eq!(HttpMethod::parse("get"), Some(HttpMethod::Get));
        assert_eq!(
            HttpMethod::parse("PROPFIND"),
            Some(HttpMethod::Other("PROPFIND".into()))
        );
        assert_eq!(HttpMethod::parse("BAD METHOD"), None);
        assert_eq!(HttpMethod::parse(""), None);
        assert!(!HttpMethod::Post.is_safe());
    }
}
