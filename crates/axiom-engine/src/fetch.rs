//! `fetch()` host side: turns a [`FetchInit`] from the JS bindings into a loader request
//! (or a rejection), and filters responses before JS sees them.
//!
//! This is the security boundary for script-initiated requests:
//!
//! * Only `http(s):` and `data:` URLs are fetchable (never `file:`, `axiom:` or others).
//! * Cross-origin `cors` requests use the CORS protocol, enforced by the network service
//!   on every hop: a preflight when the method or headers are not safelisted (or the body
//!   is streamed), and a CORS check on every cross-origin response, redirects included.
//!   Script then sees only the safelisted and exposed headers. `same-origin` mode rejects
//!   cross-origin URLs (and redirects); `no-cors` requests are limited to safelisted
//!   methods and headers and get opaque responses.
//! * Navigation never goes through CORS: documents are loaded by the navigation pipeline.
//! * Forbidden request headers (`Cookie`, `Host`, `Origin`, `Referer`, `Sec-*`, …) are
//!   dropped; `Set-Cookie` never reaches script.
//! * HTTPS documents cannot fetch non-trustworthy `http:` URLs (mixed content).
//! * `integrity` metadata is verified over the whole body before script sees the response.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use axiom_js::{FetchInit, FetchResponseHead};
use axiom_loader::{
    is_redirect_status, parse_data_url, same_origin, CacheMode, CredentialsMode, FlowControl,
    HeaderMap, HttpMethod, RedirectMode, ReferrerPolicy, RequestBody, RequestId, RequestMode,
    RequestPriority, ResourceRequest, ResourceResponse, ResourceType, StreamingBody, UploadSender,
};
use axiom_net::{
    cors_check, is_forbidden_request_header, is_no_cors_safelisted_request_header,
    is_potentially_trustworthy, origin_tainted, serialize_origin,
};
use axiom_url::Url;

use crate::keepalive::KEEPALIVE_QUOTA;
use crate::sri::{parse_integrity, Integrity};

/// Response bytes the network may deliver ahead of script reading them.
pub const FETCH_FLOW_WINDOW: u64 = 1024 * 1024;
/// Buffered streaming-upload bytes at which script is asked to wait…
pub const UPLOAD_HIGH_WATER: u64 = 256 * 1024;
/// …and the level at which it is told to resume.
pub const UPLOAD_LOW_WATER: u64 = 64 * 1024;

/// Per-document fetch state shared by the script host and the browsing context.
pub type FetchQueue = Rc<RefCell<FetchState>>;

#[derive(Debug, Default)]
pub struct FetchState {
    /// Commands queued by script, drained by the browsing context's event loop.
    pub commands: VecDeque<FetchCommand>,
    /// Streaming request bodies being written by script.
    pub uploads: HashMap<u64, UploadSlot>,
    /// Response flow control of in-flight streamed fetches.
    pub flows: HashMap<u64, FlowControl>,
}

impl FetchState {
    /// Forget every in-flight fetch: queued commands are dropped and open uploads fail.
    pub fn clear(&mut self) {
        self.commands.clear();
        self.uploads.clear();
        self.flows.clear();
    }

    /// Drop the upload and flow-control state of a finished fetch.
    pub fn finish(&mut self, id: u64) {
        self.uploads.remove(&id);
        self.flows.remove(&id);
    }

    /// Uploads that asked script to wait and have drained below the low-water mark.
    pub fn take_drained_uploads(&mut self) -> Vec<u64> {
        let mut ids = Vec::new();
        for (id, slot) in &mut self.uploads {
            if slot.wants_drain && slot.sender.buffered() <= UPLOAD_LOW_WATER {
                slot.wants_drain = false;
                ids.push(*id);
            }
        }
        ids
    }
}

#[derive(Debug)]
pub struct UploadSlot {
    pub sender: UploadSender,
    /// Script is waiting for a drain notification.
    pub wants_drain: bool,
}

#[derive(Debug)]
pub enum FetchCommand {
    Start(FetchPlan),
    Abort(u64),
}

#[derive(Debug)]
pub enum FetchPlan {
    Network {
        request: Box<ResourceRequest>,
        policy: FetchPolicy,
        /// Writer for a streaming (`ReadableStream`) request body.
        upload: Option<UploadSender>,
    },
    /// `data:` URL, answered without touching the network.
    Data {
        id: u64,
        url: String,
        mime: String,
        body: Vec<u8>,
        integrity: Option<Integrity>,
    },
}

