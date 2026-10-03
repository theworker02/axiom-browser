//! Set-Cookie parsing and cookie-date parsing (Wave D).

use crate::cookie_types::{CookieExpiration, CookieSameSite};

/// Parsed Set-Cookie before domain/path/security validation.
#[derive(Debug, Clone)]
pub struct ParsedSetCookie {
    pub name: String,
    pub value: String,
    pub domain: Option<String>,
    pub path: Option<String>,
    pub expires: Option<i64>,
    pub max_age: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<CookieSameSite>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    Empty,
    MissingNameValue,
    InvalidName,
    Oversized,
}

const MAX_HEADER_BYTES: usize = 8 * 1024;

/// Parse a single Set-Cookie header value (not a combined list).
pub fn parse_set_cookie(header: &str) -> Result<ParsedSetCookie, ParseError> {
    if header.is_empty() {
        return Err(ParseError::Empty);
    }
    if header.len() > MAX_HEADER_BYTES {
        return Err(ParseError::Oversized);
    }
    // Reject NULs / most C0 controls in the raw header (except HTAB).
    if header.bytes().any(|b| b < 0x20 && b != b'\t') {
        return Err(ParseError::InvalidName);
    }

    let mut parts = header.split(';');
    let nv = parts.next().ok_or(ParseError::MissingNameValue)?.trim();
    let (name, value) = match nv.split_once('=') {
        Some((n, v)) => (n.trim(), v.trim()),
        None => return Err(ParseError::MissingNameValue),
    };
    if name.is_empty() || !is_valid_cookie_name(name) {
        return Err(ParseError::InvalidName);
    }
    if !is_valid_cookie_value(value) {
        return Err(ParseError::InvalidName);
    }

    let mut out = ParsedSetCookie {
        name: name.to_string(),
        value: value.to_string(),
        domain: None,
        path: None,
        expires: None,
        max_age: None,
        secure: false,
        http_only: false,
        same_site: None,
    };

    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (attr, aval) = match part.split_once('=') {
            Some((a, v)) => (a.trim(), Some(v.trim())),
            None => (part, None),
        };
        let attr_l = attr.to_ascii_lowercase();
        match attr_l.as_str() {
            "domain" => {
                if let Some(v) = aval {
                    let v = v.trim_start_matches('.');
                    if !v.is_empty() {
                        out.domain = Some(v.to_ascii_lowercase());
                    }
                }
            }
            "path" => {
                if let Some(v) = aval {
                    if v.starts_with('/') {
                        out.path = Some(v.to_string());
                    }
                }
            }
            "expires" => {
                if let Some(v) = aval {
                    if let Some(ms) = parse_cookie_date(v) {
                        out.expires = Some(ms);
                    }
                }
            }
            "max-age" => {
                if let Some(v) = aval {
                    out.max_age = parse_max_age(v);
                }
            }
            "secure" => out.secure = true,
            "httponly" => out.http_only = true,
            "samesite" => {
                if let Some(v) = aval {
                    out.same_site = CookieSameSite::parse_attr(v);
                }
            }
            _ => {
                // Unknown attributes ignored safely.
            }
        }
    }

    Ok(out)
}

fn parse_max_age(v: &str) -> Option<i64> {
    let t = v.trim();
    if t.is_empty() {
        return None;
    }
    // Digits with optional leading minus.
    let mut chars = t.chars();
    let first = chars.next()?;
    let rest = if first == '-' { chars.as_str() } else { t };
    if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // Overflow → treat as invalid (ignore Max-Age).
    let n: i64 = t.parse().ok()?;
    Some(n)
}

