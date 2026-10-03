//! Bridge between engine networking/JS and the profile CookieService.

use std::sync::{Arc, Weak};

use axiom_net::{CookieProvider, CookieRequestContext};

use crate::cookie_service::CookieService;
use crate::cookie_types::{
    CookieAccessContext, CookieSource, HttpMethodKind, NavigationKind, Site,
};
use crate::store::BrowserDataStore;

/// Profile-scoped jar shared by all tabs of a Browser.
///
/// Holds a [`Weak`] to the store so dropping the [`Profile`] / last strong
/// [`Arc`] releases the profile lock (important for reopen tests).
pub struct ProfileCookieJar {
    store: Weak<BrowserDataStore>,
    service: CookieService,
}

impl ProfileCookieJar {
    pub fn new(store: Arc<BrowserDataStore>) -> Arc<Self> {
        Arc::new(Self {
            store: Arc::downgrade(&store),
            service: CookieService::new(),
        })
    }

    fn upgrade(&self) -> Option<Arc<BrowserDataStore>> {
        self.store.upgrade()
    }

    pub fn service(&self) -> &CookieService {
        &self.service
    }

    /// Map a network-layer request into the CookieService access model, so
    /// SameSite/Secure decisions see the real top-level site, navigation kind
    /// and method for every hop (including redirects).
    pub fn access_context(ctx: &CookieRequestContext<'_>) -> CookieAccessContext {
        let top_level_site = Site::from_url(ctx.top_level_url);
        CookieAccessContext {
            request_url: ctx.url.as_str(),
            initiator_site: Some(
                ctx.initiator
                    .map_or_else(|| top_level_site.clone(), Site::from_url),
            ),
            top_level_site,
            navigation: if ctx.is_top_level_navigation {
                NavigationKind::TopLevel
            } else {
                NavigationKind::Subresource
            },
            method: HttpMethodKind::from_method(ctx.method.as_str()),
            secure_context: ctx.url.scheme == "https",
            source: CookieSource::Network,
        }
    }
}

impl CookieProvider for ProfileCookieJar {
    fn cookie_header(&self, ctx: &CookieRequestContext<'_>) -> Option<String> {
        let store = self.upgrade()?;
        self.service
            .cookie_header_for_request(&store, &Self::access_context(ctx))
            .ok()
            .flatten()
    }

    fn store_set_cookies(&self, ctx: &CookieRequestContext<'_>, set_cookie_headers: &[String]) {
        let Some(store) = self.upgrade() else {
            return;
        };
        let _ = self.service.process_set_cookies(
            &store,
            &ctx.url.as_str(),
            set_cookie_headers,
            CookieSource::Network,
        );
    }
}

impl axiom_engine::CookieJar for ProfileCookieJar {
    fn cookie_header_for_request(&self, url: &str) -> Option<String> {
        let store = self.upgrade()?;
        let ctx = CookieService::default_navigation_context(url);
        self.service
            .cookie_header_for_request(&store, &ctx)
            .ok()
            .flatten()
    }

    fn store_set_cookies(&self, url: &str, set_cookie_headers: &[String]) {
        let Some(store) = self.upgrade() else {
            return;
        };
        let _ = self.service.process_set_cookies(
            &store,
            url,
            set_cookie_headers,
            CookieSource::Network,
        );
    }

    fn cookies_for_script(&self, document_url: &str) -> String {
        let Some(store) = self.upgrade() else {
            return String::new();
        };
        self.service
            .cookies_for_script(&store, document_url)
            .unwrap_or_default()
    }

    fn set_cookie_from_script(&self, document_url: &str, cookie_string: &str) {
        let Some(store) = self.upgrade() else {
            return;
        };
        let _ = self
            .service
            .set_cookie_from_script(&store, document_url, cookie_string);
    }
}
