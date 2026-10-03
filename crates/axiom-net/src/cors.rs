//! The CORS protocol (Fetch §3.2): request-header safelists, the CORS check, preflight
//! evaluation and the per-profile preflight cache.
//!
//! [`NetworkService`](crate::NetworkService) applies these on every hop of a `cors`
//! request whose URL is cross-origin to the requester; `fetch()` uses the same header
//! rules when it validates script input.

use std::time::{Duration, Instant};

use parking_lot::Mutex;

use axiom_url::Url;

use crate::headers::HeaderMap;
use crate::redirect::{same_origin, RedirectRecord};
use crate::request::HttpMethod;

/// Preflight results are kept at most this long, whatever `Access-Control-Max-Age` says.
pub const MAX_PREFLIGHT_AGE: Duration = Duration::from_secs(7200);
/// Used when a preflight response has no valid `Access-Control-Max-Age`.
pub const DEFAULT_PREFLIGHT_AGE: Duration = Duration::from_secs(5);
/// Entries the preflight cache holds before the oldest are evicted.
pub const PREFLIGHT_CACHE_CAPACITY: usize = 1024;

const FORBIDDEN_REQUEST_HEADERS: &[&str] = &[
    "accept-charset",
    "accept-encoding",
    "access-control-request-headers",
    "access-control-request-method",
    "connection",
    "content-length",
    "cookie",
    "cookie2",
    "date",
    "dnt",
    "expect",
    "host",
    "keep-alive",
    "origin",
    "referer",
    "set-cookie",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "via",
];

/// Fetch "forbidden request-header": script may not set it.
pub fn is_forbidden_request_header(name: &str, value: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if FORBIDDEN_REQUEST_HEADERS.contains(&name.as_str())
        || name.starts_with("proxy-")
        || name.starts_with("sec-")
    {
        return true;
    }
    if matches!(
        name.as_str(),
        "x-http-method" | "x-http-method-override" | "x-method-override"
    ) {
        return value.split(',').any(|m| {
            matches!(
                m.trim().to_ascii_uppercase().as_str(),
                "CONNECT" | "TRACE" | "TRACK"
            )
        });
    }
    false
}

/// Fetch "no-CORS-safelisted request-header": what a `no-cors` request may carry.
pub fn is_no_cors_safelisted_request_header(name: &str, value: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "accept" | "accept-language" | "content-language" | "content-type"
    ) && is_cors_safelisted_request_header(name, value)
}

fn is_cors_unsafe_byte(b: u8) -> bool {
    (b < 0x20 && b != b'\t') || b"\"():<>?@[\\]{}".contains(&b) || b == 0x7f
}

/// Fetch "CORS-safelisted request-header".
pub fn is_cors_safelisted_request_header(name: &str, value: &str) -> bool {
    if value.len() > 128 {
        return false;
    }
    match name.to_ascii_lowercase().as_str() {
        "accept" => !value.bytes().any(is_cors_unsafe_byte),
        "accept-language" | "content-language" => value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b" *,-.;=".contains(&b)),
        "content-type" => {
            if value.bytes().any(is_cors_unsafe_byte) {
                return false;
            }
            let essence = value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            matches!(
                essence.as_str(),
                "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain"
            )
        }
        "range" => is_simple_range(value),
        _ => false,
    }
}

/// `bytes=<start>-` or `bytes=<start>-<end>` with `start <= end`.
fn is_simple_range(value: &str) -> bool {
    let Some(spec) = value.strip_prefix("bytes=") else {
        return false;
    };
    let Some((start, end)) = spec.split_once('-') else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(start) || !(end.is_empty() || digits(end)) {
        return false;
    }
    end.is_empty()
        || match (start.parse::<u64>(), end.parse::<u64>()) {
            (Ok(s), Ok(e)) => s <= e,
            _ => false,
        }
}

