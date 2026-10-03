//! Conservative preload scanner.
//!
//! While the parser is paused at a script, [`scan_for_preloads`] looks ahead into the
//! unparsed markup for resources the parser will certainly request: classic external
//! scripts, stylesheets and non-lazy images. It never builds DOM, never executes anything,
//! skips comments and rawtext bodies, and stops at the first `<base>` (which would change
//! URL resolution) or an incomplete tag. The document loader adopts a preloaded resource
//! when the parser reaches its element; a hint the parser never reaches is merely an
//! extra fetch.

use axiom_net::ResourceType;

use crate::resource::CorsSettings;
use crate::script::is_classic_script_type;

/// Hints returned per scan.
pub const MAX_PRELOAD_HINTS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreloadHint {
    pub kind: ResourceType,
    /// Raw attribute value (unresolved).
    pub url: String,
    /// Byte offset of the tag in the scanned markup.
    pub offset: usize,
    /// The element's `crossorigin` state; the preload uses the same CORS mode.
    pub cors: Option<CorsSettings>,
}

pub fn scan_for_preloads(markup: &str) -> Vec<PreloadHint> {
    let lower = markup.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut hints = Vec::new();
    let mut i = 0;
    while i < bytes.len() && hints.len() < MAX_PRELOAD_HINTS {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        if lower[i..].starts_with("<!--") {
            match lower[i + 4..].find("-->") {
                Some(e) => i += 4 + e + 3,
                None => break,
            }
            continue;
        }
        let name_len = bytes[i + 1..]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric())
            .count();
        if name_len == 0 {
            i += 1;
            continue;
        }
        let name = &lower[i + 1..i + 1 + name_len];
        let Some(tag_end) = find_tag_end(bytes, i + 1 + name_len) else {
            break;
        };
        let attrs = parse_attrs(&markup[i + 1 + name_len..tag_end]);
        let get = |n: &str| {
            attrs
                .iter()
                .find(|(k, _)| k == n)
                .map(|(_, v)| v.trim().to_string())
        };
        let cors = CorsSettings::from_attribute(get("crossorigin").as_deref());
        match name {
            "base" => break,
            "script" => {
                if let Some(src) = get("src").filter(|s| !s.is_empty()) {
                    let ty = get("type");
                    let module = ty
                        .as_deref()
                        .is_some_and(|t| t.eq_ignore_ascii_case("module"));
                    if !module && is_classic_script_type(ty.as_deref()) {
                        hints.push(PreloadHint {
                            kind: ResourceType::Script,
                            url: src,
                            offset: i,
                            cors,
                        });
                    }
                }
            }
            "link" => {
                let rel = get("rel").unwrap_or_default().to_ascii_lowercase();
                let mut tokens = rel.split_ascii_whitespace();
                let sheet = tokens.clone().any(|t| t == "stylesheet");
                let alternate = tokens.any(|t| t == "alternate");
                if let Some(href) = get("href").filter(|h| !h.is_empty()) {
                    if sheet && !alternate {
                        hints.push(PreloadHint {
                            kind: ResourceType::Stylesheet,
                            url: href,
                            offset: i,
                            cors,
                        });
                    }
                }
            }
            "img" => {
                let lazy = get("loading").is_some_and(|l| l.eq_ignore_ascii_case("lazy"));
                if let Some(src) = get("src").filter(|s| !s.is_empty()) {
                    if !lazy {
                        hints.push(PreloadHint {
                            kind: ResourceType::Image,
                            url: src,
                            offset: i,
                            cors,
                        });
                    }
                }
            }
            _ => {}
        }
        i = tag_end + 1;
        if matches!(name, "script" | "style" | "textarea" | "title") {
            let close = format!("</{name}");
            match lower[i..].find(&close) {
                Some(e) => i += e,
                None => break,
            }
        }
    }
    hints
}

fn find_tag_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut quote = None;
    for (i, &b) in bytes.iter().enumerate().skip(from) {
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == b'>' => return Some(i),
            None => {}
        }
    }
    None
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        let start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'/' {
            i += 1;
        }
        if start == i {
            i += 1;
            continue;
        }
        let name = s[start..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                let vs = i + 1;
                i = vs;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                value = s[vs..i].to_string();
                i += 1;
            } else {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() {
                    i += 1;
                }
                value = s[vs..i].to_string();
            }
        }
        out.push((name, value));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(markup: &str) -> Vec<(ResourceType, String)> {
        scan_for_preloads(markup)
            .into_iter()
            .map(|h| (h.kind, h.url))
            .collect()
    }

    #[test]
    fn finds_scripts_stylesheets_and_eager_images() {
        let found = kinds(
            r#"<p>x</p><script src="a.js"></script><link rel="stylesheet" href="s.css">
<img src="i.png"><img loading=lazy src="lazy.png"><script type=module src=m.js></script>
<link rel="alternate stylesheet" href="alt.css"><script>var s = "<img src='in-script.png'>";</script>"#,
        );
        assert_eq!(
            found,
            [
                (ResourceType::Script, "a.js".to_string()),
                (ResourceType::Stylesheet, "s.css".to_string()),
                (ResourceType::Image, "i.png".to_string()),
            ]
        );
    }

    #[test]
    fn records_the_crossorigin_state() {
        let hints = scan_for_preloads(
            r#"<script src="a.js" crossorigin></script><img src="i.png" crossorigin="use-credentials"><link rel=stylesheet href=s.css>"#,
        );
        let cors: Vec<_> = hints.iter().map(|h| h.cors).collect();
        assert_eq!(
            cors,
            [
                Some(CorsSettings::Anonymous),
                Some(CorsSettings::UseCredentials),
                None
            ]
        );
    }

    #[test]
    fn skips_comments_and_stops_at_base_or_incomplete_tags() {
        assert!(kinds(r#"<!-- <script src="c.js"></script> -->"#).is_empty());
        assert_eq!(
            kinds(r#"<img src="a.png"><base href="/x/"><img src="b.png">"#),
            [(ResourceType::Image, "a.png".to_string())]
        );
        assert_eq!(
            kinds(r#"<img src="a.png"><img src="b.p"#),
            [(ResourceType::Image, "a.png".to_string())]
        );
    }
}
