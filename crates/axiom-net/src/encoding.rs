//! Centralized text decoding: one streaming decoder for documents, stylesheets and
//! scripts.
//!
//! Encoding selection, in order: byte order mark, transport-declared charset
//! (`Content-Type`), `<meta charset>` prescan of the first 1024 bytes (HTML only), then
//! UTF-8. Supported encodings: UTF-8, UTF-16LE/BE and windows-1252 (which WHATWG Encoding
//! also uses for the `iso-8859-1`, `latin1` and `us-ascii` labels). Unknown labels are
//! ignored (the next step decides). Chunk boundaries never change the output: incomplete
//! sequences are held until the next chunk or [`TextDecoder::finish`].

/// Bytes an HTML decoder waits for before giving up on a `<meta charset>` declaration.
pub const META_PRESCAN_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Windows1252,
}

impl TextEncoding {
    /// WHATWG label lookup for the supported encodings.
    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "utf-8" | "utf8" | "unicode-1-1-utf-8" | "unicode11utf8" | "unicode20utf8"
            | "x-unicode20utf8" => Some(Self::Utf8),
            "utf-16le" | "utf-16" | "ucs-2" | "unicode" | "csunicode" | "iso-10646-ucs-2"
            | "unicodefeff" => Some(Self::Utf16Le),
            "utf-16be" | "unicodefffe" => Some(Self::Utf16Be),
            "windows-1252" | "cp1252" | "x-cp1252" | "iso-8859-1" | "iso8859-1" | "iso_8859-1"
            | "latin1" | "l1" | "us-ascii" | "ascii" | "ansi_x3.4-1968" | "cp819" | "ibm819" => {
                Some(Self::Windows1252)
            }
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf16Le => "UTF-16LE",
            Self::Utf16Be => "UTF-16BE",
            Self::Windows1252 => "windows-1252",
        }
    }
}

/// Where the chosen encoding came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodingSource {
    ByteOrderMark,
    Transport,
    MetaPrescan,
    Default,
}

impl EncodingSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ByteOrderMark => "bom",
            Self::Transport => "transport",
            Self::MetaPrescan => "meta",
            Self::Default => "default",
        }
    }
}

#[derive(Debug)]
pub struct TextDecoder {
    encoding: Option<(TextEncoding, EncodingSource)>,
    declared: Option<TextEncoding>,
    sniff_meta: bool,
    /// Undecoded bytes: buffered for encoding detection, or an incomplete sequence.
    pending: Vec<u8>,
}

impl TextDecoder {
    /// Decoder for non-HTML text (stylesheets, scripts, plain text).
    pub fn new(charset: Option<&str>) -> Self {
        Self {
            encoding: None,
            declared: charset.and_then(TextEncoding::from_label),
            sniff_meta: false,
            pending: Vec::new(),
        }
    }

    /// Decoder for HTML documents: also honours `<meta charset>` in the first 1024 bytes.
    pub fn for_html(charset: Option<&str>) -> Self {
        Self {
            sniff_meta: true,
            ..Self::new(charset)
        }
    }

    pub fn encoding(&self) -> Option<TextEncoding> {
        self.encoding.map(|(e, _)| e)
    }

    pub fn source(&self) -> Option<EncodingSource> {
        self.encoding.map(|(_, s)| s)
    }

    /// Decode the next chunk. May return less than was fed (held for the next chunk).
    pub fn decode(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        self.run(false)
    }

    /// End of input: flush everything still held.
    pub fn finish(&mut self) -> String {
        self.run(true)
    }

    fn run(&mut self, last: bool) -> String {
        if self.encoding.is_none() && !self.select(last) {
            return String::new();
        }
        let Some((encoding, _)) = self.encoding else {
            return String::new();
        };
        let buf = std::mem::take(&mut self.pending);
        let (text, rest) = match encoding {
            TextEncoding::Utf8 => decode_utf8(&buf, last),
            TextEncoding::Utf16Le => decode_utf16(&buf, false, last),
            TextEncoding::Utf16Be => decode_utf16(&buf, true, last),
            TextEncoding::Windows1252 => (decode_windows_1252(&buf), 0),
        };
        self.pending = buf[buf.len() - rest..].to_vec();
        text
    }

