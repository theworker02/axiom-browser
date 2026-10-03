//! Search providers for omnibox queries.
//!
//! The omnibox never builds a search URL itself: it asks
//! [`SearchProviderService::search_url`], which uses the profile's selected provider.
//! Templates use the OpenSearch `{searchTerms}` placeholder.

use std::fmt;

/// Placeholder replaced by the form-encoded query in provider templates.
pub const SEARCH_TERMS: &str = "{searchTerms}";

pub const GOOGLE: &str = "google";
pub const BING: &str = "bing";
pub const DUCKDUCKGO: &str = "duckduckgo";
pub const AXIOM_SEARCH: &str = "axiom";
pub const CUSTOM: &str = "custom";

/// Provider used when a profile has no (valid) preference.
pub const DEFAULT_PROVIDER_ID: &str = DUCKDUCKGO;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchProvider {
    /// Stable id persisted in profile settings.
    pub id: String,
    pub name: String,
    /// Result page template with a `{searchTerms}` placeholder.
    pub search_url_template: String,
    /// Suggestion endpoint template. Recorded only: Axiom does not send keystrokes to a
    /// provider yet (suggestions come from local history and bookmarks).
    pub suggest_url_template: Option<String>,
    /// Icon URL for future chrome; never fetched by this wave.
    pub icon_url: Option<String>,
}

impl SearchProvider {
    pub fn google() -> Self {
        Self {
            id: GOOGLE.into(),
            name: "Google".into(),
            search_url_template: "https://www.google.com/search?q={searchTerms}".into(),
            suggest_url_template: Some(
                "https://suggestqueries.google.com/complete/search?client=firefox&q={searchTerms}"
                    .into(),
            ),
            icon_url: Some("https://www.google.com/favicon.ico".into()),
        }
    }

    pub fn bing() -> Self {
        Self {
            id: BING.into(),
            name: "Bing".into(),
            search_url_template: "https://www.bing.com/search?q={searchTerms}".into(),
            suggest_url_template: Some(
                "https://api.bing.com/osjson.aspx?query={searchTerms}".into(),
            ),
            icon_url: Some("https://www.bing.com/favicon.ico".into()),
        }
    }

    pub fn duckduckgo() -> Self {
        Self {
            id: DUCKDUCKGO.into(),
            name: "DuckDuckGo".into(),
            search_url_template: "https://duckduckgo.com/?q={searchTerms}".into(),
            suggest_url_template: Some(
                "https://duckduckgo.com/ac/?q={searchTerms}&type=list".into(),
            ),
            icon_url: Some("https://duckduckgo.com/favicon.ico".into()),
        }
    }

    /// Placeholder for Axiom's own engine (see `docs/AXIOM_SEARCH_ARCHITECTURE.md`). Its
    /// result page is a trusted internal page until the query service exists.
    pub fn axiom_search() -> Self {
        Self {
            id: AXIOM_SEARCH.into(),
            name: "Axiom Search".into(),
            search_url_template: "axiom://search?q={searchTerms}".into(),
            suggest_url_template: None,
            icon_url: None,
        }
    }

    /// A user-supplied template; `None` unless it is an `http(s)` URL with the placeholder.
    pub fn custom(name: &str, template: &str) -> Option<Self> {
        let t = template.trim();
        let lower = t.to_ascii_lowercase();
        let http = lower.starts_with("https://") || lower.starts_with("http://");
        if !http || !t.contains(SEARCH_TERMS) {
            return None;
        }
        axiom_url::Url::parse(&t.replace(SEARCH_TERMS, "x")).ok()?;
        Some(Self {
            id: CUSTOM.into(),
            name: if name.trim().is_empty() {
                "Custom".into()
            } else {
                name.trim().into()
            },
            search_url_template: t.into(),
            suggest_url_template: None,
            icon_url: None,
        })
    }

    /// Result page URL for `query` (form-encoded, so spaces become `+`).
    pub fn url_for_query(&self, query: &str) -> String {
        fill(&self.search_url_template, query)
    }

    pub fn suggest_url_for_query(&self, query: &str) -> Option<String> {
        self.suggest_url_template.as_deref().map(|t| fill(t, query))
    }

    pub fn is_builtin(&self) -> bool {
        self.id != CUSTOM
    }
}

fn fill(template: &str, query: &str) -> String {
    template.replace(SEARCH_TERMS, &urlencoding_encode(query))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownSearchProvider(pub String);

impl fmt::Display for UnknownSearchProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown search provider: {}", self.0)
    }
}

impl std::error::Error for UnknownSearchProvider {}

/// The providers a profile can choose from and its current choice.
#[derive(Debug, Clone)]
pub struct SearchProviderService {
    providers: Vec<SearchProvider>,
    selected: usize,
}

impl Default for SearchProviderService {
    fn default() -> Self {
        Self::builtin()
    }
}