/// Fetch "CORS-safelisted method".
pub fn is_cors_safelisted_method(method: &HttpMethod) -> bool {
    matches!(
        method,
        HttpMethod::Get | HttpMethod::Head | HttpMethod::Post
    )
}

/// Fetch "CORS-unsafe request-header names": sorted, lowercase, without duplicates.
/// Forbidden headers are the browser's own and are never listed.
pub fn cors_unsafe_header_names(headers: &HeaderMap) -> Vec<String> {
    // Fetch also caps the safelisted values at 1024 bytes in total; with at most five
    // safelisted names of 128 bytes each, that limit cannot be reached.
    let mut unsafe_names: Vec<String> = headers
        .iter()
        .filter(|(name, values)| {
            let value = values.join(", ");
            !is_forbidden_request_header(name, &value)
                && !is_cors_safelisted_request_header(name, &value)
        })
        .map(|(name, _)| name.to_string())
        .collect();
    unsafe_names.sort();
    unsafe_names.dedup();
    unsafe_names
}

/// Whether a cross-origin redirect in `chain` has made the request's origin opaque
/// (Fetch "tainted origin flag"): a hop moved to another origin while the requester was
/// already cross-origin to the URL being left.
pub fn origin_tainted(origin: Option<&Url>, chain: &[RedirectRecord]) -> bool {
    chain
        .iter()
        .any(|r| !same_origin(&r.from, &r.to) && !origin.is_some_and(|o| same_origin(o, &r.from)))
}

/// Serialized request origin for `Origin` headers and CORS checks.
pub fn serialize_origin(origin: Option<&Url>, tainted: bool) -> String {
    match origin {
        Some(o) if !tainted => o.origin(),
        _ => "null".into(),
    }
}

/// Exactly one value, or `None` (absent or repeated headers fail CORS checks).
fn single<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    match headers.get_all(name) {
        [value] => Some(value.trim()),
        _ => None,
    }
}

/// Fetch "CORS check": the response names the requesting origin (or `*` when no
/// credentials are involved) and, with credentials, allows them explicitly. `Err` is a
/// diagnostic for logs, never shown to web content.
pub fn cors_check(headers: &HeaderMap, origin: &str, credentials: bool) -> Result<(), String> {
    let allow_origin = single(headers, "access-control-allow-origin")
        .ok_or("no single Access-Control-Allow-Origin")?;
    if allow_origin == "*" {
        if credentials {
            return Err("wildcard Access-Control-Allow-Origin with credentials".into());
        }
        return Ok(());
    }
    if allow_origin != origin {
        return Err(format!(
            "Access-Control-Allow-Origin does not match {origin}"
        ));
    }
    if credentials && single(headers, "access-control-allow-credentials") != Some("true") {
        return Err("credentials not allowed by Access-Control-Allow-Credentials".into());
    }
    Ok(())
}

/// What a request needs a preflight to approve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightRequest {
    pub method: String,
    /// [`cors_unsafe_header_names`] of the request.
    pub unsafe_headers: Vec<String>,
    /// Credentials mode `include`.
    pub credentials: bool,
    /// Fetch "use-CORS-preflight flag": approve the method even if it is safelisted.
    pub forced: bool,
}

/// What a successful preflight allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightGrant {
    pub methods: Vec<String>,
    pub headers: Vec<String>,
    pub max_age: Duration,
}

/// Comma-separated tokens; `None` if any item is not a valid token.
fn parse_token_list(headers: &HeaderMap, name: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for value in headers.get_all(name) {
        for item in value.split(',') {
            let item = item.trim_matches([' ', '\t']);
            if item.is_empty() {
                continue;
            }
            let token = item
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b));
            if !token {
                return None;
            }
            out.push(item.to_string());
        }
    }
    Some(out)
}