    /// Choose the encoding once enough bytes are buffered. Returns `false` to wait.
    fn select(&mut self, last: bool) -> bool {
        let p = &self.pending;
        const BOMS: [(&[u8], TextEncoding); 3] = [
            (&[0xEF, 0xBB, 0xBF], TextEncoding::Utf8),
            (&[0xFF, 0xFE], TextEncoding::Utf16Le),
            (&[0xFE, 0xFF], TextEncoding::Utf16Be),
        ];
        for (bom, enc) in BOMS {
            if p.starts_with(bom) {
                self.pending.drain(..bom.len());
                self.encoding = Some((enc, EncodingSource::ByteOrderMark));
                return true;
            }
            if !last && p.len() < bom.len() && bom.starts_with(p) {
                return false;
            }
        }
        if let Some(enc) = self.declared {
            self.encoding = Some((enc, EncodingSource::Transport));
            return true;
        }
        if self.sniff_meta {
            // Before the full window has arrived, only a complete `<meta ...>` tag is
            // decisive (nothing later can override the first declaration).
            let partial = !last && p.len() < META_PRESCAN_BYTES;
            let window = &p[..p.len().min(META_PRESCAN_BYTES)];
            let found = prescan(window, partial);
            if found.is_none() && partial {
                return false;
            }
            if let Some(enc) = found {
                // HTML: a UTF-16 label in a meta declaration means UTF-8.
                let enc = match enc {
                    TextEncoding::Utf16Le | TextEncoding::Utf16Be => TextEncoding::Utf8,
                    other => other,
                };
                self.encoding = Some((enc, EncodingSource::MetaPrescan));
                return true;
            }
        }
        self.encoding = Some((TextEncoding::Utf8, EncodingSource::Default));
        true
    }
}

/// Decode `bytes` completely (one-shot form of [`TextDecoder`], non-HTML rules).
pub fn decode_text(bytes: &[u8], charset: Option<&str>) -> String {
    let mut d = TextDecoder::new(charset);
    let mut text = d.decode(bytes);
    text.push_str(&d.finish());
    text
}

/// Returns (text, number of trailing bytes held back).
fn decode_utf8(buf: &[u8], last: bool) -> (String, usize) {
    let mut out = String::with_capacity(buf.len());
    let mut i = 0;
    while i < buf.len() {
        match std::str::from_utf8(&buf[i..]) {
            Ok(s) => {
                out.push_str(s);
                return (out, 0);
            }
            Err(e) => {
                let valid = e.valid_up_to();
                out.push_str(std::str::from_utf8(&buf[i..i + valid]).unwrap_or_default());
                match e.error_len() {
                    Some(len) => {
                        out.push('\u{FFFD}');
                        i += valid + len;
                    }
                    None if last => {
                        out.push('\u{FFFD}');
                        return (out, 0);
                    }
                    None => return (out, buf.len() - (i + valid)),
                }
            }
        }
    }
    (out, 0)
}

fn decode_utf16(buf: &[u8], big_endian: bool, last: bool) -> (String, usize) {
    let mut units: Vec<u16> = buf
        .chunks_exact(2)
        .map(|c| {
            if big_endian {
                u16::from_be_bytes([c[0], c[1]])
            } else {
                u16::from_le_bytes([c[0], c[1]])
            }
        })
        .collect();
    let mut held = buf.len() % 2;
    if !last && units.last().is_some_and(|u| (0xD800..0xDC00).contains(u)) {
        units.pop();
        held += 2;
    }
    let mut text = String::from_utf16_lossy(&units);
    if last && held == 1 {
        text.push('\u{FFFD}');
        held = 0;
    }
    (text, held)
}

const WINDOWS_1252_HIGH: [u16; 32] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
];

fn decode_windows_1252(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            let cp = if (0x80..0xA0).contains(&b) {
                WINDOWS_1252_HIGH[(b - 0x80) as usize] as u32
            } else {
                b as u32
            };
            char::from_u32(cp).unwrap_or('\u{FFFD}')
        })
        .collect()
}

/// Simplified WHATWG "prescan a byte stream to determine its encoding": the first
/// `<meta charset=…>` or `<meta http-equiv=content-type content="…charset=…">` outside
/// comments.
pub fn prescan_meta_charset(bytes: &[u8]) -> Option<TextEncoding> {
    prescan(bytes, false)
}

/// `complete_only`: stop at a `<meta` tag whose `>` has not arrived yet.
fn prescan(bytes: &[u8], complete_only: bool) -> Option<TextEncoding> {
    let lower: Vec<u8> = bytes.iter().map(u8::to_ascii_lowercase).collect();
    let mut i = 0;
    while i < lower.len() {
        if lower[i..].starts_with(b"<!--") {
            i = find(&lower, i + 4, b"-->").map_or(lower.len(), |e| e + 3);
            continue;
        }
        if lower[i..].starts_with(b"<meta")
            && lower
                .get(i + 5)
                .is_some_and(|c| c.is_ascii_whitespace() || *c == b'/')
        {
            let end = match find(&lower, i, b">") {
                Some(end) => end,
                None if complete_only => return None,
                None => lower.len(),
            };
            let attrs = parse_attrs(&lower[i + 5..end]);
            let get = |n: &str| attrs.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
            if let Some(label) = get("charset") {
                if let Some(enc) = TextEncoding::from_label(label) {
                    return Some(enc);
                }
            }
            if get("http-equiv") == Some("content-type") {
                if let Some(label) = get("content").and_then(charset_from_content) {
                    if let Some(enc) = TextEncoding::from_label(label) {
                        return Some(enc);
                    }
                }
            }
            i = end;
            continue;
        }
        i += 1;
    }
    None
}

fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

fn parse_attrs(tag: &[u8]) -> Vec<(String, String)> {
    let s = String::from_utf8_lossy(tag);
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
        let name = s[start..i].to_string();
        if name.is_empty() {
            i += 1;
            continue;
        }
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
                value = s[vs..i.min(b.len())].to_string();
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

fn charset_from_content(content: &str) -> Option<&str> {
    let at = content.find("charset")?;
    let rest = content[at + 7..]
        .trim_start()
        .strip_prefix('=')?
        .trim_start();
    let rest = rest.trim_start_matches(['"', '\'']);
    let end = rest
        .find(|c: char| c == ';' || c == '"' || c == '\'' || c.is_whitespace())
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunked(mut d: TextDecoder, bytes: &[u8], size: usize) -> String {
        let mut out = String::new();
        for c in bytes.chunks(size) {
            out.push_str(&d.decode(c));
        }
        out.push_str(&d.finish());
        out
    }

    #[test]
    fn chunk_boundaries_never_change_output() {
        let text = "héllo wörld — ✓ 𝄞 end";
        for size in 1..8 {
            assert_eq!(chunked(TextDecoder::new(None), text.as_bytes(), size), text);
        }
        let mut utf16 = vec![0xFF, 0xFE];
        for u in text.encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        for size in 1..6 {
            assert_eq!(chunked(TextDecoder::new(None), &utf16, size), text);
        }
    }

    #[test]
    fn invalid_utf8_is_replaced_like_from_utf8_lossy() {
        let bytes = b"a\xFFb\xE2\x82c\xF0\x9F";
        let expected = String::from_utf8_lossy(bytes).into_owned();
        for size in 1..5 {
            assert_eq!(chunked(TextDecoder::new(None), bytes, size), expected);
        }
    }

    #[test]
    fn bom_beats_declared_charset_and_declared_beats_meta() {
        assert_eq!(
            decode_text(b"\xEF\xBB\xBFcaf\xC3\xA9", Some("windows-1252")),
            "café"
        );
        let html = b"<meta charset=\"windows-1252\"><p>caf\xE9";
        let mut d = TextDecoder::for_html(Some("utf-8"));
        let out = d.decode(html) + &d.finish();
        assert!(out.contains("caf\u{FFFD}"));
        assert_eq!(d.source(), Some(EncodingSource::Transport));
    }

    #[test]
    fn html_meta_prescan_selects_the_encoding() {
        let html =
            b"<!doctype html><!-- <meta charset=utf-8> --><head><meta charset='latin1'><p>caf\xE9";
        let mut d = TextDecoder::for_html(None);
        let out = d.decode(html) + &d.finish();
        assert!(out.ends_with("café"), "{out}");
        assert_eq!(d.encoding(), Some(TextEncoding::Windows1252));
        assert_eq!(d.source(), Some(EncodingSource::MetaPrescan));

        let eq =
            b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1252\">\x93";
        assert_eq!(prescan_meta_charset(eq), Some(TextEncoding::Windows1252));
        assert_eq!(
            prescan_meta_charset(b"<meta charset=utf-16>"),
            Some(TextEncoding::Utf16Le)
        );
        let mut d = TextDecoder::for_html(None);
        let _ = d.decode(b"<meta charset=utf-16>") + &d.finish();
        assert_eq!(d.encoding(), Some(TextEncoding::Utf8));
    }

    #[test]
    fn html_decoder_waits_for_the_prescan_window() {
        let mut d = TextDecoder::for_html(None);
        assert_eq!(
            d.decode(b"<p>hi"),
            "",
            "decoded before the prescan window filled"
        );
        assert_eq!(d.finish(), "<p>hi");
        assert_eq!(d.source(), Some(EncodingSource::Default));
    }

    #[test]
    fn complete_meta_declaration_ends_the_prescan_early() {
        let mut d = TextDecoder::for_html(None);
        assert_eq!(
            d.decode(b"<head><meta charset=\"windows-12"),
            "",
            "truncated tag decided"
        );
        assert_eq!(
            d.decode(b"52\"><p>caf\xE9"),
            "<head><meta charset=\"windows-1252\"><p>café"
        );
        assert_eq!(d.source(), Some(EncodingSource::MetaPrescan));
        assert_eq!(d.encoding(), Some(TextEncoding::Windows1252));
    }
}
