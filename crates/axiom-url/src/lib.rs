//! URL parsing for Axiom.
//!
//! Phase 1 supports `http` and `https` absolute URLs well enough for navigation
//! and relative resolution of common stylesheet/script-free pages.

use std::fmt;

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UrlError {
    #[error("empty URL")]
    Empty,
    #[error("unsupported or missing scheme")]
    BadScheme,
    #[error("missing host")]
    MissingHost,
    #[error("invalid port")]
    BadPort,
    #[error("invalid host")]
    InvalidHost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    pub path: String,
    pub query: Option<String>,
    pub fragment: Option<String>,
}

impl Url {
    pub fn parse(input: &str) -> Result<Self, UrlError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(UrlError::Empty);
        }

        let (scheme, rest) = split_scheme(input)?;
        if scheme != "http" && scheme != "https" && scheme != "axiom" {
            return Err(UrlError::BadScheme);
        }

        // `split_scheme` already consumed `scheme://`, so rest begins at the host.
        let (authority, path_query_frag) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };

        if authority.is_empty() {
            return Err(UrlError::MissingHost);
        }

        let (host, port) = split_host_port(authority)?;
        let host = canonical_host(host)?;

        let (path_query, fragment) = match path_query_frag.split_once('#') {
            Some((pq, frag)) => (pq, Some(frag.to_string())),
            None => (path_query_frag, None),
        };

        let (path, query) = match path_query.split_once('?') {
            Some((p, q)) => (normalize_path(p), Some(q.to_string())),
            None => (normalize_path(path_query), None),
        };

        Ok(Self {
            scheme: scheme.to_string(),
            host,
            port,
            path,
            query,
            fragment,
        })
    }

    pub fn join(&self, relative: &str) -> Result<Self, UrlError> {
        let relative = relative.trim();
        if relative.is_empty() {
            return Ok(self.clone());
        }
        if relative.contains("://") {
            return Self::parse(relative);
        }
        if relative.starts_with("//") {
            return Self::parse(&format!("{}:{}", self.scheme, relative));
        }

        let (path_part, query, fragment) = {
            let (without_frag, fragment) = match relative.split_once('#') {
                Some((a, b)) => (a, Some(b.to_string())),
                None => (relative, None),
            };
            let (path_part, query) = match without_frag.split_once('?') {
                Some((a, b)) => (a, Some(b.to_string())),
                None => (without_frag, None),
            };
            (path_part, query, fragment)
        };

        // RFC 3986 §5.2.2: the base query is only inherited when the reference has no path.
        let (path, query) = if path_part.is_empty() {
            (self.path.clone(), query.or_else(|| self.query.clone()))
        } else if path_part.starts_with('/') {
            (normalize_path(path_part), query)
        } else {
            let base_dir = parent_path(&self.path);
            (normalize_path(&format!("{base_dir}/{path_part}")), query)
        };

        Ok(Self {
            scheme: self.scheme.clone(),
            host: self.host.clone(),
            port: self.port,
            path,
            query,
            fragment,
        })
    }

    pub fn origin_form(&self) -> String {
        match &self.query {
            Some(q) => format!("{}?{q}", self.path),
            None => self.path.clone(),
        }
    }

    pub fn default_port(&self) -> u16 {
        match self.scheme.as_str() {
            "https" => 443,
            "http" => 80,
            _ => 0,
        }
    }

    pub fn effective_port(&self) -> u16 {
        self.port.unwrap_or_else(|| self.default_port())
    }

    /// ASCII serialization of the origin (`scheme://host[:port]`, as in the `Origin` header).
    pub fn origin(&self) -> String {
        match self.port {
            Some(p) if p != self.default_port() => format!("{}://{}:{p}", self.scheme, self.host),
            _ => format!("{}://{}", self.scheme, self.host),
        }
    }

    pub fn as_str(&self) -> String {
        let port = match self.port {
            Some(p) if p != self.default_port() => format!(":{p}"),
            _ => String::new(),
        };
        let mut s = format!("{}://{}{}{}", self.scheme, self.host, port, self.path);
        if let Some(q) = &self.query {
            s.push('?');
            s.push_str(q);
        }
        if let Some(f) = &self.fragment {
            s.push('#');
            s.push_str(f);
        }
        s
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// The canonical security origin: scheme + host + port (tuple origin), or the opaque
/// origin (`scheme == "null"`). Shared by the engine (documents, resources) and the
/// browser layer (storage, permissions).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Origin {
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

impl Origin {
    pub fn from_url(url: &Url) -> Self {
        Self {
            scheme: url.scheme.clone(),
            host: url.host.clone(),
            port: url.effective_port(),
        }
    }

    pub fn try_from_str(s: &str) -> Option<Self> {
        Url::parse(s).ok().map(|u| Self::from_url(&u))
    }

    /// Origin of a document URL: a tuple origin for `http(s)`, opaque otherwise (local
    /// files, data, internal pages).
    pub fn of_document(url: &str) -> Self {
        match Url::parse(url) {
            Ok(u) if u.scheme == "http" || u.scheme == "https" => Self::from_url(&u),
            _ => Self::opaque(),
        }
    }

    pub fn opaque() -> Self {
        Self {
            scheme: "null".into(),
            host: String::new(),
            port: 0,
        }
    }

    pub fn is_opaque(&self) -> bool {
        self.scheme == "null"
    }

    /// Opaque origins are never same-origin with anything (not even themselves).
    pub fn is_same_origin(&self, other: &Origin) -> bool {
        !self.is_opaque()
            && self.scheme == other.scheme
            && self.host == other.host
            && self.port == other.port
    }

    pub fn serialize(&self) -> String {
        if self.is_opaque() {
            return "null".into();
        }
        let default = match self.scheme.as_str() {
            "https" => 443,
            "http" => 80,
            _ => self.port,
        };
        if self.port == default {
            format!("{}://{}", self.scheme, self.host)
        } else {
            format!("{}://{}:{}", self.scheme, self.host, self.port)
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.serialize())
    }
}

fn split_scheme(input: &str) -> Result<(&'static str, &str), UrlError> {
    let (scheme, rest) = input.split_once("://").ok_or(UrlError::BadScheme)?;
    match scheme.to_ascii_lowercase().as_str() {
        "http" => Ok(("http", rest)),
        "https" => Ok(("https", rest)),
        "axiom" => Ok(("axiom", rest)),
        _ => Err(UrlError::BadScheme),
    }
}

fn split_host_port(authority: &str) -> Result<(&str, Option<u16>), UrlError> {
    if authority.starts_with('[') {
        // IPv6 literal, kept with its brackets.
        let end = authority.find(']').ok_or(UrlError::MissingHost)?;
        let host_part = &authority[..=end];
        let after = &authority[end + 1..];
        if let Some(port_str) = after.strip_prefix(':') {
            let port = port_str.parse().map_err(|_| UrlError::BadPort)?;
            return Ok((host_part, Some(port)));
        }
        return Ok((host_part, None));
    }

    match authority.rsplit_once(':') {
        Some((host, port_str))
            if !host.is_empty() && port_str.chars().all(|c| c.is_ascii_digit()) =>
        {
            let port = port_str.parse().map_err(|_| UrlError::BadPort)?;
            Ok((host, Some(port)))
        }
        _ => Ok((authority, None)),
    }
}

/// WHATWG forbidden host code points (plus ASCII whitespace and controls).
fn is_forbidden_host_char(c: char) -> bool {
    c.is_ascii_control()
        || matches!(
            c,
            ' ' | '#' | '%' | '/' | ':' | '<' | '>' | '?' | '@' | '[' | '\\' | ']' | '^' | '|'
        )
}

/// Canonical host: IPv6 literals lowercased, ASCII domains lowercased, internationalized
/// domains converted to their ASCII (punycode) form.
fn canonical_host(host: &str) -> Result<String, UrlError> {
    if host.starts_with('[') {
        let inner = &host[1..host.len() - 1];
        if inner.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(UrlError::InvalidHost);
        }
        return Ok(host.to_ascii_lowercase());
    }
    let host = if host.is_ascii() {
        host.to_ascii_lowercase()
    } else {
        idna::domain_to_ascii(host).map_err(|_| UrlError::InvalidHost)?
    };
    if host.is_empty() {
        return Err(UrlError::MissingHost);
    }
    if host.chars().any(is_forbidden_host_char) {
        return Err(UrlError::InvalidHost);
    }
    Ok(host)
}