impl FetchPlan {
    pub fn id(&self) -> u64 {
        match self {
            Self::Network { request, .. } => request.id.0,
            Self::Data { id, .. } => *id,
        }
    }
}

/// What the response filter needs to know about an in-flight fetch.
#[derive(Debug, Clone)]
pub struct FetchPolicy {
    pub mode: RequestMode,
    /// Requesting document origin (`None` for opaque origins such as `file:`).
    pub origin: Option<Url>,
    pub redirect: RedirectMode,
    /// Runs on the keepalive loader and outlives the document.
    pub keepalive: bool,
    /// The body must match before the response is revealed.
    pub integrity: Option<Integrity>,
    pub credentials: CredentialsMode,
}

/// Response headers script may read on any CORS response.
const CORS_SAFELISTED_RESPONSE_HEADERS: &[&str] = &[
    "cache-control",
    "content-language",
    "content-length",
    "content-type",
    "expires",
    "last-modified",
    "pragma",
];

fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// Origin of a document URL, or `None` when it is opaque (non-HTTP(S) documents).
pub fn document_origin(document_url: &str) -> Option<Url> {
    let u = Url::parse(document_url).ok()?;
    matches!(u.scheme.as_str(), "http" | "https").then_some(u)
}

/// Resolve a script-supplied URL against the document.
pub fn resolve_url(document_url: &str, input: &str) -> Option<String> {
    let input = input.trim();
    if let Some((scheme, _)) = input.split_once(':') {
        let scheme = scheme.to_ascii_lowercase();
        let absolute = !scheme.is_empty()
            && scheme
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
        if absolute {
            return match scheme.as_str() {
                "http" | "https" => Url::parse(input).ok().map(|u| u.as_str()),
                // Kept verbatim so `fetch` can report the unsupported scheme precisely.
                _ => Some(input.to_string()),
            };
        }
    }
    let base = Url::parse(document_url).ok()?;
    base.join(input).ok().map(|u| u.as_str())
}

/// Fetch `Request` referrer for a script-supplied URL: the URL when it is same-origin with
/// the document, otherwise `about:client` (the document itself).
pub fn resolve_referrer(document_url: &str, url: &str) -> String {
    let same = document_origin(document_url)
        .zip(Url::parse(url).ok())
        .is_some_and(|(origin, u)| same_origin(&origin, &u));
    if same {
        url.to_string()
    } else {
        "about:client".into()
    }
}

