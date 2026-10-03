//! Generated content (CSS Generated Content 3): the `content` property of `::before` /
//! `::after`, and the boxes' text once `attr()` and quotes are resolved.

use std::sync::Arc;

use crate::values::split_components;
use crate::ComputedStyle;

/// A `::before` / `::after` box: its style and its text.
#[derive(Debug, Clone)]
pub struct GeneratedBox {
    pub style: Arc<ComputedStyle>,
    pub text: String,
}

/// Computed `content`.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Content {
    /// `normal`: no box for `::before` / `::after`.
    #[default]
    Normal,
    None,
    Items(Arc<[ContentItem]>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContentItem {
    Text(Arc<str>),
    /// `attr(name)`: the element's attribute value, or nothing.
    Attr(Arc<str>),
    OpenQuote,
    CloseQuote,
}

impl Content {
    /// Whether `::before` / `::after` with this value generate a box.
    pub fn generates_box(&self) -> bool {
        matches!(self, Content::Items(_))
    }
}

/// `normal | none | [ <string> | attr() | counter() | open-quote | … ]+ [ / <alt> ]?`.
/// Counters and images contribute no text.
pub fn parse_content(raw: &str) -> Option<Content> {
    let v = raw.trim();
    match v.to_ascii_lowercase().as_str() {
        "normal" => return Some(Content::Normal),
        "none" => return Some(Content::None),
        _ => {}
    }
    let mut items = Vec::new();
    for part in split_components(v) {
        if part == "/" {
            break;
        }
        if part.starts_with(['"', '\'']) {
            let inner = &part[1..part.len().saturating_sub(1).max(1)];
            items.push(ContentItem::Text(Arc::from(unescape(inner))));
            continue;
        }
        let lower = part.to_ascii_lowercase();
        if let Some(args) = lower
            .strip_prefix("attr(")
            .and_then(|a| a.strip_suffix(')'))
        {
            let name = args.split([' ', ',']).next().unwrap_or("").trim();
            if !name.is_empty() {
                items.push(ContentItem::Attr(Arc::from(name)));
            }
            continue;
        }
        match lower.as_str() {
            "open-quote" => items.push(ContentItem::OpenQuote),
            "close-quote" => items.push(ContentItem::CloseQuote),
            "no-open-quote" | "no-close-quote" => {}
            _ if lower.starts_with("counter")
                || lower.starts_with("url(")
                || lower.contains("gradient(")
                || lower.starts_with("image-set(") => {}
            _ => return None,
        }
    }
    Some(Content::Items(items.into()))
}

/// A CSS string or `url()` body with its escapes (`\"`, `\201C `, `\` newline) resolved.
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let mut hex = String::new();
        while hex.len() < 6 && chars.peek().is_some_and(char::is_ascii_hexdigit) {
            hex.extend(chars.next());
        }
        if hex.is_empty() {
            match chars.next() {
                Some('\n') | None => {}
                Some(c) => out.push(c),
            }
            continue;
        }
        if chars.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            chars.next();
        }
        let code = u32::from_str_radix(&hex, 16).unwrap_or(0);
        out.push(match char::from_u32(code) {
            Some(c) if code != 0 => c,
            _ => char::REPLACEMENT_CHARACTER,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strings_attr_and_quotes() {
        let Some(Content::Items(items)) =
            parse_content(r#""\201C" attr(title) counter(x) '\'' open-quote / "alt""#)
        else {
            panic!("expected items");
        };
        assert_eq!(
            &*items,
            [
                ContentItem::Text("\u{201C}".into()),
                ContentItem::Attr("title".into()),
                ContentItem::Text("'".into()),
                ContentItem::OpenQuote,
            ]
        );
        assert_eq!(parse_content("none"), Some(Content::None));
        assert_eq!(
            parse_content("''"),
            Some(Content::Items(Arc::from([ContentItem::Text("".into())])))
        );
        assert_eq!(parse_content("bogus"), None);
    }
}