/// Resolve expiration with Max-Age precedence over Expires.
pub fn resolve_expiration(
    parsed: &ParsedSetCookie,
    now_ms: i64,
) -> (CookieExpiration, bool /* delete */) {
    if let Some(max_age) = parsed.max_age {
        if max_age <= 0 {
            return (CookieExpiration::Absolute(now_ms - 1), true);
        }
        let secs = max_age.min(i64::MAX / 1000);
        let exp = now_ms.saturating_add(secs.saturating_mul(1000));
        return (CookieExpiration::Absolute(exp), false);
    }
    if let Some(exp) = parsed.expires {
        if exp <= now_ms {
            return (CookieExpiration::Absolute(exp), true);
        }
        return (CookieExpiration::Absolute(exp), false);
    }
    (CookieExpiration::Session, false)
}

pub fn is_valid_cookie_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 1024 {
        return false;
    }
    name.bytes().all(|b| {
        // RFC 6265 cookie-name token-ish; reject separators & controls.
        b.is_ascii_graphic()
            && !matches!(
                b,
                b'(' | b')'
                    | b'<'
                    | b'>'
                    | b'@'
                    | b','
                    | b';'
                    | b':'
                    | b'\\'
                    | b'"'
                    | b'/'
                    | b'['
                    | b']'
                    | b'?'
                    | b'='
                    | b'{'
                    | b'}'
                    | b' '
            )
    })
}

pub fn is_valid_cookie_value(value: &str) -> bool {
    if value.len() > 4096 {
        return false;
    }
    !value.bytes().any(|b| b < 0x20 && b != b'\t')
}

