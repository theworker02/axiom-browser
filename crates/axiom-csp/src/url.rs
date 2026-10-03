//! The URL parts CSP matching looks at.

use axiom_url::Url;

/// A URL reduced to what source-expression matching needs. Unlike [`Url`] it can
/// represent URLs without a host (`data:`, `blob:`, `about:`) and `ws:` / `wss:` URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CspUrl {
    /// Lowercase scheme without the `:`.
    pub scheme: String,
    /// Lowercase host; `None` for URLs without an authority.
    pub host: Option<String>,
    /// Explicit port, `None` when absent.
    pub port: Option<u16>,
    /// The path (percent-encoded as written); for hostless URLs, everything after `scheme:`
    /// up to the query.
    pub path: String,
    serialized: String,
}

impl CspUrl {
    pub fn from_url(url: &Url) -> Self {
        let mut u = url.clone();
        u.fragment = None;
        Self {
            scheme: url.scheme.to_ascii_lowercase(),
            host: Some(url.host.to_ascii_lowercase()),
            port: url.port,
            path: url.path.clone(),
            serialized: u.as_str(),
        }
    }

    /// Parse an absolute URL. Returns `None` for input without a valid scheme.
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        let (scheme, rest) = input.split_once(':')?;
        let mut chars = scheme.chars();
        if !chars.next()?.is_ascii_alphabetic()
            || !chars.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        {
            return None;
        }
        let scheme = scheme.to_ascii_lowercase();
        let without_fragment = input.split('#').next().unwrap_or(input);
        let rest = rest.split('#').next().unwrap_or(rest);
        let Some(after) = rest.strip_prefix("//") else {
            let path = rest.split('?').next().unwrap_or(rest).to_string();
            return Some(Self {
                scheme,
                host: None,
                port: None,
                path,
                serialized: without_fragment.to_string(),
            });
        };
        let end = after.find(['/', '?']).unwrap_or(after.len());
        let authority = &after[..end];
        let tail = &after[end..];
        let host_port = authority.rsplit('@').next().unwrap_or(authority);
        let (host, port) = split_host_port(host_port)?;
        let path = tail.split('?').next().unwrap_or("");
        let path = if path.is_empty() { "/" } else { path };
        Some(Self {
            scheme,
            host: Some(host.to_ascii_lowercase()),
            port,
            path: path.to_string(),
            serialized: without_fragment.to_string(),
        })
    }

    /// The port, or the scheme's default port.
    pub fn effective_port(&self) -> Option<u16> {
        self.port.or_else(|| default_port(&self.scheme))
    }

    /// The URL without its fragment, as reported in violations.
    pub fn report_string(&self) -> String {
        self.serialized.clone()
    }

    /// `scheme://host[:port]`, or `scheme` for hostless URLs; used in reports when the
    /// full URL must not be disclosed (after a redirect).
    pub fn origin_string(&self) -> String {
        match &self.host {
            Some(host) => match self.port {
                Some(p) if Some(p) != default_port(&self.scheme) => {
                    format!("{}://{host}:{p}", self.scheme)
                }
                _ => format!("{}://{host}", self.scheme),
            },
            None => self.scheme.clone(),
        }
    }
}

pub(crate) fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "ftp" => Some(21),
        _ => None,
    }
}

fn split_host_port(s: &str) -> Option<(&str, Option<u16>)> {
    if let Some(rest) = s.strip_prefix('[') {
        let close = rest.find(']')?;
        let host = &s[..close + 2];
        return match &rest[close + 1..] {
            "" => Some((host, None)),
            p => Some((host, Some(p.strip_prefix(':')?.parse().ok()?))),
        };
    }
    match s.rsplit_once(':') {
        Some((h, "")) => Some((h, None)),
        Some((h, p)) => Some((h, Some(p.parse().ok()?))),
        None => Some((s, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hierarchical_and_opaque_urls() {
        let u = CspUrl::parse("WSS://User@Chat.Example.com:8443/room?x#f").unwrap();
        assert_eq!(u.scheme, "wss");
        assert_eq!(u.host.as_deref(), Some("chat.example.com"));
        assert_eq!(u.port, Some(8443));
        assert_eq!(u.path, "/room");
        assert_eq!(u.report_string(), "WSS://User@Chat.Example.com:8443/room?x");
        assert_eq!(u.origin_string(), "wss://chat.example.com:8443");

        let d = CspUrl::parse("data:image/png;base64,AAAA").unwrap();
        assert_eq!(d.scheme, "data");
        assert_eq!(d.host, None);
        assert_eq!(d.origin_string(), "data");

        let b = CspUrl::parse("blob:https://a.test/uuid").unwrap();
        assert_eq!(b.scheme, "blob");
        assert_eq!(b.host, None);

        assert!(CspUrl::parse("no-scheme").is_none());
        assert!(CspUrl::parse("1x:foo").is_none());
    }

    #[test]
    fn default_ports() {
        let u = CspUrl::parse("https://a.test").unwrap();
        assert_eq!(u.path, "/");
        assert_eq!(u.effective_port(), Some(443));
        let u = CspUrl::from_url(&Url::parse("http://a.test:8080/x#y").unwrap());
        assert_eq!(u.effective_port(), Some(8080));
        assert_eq!(u.report_string(), "http://a.test:8080/x");
    }
}