/// Validate a `fetch()` call and turn it into a plan. `Err` is the `TypeError` message.
pub fn plan_fetch(init: &FetchInit, document_url: &str) -> Result<FetchPlan, String> {
    let url_str = resolve_url(document_url, &init.url)
        .ok_or_else(|| format!("Failed to parse URL from {}", init.url))?;
    let integrity = parse_integrity(&init.integrity);
    let method = HttpMethod::parse(&init.method)
        .ok_or_else(|| format!("'{}' is not a valid HTTP method", init.method))?;
    if matches!(
        method.as_str().to_ascii_uppercase().as_str(),
        "CONNECT" | "TRACE" | "TRACK"
    ) {
        return Err(format!("'{}' HTTP method is unsupported", method.as_str()));
    }
    let mode = match init.mode.as_str() {
        "" | "cors" => RequestMode::Cors,
        "no-cors" => RequestMode::NoCors,
        "same-origin" => RequestMode::SameOrigin,
        other => return Err(format!("'{other}' is not a valid request mode")),
    };
    let has_body = init.body.is_some() || init.body_stream;
    if has_body && matches!(method, HttpMethod::Get | HttpMethod::Head) {
        return Err("Request with GET/HEAD method cannot have body".into());
    }
    if init.body_stream {
        if init.keepalive {
            return Err("keepalive requests cannot have a ReadableStream body".into());
        }
        if mode == RequestMode::NoCors {
            return Err(
                "ReadableStream request bodies require 'cors' or 'same-origin' mode".into(),
            );
        }
    }
    let body_len = init.body.as_ref().map_or(0, |b| b.len() as u64);
    if init.keepalive && body_len > KEEPALIVE_QUOTA {
        return Err(format!(
            "keepalive request body ({body_len} bytes) exceeds the {KEEPALIVE_QUOTA}-byte quota"
        ));
    }

    let scheme = url_str
        .split_once(':')
        .map(|(s, _)| s.to_ascii_lowercase())
        .unwrap_or_default();
    if scheme == "data" {
        let (mime, body) =
            parse_data_url(&url_str).ok_or_else(|| "Invalid data: URL".to_string())?;
        return Ok(FetchPlan::Data {
            id: RequestId::new().0,
            url: url_str,
            mime,
            body,
            integrity,
        });
    }
    if scheme != "http" && scheme != "https" {
        return Err(format!("Fetch API cannot load {scheme}: URLs"));
    }
    // Userinfo (`user:pass@host`) never parses: '@' is a forbidden host code point.
    let url = Url::parse(&url_str).map_err(|e| format!("Failed to parse URL: {e}"))?;

    let origin = document_origin(document_url);
    let same = origin.as_ref().is_some_and(|o| same_origin(o, &url));
    if let Some(o) = &origin {
        if o.scheme == "https" && !is_potentially_trustworthy(&url) {
            return Err(format!(
                "Mixed content: the HTTPS page requested insecure resource {}",
                url.origin()
            ));
        }
    }
    if !same && mode == RequestMode::SameOrigin {
        return Err("Request mode is 'same-origin' but the URL's origin differs".into());
    }
    if mode == RequestMode::NoCors
        && !matches!(
            method,
            HttpMethod::Get | HttpMethod::Head | HttpMethod::Post
        )
    {
        return Err(format!(
            "'{}' is unsupported in no-cors mode",
            method.as_str()
        ));
    }

    let mut combined: Vec<(String, String)> = Vec::new();
    for (name, value) in &init.headers {
        let value = value.trim_matches(|c| matches!(c, ' ' | '\t' | '\r' | '\n'));
        if !is_token(name) || value.contains(['\0', '\r', '\n']) {
            return Err(format!("Invalid header: {name}"));
        }
        if is_forbidden_request_header(name, value)
            || (mode == RequestMode::NoCors && !is_no_cors_safelisted_request_header(name, value))
        {
            continue;
        }
        let name = name.to_ascii_lowercase();
        match combined.iter_mut().find(|(n, _)| *n == name) {
            Some((_, v)) => {
                v.push_str(", ");
                v.push_str(value);
            }
            None => combined.push((name, value.to_string())),
        }
    }
    let mut headers = HeaderMap::new();
    for (name, value) in combined {
        headers.set(&name, value);
    }
    if !headers.contains("accept") {
        headers.set("Accept", "*/*");
    }
    if !matches!(method, HttpMethod::Get | HttpMethod::Head) || (mode == RequestMode::Cors && !same)
    {
        let serialized = origin.as_ref().map_or("null".to_string(), Url::origin);
        headers.set("Origin", serialized);
    }

    let credentials = match init.credentials.as_str() {
        "omit" => CredentialsMode::Omit,
        "include" => CredentialsMode::Include,
        "" | "same-origin" if origin.is_none() => CredentialsMode::Omit,
        "" | "same-origin" => CredentialsMode::SameOrigin,
        other => return Err(format!("'{other}' is not a valid credentials mode")),
    };
    let cache = match init.cache.as_str() {
        "" | "default" => CacheMode::Default,
        "no-store" => CacheMode::NoStore,
        "reload" => CacheMode::Reload,
        "no-cache" => CacheMode::NoCache,
        "force-cache" => CacheMode::ForceCache,
        "only-if-cached" if mode == RequestMode::SameOrigin => CacheMode::OnlyIfCached,
        "only-if-cached" => {
            return Err("'only-if-cached' can be set only with 'same-origin' mode".into())
        }
        other => return Err(format!("'{other}' is not a valid cache mode")),
    };
    let redirect = match init.redirect.as_str() {
        "" | "follow" => RedirectMode::Follow,
        "error" => RedirectMode::Error,
        "manual" => RedirectMode::Manual,
        other => return Err(format!("'{other}' is not a valid redirect mode")),
    };
    let referrer_policy = if init.referrer.is_empty() {
        ReferrerPolicy::NoReferrer
    } else if init.referrer_policy.is_empty() {
        ReferrerPolicy::default()
    } else {
        ReferrerPolicy::parse(&init.referrer_policy)
            .ok_or_else(|| format!("'{}' is not a valid referrer policy", init.referrer_policy))?
    };
    // A referrer URL replaces the document as the referrer source only when it is
    // same-origin with the document; anything else falls back to the client.
    let referrer_source = match init.referrer.as_str() {
        "" | "about:client" => None,
        other => Url::parse(other)
            .ok()
            .filter(|u| origin.as_ref().is_some_and(|o| same_origin(o, u))),
    };
    let priority = match init.priority.as_str() {
        "high" => RequestPriority::High,
        "low" => RequestPriority::Low,
        _ => ResourceType::Fetch.default_priority(),
    };

    let mut request = ResourceRequest::new(url, ResourceType::Fetch);
    request.method = method;
    request.headers = headers;
    let mut upload = None;
    request.body = if init.body_stream {
        let (sender, body) = StreamingBody::channel();
        upload = Some(sender);
        RequestBody::Stream(body)
    } else {
        init.body
            .clone()
            .map_or(RequestBody::Empty, RequestBody::Bytes)
    };
    request.initiator = "fetch".into();
    request.document_url = origin.as_ref().and_then(|_| Url::parse(document_url).ok());
    request.referrer = referrer_source;
    request.cache_mode = cache;
    request.credentials_mode = credentials;
    request.referrer_policy = referrer_policy;
    request.priority = priority;
    request.mode = mode;
    request.redirect_mode = redirect;
    request.unsafe_request = true;
    request.use_cors_preflight = init.body_stream;
    request.stream_body = true;
    // Keepalive requests may outlive their document, so they keep the whole-request deadline
    // (an orphaned request must not hold a scheduler worker indefinitely) and no flow control.
    if !init.keepalive {
        // Script owns cancellation (AbortSignal); the body is paced by script reads unless
        // it has to be buffered whole for the integrity check.
        request.request_timeout_ms = None;
        if integrity.is_none() {
            request.flow = Some(FlowControl::new(FETCH_FLOW_WINDOW));
        }
    }
    Ok(FetchPlan::Network {
        request: Box::new(request),
        policy: FetchPolicy {
            mode,
            origin,
            redirect,
            keepalive: init.keepalive,
            integrity,
            credentials,
        },
        upload,
    })
}

