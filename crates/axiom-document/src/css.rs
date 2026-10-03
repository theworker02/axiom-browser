//! References inside CSS text: `@import` and `@font-face` sources. Callers resolve the
//! returned URLs against the stylesheet's own (final) URL — or the document base URL for
//! inline `<style>` — never against the document URL of an external sheet.

/// One `@font-face` rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontFace {
    /// Family name as declared (quotes removed).
    pub family: String,
    /// `url()` sources in declaration order (`local()` sources are skipped).
    pub sources: Vec<String>,
    /// `font-weight` is bold (600 or more) / `font-style` is italic or oblique.
    pub bold: bool,
    pub italic: bool,
}

/// `@import` URLs at the start of a sheet (after `@charset`/`@layer` statements and
/// before the first other rule, as CSS requires).
pub fn css_imports(css: &str) -> Vec<String> {
    let s = strip_comments(css);
    let (imports, _) = leading_imports(&s);
    imports
}

/// The sheet without its leading `@charset`/`@import` statements.
pub fn strip_css_imports(css: &str) -> String {
    let s = strip_comments(css);
    let (_, rest) = leading_imports(&s);
    rest.to_string()
}

/// `css` with each relative `url()` resolved against `base`, so the sheet's references
/// keep pointing at the right resources once its text is merged into the document.
/// `data:` URLs and fragment references (`url(#clip)`) are left as they are.
pub fn absolutize_urls(css: &str, base: &str) -> String {
    let Ok(base) = axiom_url::Url::parse(base) else {
        return css.to_string();
    };
    let lower = css.to_ascii_lowercase();
    let mut out = String::with_capacity(css.len());
    let mut copied = 0;
    let mut from = 0;
    while let Some(at) = lower[from..].find("url(") {
        let open = from + at;
        let start = open + 4;
        let Some((raw, used)) = url_function(&css[start..]) else {
            break;
        };
        from = start + used;
        let named = css[..open]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if named || raw.is_empty() || raw.starts_with('#') || starts_with_ci(&raw, "data:") {
            continue;
        }
        let Ok(url) = base.join(&raw) else {
            continue;
        };
        out.push_str(&css[copied..open]);
        out.push_str("url(\"");
        for c in url.as_str().chars() {
            match c {
                '"' => out.push_str("%22"),
                '\\' => out.push_str("%5C"),
                '\n' | '\r' => {}
                c => out.push(c),
            }
        }
        out.push_str("\")");
        copied = from;
    }
    out.push_str(&css[copied..]);
    out
}

fn leading_imports(s: &str) -> (Vec<String>, &str) {
    let mut imports = Vec::new();
    let mut rest = s.trim_start();
    loop {
        let statement = ["@charset", "@import", "@layer"]
            .into_iter()
            .find(|kw| starts_with_ci(rest, kw));
        let Some(kw) = statement else {
            break;
        };
        let end = match (rest.find(';'), rest.find('{')) {
            (Some(semi), Some(brace)) if brace < semi => break, // `@layer x { … }` block
            (Some(semi), _) => semi,
            (None, _) => break,
        };
        if kw == "@import" {
            if let Some(url) = first_url(&rest[kw.len()..end]) {
                imports.push(url);
            }
        }
        rest = rest[end + 1..].trim_start();
    }
    (imports, rest)
}

/// Every `@font-face` rule with a family and at least one `url()` source.
pub fn font_faces(css: &str) -> Vec<FontFace> {
    let s = strip_comments(css);
    let mut out = Vec::new();
    for block in font_face_blocks(&s) {
        let mut family = None;
        let mut sources = Vec::new();
        let (mut bold, mut italic) = (false, false);
        for decl in block.split(';') {
            let Some((name, raw)) = decl.split_once(':') else {
                continue;
            };
            let value = raw.trim().to_ascii_lowercase();
            match name.trim().to_ascii_lowercase().as_str() {
                "font-family" => family = Some(unquote(raw.trim()).to_string()),
                "src" => sources.extend(all_urls(raw)),
                "font-weight" => {
                    // A range (`100 900`) is represented by its lower end.
                    let first = value.split_whitespace().next().unwrap_or("");
                    bold = first == "bold"
                        || first == "bolder"
                        || first.parse::<f32>().is_ok_and(|w| w >= 600.0);
                }
                "font-style" => {
                    italic = value.starts_with("italic") || value.starts_with("oblique")
                }
                _ => {}
            }
        }
        if let Some(family) = family.filter(|f| !f.is_empty()) {
            if !sources.is_empty() {
                out.push(FontFace {
                    family,
                    sources,
                    bold,
                    italic,
                });
            }
        }
    }
    out
}

/// Lower-cased family names used by `font-family` declarations outside `@font-face`.
pub fn font_families_used(css: &str) -> Vec<String> {
    let s = strip_comments(css);
    let mut without_faces = String::with_capacity(s.len());
    let lower = s.to_ascii_lowercase();
    let mut i = 0;
    while let Some(at) = lower[i..].find("@font-face") {
        without_faces.push_str(&s[i..i + at]);
        i = match lower[i + at..].find('}') {
            Some(close) => i + at + close + 1,
            None => s.len(),
        };
    }
    without_faces.push_str(&s[i..]);
    let lower = without_faces.to_ascii_lowercase();
    let mut families: Vec<String> = Vec::new();
    let mut add = |fam: &str| {
        let fam = unquote(fam.trim());
        if !fam.is_empty() && !families.iter().any(|f| f == fam) {
            families.push(fam.to_string());
        }
    };
    let mut from = 0;
    while let Some(at) = lower[from..].find("font") {
        let start = from + at + "font".len();
        from = start;
        let preceded = lower[..start - 4]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-');
        let (shorthand, rest) = match lower[start..].strip_prefix("-family") {
            Some(rest) => (false, rest),
            None => (true, &lower[start..]),
        };
        if preceded {
            continue;
        }
        let Some(value) = rest.trim_start().strip_prefix(':') else {
            continue;
        };
        let end = value.find([';', '}']).unwrap_or(value.len());
        let value = value[..end].trim().trim_end_matches("!important");
        for (i, fam) in value.split(',').enumerate() {
            if shorthand && i == 0 {
                // `font: italic 20px/1 Some Family, …` — the family follows the size,
                // so any trailing run of words may be it.
                let words: Vec<&str> = fam.split_whitespace().collect();
                for k in 1..words.len() {
                    add(&words[k..].join(" "));
                }
            } else {
                add(fam);
            }
        }
    }
    families
}

