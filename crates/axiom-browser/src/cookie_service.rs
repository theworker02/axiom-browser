//! Central CookieService — single cookie model for HTTP + JS (Wave D).

use std::sync::Arc;

use axiom_url::Url;

use crate::cookie_parse::{
    default_path, domain_matches, is_valid_cookie_name, is_valid_cookie_value, parse_set_cookie,
    path_matches, resolve_expiration, ParseError,
};
use crate::cookie_psl::{PslPublicSuffixProvider, PublicSuffixProvider};
use crate::cookie_repo::CookieRepository;
use crate::cookie_types::{
    Cookie, CookieAccessContext, CookieExpiration, CookieId, CookieInspectRecord, CookiePriority,
    CookieSameSite, CookieSource, HttpMethodKind, NavigationKind, Site,
};
use crate::store::{BrowserDataStore, StoreError};
use crate::time_util;

/// Injectable clock for deterministic tests.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        time_util::now_ms()
    }
}

#[derive(Debug, Clone)]
pub struct FixedClock {
    pub ms: i64,
}

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.ms
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookiePolicy {
    AllowAll,
    BlockAll,
}

#[derive(Debug, Clone)]
pub struct CookieLimits {
    pub max_name_bytes: usize,
    pub max_value_bytes: usize,
    pub max_cookie_bytes: usize,
    pub max_per_domain: usize,
    pub max_total: usize,
}

