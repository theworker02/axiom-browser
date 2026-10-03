//! MIME type parsing and text decoding.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MimeType {
    pub type_: String,
    pub subtype: String,
    /// Parameters with lower-cased names, in header order (quotes removed).
    pub params: Vec<(String, String)>,
    /// Lower-cased `charset` parameter, if present.
    pub charset: Option<String>,
    pub raw: String,
}

const JAVASCRIPT_ESSENCES: &[&str] = &[
    "application/ecmascript",
    "application/javascript",
    "application/x-ecmascript",
    "application/x-javascript",
    "text/ecmascript",
    "text/javascript",
    "text/javascript1.0",
    "text/javascript1.1",
    "text/javascript1.2",
    "text/javascript1.3",
    "text/javascript1.4",
    "text/javascript1.5",
    "text/jscript",
    "text/livescript",
    "text/x-ecmascript",
    "text/x-javascript",
];

impl MimeType {
    pub fn parse(input: &str) -> Option<Self> {
        let raw = input.trim().to_string();
        if raw.is_empty() {
            return None;
        }
        let mut parts = raw.split(';');
        let main = parts.next()?.trim();
        let (type_, subtype) = main.split_once('/')?;
        let (type_, subtype) = (type_.trim(), subtype.trim());
        if type_.is_empty() || subtype.is_empty() {
            return None;
        }
        let mut params = Vec::new();
        for p in parts {
            let Some((name, value)) = p.split_once('=') else {
                continue;
            };
            let name = name.trim().to_ascii_lowercase();
            if name.is_empty() {
                continue;
            }
            let value = value.trim().trim_matches('"').to_string();
            params.push((name, value));
        }
        let charset = params
            .iter()
            .find(|(n, _)| n == "charset")
            .map(|(_, v)| v.to_ascii_lowercase());
        Some(Self {
            type_: type_.to_ascii_lowercase(),
            subtype: subtype.to_ascii_lowercase(),
            params,
            charset,
            raw,
        })
    }

    pub fn essence(&self) -> String {
        format!("{}/{}", self.type_, self.subtype)
    }

    pub fn param(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.params
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn is_html(&self) -> bool {
        self.type_ == "text" && self.subtype == "html"
    }

    pub fn is_css(&self) -> bool {
        self.type_ == "text" && self.subtype == "css"
    }

    pub fn is_javascript(&self) -> bool {
        JAVASCRIPT_ESSENCES.contains(&self.essence().as_str())
    }

    pub fn is_image(&self) -> bool {
        self.type_ == "image"
    }

    /// MIME types that must never execute as script (Fetch "bad script MIME type").
    pub fn is_blocked_for_script(&self) -> bool {
        matches!(self.type_.as_str(), "audio" | "image" | "video") || self.essence() == "text/csv"
    }
}

pub use crate::encoding::decode_text;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_params_and_charset() {
        let m = MimeType::parse("Text/HTML; Charset=\"UTF-8\"; foo=bar").unwrap();
        assert_eq!(m.essence(), "text/html");
        assert_eq!(m.charset.as_deref(), Some("utf-8"));
        assert_eq!(m.param("FOO"), Some("bar"));
        assert!(m.is_html());
        assert!(MimeType::parse("nonsense").is_none());
    }

    #[test]
    fn classifies_script_types() {
        assert!(MimeType::parse("text/javascript").unwrap().is_javascript());
        assert!(MimeType::parse("application/javascript; charset=utf-8")
            .unwrap()
            .is_javascript());
        assert!(MimeType::parse("image/png")
            .unwrap()
            .is_blocked_for_script());
        assert!(MimeType::parse("text/csv").unwrap().is_blocked_for_script());
    }

    #[test]
    fn decodes_charsets() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFhi", None), "hi");
        assert_eq!(decode_text(b"caf\xE9", Some("iso-8859-1")), "café");
        assert_eq!(
            decode_text(b"\x93q\x94", Some("windows-1252")),
            "\u{201C}q\u{201D}"
        );
        assert_eq!(decode_text(&[0xFF, 0xFE, b'o', 0, b'k', 0], None), "ok");
        assert_eq!(decode_text("ü".as_bytes(), Some("utf-8")), "ü");
    }
}