fn font_face_blocks(s: &str) -> Vec<&str> {
    let lower = s.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("@font-face") {
        let start = from + at;
        let Some(open) = s[start..].find('{').map(|o| start + o) else {
            break;
        };
        let close = s[open..].find('}').map_or(s.len(), |c| open + c);
        out.push(&s[open + 1..close]);
        from = close.min(s.len());
        if from >= s.len() {
            break;
        }
    }
    out
}

/// The first `url(…)` or string in `s`.
fn first_url(s: &str) -> Option<String> {
    let t = s.trim_start();
    if starts_with_ci(t, "url(") {
        return url_function(&t[4..]).map(|(u, _)| u);
    }
    if t.starts_with(['"', '\'']) {
        return quoted(t).map(|(u, _)| u);
    }
    None
}

fn all_urls(s: &str) -> Vec<String> {
    let lower = s.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("url(") {
        let start = from + at + 4;
        match url_function(&s[start..]) {
            Some((u, used)) => {
                if !u.is_empty() {
                    out.push(u);
                }
                from = start + used;
            }
            None => break,
        }
    }
    out
}

/// Contents of `url(` … `)` (after the opening parenthesis) and bytes consumed.
fn url_function(s: &str) -> Option<(String, usize)> {
    let lead = s.len() - s.trim_start().len();
    let t = &s[lead..];
    if t.starts_with(['"', '\'']) {
        let (u, used) = quoted(t)?;
        let after = &t[used..];
        let close = after.find(')')?;
        return Some((u, lead + used + close + 1));
    }
    let close = t.find(')')?;
    Some((t[..close].trim().to_string(), lead + close + 1))
}

fn quoted(t: &str) -> Option<(String, usize)> {
    let q = t.chars().next()?;
    let end = t[1..].find(q)? + 1;
    Some((t[1..end].to_string(), end + 1))
}

fn unquote(s: &str) -> &str {
    s.trim_matches(|c| c == '"' || c == '\'')
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = match rest[start + 2..].find("*/") {
            Some(end) => &rest[start + 2 + end + 2..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_leading_imports_in_every_syntax() {
        let css = r#"@charset "utf-8";
/* comment */ @import url("a.css"); @import url(b.css) screen; @import 'c.css';
@IMPORT "d.css";
p { color: red; }
@import "ignored-after-rules.css";"#;
        assert_eq!(css_imports(css), ["a.css", "b.css", "c.css", "d.css"]);
        let own = strip_css_imports(css);
        assert!(own.starts_with("p { color: red; }"), "{own}");
    }

    #[test]
    fn extracts_font_faces_and_used_families() {
        let css = r#"
@font-face { font-family: "Axiom Sans"; src: local("x"), url(fonts/axiom.woff2) format("woff2"), url('fonts/axiom.ttf'); }
@font-face { font-family: Unused; src: url(unused.woff); }
@font-face { font-family: NoSource; src: local(Arial); }
@font-face { font-family: "Axiom Sans"; src: url(b.ttf); font-weight: 700; font-style: italic }
body { font-family: 'Axiom Sans', serif; -webkit-font-smoothing: auto; font-size: 2px }
h1 { font-family: Georgia !important }
p { font: italic 20px/1 Ahem, monospace }"#;
        let faces = font_faces(css);
        assert_eq!(faces.len(), 3);
        assert_eq!(faces[0].family, "Axiom Sans");
        assert_eq!(faces[0].sources, ["fonts/axiom.woff2", "fonts/axiom.ttf"]);
        assert!(!faces[0].bold && !faces[0].italic);
        assert!(faces[2].bold && faces[2].italic);
        let used = font_families_used(css);
        assert!(
            used.starts_with(&["axiom sans".into(), "serif".into(), "georgia".into()]),
            "{used:?}"
        );
        assert!(used.contains(&"ahem".to_string()) && used.contains(&"monospace".to_string()));
        assert!(!used.contains(&"unused".to_string()));
    }

    #[test]
    fn absolutizes_relative_urls_against_the_sheet() {
        let css = r#"a { background: url(img/a.png) } b { mask: URL( '../b.svg' ) center }
c { filter: url(#f); background: url("data:image/png;base64,AA==") } d { x: myurl(e) }"#;
        let out = absolutize_urls(css, "https://example.com/css/site.css");
        assert!(
            out.contains(r#"url("https://example.com/css/img/a.png")"#),
            "{out}"
        );
        assert!(
            out.contains(r#"url("https://example.com/b.svg") center"#),
            "{out}"
        );
        assert!(out.contains("url(#f)") && out.contains(r#"url("data:image/png"#));
        assert!(out.contains("myurl(e)"), "{out}");
    }
}
