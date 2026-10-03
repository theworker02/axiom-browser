//! Source lists (CSP3 §2.3.1) and URL matching (§6.7.2).

use crate::hash::{is_base64_value, normalize_base64, HashAlgorithm};
use crate::url::{default_port, CspUrl};
use axiom_url::Origin;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PortPattern {
    Any,
    Exact(u16),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Keyword {
    SelfOrigin,
    UnsafeInline,
    UnsafeEval,
    UnsafeHashes,
    StrictDynamic,
    ReportSample,
    WasmUnsafeEval,
    UnsafeAllowRedirects,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceExpression {
    Star,
    Scheme(String),
    Host {
        scheme: Option<String>,
        /// Lowercase; `*` for any host, or starting with `*.` for subdomains.
        host: String,
        port: Option<PortPattern>,
        path: Option<String>,
    },
    Keyword(Keyword),
    Nonce(String),
    /// The digest as normalized base64.
    Hash(HashAlgorithm, String),
}

/// What `'self'` and scheme-less expressions are resolved against.
#[derive(Debug, Clone)]
pub(crate) struct SelfContext {
    pub origin: Origin,
    /// The document URL's scheme, which is known even when the origin is opaque.
    pub scheme: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceList {
    expressions: Vec<SourceExpression>,
}

impl SourceList {
    pub(crate) fn parse<'a>(tokens: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            expressions: tokens.into_iter().filter_map(parse_expression).collect(),
        }
    }

    pub(crate) fn has_keyword(&self, keyword: Keyword) -> bool {
        self.expressions
            .iter()
            .any(|e| *e == SourceExpression::Keyword(keyword.clone()))
    }

    pub(crate) fn has_nonce_or_hash(&self) -> bool {
        self.expressions
            .iter()
            .any(|e| matches!(e, SourceExpression::Nonce(_) | SourceExpression::Hash(..)))
    }

    pub(crate) fn matches_nonce(&self, nonce: Option<&str>) -> bool {
        let Some(nonce) = nonce.filter(|n| !n.is_empty()) else {
            return false;
        };
        self.expressions
            .iter()
            .any(|e| matches!(e, SourceExpression::Nonce(n) if n == nonce))
    }

    pub(crate) fn matches_hash(&self, source: &str) -> bool {
        let mut computed: Vec<(HashAlgorithm, String)> = Vec::new();
        self.expressions.iter().any(|e| {
            let SourceExpression::Hash(alg, expected) = e else {
                return false;
            };
            let actual = match computed.iter().find(|(a, _)| a == alg) {
                Some((_, d)) => d.clone(),
                None => {
                    let d = alg.digest_base64(source.as_bytes());
                    computed.push((*alg, d.clone()));
                    d
                }
            };
            actual == *expected
        })
    }

    /// §6.7.2.5 "Does url match source list in origin with redirect count".
    pub(crate) fn matches_url(&self, url: &CspUrl, ctx: &SelfContext, redirected: bool) -> bool {
        self.expressions
            .iter()
            .any(|e| matches_expression(e, url, ctx, redirected))
    }
}

fn parse_expression(token: &str) -> Option<SourceExpression> {
    let lower = token.to_ascii_lowercase();
    if token == "*" {
        return Some(SourceExpression::Star);
    }
    if lower.starts_with('\'') && lower.ends_with('\'') && lower.len() >= 2 {
        let inner = &token[1..token.len() - 1];
        let keyword = match inner.to_ascii_lowercase().as_str() {
            "self" => Some(Keyword::SelfOrigin),
            "unsafe-inline" => Some(Keyword::UnsafeInline),
            "unsafe-eval" => Some(Keyword::UnsafeEval),
            "unsafe-hashes" => Some(Keyword::UnsafeHashes),
            "strict-dynamic" => Some(Keyword::StrictDynamic),
            "report-sample" => Some(Keyword::ReportSample),
            "wasm-unsafe-eval" => Some(Keyword::WasmUnsafeEval),
            "unsafe-allow-redirects" => Some(Keyword::UnsafeAllowRedirects),
            "none" => Some(Keyword::None),
            _ => None,
        };
        if let Some(k) = keyword {
            return Some(SourceExpression::Keyword(k));
        }
        let (prefix, value) = inner.split_once('-')?;
        if !is_base64_value(value) {
            return None;
        }
        if prefix.eq_ignore_ascii_case("nonce") {
            return Some(SourceExpression::Nonce(value.to_string()));
        }
        return HashAlgorithm::parse(prefix)
            .map(|a| SourceExpression::Hash(a, normalize_base64(value)));
    }
    if let Some(scheme) = lower.strip_suffix(':') {
        return is_scheme(scheme).then(|| SourceExpression::Scheme(scheme.to_string()));
    }
    parse_host_source(&lower)
}

fn is_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
}