/// The response as script may see it (Fetch "filtered response"). `Err` means the CORS
/// check failed: script gets a network error and nothing of the response.
pub fn filter_response(
    resp: &ResourceResponse,
    policy: &FetchPolicy,
) -> Result<FetchResponseHead, String> {
    if policy.redirect == RedirectMode::Manual && is_redirect_status(resp.status) {
        return Ok(opaque("opaqueredirect"));
    }
    let cross_origin = |u: &Url| !policy.origin.as_ref().is_some_and(|o| same_origin(o, u));
    let crossed =
        cross_origin(&resp.url) || resp.redirect_chain.iter().any(|r| cross_origin(&r.to));
    if policy.mode == RequestMode::NoCors && crossed {
        return Ok(opaque("opaque"));
    }
    let cors = policy.mode == RequestMode::Cors && crossed;
    if cors {
        // The network service already checked every hop; this repeats the check on the
        // final response so nothing reaches script if a response bypassed the service.
        let tainted = origin_tainted(policy.origin.as_ref(), &resp.redirect_chain);
        let origin = serialize_origin(policy.origin.as_ref(), tainted);
        let credentials = policy.credentials == CredentialsMode::Include;
        cors_check(&resp.headers, &origin, credentials)
            .map_err(|why| format!("CORS: {}: {why}", resp.url.origin()))?;
    }
    let mut url = resp.url.clone();
    url.fragment = None;
    let exposed = if cors {
        Some(exposed_headers(resp, policy))
    } else {
        None
    };
    let headers = resp
        .headers
        .iter()
        .filter(|(name, _)| !matches!(*name, "set-cookie" | "set-cookie2"))
        .filter(|(name, _)| {
            exposed.as_ref().is_none_or(|e| {
                CORS_SAFELISTED_RESPONSE_HEADERS.contains(name) || e.contains(&name.to_string())
            })
        })
        .flat_map(|(name, values)| values.iter().map(move |v| (name.to_string(), v.clone())))
        .collect();
    Ok(FetchResponseHead {
        response_type: if cors { "cors" } else { "basic" },
        status: resp.status,
        status_text: resp.status_text.clone(),
        url: url.as_str(),
        headers,
        redirected: !resp.redirect_chain.is_empty(),
    })
}

