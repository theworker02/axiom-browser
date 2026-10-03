//! `@media` query and `@supports` condition evaluation.
//!
//! Axiom presents itself as a light-scheme screen with a fine hovering pointer at 1dppx
//! and no reduced-motion preference. Unknown media features evaluate to false, as the
//! Media Queries spec requires.

use crate::values::{parse_length, UnitContext};

#[derive(Debug, Clone, Copy)]
pub struct MediaEnvironment {
    pub width: f32,
    pub height: f32,
}

/// Whether a media query list matches (an empty list matches).
pub fn media_matches(query: &str, env: &MediaEnvironment) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return true;
    }
    crate::values::split_top_level(q, ',')
        .into_iter()
        .any(|one| query_matches(one, env))
}

fn query_matches(q: &str, env: &MediaEnvironment) -> bool {
    let lower = q.trim().to_ascii_lowercase();
    let mut rest = lower.as_str();
    let mut negate = false;
    if let Some(r) = rest.strip_prefix("not ") {
        negate = true;
        rest = r.trim_start();
    } else if let Some(r) = rest.strip_prefix("only ") {
        rest = r.trim_start();
    }
    let result = if rest.starts_with('(') {
        condition(rest, env)
    } else {
        let (ty, cond) = match rest.split_once(" and ") {
            Some((t, c)) => (t.trim(), Some(c)),
            None => (rest.trim(), None),
        };
        let type_ok = matches!(ty, "all" | "screen");
        let known = matches!(
            ty,
            "all"
                | "screen"
                | "print"
                | "speech"
                | "tty"
                | "tv"
                | "projection"
                | "handheld"
                | "braille"
                | "embossed"
                | "aural"
        );
        if !known {
            return false;
        }
        type_ok && cond.is_none_or(|c| condition(c, env))
    };
    result != negate
}

/// `(a) and (b)`, `(a) or (b)`, `not (a)`, nested parentheses.
fn condition(s: &str, env: &MediaEnvironment) -> bool {
    eval_condition(s, &|inner| feature(inner, env)).unwrap_or(false)
}

fn eval_condition(s: &str, leaf: &dyn Fn(&str) -> Option<bool>) -> Option<bool> {
    let s = s.trim();
    if let Some(r) = s.strip_prefix("not ").or_else(|| s.strip_prefix("not(")) {
        let r = if s.starts_with("not(") { &s[3..] } else { r };
        return eval_condition(r, leaf).map(|v| !v);
    }
    let parts = split_groups(s)?;
    let mut acc: Option<bool> = None;
    let mut op: Option<&str> = None;
    for part in parts {
        match part {
            Group::Op(o) => op = Some(o),
            Group::Paren(inner) => {
                let inner_trim = inner.trim();
                let v = if inner_trim.starts_with('(') || inner_trim.starts_with("not ") {
                    eval_condition(inner_trim, leaf)
                } else {
                    leaf(inner_trim)
                }
                .unwrap_or(false);
                acc = Some(match (acc, op) {
                    (None, _) => v,
                    (Some(a), Some("or")) => a || v,
                    (Some(a), _) => a && v,
                });
            }
            Group::Func(name, inner) => {
                let v = leaf(&format!("{name}({inner})")).unwrap_or(false);
                acc = Some(match (acc, op) {
                    (None, _) => v,
                    (Some(a), Some("or")) => a || v,
                    (Some(a), _) => a && v,
                });
            }
        }
    }
    acc
}

enum Group<'a> {
    Paren(&'a str),
    Func(&'a str, &'a str),
    Op(&'a str),
}

fn split_groups(s: &str) -> Option<Vec<Group<'_>>> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let name_start = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-') {
            i += 1;
        }
        let name = &s[name_start..i];
        if i < b.len() && b[i] == b'(' {
            let open = i;
            let mut depth = 0;
            while i < b.len() {
                match b[i] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            let inner = &s[open + 1..i.min(b.len())];
            i += 1;
            if name.is_empty() {
                out.push(Group::Paren(inner));
            } else {
                out.push(Group::Func(name, inner));
            }
        } else if name == "and" || name == "or" {
            out.push(Group::Op(name));
        } else if name.is_empty() {
            return None;
        } else {
            // A bare word where a group is expected.
            return None;
        }
    }
    Some(out)
}

const CX: UnitContext = UnitContext {
    em: 16.0,
    rem: 16.0,
    vw: 0.0,
    vh: 0.0,
    ex: 8.0,
    ch: 8.0,
};