/// Evaluate a preflight response (Fetch "CORS-preflight fetch", steps 7.1–7.9). `origin`
/// is the serialized request origin. `Err` is a diagnostic for logs.
pub fn evaluate_preflight(
    status: u16,
    headers: &HeaderMap,
    origin: &str,
    req: &PreflightRequest,
) -> Result<PreflightGrant, String> {
    cors_check(headers, origin, req.credentials)?;
    if !(200..300).contains(&status) {
        return Err(format!("preflight status {status}"));
    }
    let mut methods = parse_token_list(headers, "access-control-allow-methods")
        .ok_or("invalid Access-Control-Allow-Methods")?;
    let allowed_headers = parse_token_list(headers, "access-control-allow-headers")
        .ok_or("invalid Access-Control-Allow-Headers")?;
    if methods.is_empty() && req.forced {
        methods.push(req.method.clone());
    }
    let wildcard_methods = !req.credentials && methods.iter().any(|m| m == "*");
    let method_ok = methods.contains(&req.method)
        || (!req.forced && is_cors_safelisted_method_name(&req.method))
        || wildcard_methods;
    if !method_ok {
        return Err(format!(
            "method {} not in Access-Control-Allow-Methods",
            req.method
        ));
    }
    let listed = |name: &str| allowed_headers.iter().any(|h| h.eq_ignore_ascii_case(name));
    let wildcard_headers = !req.credentials && allowed_headers.iter().any(|h| h == "*");
    for name in &req.unsafe_headers {
        // `Authorization` is never covered by the wildcard.
        let ok = listed(name) || (wildcard_headers && name != "authorization");
        if !ok {
            return Err(format!("header {name} not in Access-Control-Allow-Headers"));
        }
    }
    let max_age = single(headers, "access-control-max-age")
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(DEFAULT_PREFLIGHT_AGE, Duration::from_secs)
        .min(MAX_PREFLIGHT_AGE);
    Ok(PreflightGrant {
        methods,
        headers: allowed_headers,
        max_age,
    })
}

