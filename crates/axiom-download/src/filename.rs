//! Safe file names for downloads.
//!
//! Names come from `Content-Disposition`, the URL or the caller, i.e. from the network.
//! The result is always a single path component: no separators, no `..`, no drive or
//! device names, no control characters, and bounded length.

use std::path::{Path, PathBuf};

const MAX_NAME_BYTES: usize = 200;
const FALLBACK: &str = "download";
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Reduce `raw` to a file name that stays inside the download directory. Returns `None`
/// when nothing usable is left.
pub fn sanitize_component(raw: &str) -> Option<String> {
    // Only the last path segment counts, whatever separator the server used.
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let mut name: String = last
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        .collect();
    // Windows strips trailing dots and spaces; leading/trailing whitespace is never useful.
    name = name.trim().trim_end_matches(['.', ' ']).to_string();
    if name.is_empty() || name.chars().all(|c| c == '.') {
        return None;
    }
    // Hidden or relative-looking names ("..foo", ".bashrc") get a visible prefix.
    if name.starts_with('.') {
        name = format!("_{}", name.trim_start_matches('.'));
    }
    let stem = name.split('.').next().unwrap_or("");
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem.trim())) {
        name = format!("_{name}");
    }
    Some(truncate_keeping_extension(&name, MAX_NAME_BYTES))
}

/// File name for a download: the server-suggested name, else the URL's last path
/// segment, else `"download"`.
pub fn sanitize_filename(suggested: Option<&str>, url_path: &str) -> String {
    suggested
        .and_then(sanitize_component)
        .or_else(|| {
            let segment = url_path.rsplit('/').find(|s| !s.is_empty())?;
            sanitize_component(&percent_decode(segment))
        })
        .unwrap_or_else(|| FALLBACK.to_string())
}

fn truncate_keeping_extension(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i <= 16 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let mut budget = max - ext.len();
    while !stem.is_char_boundary(budget) {
        budget -= 1;
    }
    format!("{}{ext}", &stem[..budget])
}

/// `dir/name`, or `dir/name (1).ext`, `dir/name (2).ext`, … — the first path for which
/// `taken` is false.
pub fn unique_path(dir: &Path, name: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1.. {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !taken(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi << 4 | lo);
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

    #[test]
    fn traversal_never_escapes() {
        for raw in [
            "../../something",
            "..\\..\\windows\\system32\\evil.dll",
            "/etc/passwd",
            "C:\\Users\\x\\file.txt",
            "a/../../b.txt",
        ] {
            let name = sanitize_component(raw).unwrap();
            assert!(
                !name.contains('/') && !name.contains('\\'),
                "{raw} -> {name}"
            );
            assert!(!name.starts_with(".."), "{raw} -> {name}");
            assert!(!name.contains(':'), "{raw} -> {name}");
        }
        assert_eq!(sanitize_component("../../something").unwrap(), "something");
        assert_eq!(
            sanitize_component("C:\\Users\\x\\file.txt").unwrap(),
            "file.txt"
        );
        assert_eq!(sanitize_component(".."), None);
        assert_eq!(sanitize_component("../"), None);
    }

    #[test]
    fn hostile_characters_and_names() {
        assert_eq!(
            sanitize_component("a<b>c:d\"e|f?g*h.txt").unwrap(),
            "abcdefgh.txt"
        );
        assert_eq!(
            sanitize_component("bad\u{0}\u{7}name.bin").unwrap(),
            "badname.bin"
        );
        assert_eq!(sanitize_component("trailing. . .").unwrap(), "trailing");
        assert_eq!(sanitize_component(".bashrc").unwrap(), "_bashrc");
        assert_eq!(sanitize_component("CON").unwrap(), "_CON");
        assert_eq!(sanitize_component("nul.txt").unwrap(), "_nul.txt");
        assert_eq!(sanitize_component("com1.tar.gz").unwrap(), "_com1.tar.gz");
        assert_eq!(sanitize_component("console.log").unwrap(), "console.log");
    }

    #[test]
    fn long_names_keep_extension() {
        let long = format!("{}.pdf", "é".repeat(300));
        let name = sanitize_component(&long).unwrap();
        assert!(name.len() <= MAX_NAME_BYTES);
        assert!(name.ends_with(".pdf"));
    }

    #[test]
    fn fallbacks() {
        assert_eq!(sanitize_filename(Some("report.pdf"), "/x/y"), "report.pdf");
        assert_eq!(
            sanitize_filename(None, "/files/r%C3%A9sum%C3%A9.pdf"),
            "résumé.pdf"
        );
        assert_eq!(
            sanitize_filename(Some("../.."), "/files/data.csv"),
            "data.csv"
        );
        assert_eq!(sanitize_filename(None, "/"), "download");
        assert_eq!(sanitize_filename(None, "/dir/..%2F..%2Fescape"), "escape");
    }

    #[test]
    fn unique_paths() {
        let dir = Path::new("dl");
        let taken = |p: &Path| p == dir.join("a.txt") || p == dir.join("a (1).txt");
        assert_eq!(unique_path(dir, "a.txt", taken), dir.join("a (2).txt"));
        assert_eq!(unique_path(dir, "b", |_| false), dir.join("b"));
        assert_eq!(
            unique_path(dir, "c", |p| p == dir.join("c")),
            dir.join("c (1)")
        );
    }
}
