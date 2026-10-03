use axiom_url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReferrerPolicy {
    NoReferrer,
    NoReferrerWhenDowngrade,
    Origin,
    OriginWhenCrossOrigin,
    SameOrigin,
    StrictOrigin,
    #[default]
    StrictOriginWhenCrossOrigin,
    UnsafeUrl,
}

impl ReferrerPolicy {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "no-referrer" => Self::NoReferrer,
            "no-referrer-when-downgrade" => Self::NoReferrerWhenDowngrade,
            "origin" => Self::Origin,
            "origin-when-cross-origin" => Self::OriginWhenCrossOrigin,
            "same-origin" => Self::SameOrigin,
            "strict-origin" => Self::StrictOrigin,
            "strict-origin-when-cross-origin" => Self::StrictOriginWhenCrossOrigin,
            "unsafe-url" => Self::UnsafeUrl,
            _ => return None,
        })
    }
}

/// Referrer header value for `request_url` (W3C Referrer Policy §8.3).
pub fn compute_referrer(
    policy: ReferrerPolicy,
    referrer: Option<&str>,
    request_url: &Url,
) -> Option<String> {
    let ref_url = Url::parse(referrer?).ok()?;
    let full = referrer_url_string(&ref_url);
    let origin = origin_string(&ref_url);
    let same = crate::redirect::same_origin(&ref_url, request_url);
    let downgrade =
        is_potentially_trustworthy(&ref_url) && !is_potentially_trustworthy(request_url);
    match policy {
        ReferrerPolicy::NoReferrer => None,
        ReferrerPolicy::NoReferrerWhenDowngrade => (!downgrade).then_some(full),
        ReferrerPolicy::Origin => Some(origin),
        ReferrerPolicy::OriginWhenCrossOrigin => Some(if same { full } else { origin }),
        ReferrerPolicy::SameOrigin => same.then_some(full),
        ReferrerPolicy::StrictOrigin => (!downgrade).then_some(origin),
        ReferrerPolicy::StrictOriginWhenCrossOrigin => {
            if same {
                Some(full)
            } else if !downgrade {
                Some(origin)
            } else {
                None
            }
        }
        ReferrerPolicy::UnsafeUrl => Some(full),
    }
}

/// Serialized origin with trailing slash, keeping non-default ports.
fn origin_string(u: &Url) -> String {
    match u.port {
        Some(p) if p != u.default_port() => format!("{}://{}:{}/", u.scheme, u.host, p),
        _ => format!("{}://{}/", u.scheme, u.host),
    }
}

/// Full referrer URL: fragment stripped (credentials are not representable in `Url`).
fn referrer_url_string(u: &Url) -> String {
    let mut u = u.clone();
    u.fragment = None;
    u.as_str()
}

/// "Potentially trustworthy URL" (secure contexts / mixed content): HTTPS or loopback.
pub fn is_potentially_trustworthy(u: &Url) -> bool {
    u.scheme == "https" || u.host == "localhost" || u.host == "127.0.0.1" || u.host == "[::1]"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn origin_keeps_port_and_strips_fragment() {
        let r = compute_referrer(
            ReferrerPolicy::StrictOriginWhenCrossOrigin,
            Some("http://a.test:8080/page?q#frag"),
            &url("http://b.test/"),
        );
        assert_eq!(r.as_deref(), Some("http://a.test:8080/"));
        let same = compute_referrer(
            ReferrerPolicy::StrictOriginWhenCrossOrigin,
            Some("http://a.test:8080/page?q#frag"),
            &url("http://a.test:8080/x"),
        );
        assert_eq!(same.as_deref(), Some("http://a.test:8080/page?q"));
    }

    #[test]
    fn downgrade_sends_nothing() {
        let r = compute_referrer(
            ReferrerPolicy::StrictOriginWhenCrossOrigin,
            Some("https://a.test/page"),
            &url("http://b.test/"),
        );
        assert_eq!(r, None);
        assert_eq!(
            compute_referrer(
                ReferrerPolicy::NoReferrer,
                Some("https://a.test/"),
                &url("https://a.test/")
            ),
            None
        );
    }
}
