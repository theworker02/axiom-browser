//! Omnibox input classification: navigate to a URL, or search for the text.
//!
//! Rules, in order:
//!
//! 1. A leading `?` forces a search; so does text wrapped in double quotes.
//! 2. `http://`, `https://` and `axiom://` inputs are URLs if they parse; `file:` and
//!    `about:` are passed through. Any other `scheme:` (`javascript:`, `mailto:`, …) is
//!    searched for, never navigated to.
//! 3. Whitespace anywhere means a search.
//! 4. Otherwise the text before the first `/`, `?` or `#` is split into host and port.
//!    Userinfo (`a@b.com`, usually an e-mail address) means a search; a non-numeric or
//!    out-of-range port means a search.
//! 5. The host is a URL host when it is `localhost` (or `*.localhost`), an IPv4 or
//!    bracketed IPv6 literal, a domain whose suffix is on the Public Suffix List (so
//!    `rust-lang.org` and `münchen.de` navigate but `file.txt` and `node.js` do not), a
//!    reserved local-use name (`.test`, `.local`, `.internal`, `.example`, `.home.arpa`),
//!    or a single label followed by a port or a `/`.
//!
//! Hosts are converted to their canonical ASCII form (lowercase, punycode) by
//! [`axiom_url::Url::parse`]. Local names and IP literals default to `http`; public
//! domains default to `https`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassifiedInput {
    /// A canonical absolute URL.
    Url(String),
    /// Search terms (trimmed; a forcing `?` removed).
    Search(String),
}

/// The name used by the Wave F plan for the same type.
pub type OmniboxInput = ClassifiedInput;

pub struct OmniboxInputClassifier;

/// Special-use names that never appear on the PSL but are typed as hosts.
const LOCAL_SUFFIXES: &[&str] = &[
    "localhost",
    "test",
    "local",
    "internal",
    "example",
    "home.arpa",
];

impl OmniboxInputClassifier {
    pub fn classify(input: &str) -> ClassifiedInput {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return ClassifiedInput::Search(String::new());
        }
        if let Some(rest) = trimmed.strip_prefix('?') {
            return ClassifiedInput::Search(rest.trim().to_string());
        }
        if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
            return ClassifiedInput::Search(trimmed.to_string());
        }
        let search = || ClassifiedInput::Search(trimmed.to_string());

        if let Some(scheme) = explicit_scheme(trimmed) {
            return match scheme.as_str() {
                "http" | "https" | "axiom" if has_authority(trimmed) => {
                    match axiom_url::Url::parse(trimmed) {
                        Ok(u) => ClassifiedInput::Url(u.as_str()),
                        Err(_) => search(),
                    }
                }
                "file" | "about" => ClassifiedInput::Url(trimmed.to_string()),
                _ => search(),
            };
        }

        if trimmed.chars().any(char::is_whitespace) {
            return search();
        }

        let (authority, rest) = split_authority(trimmed);
        if authority.is_empty() || authority.contains('@') {
            return search();
        }
        let Some((host, port)) = split_host_port(authority) else {
            return search();
        };
        let scheme = match classify_host(host, port.is_some(), rest.starts_with('/')) {
            Some(scheme) => scheme,
            None => return search(),
        };
        match axiom_url::Url::parse(&format!("{scheme}://{trimmed}")) {
            Ok(u) => ClassifiedInput::Url(u.as_str()),
            Err(_) => search(),
        }
    }
}

/// The lowercase scheme of `s` if it starts with `scheme:` — but not when the part after
/// the colon is a port (`localhost:8080`, `example.com:443/x`).
fn explicit_scheme(s: &str) -> Option<String> {
    let (scheme, after) = s.split_once(':')?;
    let mut chars = scheme.chars();
    if !chars.next()?.is_ascii_alphabetic()
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    let port_like = after
        .split(['/', '?', '#'])
        .next()
        .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    if port_like && !after.starts_with("//") {
        return None;
    }
    Some(scheme.to_ascii_lowercase())
}

fn has_authority(s: &str) -> bool {
    s.split_once(':')
        .is_some_and(|(_, after)| after.starts_with("//") && after.len() > 2)
}

/// `host[:port]` and everything from the first `/`, `?` or `#`.
fn split_authority(s: &str) -> (&str, &str) {
    let s = s.trim_start_matches('/');
    match s.find(['/', '?', '#']) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    }
}

fn split_host_port(authority: &str) -> Option<(&str, Option<u16>)> {
    if authority.starts_with('[') {
        let end = authority.find(']')?;
        let host = &authority[..=end];
        let port = match &authority[end + 1..] {
            "" => None,
            p => Some(parse_port(p.strip_prefix(':')?)?),
        };
        return Some((host, port));
    }
    match authority.rsplit_once(':') {
        Some((host, p)) => Some((host, Some(parse_port(p)?))),
        None => Some((authority, None)),
    }
}

fn parse_port(p: &str) -> Option<u16> {
    if p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    p.parse().ok()
}

