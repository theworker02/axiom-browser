//! Base URL handling and subresource URL resolution.

use std::path::{Path, PathBuf};

use axiom_url::Url;

/// Where a subresource reference points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubresourceTarget {
    /// Fetched through the resource loader and the network service.
    Network(Url),
    /// A file next to a local document (read from disk; never from the network).
    Local(PathBuf),
    /// Inline `data:` bytes.
    Data {
        mime: String,
        bytes: Vec<u8>,
    },
    Invalid(String),
}

/// Resolve `src` against `base` (the document base URL, or the stylesheet URL for
/// references inside CSS). HTTP(S) bases only resolve to HTTP(S) URLs; other schemes
/// (`javascript:`, `file:` from a web page, `axiom:`) are invalid.
pub fn resolve_subresource(base: &str, src: &str) -> SubresourceTarget {
    let src = src.trim();
    if src.is_empty() {
        return SubresourceTarget::Invalid("empty URL".into());
    }
    if src.len() >= 5 && src[..5].eq_ignore_ascii_case("data:") {
        return match axiom_loader::parse_data_url(&format!("data:{}", &src[5..])) {
            Some((mime, bytes)) => SubresourceTarget::Data { mime, bytes },
            None => SubresourceTarget::Invalid("malformed data: URL".into()),
        };
    }
    let is_http = |u: &Url| u.scheme == "http" || u.scheme == "https";
    let lower = src.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return match Url::parse(src) {
            Ok(u) => SubresourceTarget::Network(u),
            Err(e) => SubresourceTarget::Invalid(e.to_string()),
        };
    }
    if let Ok(base_url) = Url::parse(base) {
        if has_scheme(src) && !src.starts_with("//") {
            return SubresourceTarget::Invalid(format!(
                "unsupported scheme in {}",
                src.split(':').next().unwrap_or("")
            ));
        }
        return match base_url.join(src) {
            Ok(u) if is_http(&u) => SubresourceTarget::Network(u),
            Ok(u) => SubresourceTarget::Invalid(format!("{} URLs are not loaded", u.scheme)),
            Err(e) => SubresourceTarget::Invalid(e.to_string()),
        };
    }
    if has_scheme(src) {
        return SubresourceTarget::Invalid("unsupported scheme".into());
    }
    let base_path = base
        .strip_prefix("file:///")
        .or_else(|| base.strip_prefix("file://"))
        .unwrap_or(base);
    let parent = Path::new(base_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    SubresourceTarget::Local(parent.join(src))
}

/// Effective URL of a `<base href>`: resolved against the document URL, HTTP(S) only.
pub fn resolve_base_href(document_url: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    match resolve_subresource(document_url, href) {
        SubresourceTarget::Network(u) => Some(u.as_str()),
        _ => None,
    }
}

fn has_scheme(s: &str) -> bool {
    let Some((scheme, _)) = s.split_once(':') else {
        return false;
    };
    // A single letter is a Windows drive (`C:\…`), not a scheme.
    scheme.len() > 1
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(base: &str, src: &str) -> String {
        match resolve_subresource(base, src) {
            SubresourceTarget::Network(u) => u.as_str(),
            other => panic!("{src} against {base}: {other:?}"),
        }
    }

    #[test]
    fn resolves_relative_parent_absolute_path_and_absolute_urls() {
        let base = "http://a.test/dir/sub/page.html";
        assert_eq!(net(base, "x.css"), "http://a.test/dir/sub/x.css");
        assert_eq!(net(base, "../x.js"), "http://a.test/dir/x.js");
        assert_eq!(net(base, "/root.png"), "http://a.test/root.png");
        assert_eq!(net(base, "https://b.test/y.css"), "https://b.test/y.css");
        assert_eq!(net(base, "//c.test/z.png"), "http://c.test/z.png");
    }

    #[test]
    fn rejects_non_http_schemes_from_web_documents() {
        let base = "https://a.test/";
        for src in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "axiom://settings",
            "ftp://x/y",
        ] {
            assert!(
                matches!(
                    resolve_subresource(base, src),
                    SubresourceTarget::Invalid(_)
                ),
                "{src}"
            );
        }
        // Internal documents resolve relative references to axiom: URLs: never loaded.
        assert!(matches!(
            resolve_subresource("axiom://newtab/", "style.css"),
            SubresourceTarget::Invalid(_)
        ));
    }

    #[test]
    fn data_urls_and_local_files() {
        assert_eq!(
            resolve_subresource("http://a/", "data:text/css,p%7B%7D"),
            SubresourceTarget::Data {
                mime: "text/css".into(),
                bytes: b"p{}".to_vec()
            }
        );
        assert_eq!(
            resolve_subresource("file:///C:/site/index.html", "img/a.png"),
            SubresourceTarget::Local(Path::new("C:/site").join("img/a.png"))
        );
    }

    #[test]
    fn base_href_resolves_against_the_document() {
        assert_eq!(
            resolve_base_href("http://a.test/x/page.html", "/assets/").as_deref(),
            Some("http://a.test/assets/")
        );
        assert_eq!(
            resolve_base_href("http://a.test/x/page.html", "https://cdn.test/v1/").as_deref(),
            Some("https://cdn.test/v1/")
        );
        assert_eq!(resolve_base_href("http://a.test/", "javascript:x"), None);
    }
}