fn normalize_path(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    // A directory reference (`dir/`, `a/.`, `a/b/..`) keeps its trailing slash so that
    // later relative references resolve inside it.
    let last = path.rsplit('/').next().unwrap_or("");
    let directory = path.ends_with('/') || last == "." || last == "..";
    if out.is_empty() {
        "/".to_string()
    } else if directory {
        format!("/{}/", out.join("/"))
    } else {
        format!("/{}", out.join("/"))
    }
}

fn parent_path(path: &str) -> String {
    if let Some(i) = path.rfind('/') {
        if i == 0 {
            "/".to_string()
        } else {
            path[..i].to_string()
        }
    } else {
        "/".to_string()
    }
}

/// Decode `%XX` escapes (malformed escapes stay as written); invalid UTF-8 becomes U+FFFD.
pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (bytes[i], bytes.get(i + 1), bytes.get(i + 2)) {
            (b'%', Some(&h), Some(&l)) if hex(h).is_some() && hex(l).is_some() => {
                out.push((hex(h).unwrap() * 16 + hex(l).unwrap()) as u8);
                i += 3;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_keeps_malformed_escapes() {
        assert_eq!(percent_decode("a%20b%zz%e2%9c%93%"), "a b%zz\u{2713}%");
    }

    #[test]
    fn parses_example_com() {
        let u = Url::parse("https://example.com").unwrap();
        assert_eq!(u.scheme, "https");
        assert_eq!(u.host, "example.com");
        assert_eq!(u.path, "/");
        assert_eq!(u.as_str(), "https://example.com/");
    }

    #[test]
    fn parses_axiom_internal() {
        let u = Url::parse("axiom://newtab").unwrap();
        assert_eq!(u.scheme, "axiom");
        assert_eq!(u.host, "newtab");
        assert_eq!(u.path, "/");
        let u2 = Url::parse("axiom://settings").unwrap();
        assert_eq!(u2.host, "settings");
    }

    #[test]
    fn joins_relative() {
        let base = Url::parse("https://example.com/dir/page.html").unwrap();
        let joined = base.join("style.css").unwrap();
        assert_eq!(joined.as_str(), "https://example.com/dir/style.css");
    }

    #[test]
    fn hosts_are_canonical() {
        assert_eq!(
            Url::parse("HTTPS://Example.COM/Path").unwrap().as_str(),
            "https://example.com/Path"
        );
        assert_eq!(
            Url::parse("https://münchen.de/").unwrap().host,
            "xn--mnchen-3ya.de"
        );
        assert_eq!(Url::parse("http://[::1]:8080/").unwrap().host, "[::1]");
        assert_eq!(Url::parse("http://[zz::1]/"), Err(UrlError::InvalidHost));
        assert_eq!(
            Url::parse("https://exa mple.com/"),
            Err(UrlError::InvalidHost)
        );
        assert_eq!(Url::parse("https://a<b.com/"), Err(UrlError::InvalidHost));
    }

    #[test]
    fn directory_references_keep_their_trailing_slash() {
        let base = Url::parse("https://example.com/dir/page.html").unwrap();
        assert_eq!(base.join("sub/").unwrap().path, "/dir/sub/");
        assert_eq!(base.join("..").unwrap().path, "/");
        assert_eq!(base.join("/a/b/..").unwrap().path, "/a/");
        assert_eq!(
            base.join("/assets/").unwrap().join("x.css").unwrap().path,
            "/assets/x.css"
        );
        assert_eq!(
            Url::parse("https://example.com/docs/").unwrap().path,
            "/docs/"
        );
    }
}