fn feature(f: &str, env: &MediaEnvironment) -> Option<bool> {
    let f = f.trim();
    // Range syntax: `width >= 600px`, `400px <= width <= 700px`.
    if f.contains(['<', '>', '=']) && !f.contains(':') {
        return range_feature(f, env);
    }
    let (name, value) = match f.split_once(':') {
        Some((n, v)) => (n.trim(), Some(v.trim())),
        None => (f, None),
    };
    let len = |v: &str| parse_length(v, &CX).and_then(|l| l.resolve(0.0));
    let ratio = |v: &str| -> Option<f32> {
        match v.split_once('/') {
            Some((a, b)) => Some(a.trim().parse::<f32>().ok()? / b.trim().parse::<f32>().ok()?),
            None => v.parse().ok(),
        }
    };
    let resolution = |v: &str| -> Option<f32> {
        let v = v.trim();
        if let Some(n) = v
            .strip_suffix("dppx")
            .or_else(|| v.strip_suffix('x'))
            .filter(|n| n.parse::<f32>().is_ok())
        {
            n.parse().ok()
        } else if let Some(n) = v.strip_suffix("dpi") {
            n.parse::<f32>().ok().map(|d| d / 96.0)
        } else if let Some(n) = v.strip_suffix("dpcm") {
            n.parse::<f32>().ok().map(|d| d * 2.54 / 96.0)
        } else {
            v.parse().ok()
        }
    };
    let aspect = env.width / env.height.max(1.0);
    Some(match (name, value) {
        ("width", Some(v)) => (env.width - len(v)?).abs() < 0.5,
        ("min-width", Some(v)) => env.width >= len(v)?,
        ("max-width", Some(v)) => env.width <= len(v)?,
        ("height", Some(v)) => (env.height - len(v)?).abs() < 0.5,
        ("min-height", Some(v)) => env.height >= len(v)?,
        ("max-height", Some(v)) => env.height <= len(v)?,
        ("width" | "height", None) => true,
        ("device-width", Some(v)) => (env.width - len(v)?).abs() < 0.5,
        ("min-device-width", Some(v)) => env.width >= len(v)?,
        ("max-device-width", Some(v)) => env.width <= len(v)?,
        ("min-device-height", Some(v)) => env.height >= len(v)?,
        ("max-device-height", Some(v)) => env.height <= len(v)?,
        ("aspect-ratio", Some(v)) => (aspect - ratio(v)?).abs() < 0.01,
        ("min-aspect-ratio", Some(v)) => aspect >= ratio(v)?,
        ("max-aspect-ratio", Some(v)) => aspect <= ratio(v)?,
        ("orientation", Some(v)) => {
            let portrait = env.height >= env.width;
            match v {
                "portrait" => portrait,
                "landscape" => !portrait,
                _ => return None,
            }
        }
        ("prefers-color-scheme", Some(v)) => v == "light",
        ("prefers-reduced-motion", Some(v)) => v == "no-preference",
        ("prefers-reduced-motion", None) => false,
        ("prefers-reduced-transparency", Some(v)) => v == "no-preference",
        ("prefers-contrast", Some(v)) => v == "no-preference",
        ("prefers-contrast", None) => false,
        ("forced-colors", Some(v)) => v == "none",
        ("forced-colors", None) => false,
        ("inverted-colors", Some(v)) => v == "none",
        ("hover" | "any-hover", Some(v)) => v == "hover",
        ("hover" | "any-hover", None) => true,
        ("pointer" | "any-pointer", Some(v)) => v == "fine",
        ("pointer" | "any-pointer", None) => true,
        ("color", None) => true,
        ("color", Some(v)) => v.parse::<u32>().ok()? == 8,
        ("min-color", Some(v)) => v.parse::<u32>().ok()? <= 8,
        ("max-color", Some(v)) => v.parse::<u32>().ok()? >= 8,
        ("monochrome", None) => false,
        ("monochrome", Some(v)) => v.parse::<u32>().ok()? == 0,
        ("min-monochrome", Some(v)) => v.parse::<u32>().ok()? == 0,
        ("max-monochrome", Some(_)) => true,
        ("color-gamut", Some(v)) => v == "srgb",
        ("dynamic-range" | "video-dynamic-range", Some(v)) => v == "standard",
        ("grid", Some(v)) => v == "0",
        ("grid", None) => false,
        ("scan", Some(_)) => false,
        ("update", Some(v)) => v == "fast",
        ("overflow-block", Some(v)) => v == "scroll",
        ("overflow-inline", Some(v)) => v == "scroll",
        ("scripting", Some(v)) => v == "enabled",
        ("display-mode", Some(v)) => v == "browser",
        ("resolution", Some(v)) => (resolution(v)? - 1.0).abs() < 0.01,
        ("min-resolution", Some(v)) => resolution(v)? <= 1.0,
        ("max-resolution", Some(v)) => resolution(v)? >= 1.0,
        ("-webkit-device-pixel-ratio", Some(v)) => v.parse::<f32>().ok()? == 1.0,
        ("-webkit-min-device-pixel-ratio" | "min--moz-device-pixel-ratio", Some(v)) => {
            v.parse::<f32>().ok()? <= 1.0
        }
        ("-webkit-max-device-pixel-ratio" | "max--moz-device-pixel-ratio", Some(v)) => {
            v.parse::<f32>().ok()? >= 1.0
        }
        _ => return None,
    })
}

