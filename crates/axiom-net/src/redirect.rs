use crate::request::HttpMethod;
use axiom_url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectMode {
    Follow,
    Error,
    Manual,
}

#[derive(Debug, Clone)]
pub struct RedirectRecord {
    pub from: Url,
    pub to: Url,
    pub status: u16,
    pub method_in: HttpMethod,
    pub method_out: HttpMethod,
    /// `to` is a different origin than `from` (credentials headers were stripped).
    pub cross_origin: bool,
}

pub fn is_redirect_status(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// Method after following a redirect (Fetch "HTTP-redirect fetch", step 12).
pub fn method_after_redirect(status: u16, method: &HttpMethod) -> HttpMethod {
    match status {
        301 | 302 if *method == HttpMethod::Post => HttpMethod::Get,
        303 if *method != HttpMethod::Head => HttpMethod::Get,
        _ => method.clone(),
    }
}

/// Whether the request body (and its content headers) must be dropped.
pub fn drop_body_after_redirect(
    status: u16,
    method_in: &HttpMethod,
    method_out: &HttpMethod,
) -> bool {
    method_in != method_out || status == 303
}

pub fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme == b.scheme
        && a.host.eq_ignore_ascii_case(&b.host)
        && a.effective_port() == b.effective_port()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_matrix() {
        use HttpMethod::*;
        assert_eq!(method_after_redirect(301, &Post), Get);
        assert_eq!(method_after_redirect(302, &Post), Get);
        assert_eq!(method_after_redirect(303, &Post), Get);
        assert_eq!(method_after_redirect(303, &Put), Get);
        assert_eq!(method_after_redirect(303, &Head), Head);
        assert_eq!(method_after_redirect(307, &Post), Post);
        assert_eq!(method_after_redirect(308, &Post), Post);
        assert_eq!(method_after_redirect(301, &Put), Put);
        assert!(drop_body_after_redirect(302, &Post, &Get));
        assert!(!drop_body_after_redirect(307, &Post, &Post));
    }
}