fn is_cors_safelisted_method_name(method: &str) -> bool {
    matches!(method, "GET" | "HEAD" | "POST")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Grant {
    Method(String),
    /// Lowercased header name (or `*`).
    Header(String),
}

#[derive(Debug, Clone)]
struct PreflightEntry {
    origin: String,
    url: String,
    credentials: bool,
    grant: Grant,
    expires: Instant,
}

/// Fetch "CORS-preflight cache", one per profile. It lives in memory only (private
/// profiles never write it anywhere) and is cleared with the profile's network state.
#[derive(Debug, Default)]
pub struct PreflightCache {
    entries: Mutex<Vec<PreflightEntry>>,
}

impl PreflightCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether earlier preflights already approve `req` for `origin` and `url`.
    pub fn allows(&self, origin: &str, url: &Url, req: &PreflightRequest) -> bool {
        let now = Instant::now();
        let url = url.as_str();
        let mut entries = self.entries.lock();
        entries.retain(|e| e.expires > now);
        let matching = |grant: &dyn Fn(&Grant) -> bool| {
            entries.iter().any(|e| {
                e.origin == origin
                    && e.url == url
                    && e.credentials == req.credentials
                    && grant(&e.grant)
            })
        };
        let method_ok = (!req.forced && is_cors_safelisted_method_name(&req.method))
            || matching(&|g| match g {
                Grant::Method(m) => *m == req.method || (m == "*" && !req.credentials),
                Grant::Header(_) => false,
            });
        method_ok
            && req.unsafe_headers.iter().all(|name| {
                matching(&|g| match g {
                    Grant::Header(h) => {
                        h == name || (h == "*" && !req.credentials && name != "authorization")
                    }
                    Grant::Method(_) => false,
                })
            })
    }

    /// Record a successful preflight. A zero max-age stores nothing.
    pub fn store(&self, origin: &str, url: &Url, credentials: bool, grant: &PreflightGrant) {
        if grant.max_age.is_zero() {
            return;
        }
        let expires = Instant::now() + grant.max_age;
        let url = url.as_str();
        let mut entries = self.entries.lock();
        let grants = grant
            .methods
            .iter()
            .map(|m| Grant::Method(m.clone()))
            .chain(
                grant
                    .headers
                    .iter()
                    .map(|h| Grant::Header(h.to_ascii_lowercase())),
            );
        for g in grants {
            match entries.iter_mut().find(|e| {
                e.origin == origin && e.url == url && e.credentials == credentials && e.grant == g
            }) {
                Some(e) => e.expires = expires,
                None => entries.push(PreflightEntry {
                    origin: origin.to_string(),
                    url: url.to_string(),
                    credentials,
                    grant: g,
                    expires,
                }),
            }
        }
        if entries.len() > PREFLIGHT_CACHE_CAPACITY {
            let now = Instant::now();
            entries.retain(|e| e.expires > now);
            let excess = entries.len().saturating_sub(PREFLIGHT_CACHE_CAPACITY);
            entries.drain(..excess);
        }
    }

    pub fn clear(&self) {
        self.entries.lock().clear();
    }

    /// Cached entries: one per granted method or header name, per origin, URL and
    /// credentials mode (as in the Fetch standard's CORS-preflight cache).
    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (n, v) in pairs {
            h.append(n, *v);
        }
        h
    }

    fn preflight(method: &str, unsafe_headers: &[&str], credentials: bool) -> PreflightRequest {
        PreflightRequest {
            method: method.into(),
            unsafe_headers: unsafe_headers.iter().map(|s| s.to_string()).collect(),
            credentials,
            forced: false,
        }
    }

    const ORIGIN: &str = "http://app.test";

    #[test]
    fn safelisted_headers_follow_the_fetch_rules() {
        assert!(is_cors_safelisted_request_header("Accept", "text/html"));
        assert!(!is_cors_safelisted_request_header("Accept", "a(b)"));
        assert!(is_cors_safelisted_request_header(
            "Content-Language",
            "en-US"
        ));
        assert!(!is_cors_safelisted_request_header(
            "Content-Language",
            "en_US"
        ));
        assert!(is_cors_safelisted_request_header(
            "content-type",
            "text/plain; charset=utf-8"
        ));
        assert!(!is_cors_safelisted_request_header(
            "content-type",
            "application/json"
        ));
        assert!(is_cors_safelisted_request_header("range", "bytes=0-"));
        assert!(is_cors_safelisted_request_header("range", "bytes=5-10"));
        assert!(!is_cors_safelisted_request_header("range", "bytes=10-5"));
        assert!(!is_cors_safelisted_request_header("range", "bytes=0-1,4-5"));
        assert!(!is_cors_safelisted_request_header(
            "accept",
            &"a".repeat(129)
        ));
        assert!(!is_no_cors_safelisted_request_header("range", "bytes=0-"));
        assert!(is_forbidden_request_header("Sec-Fetch-Mode", "cors"));
        assert!(is_forbidden_request_header(
            "X-HTTP-Method-Override",
            "TRACE"
        ));
        assert!(!is_forbidden_request_header(
            "X-HTTP-Method-Override",
            "PUT"
        ));
    }

    #[test]
    fn unsafe_header_names_are_sorted_and_skip_forbidden_and_safelisted_ones() {
        let h = headers(&[
            ("X-Zeta", "1"),
            ("Content-Type", "application/json"),
            ("Accept", "*/*"),
            ("Origin", "http://app.test"),
            ("x-alpha", "2"),
        ]);
        assert_eq!(
            cors_unsafe_header_names(&h),
            ["content-type", "x-alpha", "x-zeta"]
        );
        // Repeated values are combined before the 128-byte limit applies.
        let mut repeated = HeaderMap::new();
        repeated.append("Accept", "a".repeat(100));
        repeated.append("Accept", "b".repeat(100));
        assert_eq!(cors_unsafe_header_names(&repeated), ["accept"]);
    }

    #[test]
    fn cors_check_matches_origin_and_credentials() {
        assert!(cors_check(&headers(&[]), ORIGIN, false).is_err());
        let star = headers(&[("Access-Control-Allow-Origin", "*")]);
        assert!(cors_check(&star, ORIGIN, false).is_ok());
        assert!(cors_check(&star, ORIGIN, true).is_err());
        let exact = headers(&[("Access-Control-Allow-Origin", ORIGIN)]);
        assert!(cors_check(&exact, ORIGIN, false).is_ok());
        assert!(cors_check(&exact, "null", false).is_err());
        assert!(cors_check(&exact, ORIGIN, true).is_err());
        let creds = headers(&[
            ("Access-Control-Allow-Origin", ORIGIN),
            ("Access-Control-Allow-Credentials", "true"),
        ]);
        assert!(cors_check(&creds, ORIGIN, true).is_ok());
        let doubled = headers(&[
            ("Access-Control-Allow-Origin", "*"),
            ("Access-Control-Allow-Origin", "*"),
        ]);
        assert!(cors_check(&doubled, ORIGIN, false).is_err());
        let null = headers(&[("Access-Control-Allow-Origin", "null")]);
        assert!(cors_check(&null, "null", false).is_ok());
    }

    #[test]
    fn preflight_responses_must_allow_the_method_and_headers() {
        let ok = headers(&[
            ("Access-Control-Allow-Origin", ORIGIN),
            ("Access-Control-Allow-Methods", "PUT, DELETE"),
            ("Access-Control-Allow-Headers", "X-Custom, content-type"),
            ("Access-Control-Max-Age", "600"),
        ]);
        let req = preflight("PUT", &["content-type", "x-custom"], false);
        let grant = evaluate_preflight(200, &ok, ORIGIN, &req).unwrap();
        assert_eq!(grant.max_age, Duration::from_secs(600));
        assert!(evaluate_preflight(204, &ok, ORIGIN, &req).is_ok());
        assert!(evaluate_preflight(404, &ok, ORIGIN, &req).is_err());
        assert!(evaluate_preflight(200, &ok, ORIGIN, &preflight("PATCH", &[], false)).is_err());
        assert!(
            evaluate_preflight(200, &ok, ORIGIN, &preflight("PUT", &["x-other"], false)).is_err()
        );
        // Safelisted methods need no listing; header names are case-insensitive.
        assert!(
            evaluate_preflight(200, &ok, ORIGIN, &preflight("POST", &["x-custom"], false)).is_ok()
        );
        let invalid = headers(&[
            ("Access-Control-Allow-Origin", ORIGIN),
            ("Access-Control-Allow-Methods", "PUT, (bad)"),
        ]);
        assert!(evaluate_preflight(200, &invalid, ORIGIN, &req).is_err());
        let no_origin = headers(&[("Access-Control-Allow-Methods", "PUT")]);
        assert!(
            evaluate_preflight(200, &no_origin, ORIGIN, &preflight("PUT", &[], false)).is_err()
        );
    }

    #[test]
    fn preflight_wildcards_do_not_apply_with_credentials_or_to_authorization() {
        let wild = headers(&[
            ("Access-Control-Allow-Origin", ORIGIN),
            ("Access-Control-Allow-Credentials", "true"),
            ("Access-Control-Allow-Methods", "*"),
            ("Access-Control-Allow-Headers", "*"),
        ]);
        let plain = preflight("DELETE", &["x-a"], false);
        assert!(evaluate_preflight(200, &wild, ORIGIN, &plain).is_ok());
        let auth = preflight("DELETE", &["authorization"], false);
        assert!(evaluate_preflight(200, &wild, ORIGIN, &auth).is_err());
        let creds = preflight("DELETE", &[], true);
        assert!(evaluate_preflight(200, &wild, ORIGIN, &creds).is_err());
        let max = headers(&[
            ("Access-Control-Allow-Origin", "*"),
            ("Access-Control-Allow-Methods", "PUT"),
            ("Access-Control-Max-Age", "999999"),
        ]);
        let grant = evaluate_preflight(200, &max, ORIGIN, &preflight("PUT", &[], false)).unwrap();
        assert_eq!(grant.max_age, MAX_PREFLIGHT_AGE);
        let forced = PreflightRequest {
            forced: true,
            ..preflight("POST", &[], false)
        };
        let none = headers(&[("Access-Control-Allow-Origin", "*")]);
        assert_eq!(
            evaluate_preflight(200, &none, ORIGIN, &forced)
                .unwrap()
                .methods,
            ["POST"]
        );
    }

    #[test]
    fn preflight_cache_matches_origin_url_credentials_and_grants() {
        let cache = PreflightCache::new();
        let url = Url::parse("http://api.test/items").unwrap();
        let req = preflight("PUT", &["x-custom"], false);
        assert!(!cache.allows(ORIGIN, &url, &req));
        let grant = PreflightGrant {
            methods: vec!["PUT".into()],
            headers: vec!["X-Custom".into()],
            max_age: Duration::from_secs(60),
        };
        cache.store(ORIGIN, &url, false, &grant);
        assert!(cache.allows(ORIGIN, &url, &req));
        assert!(!cache.allows("http://evil.test", &url, &req));
        assert!(!cache.allows(ORIGIN, &Url::parse("http://api.test/other").unwrap(), &req));
        assert!(!cache.allows(ORIGIN, &url, &preflight("PUT", &["x-custom"], true)));
        assert!(!cache.allows(ORIGIN, &url, &preflight("DELETE", &[], false)));
        assert!(!cache.allows(ORIGIN, &url, &preflight("PUT", &["x-more"], false)));
        assert!(cache.allows(ORIGIN, &url, &preflight("GET", &["x-custom"], false)));
        cache.clear();
        assert!(cache.is_empty());

        let zero = PreflightGrant {
            max_age: Duration::ZERO,
            ..grant.clone()
        };
        cache.store(ORIGIN, &url, false, &zero);
        assert!(!cache.allows(ORIGIN, &url, &req));

        let wildcard = PreflightGrant {
            methods: vec!["*".into()],
            headers: vec!["*".into()],
            max_age: Duration::from_secs(60),
        };
        cache.store(ORIGIN, &url, false, &wildcard);
        assert!(cache.allows(ORIGIN, &url, &preflight("PATCH", &["x-any"], false)));
        assert!(!cache.allows(ORIGIN, &url, &preflight("PATCH", &["authorization"], false)));
    }

    #[test]
    fn cross_origin_hops_taint_the_origin_only_when_the_requester_was_already_foreign() {
        let u = |s: &str| Url::parse(s).unwrap();
        let hop = |from: &str, to: &str| RedirectRecord {
            from: u(from),
            to: u(to),
            status: 302,
            method_in: HttpMethod::Get,
            method_out: HttpMethod::Get,
            cross_origin: !same_origin(&u(from), &u(to)),
        };
        let origin = u("http://app.test/");
        // app → api: the requester was same-origin with the URL it left.
        assert!(!origin_tainted(
            Some(&origin),
            &[hop("http://app.test/a", "http://api.test/b")]
        ));
        // api → cdn: a foreign server sent the request to a third origin.
        assert!(origin_tainted(
            Some(&origin),
            &[hop("http://api.test/a", "http://cdn.test/b")]
        ));
        // api → api: same-origin hops never taint.
        assert!(!origin_tainted(
            Some(&origin),
            &[hop("http://api.test/a", "http://api.test/b")]
        ));
        assert_eq!(serialize_origin(Some(&origin), false), "http://app.test");
        assert_eq!(serialize_origin(Some(&origin), true), "null");
        assert_eq!(serialize_origin(None, false), "null");
    }
}