impl SearchProviderService {
    /// Google, Bing, DuckDuckGo and Axiom Search, with [`DEFAULT_PROVIDER_ID`] selected.
    pub fn builtin() -> Self {
        let providers = vec![
            SearchProvider::google(),
            SearchProvider::bing(),
            SearchProvider::duckduckgo(),
            SearchProvider::axiom_search(),
        ];
        let selected = providers
            .iter()
            .position(|p| p.id == DEFAULT_PROVIDER_ID)
            .unwrap_or(0);
        Self {
            providers,
            selected,
        }
    }

    /// Built-ins plus an optional custom provider, with `id` selected when it exists.
    pub fn with_selection(id: &str, custom: Option<SearchProvider>) -> Self {
        let mut service = Self::builtin();
        if let Some(c) = custom {
            service.providers.push(c);
        }
        let _ = service.select(id);
        service
    }

    pub fn providers(&self) -> &[SearchProvider] {
        &self.providers
    }

    pub fn get(&self, id: &str) -> Option<&SearchProvider> {
        self.providers.iter().find(|p| p.id == id)
    }

    pub fn default_provider(&self) -> &SearchProvider {
        &self.providers[self.selected]
    }

    pub fn select(&mut self, id: &str) -> Result<&SearchProvider, UnknownSearchProvider> {
        let i = self
            .providers
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| UnknownSearchProvider(id.to_string()))?;
        self.selected = i;
        Ok(&self.providers[i])
    }

    /// Result page URL for an omnibox search with the selected provider.
    pub fn search_url(&self, query: &str) -> String {
        self.default_provider().url_for_query(query)
    }

    pub fn suggest_url(&self, query: &str) -> Option<String> {
        self.default_provider().suggest_url_for_query(query)
    }
}

/// `application/x-www-form-urlencoded` byte encoding (UTF-8; space as `+`).
pub fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                out.push(*b as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(hex((*b >> 4) & 0xf));
                out.push(hex(*b & 0xf));
            }
        }
    }
    out
}

fn hex(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'A' + (n - 10)) as char,
        _ => '0',
    }
}

/// Decode a form-encoded query value (`+` → space, `%XX` → byte; invalid UTF-8 replaced).
pub fn urlencoding_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let h = std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok());
                match h {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Value of `key` in a URL's query string, form-decoded.
pub fn query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    let query = query.split('#').next().unwrap_or(query);
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (urlencoding_decode(k) == key).then(|| urlencoding_decode(v))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_search_query() {
        let p = SearchProvider::duckduckgo();
        let u = p.url_for_query("rust browser");
        assert_eq!(u, "https://duckduckgo.com/?q=rust+browser");
        let u2 = p.url_for_query("a&b=c");
        assert!(u2.contains("a%26b%3Dc"));
    }

    #[test]
    fn google_query_is_form_encoded() {
        let g = SearchProvider::google();
        assert_eq!(
            g.url_for_query("rust programming language"),
            "https://www.google.com/search?q=rust+programming+language"
        );
        assert_eq!(
            g.url_for_query("\"how does rust ownership work\""),
            "https://www.google.com/search?q=%22how+does+rust+ownership+work%22"
        );
        assert_eq!(
            g.url_for_query("c++ & rust?"),
            "https://www.google.com/search?q=c%2B%2B+%26+rust%3F"
        );
        assert_eq!(
            g.url_for_query("München"),
            "https://www.google.com/search?q=M%C3%BCnchen"
        );
    }

    #[test]
    fn service_selects_providers_by_id() {
        let mut s = SearchProviderService::builtin();
        assert_eq!(s.default_provider().id, DEFAULT_PROVIDER_ID);
        let ids: Vec<&str> = s.providers().iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, [GOOGLE, BING, DUCKDUCKGO, AXIOM_SEARCH]);
        s.select(GOOGLE).unwrap();
        assert_eq!(s.search_url("a b"), "https://www.google.com/search?q=a+b");
        assert!(s.suggest_url("a").unwrap().contains("q=a"));
        assert_eq!(s.select("nope"), Err(UnknownSearchProvider("nope".into())));
        assert_eq!(
            s.default_provider().id,
            GOOGLE,
            "failed select keeps choice"
        );
        s.select(AXIOM_SEARCH).unwrap();
        assert_eq!(s.search_url("x y"), "axiom://search?q=x+y");
    }

    #[test]
    fn custom_templates_must_be_http_with_placeholder() {
        assert!(SearchProvider::custom("Mine", "https://s.example/?q={searchTerms}").is_some());
        assert!(SearchProvider::custom("Bad", "https://s.example/?q=").is_none());
        assert!(SearchProvider::custom("Bad", "javascript:{searchTerms}").is_none());
        assert!(SearchProvider::custom("Bad", "axiom://x?q={searchTerms}").is_none());
    }

    #[test]
    fn query_params_round_trip() {
        let url = SearchProvider::axiom_search().url_for_query("a+b & c/ü");
        assert_eq!(query_param(&url, "q").as_deref(), Some("a+b & c/ü"));
        assert_eq!(query_param("axiom://x?p=1#q=2", "q"), None);
        assert_eq!(
            query_param("axiom://x?bad=%zz", "bad").as_deref(),
            Some("%zz")
        );
    }
}