/// `[ scheme-part "://" ] host-part [ ":" port-part ] [ path-part ]`
fn parse_host_source(s: &str) -> Option<SourceExpression> {
    let (scheme, rest) = match s.split_once("://") {
        Some((scheme, rest)) => {
            if !is_scheme(scheme) {
                return None;
            }
            (Some(scheme.to_string()), rest)
        }
        None => (None, s),
    };
    let host_end = rest.find([':', '/']).unwrap_or(rest.len());
    let host = rest[..host_end].trim_end_matches('.');
    if !valid_host_pattern(host) {
        return None;
    }
    let mut rest = &rest[host_end..];
    let port = if let Some(after) = rest.strip_prefix(':') {
        let end = after.find('/').unwrap_or(after.len());
        let port = match &after[..end] {
            "*" => PortPattern::Any,
            digits if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => {
                PortPattern::Exact(digits.parse().ok()?)
            }
            _ => return None,
        };
        rest = &after[end..];
        Some(port)
    } else {
        None
    };
    let path = if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    };
    Some(SourceExpression::Host {
        scheme,
        host: host.to_string(),
        port,
        path,
    })
}

fn valid_host_pattern(host: &str) -> bool {
    if host == "*" {
        return true;
    }
    let labels = host.strip_prefix("*.").unwrap_or(host);
    !labels.is_empty()
        && labels.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// §6.7.2.9 "scheme-part matching": `a` also matches the secure upgrade of itself.
fn scheme_part_matches(expr: &str, url: &str) -> bool {
    expr == url
        || (expr == "http" && url == "https")
        || (expr == "ws" && matches!(url, "wss" | "http" | "https"))
        || (expr == "wss" && url == "https")
}

fn matches_expression(
    expr: &SourceExpression,
    url: &CspUrl,
    ctx: &SelfContext,
    redirected: bool,
) -> bool {
    match expr {
        SourceExpression::Star => {
            matches!(url.scheme.as_str(), "http" | "https" | "ws" | "wss")
                || url.scheme == ctx.scheme
        }
        SourceExpression::Scheme(scheme) => scheme_part_matches(scheme, &url.scheme),
        SourceExpression::Host {
            scheme,
            host,
            port,
            path,
        } => {
            let Some(url_host) = &url.host else {
                return false;
            };
            let scheme_ok = match scheme {
                Some(s) => scheme_part_matches(s, &url.scheme),
                None => scheme_part_matches(&ctx.scheme, &url.scheme),
            };
            scheme_ok
                && host_matches(host, url_host)
                && port_matches(port.as_ref(), url)
                && (redirected || path_matches(path.as_deref(), &url.path))
        }
        SourceExpression::Keyword(Keyword::SelfOrigin) => self_matches(&ctx.origin, url),
        SourceExpression::Keyword(_) | SourceExpression::Nonce(_) | SourceExpression::Hash(..) => {
            false
        }
    }
}

fn host_matches(pattern: &str, host: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let host = host.trim_end_matches('.');
    match pattern.strip_prefix('*') {
        // `*.example.com` matches subdomains of example.com, not example.com itself.
        Some(suffix) => host.len() > suffix.len() && host.ends_with(suffix),
        None => host == pattern,
    }
}

/// §6.7.2.10 "port-part matching". An explicit `:80` also admits the default port of the
/// upgraded `https` / `wss` URL.
fn port_matches(pattern: Option<&PortPattern>, url: &CspUrl) -> bool {
    match pattern {
        Some(PortPattern::Any) => true,
        None => url.port.is_none() || url.port == default_port(&url.scheme),
        Some(PortPattern::Exact(p)) => {
            let url_port = url.effective_port();
            Some(*p) == url_port
                || (*p == 80
                    && url_port == Some(443)
                    && matches!(url.scheme.as_str(), "https" | "wss"))
        }
    }
}

/// §6.7.2.11 "path-part matching": a trailing `/` matches the whole subtree; otherwise the
/// decoded paths must be equal.
fn path_matches(pattern: Option<&str>, url_path: &str) -> bool {
    let Some(pattern) = pattern else {
        return true;
    };
    if pattern.is_empty() || pattern == "/" {
        return true;
    }
    let pattern = percent_decode(pattern);
    let path = percent_decode(url_path);
    if pattern.ends_with('/') {
        path.starts_with(&pattern)
    } else {
        path == pattern
    }
}

fn self_matches(origin: &Origin, url: &CspUrl) -> bool {
    if origin.is_opaque() {
        return false;
    }
    let Some(host) = &url.host else {
        return false;
    };
    if *host != origin.host {
        return false;
    }
    let scheme_ok = url.scheme == origin.scheme
        || (origin.scheme == "http" && matches!(url.scheme.as_str(), "https" | "ws" | "wss"))
        || (origin.scheme == "https" && url.scheme == "wss");
    let origin_default = default_port(&origin.scheme) == Some(origin.port);
    let url_default = url.port.is_none() || url.port == default_port(&url.scheme);
    let port_ok = url.effective_port() == Some(origin.port) || (origin_default && url_default);
    scheme_ok && port_ok
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(url: &str) -> SelfContext {
        SelfContext {
            origin: Origin::of_document(url),
            scheme: CspUrl::parse(url).unwrap().scheme,
        }
    }

    fn list(s: &str) -> SourceList {
        SourceList::parse(s.split_ascii_whitespace())
    }

    fn allows(list_src: &str, doc: &str, url: &str) -> bool {
        list(list_src).matches_url(&CspUrl::parse(url).unwrap(), &ctx(doc), false)
    }

    const DOC: &str = "http://example.com/page";

    #[test]
    fn star_matches_network_schemes_and_the_documents_own() {
        assert!(allows("*", DOC, "https://any.test/x"));
        assert!(allows("*", DOC, "wss://any.test/x"));
        assert!(!allows("*", DOC, "data:text/plain,hi"));
        assert!(!allows("*", DOC, "blob:http://example.com/1"));
    }

    #[test]
    fn scheme_sources_allow_secure_upgrades_only() {
        assert!(allows("https:", DOC, "https://a.test/"));
        assert!(!allows("https:", DOC, "http://a.test/"));
        assert!(allows("http:", DOC, "https://a.test/"));
        assert!(allows("data:", DOC, "data:image/png,x"));
        assert!(allows("ws:", DOC, "wss://a.test/"));
        assert!(allows("wss:", DOC, "https://a.test/"));
        assert!(!allows("wss:", DOC, "ws://a.test/"));
    }

    #[test]
    fn host_sources_with_wildcards_ports_and_paths() {
        assert!(allows("cdn.test", DOC, "http://cdn.test/a.js"));
        assert!(allows("cdn.test", DOC, "https://cdn.test/a.js"));
        assert!(!allows("cdn.test", DOC, "http://evil.test/a.js"));
        assert!(!allows("cdn.test", DOC, "http://cdn.test:8080/a.js"));
        assert!(allows("cdn.test:8080", DOC, "http://cdn.test:8080/a.js"));
        assert!(allows("cdn.test:*", DOC, "http://cdn.test:9999/a.js"));
        assert!(allows("*.cdn.test", DOC, "http://a.b.cdn.test/x"));
        assert!(!allows("*.cdn.test", DOC, "http://cdn.test/x"));
        assert!(allows("https://*", DOC, "https://whatever.test/"));
        assert!(!allows("https://cdn.test", DOC, "http://cdn.test/"));
        assert!(allows("cdn.test/js/", DOC, "http://cdn.test/js/a.js"));
        assert!(!allows("cdn.test/js/", DOC, "http://cdn.test/css/a.css"));
        assert!(allows("cdn.test/js/a.js", DOC, "http://cdn.test/js/a.js"));
        assert!(!allows("cdn.test/js/a.js", DOC, "http://cdn.test/js/a.jsx"));
        assert!(allows("cdn.test/a%20b.js", DOC, "http://cdn.test/a b.js"));
        assert!(!allows("cdn.test", DOC, "data:text/plain,x"));
        // An https document does not match plain-http scheme-less sources.
        assert!(!allows(
            "cdn.test",
            "https://example.com/",
            "http://cdn.test/"
        ));
        // Port 80 in the expression also admits the upgraded https default port.
        assert!(allows("http://cdn.test:80", DOC, "https://cdn.test/"));
    }

    #[test]
    fn paths_are_ignored_after_a_redirect() {
        let l = list("cdn.test/only/this.js");
        let url = CspUrl::parse("http://cdn.test/other.js").unwrap();
        assert!(!l.matches_url(&url, &ctx(DOC), false));
        assert!(l.matches_url(&url, &ctx(DOC), true));
    }

    #[test]
    fn self_matches_the_origin_and_its_secure_upgrade() {
        assert!(allows("'self'", DOC, "http://example.com/a"));
        assert!(allows("'SELF'", DOC, "https://example.com/a"));
        assert!(allows("'self'", DOC, "wss://example.com/socket"));
        assert!(!allows("'self'", DOC, "http://example.com:8080/a"));
        assert!(!allows("'self'", DOC, "http://sub.example.com/a"));
        assert!(!allows(
            "'self'",
            "https://example.com/",
            "http://example.com/a"
        ));
        assert!(allows(
            "'self'",
            "http://127.0.0.1:4000/",
            "http://127.0.0.1:4000/x"
        ));
        assert!(!allows(
            "'self'",
            "http://127.0.0.1:4000/",
            "http://127.0.0.1:4001/x"
        ));
    }

    #[test]
    fn none_and_empty_lists_match_nothing() {
        assert!(!allows("'none'", DOC, "http://example.com/"));
        assert!(!allows("", DOC, "http://example.com/"));
    }

    #[test]
    fn invalid_expressions_are_ignored() {
        let l = list("'nonce-' 'sha1-abc' ht!tp: bad_host.test cdn.test:x 'bogus' 'nonce-a*b'");
        assert!(l.expressions.is_empty(), "{:?}", l.expressions);
    }

    #[test]
    fn nonces_and_hashes() {
        let l = list("'nonce-abc123' 'sha256-bhHHL3z2vDgxUt0W3dWQOrprscmda2Y5pLsLg4GF+pI='");
        assert!(l.matches_nonce(Some("abc123")));
        assert!(!l.matches_nonce(Some("abc")));
        assert!(!l.matches_nonce(Some("")));
        assert!(!l.matches_nonce(None));
        assert!(l.matches_hash("alert(1)"));
        assert!(!l.matches_hash("alert(2)"));
        let url_safe = list("'sha256-bhHHL3z2vDgxUt0W3dWQOrprscmda2Y5pLsLg4GF-pI'");
        assert!(url_safe.matches_hash("alert(1)"));
        assert!(l.has_nonce_or_hash());
        assert!(!list("'self'").has_nonce_or_hash());
    }
}