impl Default for CookieLimits {
    fn default() -> Self {
        Self {
            max_name_bytes: 1024,
            max_value_bytes: 4096,
            max_cookie_bytes: 4096,
            max_per_domain: 180,
            max_total: 3000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CookieRejectReason {
    PolicyBlocked,
    Parse(ParseError),
    InvalidDomain,
    PublicSuffix,
    DomainMismatch,
    InvalidPath,
    SecureRequired,
    PrefixViolation,
    HttpOnlyProtected,
    Oversized,
    NonSecureContext,
}

impl std::fmt::Display for CookieRejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PolicyBlocked => write!(f, "policy_blocked"),
            Self::Parse(e) => write!(f, "parse:{e:?}"),
            Self::InvalidDomain => write!(f, "invalid_domain"),
            Self::PublicSuffix => write!(f, "public_suffix"),
            Self::DomainMismatch => write!(f, "domain_mismatch"),
            Self::InvalidPath => write!(f, "invalid_path"),
            Self::SecureRequired => write!(f, "secure_required"),
            Self::PrefixViolation => write!(f, "prefix_violation"),
            Self::HttpOnlyProtected => write!(f, "httponly_protected"),
            Self::Oversized => write!(f, "oversized"),
            Self::NonSecureContext => write!(f, "non_secure_context"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessResult {
    Accepted,
    Deleted,
    Rejected(CookieRejectReason),
}

pub struct CookieService {
    psl: Arc<dyn PublicSuffixProvider>,
    clock: Arc<dyn Clock>,
    policy: CookiePolicy,
    limits: CookieLimits,
}

impl Default for CookieService {
    fn default() -> Self {
        Self::new()
    }
}

impl CookieService {
    pub fn new() -> Self {
        Self {
            psl: Arc::new(PslPublicSuffixProvider),
            clock: Arc::new(SystemClock),
            policy: CookiePolicy::AllowAll,
            limits: CookieLimits::default(),
        }
    }

    pub fn with_parts(
        psl: Arc<dyn PublicSuffixProvider>,
        clock: Arc<dyn Clock>,
        policy: CookiePolicy,
        limits: CookieLimits,
    ) -> Self {
        Self {
            psl,
            clock,
            policy,
            limits,
        }
    }

    pub fn set_policy(&mut self, policy: CookiePolicy) {
        self.policy = policy;
    }

    pub fn set_clock(&mut self, clock: Arc<dyn Clock>) {
        self.clock = clock;
    }

    pub fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    pub fn process_set_cookie(
        &self,
        store: &BrowserDataStore,
        request_url: &str,
        header: &str,
        source: CookieSource,
    ) -> Result<ProcessResult, StoreError> {
        if self.policy == CookiePolicy::BlockAll {
            log::info!(target: "axiom_cookies", "cookie rejected reason=policy_blocked");
            return Ok(ProcessResult::Rejected(CookieRejectReason::PolicyBlocked));
        }

        let parsed = match parse_set_cookie(header) {
            Ok(p) => p,
            Err(e) => {
                log::info!(target: "axiom_cookies", "cookie rejected reason=parse");
                return Ok(ProcessResult::Rejected(CookieRejectReason::Parse(e)));
            }
        };

        let url = match Url::parse(request_url) {
            Ok(u) => u,
            Err(_) => {
                return Ok(ProcessResult::Rejected(CookieRejectReason::InvalidDomain));
            }
        };
        let host = url.host.to_ascii_lowercase();
        let secure_ctx = url.scheme == "https";
        let now = self.clock.now_ms();

        if parsed.name.len() > self.limits.max_name_bytes
            || parsed.value.len() > self.limits.max_value_bytes
            || parsed.name.len() + parsed.value.len() > self.limits.max_cookie_bytes
        {
            log::info!(target: "axiom_cookies", "cookie rejected reason=oversized");
            return Ok(ProcessResult::Rejected(CookieRejectReason::Oversized));
        }

        // Domain attribute
        let (domain, host_only) = if let Some(ref d) = parsed.domain {
            if self.psl.is_public_suffix(d) {
                log::info!(target: "axiom_cookies", "cookie rejected reason=public_suffix");
                return Ok(ProcessResult::Rejected(CookieRejectReason::PublicSuffix));
            }
            if !domain_matches(&host, d, false) {
                log::info!(target: "axiom_cookies", "cookie rejected reason=domain_mismatch");
                return Ok(ProcessResult::Rejected(CookieRejectReason::DomainMismatch));
            }
            (d.clone(), false)
        } else {
            (host.clone(), true)
        };

        let path = parsed
            .path
            .clone()
            .unwrap_or_else(|| default_path(&url.path));

        let mut secure = parsed.secure;
        let http_only = parsed.http_only;
        let same_site = parsed.same_site.unwrap_or(CookieSameSite::Lax);

        // Prefixes
        if let Err(reason) = validate_prefixes(&parsed.name, secure, host_only, &path, secure_ctx) {
            log::info!(target: "axiom_cookies", "cookie rejected reason={reason}");
            return Ok(ProcessResult::Rejected(reason));
        }
        if parsed.name.starts_with("__Secure-") || parsed.name.starts_with("__Host-") {
            secure = true;
        }

        // SameSite=None requires Secure (modern behavior)
        if same_site == CookieSameSite::None && !secure {
            log::info!(target: "axiom_cookies", "cookie rejected reason=secure_required");
            return Ok(ProcessResult::Rejected(CookieRejectReason::SecureRequired));
        }

        // Secure attribute from non-secure origin
        if secure && !secure_ctx {
            log::info!(target: "axiom_cookies", "cookie rejected reason=non_secure_context");
            return Ok(ProcessResult::Rejected(
                CookieRejectReason::NonSecureContext,
            ));
        }

        // Script cannot set HttpOnly; cannot overwrite HttpOnly via script
        if source == CookieSource::Script {
            if http_only {
                return Ok(ProcessResult::Rejected(
                    CookieRejectReason::HttpOnlyProtected,
                ));
            }
            let existing = store.with_conn(|c| {
                CookieRepository::new(c).get_by_identity(&parsed.name, &domain, &path)
            })?;
            if let Some(ex) = existing {
                if ex.http_only {
                    log::info!(target: "axiom_cookies", "cookie rejected reason=httponly_protected");
                    return Ok(ProcessResult::Rejected(
                        CookieRejectReason::HttpOnlyProtected,
                    ));
                }
            }
        }

        let (expiration, delete) = resolve_expiration(&parsed, now);
        if delete {
            store.with_conn(|c| {
                CookieRepository::new(c).delete_identity(&parsed.name, &domain, &path)?;
                Ok(())
            })?;
            log::info!(target: "axiom_cookies", "cookie expired/deleted name_len={}", parsed.name.len());
            return Ok(ProcessResult::Deleted);
        }

        let persistent = matches!(expiration, CookieExpiration::Absolute(_));
        let creation = store
            .with_conn(|c| {
                Ok(CookieRepository::new(c)
                    .get_by_identity(&parsed.name, &domain, &path)?
                    .map(|c| c.creation_time_ms)
                    .unwrap_or(now))
            })
            .unwrap_or(now);

        let cookie = Cookie {
            id: CookieId::new(),
            name: parsed.name,
            value: parsed.value,
            domain: domain.clone(),
            path,
            creation_time_ms: creation,
            last_access_time_ms: now,
            expiration,
            secure,
            http_only,
            same_site,
            host_only,
            persistent,
            priority: CookiePriority::Medium,
            source,
        };

        store.with_conn(|c| {
            let repo = CookieRepository::new(c);
            // Limits / eviction
            let total = repo.count()? as usize;
            if total >= self.limits.max_total {
                let _ = repo.evict_oldest_global()?;
                log::info!(target: "axiom_cookies", "cookie evicted reason=total_limit");
            }
            let per = repo.count_for_domain(&domain)? as usize;
            if per >= self.limits.max_per_domain {
                let _ = repo.evict_oldest_for_domain(&domain)?;
                log::info!(target: "axiom_cookies", "cookie evicted reason=domain_limit");
            }
            repo.upsert(&cookie)?;
            Ok(())
        })?;

        log::info!(
            target: "axiom_cookies",
            "cookie accepted domain={} path={} secure={} httponly={} samesite={}",
            cookie.domain,
            cookie.path,
            cookie.secure,
            cookie.http_only,
            cookie.same_site.as_str()
        );
        Ok(ProcessResult::Accepted)
    }

    pub fn process_set_cookies(
        &self,
        store: &BrowserDataStore,
        request_url: &str,
        headers: &[String],
        source: CookieSource,
    ) -> Result<Vec<ProcessResult>, StoreError> {
        let mut out = Vec::with_capacity(headers.len());
        for h in headers {
            out.push(self.process_set_cookie(store, request_url, h, source)?);
        }
        Ok(out)
    }

    pub fn cookies_for_request(
        &self,
        store: &BrowserDataStore,
        ctx: &CookieAccessContext,
    ) -> Result<Vec<Cookie>, StoreError> {
        if self.policy == CookiePolicy::BlockAll {
            return Ok(Vec::new());
        }
        let url = match Url::parse(&ctx.request_url) {
            Ok(u) => u,
            Err(_) => return Ok(Vec::new()),
        };
        let host = url.host.to_ascii_lowercase();
        let path = if url.path.is_empty() {
            "/".to_string()
        } else {
            url.path.clone()
        };
        let now = self.clock.now_ms();
        let secure_req = url.scheme == "https";

        // Lazy expiration cleanup (bounded)
        store.with_conn(|c| {
            let n = CookieRepository::new(c).delete_expired(now)?;
            if n > 0 {
                log::info!(target: "axiom_cookies", "cookie expired cleaned={n}");
            }
            Ok(())
        })?;

        let mut candidates =
            store.with_conn(|c| CookieRepository::new(c).candidates_for_host(&host))?;
        candidates.retain(|c| {
            if c.expiration.is_expired(now) {
                return false;
            }
            if !domain_matches(&host, &c.domain, c.host_only) {
                return false;
            }
            if !path_matches(&path, &c.path) {
                return false;
            }
            if c.secure && !secure_req {
                return false;
            }
            if !same_site_allows(c.same_site, ctx, &self.psl) {
                return false;
            }
            true
        });

        // Deterministic order: longer path first, then earlier creation
        candidates.sort_by(|a, b| {
            b.path
                .len()
                .cmp(&a.path.len())
                .then(a.creation_time_ms.cmp(&b.creation_time_ms))
        });

        // Touch access times (best-effort)
        let _ = store.with_conn(|c| {
            let repo = CookieRepository::new(c);
            for ck in &candidates {
                let _ = repo.touch_access(&ck.id.0.to_string(), now);
            }
            Ok(())
        });

        Ok(candidates)
    }

    pub fn cookie_header_for_request(
        &self,
        store: &BrowserDataStore,
        ctx: &CookieAccessContext,
    ) -> Result<Option<String>, StoreError> {
        let cookies = self.cookies_for_request(store, ctx)?;
        if cookies.is_empty() {
            return Ok(None);
        }
        let header = cookies
            .iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; ");
        Ok(Some(header))
    }

    pub fn cookies_for_script(
        &self,
        store: &BrowserDataStore,
        document_url: &str,
    ) -> Result<String, StoreError> {
        let site = Site::from_url_str(document_url).unwrap_or(Site {
            scheme: "null".into(),
            host: String::new(),
        });
        let ctx = CookieAccessContext {
            request_url: document_url.to_string(),
            top_level_site: site.clone(),
            initiator_site: Some(site),
            navigation: NavigationKind::TopLevel,
            method: HttpMethodKind::SafeGet,
            secure_context: document_url.starts_with("https://"),
            source: CookieSource::Script,
        };
        let mut cookies = self.cookies_for_request(store, &ctx)?;
        cookies.retain(|c| !c.http_only);
        Ok(cookies
            .iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; "))
    }

    pub fn set_cookie_from_script(
        &self,
        store: &BrowserDataStore,
        document_url: &str,
        cookie_string: &str,
    ) -> Result<ProcessResult, StoreError> {
        self.process_set_cookie(store, document_url, cookie_string, CookieSource::Script)
    }

    pub fn cleanup_expired(&self, store: &BrowserDataStore) -> Result<u64, StoreError> {
        let now = self.clock.now_ms();
        store.with_conn(|c| CookieRepository::new(c).delete_expired(now))
    }

    pub fn clear(&self, store: &BrowserDataStore) -> Result<(), StoreError> {
        store.with_conn(|c| CookieRepository::new(c).clear())
    }

    pub fn clear_for_site(&self, store: &BrowserDataStore, host: &str) -> Result<u64, StoreError> {
        store.with_conn(|c| CookieRepository::new(c).clear_for_domain_suffix(host))
    }

    pub fn delete_by_id(&self, store: &BrowserDataStore, id: &str) -> Result<u64, StoreError> {
        store.with_conn(|c| CookieRepository::new(c).delete_by_id(id))
    }

    pub fn list_for_site(
        &self,
        store: &BrowserDataStore,
        host: &str,
    ) -> Result<Vec<CookieInspectRecord>, StoreError> {
        let now = self.clock.now_ms();
        let host = host.to_ascii_lowercase();
        let all = store.with_conn(|c| CookieRepository::new(c).list_all())?;
        Ok(all
            .iter()
            .filter(|c| {
                !c.expiration.is_expired(now)
                    && (c.domain == host || host.ends_with(&format!(".{}", c.domain)))
            })
            .map(CookieInspectRecord::from)
            .collect())
    }

    pub fn list_all_inspect(
        &self,
        store: &BrowserDataStore,
    ) -> Result<Vec<CookieInspectRecord>, StoreError> {
        let now = self.clock.now_ms();
        let all = store.with_conn(|c| CookieRepository::new(c).list_all())?;
        Ok(all
            .iter()
            .filter(|c| !c.expiration.is_expired(now))
            .map(CookieInspectRecord::from)
            .collect())
    }

    pub fn count(&self, store: &BrowserDataStore) -> Result<i64, StoreError> {
        store.with_conn(|c| CookieRepository::new(c).count())
    }

    pub fn purge_session_cookies(&self, store: &BrowserDataStore) -> Result<u64, StoreError> {
        store.with_conn(|c| CookieRepository::new(c).delete_session_cookies())
    }

    pub fn default_navigation_context(url: &str) -> CookieAccessContext {
        let site = Site::from_url_str(url).unwrap_or(Site {
            scheme: "http".into(),
            host: String::new(),
        });
        CookieAccessContext {
            request_url: url.to_string(),
            top_level_site: site.clone(),
            initiator_site: Some(site),
            navigation: NavigationKind::TopLevel,
            method: HttpMethodKind::SafeGet,
            secure_context: url.starts_with("https://"),
            source: CookieSource::Network,
        }
    }
}

fn validate_prefixes(
    name: &str,
    secure: bool,
    host_only: bool,
    path: &str,
    secure_ctx: bool,
) -> Result<(), CookieRejectReason> {
    if name.starts_with("__Host-") {
        if !secure_ctx || !secure || !host_only || path != "/" {
            return Err(CookieRejectReason::PrefixViolation);
        }
    } else if name.starts_with("__Secure-") && (!secure_ctx || !secure) {
        return Err(CookieRejectReason::PrefixViolation);
    }
    Ok(())
}

fn same_site_allows(
    same_site: CookieSameSite,
    ctx: &CookieAccessContext,
    psl: &Arc<dyn PublicSuffixProvider>,
) -> bool {
    let request_site = match Site::from_url_str(&ctx.request_url) {
        Some(s) => s,
        None => return false,
    };
    let etld = |h: &str| psl.registrable_domain(h);
    // A top-level navigation's site for cookies is its own URL, so a cross-site initiator
    // (e.g. a form on another site POSTing here) is what makes it cross-site.
    let same = ctx.top_level_site.is_same_site(&request_site, etld)
        && ctx
            .initiator_site
            .as_ref()
            .is_none_or(|initiator| initiator.is_same_site(&request_site, etld));

    match same_site {
        CookieSameSite::None => true,
        CookieSameSite::Strict => same,
        CookieSameSite::Lax => {
            if same {
                return true;
            }
            // Cross-site: only top-level navigations with safe methods.
            matches!(ctx.navigation, NavigationKind::TopLevel)
                && matches!(ctx.method, HttpMethodKind::SafeGet)
        }
    }
}

/// Fuzz/property-test entry: parse only, never panics.
pub fn fuzz_parse_set_cookie(input: &str) -> Option<(String, String)> {
    let capped = if input.len() > 16 * 1024 {
        &input[..16 * 1024]
    } else {
        input
    };
    match parse_set_cookie(capped) {
        Ok(p) if is_valid_cookie_name(&p.name) && is_valid_cookie_value(&p.value) => {
            Some((p.name, p.value))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::BrowserDataStore;

    fn mem_store() -> BrowserDataStore {
        BrowserDataStore::open_private().unwrap()
    }

    #[test]
    fn httponly_hidden_from_script() {
        let store = mem_store();
        let svc = CookieService::new();
        let url = "https://example.test/";
        svc.process_set_cookie(
            &store,
            url,
            "session=abc; Path=/; HttpOnly",
            CookieSource::Network,
        )
        .unwrap();
        let script = svc.cookies_for_script(&store, url).unwrap();
        assert!(!script.contains("session"));
        let ctx = CookieService::default_navigation_context(url);
        let header = svc
            .cookie_header_for_request(&store, &ctx)
            .unwrap()
            .unwrap();
        assert!(header.contains("session=abc"));
    }

    #[test]
    fn script_cannot_overwrite_httponly() {
        let store = mem_store();
        let svc = CookieService::new();
        let url = "https://example.test/";
        svc.process_set_cookie(
            &store,
            url,
            "session=abc; Path=/; HttpOnly",
            CookieSource::Network,
        )
        .unwrap();
        let r = svc
            .set_cookie_from_script(&store, url, "session=hacked; Path=/")
            .unwrap();
        assert!(matches!(r, ProcessResult::Rejected(_)));
        let ctx = CookieService::default_navigation_context(url);
        let header = svc
            .cookie_header_for_request(&store, &ctx)
            .unwrap()
            .unwrap();
        assert!(header.contains("session=abc"));
    }

    #[test]
    fn host_prefix_requires_path_root() {
        let store = mem_store();
        let svc = CookieService::new();
        let url = "https://example.test/";
        let r = svc
            .process_set_cookie(
                &store,
                url,
                "__Host-a=1; Secure; Path=/account",
                CookieSource::Network,
            )
            .unwrap();
        assert!(matches!(r, ProcessResult::Rejected(_)));
        let ok = svc
            .process_set_cookie(
                &store,
                url,
                "__Host-a=1; Secure; Path=/",
                CookieSource::Network,
            )
            .unwrap();
        assert_eq!(ok, ProcessResult::Accepted);
    }

    #[test]
    fn domain_unrelated_rejected() {
        let store = mem_store();
        let svc = CookieService::new();
        let r = svc
            .process_set_cookie(
                &store,
                "https://evil.example/",
                "x=1; Domain=bank.example",
                CookieSource::Network,
            )
            .unwrap();
        assert!(matches!(
            r,
            ProcessResult::Rejected(CookieRejectReason::DomainMismatch)
        ));
    }

    #[test]
    fn psl_rejects_public_and_private_suffix_domains() {
        let store = mem_store();
        let svc = CookieService::new();
        for (url, domain) in [
            ("https://example.co.uk/", "co.uk"),
            ("https://example.com.au/", "com.au"),
            ("https://pages.github.io/", "github.io"),
            ("https://example.com/", "com"),
        ] {
            let r = svc
                .process_set_cookie(
                    &store,
                    url,
                    &format!("x=1; Domain={domain}; Path=/"),
                    CookieSource::Network,
                )
                .unwrap();
            assert!(
                matches!(r, ProcessResult::Rejected(CookieRejectReason::PublicSuffix)),
                "expected PublicSuffix for Domain={domain}, got {r:?}"
            );
        }
        // Valid registrable Domain still accepted
        let ok = svc
            .process_set_cookie(
                &store,
                "https://shop.example.co.uk/",
                "ok=1; Domain=example.co.uk; Path=/",
                CookieSource::Network,
            )
            .unwrap();
        assert_eq!(ok, ProcessResult::Accepted);
        let ctx = CookieService::default_navigation_context("https://other.example.co.uk/");
        let h = svc
            .cookie_header_for_request(&store, &ctx)
            .unwrap()
            .unwrap();
        assert!(h.contains("ok=1"));
    }

    #[test]
    fn host_only_preserved_without_domain_attr() {
        let store = mem_store();
        let svc = CookieService::new();
        svc.process_set_cookie(
            &store,
            "https://shop.example.co.uk/",
            "hostonly=1; Path=/",
            CookieSource::Network,
        )
        .unwrap();
        let ctx_shop = CookieService::default_navigation_context("https://shop.example.co.uk/");
        assert!(svc
            .cookie_header_for_request(&store, &ctx_shop)
            .unwrap()
            .unwrap()
            .contains("hostonly=1"));
        let ctx_sib = CookieService::default_navigation_context("https://other.example.co.uk/");
        assert!(svc
            .cookie_header_for_request(&store, &ctx_sib)
            .unwrap()
            .is_none());
    }

    #[test]
    fn samesite_strict_blocks_cross_site_subresource() {
        let store = mem_store();
        let svc = CookieService::new();
        svc.process_set_cookie(
            &store,
            "https://a.example/",
            "s=1; Path=/; SameSite=Strict",
            CookieSource::Network,
        )
        .unwrap();
        let ctx = CookieAccessContext {
            request_url: "https://a.example/pixel".into(),
            top_level_site: Site {
                scheme: "https".into(),
                host: "other.example".into(),
            },
            initiator_site: Some(Site {
                scheme: "https".into(),
                host: "other.example".into(),
            }),
            navigation: NavigationKind::Subresource,
            method: HttpMethodKind::SafeGet,
            secure_context: true,
            source: CookieSource::Network,
        };
        let h = svc.cookie_header_for_request(&store, &ctx).unwrap();
        assert!(h.is_none());
    }
}