/// Default scheme when `host` names a navigable host, `None` when the input is a search.
fn classify_host(host: &str, has_port: bool, has_path: bool) -> Option<&'static str> {
    if host.starts_with('[') {
        return host[1..host.len() - 1]
            .parse::<std::net::Ipv6Addr>()
            .ok()
            .map(|_| "http");
    }
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() {
        return None;
    }
    if is_ipv4(host) {
        return Some("http");
    }
    if !host
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return None;
    }
    let ascii = if host.is_ascii() {
        host.to_ascii_lowercase()
    } else {
        idna::domain_to_ascii(host).ok()?
    };
    let labels: Vec<&str> = ascii.split('.').collect();
    if labels
        .iter()
        .any(|l| l.is_empty() || l.len() > 63 || l.starts_with('-') || l.ends_with('-'))
    {
        return None;
    }
    if labels.len() == 1 {
        if ascii == "localhost" || has_port || has_path {
            return Some("http");
        }
        return None;
    }
    if LOCAL_SUFFIXES
        .iter()
        .any(|s| ascii == *s || ascii.ends_with(&format!(".{s}")))
    {
        return Some("http");
    }
    if labels
        .last()
        .is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let known = psl::suffix(ascii.as_bytes()).is_some_and(|s| s.is_known());
    let registrable = psl::domain(ascii.as_bytes()).is_some();
    (known && registrable).then_some("https")
}

fn is_ipv4(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 3
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(input: &str) -> String {
        match OmniboxInputClassifier::classify(input) {
            ClassifiedInput::Url(u) => u,
            other => panic!("{input:?} should be a URL, got {other:?}"),
        }
    }

    fn search(input: &str) -> String {
        match OmniboxInputClassifier::classify(input) {
            ClassifiedInput::Search(s) => s,
            other => panic!("{input:?} should be a search, got {other:?}"),
        }
    }

    #[test]
    fn plan_examples() {
        assert_eq!(url("google.com"), "https://google.com/");
        assert_eq!(url("https://google.com"), "https://google.com/");
        assert_eq!(url("localhost:8080"), "http://localhost:8080/");
        assert_eq!(url("127.0.0.1:3000"), "http://127.0.0.1:3000/");
        assert_eq!(url("rust-lang.org"), "https://rust-lang.org/");
        assert_eq!(
            search("rust programming language"),
            "rust programming language"
        );
        assert_eq!(
            search("\"how does rust ownership work\""),
            "\"how does rust ownership work\""
        );
    }

    #[test]
    fn explicit_schemes() {
        assert_eq!(url("http://example.com"), "http://example.com/");
        assert_eq!(
            url("HTTPS://Example.COM/A?b=1#c"),
            "https://example.com/A?b=1#c"
        );
        assert_eq!(url("axiom://settings/search"), "axiom://settings/search");
        assert_eq!(url("file:///C:/x.html"), "file:///C:/x.html");
        assert_eq!(url("about:blank"), "about:blank");
        assert_eq!(search("javascript:alert(1)"), "javascript:alert(1)");
        assert_eq!(search("mailto:a@b.com"), "mailto:a@b.com");
        assert_eq!(search("https://"), "https://");
        assert_eq!(search("https://exa mple.com"), "https://exa mple.com");
    }

    #[test]
    fn hosts_ports_and_paths() {
        assert_eq!(
            url("example.com/docs/page?x=1"),
            "https://example.com/docs/page?x=1"
        );
        assert_eq!(url("example.com:8443"), "https://example.com:8443/");
        assert_eq!(url("www.bbc.co.uk"), "https://www.bbc.co.uk/");
        assert_eq!(url("Example.COM."), "https://example.com./");
        assert_eq!(url("localhost"), "http://localhost/");
        assert_eq!(url("app.localhost:5173/x"), "http://app.localhost:5173/x");
        assert_eq!(url("192.168.1.1"), "http://192.168.1.1/");
        assert_eq!(url("[::1]:8080/a"), "http://[::1]:8080/a");
        assert_eq!(url("intranet:8080"), "http://intranet:8080/");
        assert_eq!(url("intranet/"), "http://intranet/");
        assert_eq!(url("printer.local"), "http://printer.local/");
        assert_eq!(url("site.test/a"), "http://site.test/a");
        assert_eq!(search("example.com:99999"), "example.com:99999");
        assert_eq!(search("foo:bar"), "foo:bar");
        assert_eq!(search("user@example.com"), "user@example.com");
        assert_eq!(search("[zz::1]"), "[zz::1]");
        assert_eq!(search("256.1.1.1"), "256.1.1.1");
    }

    #[test]
    fn suffixes_come_from_the_public_suffix_list() {
        assert_eq!(search("rust"), "rust");
        assert_eq!(search("file.txt"), "file.txt");
        assert_eq!(search("node.js"), "node.js");
        assert_eq!(search("3.14"), "3.14");
        assert_eq!(search("-bad-.com"), "-bad-.com");
        assert_eq!(search("a..com"), "a..com");
        assert_eq!(url("docs.rs"), "https://docs.rs/");
        assert_eq!(
            url("readme.md"),
            "https://readme.md/",
            ".md is Moldova's TLD"
        );
    }

    #[test]
    fn internationalized_domains_become_punycode() {
        assert_eq!(url("münchen.de"), "https://xn--mnchen-3ya.de/");
        let jp = url("例え.jp/パス");
        assert!(
            jp.starts_with("https://xn--") && jp.ends_with(".jp/パス"),
            "{jp}"
        );
        assert_eq!(search("日本語 検索"), "日本語 検索");
    }

    #[test]
    fn searches_with_punctuation() {
        assert_eq!(search("?example.com"), "example.com");
        assert_eq!(search("? rust"), "rust");
        assert_eq!(search("what is 3.14?"), "what is 3.14?");
        assert_eq!(search("c++"), "c++");
        assert_eq!(search("rust: ownership"), "rust: ownership");
        assert_eq!(search("  "), "");
    }
}