fn range_feature(f: &str, env: &MediaEnvironment) -> Option<bool> {
    let mut tokens: Vec<(String, &str)> = Vec::new(); // (operand, operator before it)
    let mut rest = f;
    let mut op = "";
    loop {
        let pos = rest.find(['<', '>', '=']);
        let (operand, tail) = match pos {
            Some(p) => (&rest[..p], &rest[p..]),
            None => (rest, ""),
        };
        tokens.push((operand.trim().to_string(), op));
        if tail.is_empty() {
            break;
        }
        let op_len = if tail[1..].starts_with('=') { 2 } else { 1 };
        op = &tail[..op_len];
        rest = &tail[op_len..];
    }
    let value_of = |s: &str| -> Option<f32> {
        match s {
            "width" => Some(env.width),
            "height" => Some(env.height),
            "aspect-ratio" => Some(env.width / env.height.max(1.0)),
            "resolution" => Some(1.0),
            _ => {
                if let Some((a, b)) = s.split_once('/') {
                    return Some(a.trim().parse::<f32>().ok()? / b.trim().parse::<f32>().ok()?);
                }
                if let Some(n) = s.strip_suffix("dppx").or_else(|| s.strip_suffix('x')) {
                    if let Ok(v) = n.parse() {
                        return Some(v);
                    }
                }
                parse_length(s, &CX).and_then(|l| l.resolve(0.0))
            }
        }
    };
    if tokens.len() < 2 {
        return None;
    }
    let mut ok = true;
    for w in tokens.windows(2) {
        let a = value_of(&w[0].0)?;
        let b = value_of(&w[1].0)?;
        ok &= match w[1].1 {
            "<" => a < b,
            "<=" => a <= b,
            ">" => a > b,
            ">=" => a >= b,
            "=" => (a - b).abs() < 0.5,
            _ => return None,
        };
    }
    Some(ok)
}

/// `@supports` condition: declarations are supported when Axiom knows the property;
/// `selector(…)` when the selector parses.
pub fn supports_matches(condition: &str) -> bool {
    let lower = condition.trim().to_string();
    eval_condition(&lower, &|leaf| {
        let leaf = leaf.trim();
        if let Some(sel) = leaf
            .strip_prefix("selector(")
            .and_then(|s| s.strip_suffix(')'))
        {
            return Some(axiom_dom::parse_selector_list(sel).is_some());
        }
        if leaf.starts_with("font-tech(") || leaf.starts_with("font-format(") {
            return Some(false);
        }
        let (prop, value) = leaf.split_once(':')?;
        let prop = prop.trim().to_ascii_lowercase();
        let value = value.trim().to_ascii_lowercase();
        if prop.starts_with("--") {
            return Some(true);
        }
        if !crate::is_supported_property(&prop) {
            return Some(false);
        }
        if matches!(
            value.as_str(),
            "inherit" | "initial" | "unset" | "revert" | "revert-layer"
        ) {
            return Some(true);
        }
        Some(match prop.as_str() {
            "display" => {
                crate::parse_display(&value).is_some()
                    && !matches!(
                        value.as_str(),
                        "ruby" | "ruby-text" | "run-in" | "math" | "-webkit-box" | "-ms-flexbox"
                    )
            }
            _ => !value.is_empty(),
        })
    })
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENV: MediaEnvironment = MediaEnvironment {
        width: 1024.0,
        height: 768.0,
    };

    #[test]
    fn media_queries() {
        assert!(media_matches("", &ENV));
        assert!(media_matches("screen", &ENV));
        assert!(!media_matches("print", &ENV));
        assert!(media_matches("print, screen", &ENV));
        assert!(media_matches("screen and (min-width: 600px)", &ENV));
        assert!(!media_matches("screen and (max-width: 600px)", &ENV));
        assert!(media_matches(
            "(min-width: 40em) and (max-width: 80em)",
            &ENV
        ));
        assert!(media_matches("not print", &ENV));
        assert!(!media_matches("not screen and (min-width: 100px)", &ENV));
        assert!(media_matches("(width >= 600px)", &ENV));
        assert!(media_matches("(400px <= width <= 1100px)", &ENV));
        assert!(!media_matches("(prefers-color-scheme: dark)", &ENV));
        assert!(media_matches("(prefers-color-scheme: light)", &ENV));
        assert!(!media_matches("(unknown-feature: 1)", &ENV));
        assert!(media_matches("(orientation: landscape)", &ENV));
        assert!(media_matches(
            "((min-width: 2000px) or (hover: hover))",
            &ENV
        ));
        assert!(!media_matches("bogus-type", &ENV));
    }

    #[test]
    fn supports_conditions() {
        assert!(supports_matches("(display: grid)"));
        assert!(supports_matches("(display: flex) and (gap: 1rem)"));
        assert!(!supports_matches("(-webkit-nonsense: 1)"));
        assert!(supports_matches("not (-webkit-nonsense: 1)"));
        assert!(supports_matches("selector(:is(a, b))"));
        assert!(supports_matches("(--x: 1)"));
    }
}