/// Lowercased names listed in `Access-Control-Expose-Headers`; `*` exposes every header
/// when credentials are not included.
fn exposed_headers(resp: &ResourceResponse, policy: &FetchPolicy) -> Vec<String> {
    let listed: Vec<String> = resp
        .headers
        .get_all("access-control-expose-headers")
        .iter()
        .flat_map(|v| v.split(','))
        .map(|n| n.trim().to_ascii_lowercase())
        .filter(|n| !n.is_empty())
        .collect();
    if listed.iter().any(|n| n == "*") && policy.credentials != CredentialsMode::Include {
        return resp.headers.iter().map(|(n, _)| n.to_string()).collect();
    }
    listed
}

fn opaque(response_type: &'static str) -> FetchResponseHead {
    FetchResponseHead {
        response_type,
        status: 0,
        status_text: String::new(),
        url: String::new(),
        headers: Vec::new(),
        redirected: false,
    }
}

/// Head for a `data:` URL response.
pub fn data_response(url: &str, mime: &str) -> FetchResponseHead {
    FetchResponseHead {
        response_type: "basic",
        status: 200,
        status_text: "OK".into(),
        url: url.to_string(),
        headers: vec![("content-type".into(), mime.to_string())],
        redirected: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init(url: &str) -> FetchInit {
        FetchInit {
            method: "GET".into(),
            url: url.into(),
            referrer: "about:client".into(),
            ..FetchInit::default()
        }
    }

    fn network(plan: FetchPlan) -> (ResourceRequest, FetchPolicy) {
        match plan {
            FetchPlan::Network {
                request, policy, ..
            } => (*request, policy),
            FetchPlan::Data { .. } => panic!("expected a network plan"),
        }
    }

    const DOC: &str = "http://app.test/dir/page.html";

    #[test]
    fn resolves_relative_urls_against_the_document() {
        assert_eq!(
            resolve_url(DOC, "api?x=1").as_deref(),
            Some("http://app.test/dir/api?x=1")
        );
        assert_eq!(
            resolve_url(DOC, "/root").as_deref(),
            Some("http://app.test/root")
        );
        assert_eq!(
            resolve_url(DOC, "file:///etc/passwd").as_deref(),
            Some("file:///etc/passwd")
        );
    }

    #[test]
    fn only_http_and_data_schemes_are_fetchable() {
        for url in [
            "file:///etc/passwd",
            "axiom://network",
            "javascript:alert(1)",
            "ftp://x/",
        ] {
            let err = plan_fetch(&init(url), DOC).unwrap_err();
            assert!(err.contains("cannot load"), "{url}: {err}");
        }
        assert!(matches!(
            plan_fetch(&init("data:text/plain,hi"), DOC).unwrap(),
            FetchPlan::Data { .. }
        ));
    }

    #[test]
    fn cross_origin_cors_requests_are_left_to_the_network_service_to_preflight() {
        let (req, policy) = network(plan_fetch(&init("http://other.test/"), DOC).unwrap());
        assert_eq!(req.headers.get("origin"), Some("http://app.test"));
        assert_eq!(policy.mode, RequestMode::Cors);
        assert_eq!(req.credentials_mode, CredentialsMode::SameOrigin);
        assert!(req.unsafe_request);
        assert!(!req.use_cors_preflight);

        let mut json = init("http://other.test/api");
        json.method = "PUT".into();
        json.body = Some(b"{}".to_vec());
        json.headers = vec![
            ("Content-Type".into(), "application/json".into()),
            ("X-Custom".into(), "1".into()),
        ];
        let (req, _) = network(plan_fetch(&json, DOC).unwrap());
        assert_eq!(req.method, HttpMethod::Put);
        assert_eq!(req.headers.get("x-custom"), Some("1"));

        let mut stream = init("http://other.test/upload");
        stream.method = "POST".into();
        stream.body_stream = true;
        let (req, _) = network(plan_fetch(&stream, DOC).unwrap());
        assert!(
            req.use_cors_preflight,
            "streamed bodies are always preflighted"
        );
    }

    #[test]
    fn same_origin_mode_rejects_and_no_cors_limits_methods() {
        let mut i = init("http://other.test/");
        i.mode = "same-origin".into();
        assert!(plan_fetch(&i, DOC).is_err());
        i.mode = "no-cors".into();
        assert!(plan_fetch(&i, DOC).is_ok());
        i.method = "PUT".into();
        assert!(plan_fetch(&i, DOC).unwrap_err().contains("no-cors"));
    }

    #[test]
    fn opaque_origin_documents_send_origin_null_and_no_cookies() {
        let (req, policy) =
            network(plan_fetch(&init("http://app.test/"), "file:///C:/page.html").unwrap());
        assert_eq!(req.headers.get("origin"), Some("null"));
        assert_eq!(req.credentials_mode, CredentialsMode::Omit);
        assert!(policy.origin.is_none());
        let mut i = init("http://app.test/");
        i.mode = "no-cors".into();
        let (req, _) = network(plan_fetch(&i, "file:///C:/page.html").unwrap());
        assert_eq!(req.credentials_mode, CredentialsMode::Omit);
        assert!(req.document_url.is_none());
    }

    #[test]
    fn forbidden_headers_are_dropped_and_invalid_ones_rejected() {
        let mut i = init("http://app.test/api");
        i.headers = vec![
            ("Cookie".into(), "a=b".into()),
            ("Host".into(), "evil".into()),
            ("Sec-Fetch-Site".into(), "none".into()),
            ("Proxy-Authorization".into(), "x".into()),
            ("X-HTTP-Method-Override".into(), "TRACE".into()),
            ("X-Custom".into(), "1".into()),
            ("x-custom".into(), "2".into()),
        ];
        let (req, _) = network(plan_fetch(&i, DOC).unwrap());
        assert_eq!(req.headers.get("x-custom"), Some("1, 2"));
        for h in [
            "cookie",
            "host",
            "sec-fetch-site",
            "proxy-authorization",
            "x-http-method-override",
        ] {
            assert!(!req.headers.contains(h), "{h} leaked");
        }
        i.headers = vec![("Bad Name".into(), "v".into())];
        assert!(plan_fetch(&i, DOC).is_err());
        i.headers = vec![("X-Ok".into(), "a\r\nInjected: 1".into())];
        assert!(plan_fetch(&i, DOC).is_err());
    }

    #[test]
    fn methods_and_bodies_are_validated() {
        let mut i = init("http://app.test/");
        i.method = "TRACE".into();
        assert!(plan_fetch(&i, DOC).is_err());
        i.method = "GET".into();
        i.body = Some(b"x".to_vec());
        assert!(plan_fetch(&i, DOC).is_err());
        i.method = "post".into();
        let (req, _) = network(plan_fetch(&i, DOC).unwrap());
        assert_eq!(req.method, HttpMethod::Post);
        assert_eq!(req.headers.get("origin"), Some("http://app.test"));
    }

    #[test]
    fn mixed_content_is_blocked_but_loopback_is_trustworthy() {
        let doc = "https://secure.test/";
        let mut i = init("http://insecure.test/");
        i.mode = "no-cors".into();
        assert!(plan_fetch(&i, doc).unwrap_err().contains("Mixed content"));
        i.url = "http://127.0.0.1:8080/".into();
        assert!(plan_fetch(&i, doc).is_ok());
    }

    #[test]
    fn integrity_and_only_if_cached_rules() {
        let mut i = init("http://app.test/");
        i.integrity = "sha256-abc".into();
        let (req, policy) = network(plan_fetch(&i, DOC).unwrap());
        assert!(policy.integrity.is_some());
        assert!(
            req.flow.is_none(),
            "integrity bodies are buffered, not paced"
        );
        i.integrity = "md5-abc".into();
        let (req, policy) = network(plan_fetch(&i, DOC).unwrap());
        assert!(policy.integrity.is_none());
        assert!(req.flow.is_some());
        assert!(req.request_timeout_ms.is_none());
        let mut i = init("http://app.test/");
        i.cache = "only-if-cached".into();
        assert!(plan_fetch(&i, DOC).is_err());
        i.mode = "same-origin".into();
        let (req, _) = network(plan_fetch(&i, DOC).unwrap());
        assert_eq!(req.cache_mode, CacheMode::OnlyIfCached);
    }

    #[test]
    fn referrer_urls_are_used_only_when_same_origin() {
        let mut i = init("http://app.test/api");
        i.referrer = "http://app.test/other/page?q=1".into();
        let (req, _) = network(plan_fetch(&i, DOC).unwrap());
        assert_eq!(
            req.referrer.map(|u| u.as_str()).as_deref(),
            Some("http://app.test/other/page?q=1")
        );
        i.referrer = "http://evil.test/".into();
        let (req, _) = network(plan_fetch(&i, DOC).unwrap());
        assert!(
            req.referrer.is_none(),
            "cross-origin referrer falls back to the client"
        );
        assert_eq!(
            resolve_referrer(DOC, "http://app.test/x"),
            "http://app.test/x"
        );
        assert_eq!(resolve_referrer(DOC, "http://evil.test/"), "about:client");
        assert_eq!(
            resolve_referrer("file:///C:/p.html", "http://app.test/"),
            "about:client"
        );
    }

    #[test]
    fn streaming_bodies_and_keepalive_limits() {
        let mut i = init("http://app.test/upload");
        i.method = "POST".into();
        i.body_stream = true;
        match plan_fetch(&i, DOC).unwrap() {
            FetchPlan::Network {
                request, upload, ..
            } => {
                assert!(upload.is_some());
                assert!(matches!(request.body, RequestBody::Stream(_)));
            }
            FetchPlan::Data { .. } => panic!("expected a network plan"),
        }
        i.keepalive = true;
        assert!(plan_fetch(&i, DOC).unwrap_err().contains("keepalive"));
        i.keepalive = false;
        i.mode = "no-cors".into();
        assert!(plan_fetch(&i, DOC).unwrap_err().contains("ReadableStream"));

        let mut i = init("http://app.test/beacon");
        i.method = "POST".into();
        i.keepalive = true;
        i.body = Some(vec![0; KEEPALIVE_QUOTA as usize + 1]);
        assert!(plan_fetch(&i, DOC).unwrap_err().contains("quota"));
        i.body = Some(vec![0; 16]);
        let (req, policy) = network(plan_fetch(&i, DOC).unwrap());
        assert!(policy.keepalive);
        assert!(req.flow.is_none());
        assert!(req.request_timeout_ms.is_some());
    }

    #[test]
    fn response_filter_hides_set_cookie_and_makes_opaque_responses() {
        let mut headers = HeaderMap::new();
        headers.append("Set-Cookie", "s=1");
        headers.append("Content-Type", "text/plain");
        let resp = ResourceResponse {
            id: RequestId::new(),
            resource_type: ResourceType::Fetch,
            url: Url::parse("http://app.test/x#frag").unwrap(),
            status: 200,
            status_text: "OK".into(),
            headers,
            mime: None,
            body: Vec::new(),
            cache_state: axiom_loader::CacheState::Miss,
            protocol: axiom_loader::HttpProtocol::Http11,
            tls: None,
            transferred_bytes: 0,
            decoded_bytes: 0,
            redirect_chain: Vec::new(),
            download: None,
            timing: Default::default(),
        };
        let policy = FetchPolicy {
            mode: RequestMode::Cors,
            origin: document_origin(DOC),
            redirect: RedirectMode::Follow,
            keepalive: false,
            integrity: None,
            credentials: CredentialsMode::SameOrigin,
        };
        let head = filter_response(&resp, &policy).unwrap();
        assert_eq!(head.response_type, "basic");
        assert_eq!(head.url, "http://app.test/x");
        assert!(head.headers.iter().all(|(n, _)| n != "set-cookie"));

        let mut cross = resp.clone();
        cross.url = Url::parse("http://other.test/").unwrap();
        let no_cors = FetchPolicy {
            mode: RequestMode::NoCors,
            ..policy.clone()
        };
        let head = filter_response(&cross, &no_cors).unwrap();
        assert_eq!((head.response_type, head.status), ("opaque", 0));
        assert!(head.headers.is_empty());

        let mut redirect = resp;
        redirect.status = 302;
        let manual = FetchPolicy {
            redirect: RedirectMode::Manual,
            ..policy
        };
        assert_eq!(
            filter_response(&redirect, &manual).unwrap().response_type,
            "opaqueredirect"
        );
    }

    fn cross_origin_response(headers: &[(&str, &str)]) -> ResourceResponse {
        let mut map = HeaderMap::new();
        for (n, v) in headers {
            map.append(n, *v);
        }
        ResourceResponse {
            id: RequestId::new(),
            resource_type: ResourceType::Fetch,
            url: Url::parse("http://api.test/data").unwrap(),
            status: 200,
            status_text: "OK".into(),
            headers: map,
            mime: None,
            body: Vec::new(),
            cache_state: axiom_loader::CacheState::Miss,
            protocol: axiom_loader::HttpProtocol::Http11,
            tls: None,
            transferred_bytes: 0,
            decoded_bytes: 0,
            redirect_chain: Vec::new(),
            download: None,
            timing: Default::default(),
        }
    }

    #[test]
    fn cors_check_requires_matching_allow_origin_and_filters_headers() {
        let policy = FetchPolicy {
            mode: RequestMode::Cors,
            origin: document_origin(DOC),
            redirect: RedirectMode::Follow,
            keepalive: false,
            integrity: None,
            credentials: CredentialsMode::SameOrigin,
        };
        assert!(filter_response(&cross_origin_response(&[]), &policy).is_err());
        let wrong = cross_origin_response(&[("Access-Control-Allow-Origin", "http://evil.test")]);
        assert!(filter_response(&wrong, &policy).is_err());
        let doubled = cross_origin_response(&[
            ("Access-Control-Allow-Origin", "*"),
            ("Access-Control-Allow-Origin", "*"),
        ]);
        assert!(filter_response(&doubled, &policy).is_err());

        let ok = cross_origin_response(&[
            ("Access-Control-Allow-Origin", "http://app.test"),
            ("Access-Control-Expose-Headers", "X-Total"),
            ("Content-Type", "application/json"),
            ("X-Total", "3"),
            ("X-Secret", "s"),
            ("Set-Cookie", "a=1"),
        ]);
        let head = filter_response(&ok, &policy).unwrap();
        assert_eq!(head.response_type, "cors");
        let names: Vec<&str> = head.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"content-type"));
        assert!(names.contains(&"x-total"));
        assert!(!names.contains(&"x-secret"));
        assert!(!names.contains(&"set-cookie"));
        assert!(!names.contains(&"access-control-allow-origin"));

        let wildcard = cross_origin_response(&[("Access-Control-Allow-Origin", "*")]);
        assert!(filter_response(&wildcard, &policy).is_ok());
        let with_credentials = FetchPolicy {
            credentials: CredentialsMode::Include,
            ..policy.clone()
        };
        assert!(filter_response(&wildcard, &with_credentials).is_err());
        let exact = cross_origin_response(&[("Access-Control-Allow-Origin", "http://app.test")]);
        assert!(filter_response(&exact, &with_credentials).is_err());
        let allowed = cross_origin_response(&[
            ("Access-Control-Allow-Origin", "http://app.test"),
            ("Access-Control-Allow-Credentials", "true"),
        ]);
        assert!(filter_response(&allowed, &with_credentials).is_ok());
    }

    #[test]
    fn after_a_tainting_redirect_the_cors_check_expects_origin_null() {
        let policy = FetchPolicy {
            mode: RequestMode::Cors,
            origin: document_origin(DOC),
            redirect: RedirectMode::Follow,
            keepalive: false,
            integrity: None,
            credentials: CredentialsMode::SameOrigin,
        };
        let hop = |allow: &str| {
            let mut resp = cross_origin_response(&[("Access-Control-Allow-Origin", allow)]);
            resp.redirect_chain = vec![axiom_loader::RedirectRecord {
                from: Url::parse("http://other.test/start").unwrap(),
                to: resp.url.clone(),
                status: 302,
                method_in: HttpMethod::Get,
                method_out: HttpMethod::Get,
                cross_origin: true,
            }];
            resp
        };
        assert!(filter_response(&hop("http://app.test"), &policy).is_err());
        assert_eq!(
            filter_response(&hop("null"), &policy)
                .unwrap()
                .response_type,
            "cors"
        );
    }
}