/// Cookie-date parsing (HTTP-date / cookie-date variants). Returns UTC epoch ms.
pub fn parse_cookie_date(input: &str) -> Option<i64> {
    let s = input.trim();
    if s.is_empty() || s.len() > 128 {
        return None;
    }
    // Collect alphanumeric tokens; ignore delimiters.
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            cur.push(ch);
        } else if !cur.is_empty() {
            tokens.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    if tokens.is_empty() {
        return None;
    }

    let (hour, minute, second) = extract_time(s).unwrap_or((0, 0, 0));

    let mut day = None;
    let mut month = None;
    let mut year = None;

    for tok in &tokens {
        let lower = tok.to_ascii_lowercase();
        if let Some(m) = month_from_name(&lower) {
            month = Some(m);
            continue;
        }
        if !tok.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let n: u32 = match tok.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        if tok.len() >= 4 {
            if (1601..=30827).contains(&n) {
                year = Some(n);
            }
        } else if tok.len() == 2 || tok.len() == 1 {
            if day.is_none() && (1..=31).contains(&n) {
                day = Some(n);
            } else if year.is_none() && tok.len() == 2 {
                year = Some(if n >= 70 { 1900 + n } else { 2000 + n });
            }
        }
    }

    let day = day?;
    let month = month?;
    let mut year = year?;
    if (70..=99).contains(&year) {
        year += 1900;
    } else if year <= 69 {
        year += 2000;
    }
    if year < 1601 {
        return None;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    datetime_to_unix_ms(year, month, day, hour, minute, second)
}

fn extract_time(s: &str) -> Option<(u32, u32, u32)> {
    // Find pattern digits:digits:digits
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 5 < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b':' {
                let h: u32 = std::str::from_utf8(&bytes[start..i]).ok()?.parse().ok()?;
                i += 1;
                let mstart = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b':' {
                    let m: u32 = std::str::from_utf8(&bytes[mstart..i]).ok()?.parse().ok()?;
                    i += 1;
                    let sstart = i;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    let sec: u32 = std::str::from_utf8(&bytes[sstart..i]).ok()?.parse().ok()?;
                    return Some((h, m, sec));
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

fn month_from_name(s: &str) -> Option<u32> {
    match &s[..s.len().min(3)] {
        "jan" => Some(1),
        "feb" => Some(2),
        "mar" => Some(3),
        "apr" => Some(4),
        "may" => Some(5),
        "jun" => Some(6),
        "jul" => Some(7),
        "aug" => Some(8),
        "sep" => Some(9),
        "oct" => Some(10),
        "nov" => Some(11),
        "dec" => Some(12),
        _ => None,
    }
}

fn datetime_to_unix_ms(year: u32, month: u32, day: u32, h: u32, m: u32, s: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || day == 0 {
        return None;
    }
    let days_in_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let dim = if month == 2 && leap {
        29
    } else {
        days_in_month[(month - 1) as usize]
    };
    if day > dim {
        return None;
    }
    // Days since 1970-01-01
    let mut days: i64 = 0;
    for y in 1970..year {
        days += if y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)) {
            366
        } else {
            365
        };
    }
    for mo in 1..month {
        let leap_feb = mo == 2 && leap;
        days += if leap_feb {
            29
        } else {
            days_in_month[(mo - 1) as usize] as i64
        };
    }
    days += (day - 1) as i64;
    let secs = days * 86400 + (h as i64) * 3600 + (m as i64) * 60 + s as i64;
    Some(secs.saturating_mul(1000))
}

/// Default-path algorithm (RFC 6265 §5.1.4).
pub fn default_path(request_path: &str) -> String {
    if !request_path.starts_with('/') {
        return "/".into();
    }
    if request_path == "/" {
        return "/".into();
    }
    match request_path.rfind('/') {
        Some(0) => "/".into(),
        Some(i) => request_path[..i].to_string(),
        None => "/".into(),
    }
}

/// Path-matches (RFC 6265 §5.1.4).
pub fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if request_path.starts_with(cookie_path) {
        if cookie_path.ends_with('/') {
            return true;
        }
        if request_path.as_bytes().get(cookie_path.len()) == Some(&b'/') {
            return true;
        }
    }
    false
}

/// Domain-matches request-host (cookie-domain already normalized, no leading dot).
pub fn domain_matches(host: &str, cookie_domain: &str, host_only: bool) -> bool {
    let host = host.to_ascii_lowercase();
    let domain = cookie_domain.to_ascii_lowercase();
    if host_only {
        return host == domain;
    }
    if host == domain {
        return true;
    }
    if host.len() > domain.len()
        && host.ends_with(&domain)
        && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        let p = parse_set_cookie("a=b; Path=/; HttpOnly; Secure; SameSite=Lax").unwrap();
        assert_eq!(p.name, "a");
        assert_eq!(p.value, "b");
        assert!(p.http_only);
        assert!(p.secure);
        assert_eq!(p.same_site, Some(CookieSameSite::Lax));
        assert_eq!(p.path.as_deref(), Some("/"));
    }

    #[test]
    fn max_age_zero_deletes() {
        let p = parse_set_cookie("x=y; Max-Age=0").unwrap();
        let (exp, del) = resolve_expiration(&p, 1_000_000);
        assert!(del);
        assert!(matches!(exp, CookieExpiration::Absolute(_)));
    }

    #[test]
    fn max_age_precedes_expires() {
        let p = parse_set_cookie("x=y; Max-Age=10; Expires=Wed, 09 Jun 2021 10:18:14 GMT").unwrap();
        let (exp, del) = resolve_expiration(&p, 0);
        assert!(!del);
        assert_eq!(exp, CookieExpiration::Absolute(10_000));
    }

    #[test]
    fn cookie_date_imf_fix() {
        let ms = parse_cookie_date("Wed, 09 Jun 2021 10:18:14 GMT").unwrap();
        assert!(ms > 1_600_000_000_000);
    }

    #[test]
    fn path_match_prefix() {
        assert!(path_matches("/account/profile", "/account"));
        assert!(!path_matches("/images/", "/account"));
        assert!(path_matches("/account", "/account"));
        assert!(path_matches("/", "/"));
    }

    #[test]
    fn hostile_empty_and_control() {
        assert!(parse_set_cookie("").is_err());
        assert!(parse_set_cookie("=novalue").is_err());
        assert!(parse_set_cookie("a=b\0c").is_err());
    }

    #[test]
    fn default_path_dir() {
        assert_eq!(default_path("/foo/bar.html"), "/foo");
        assert_eq!(default_path("/"), "/");
    }
}
