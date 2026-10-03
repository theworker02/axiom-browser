//! Cookie jar bridge for networking and document.cookie (implemented by browser).

/// Profile-scoped cookie access used by `BrowsingContext` and `DocumentJsHost`.
///
/// Networking and JS must never touch SQLite; they go through this facade.
pub trait CookieJar: Send + Sync {
    fn cookie_header_for_request(&self, url: &str) -> Option<String>;
    fn store_set_cookies(&self, url: &str, set_cookie_headers: &[String]);
    fn cookies_for_script(&self, document_url: &str) -> String;
    fn set_cookie_from_script(&self, document_url: &str, cookie_string: &str);
}
