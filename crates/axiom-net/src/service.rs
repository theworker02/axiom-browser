//! NetworkService — the network boundary: headers, cookies (via [`CookieProvider`]),
//! the HTTP cache, redirects, policies, content decoding, logging and metrics.
//!
//! One service is owned per browser profile and shared by all of its tabs.

use std::collections::VecDeque;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use parking_lot::{Mutex, RwLock};

use axiom_url::Url;

use crate::activity::{ActivityHub, NetworkActivity};
use crate::body::{
    counting, decoding, parse_content_encoding, BodyCounters, BodyOutcome, CacheTee, Instrumented,
    RequestBody, ResponseBodyReader, SharedBytesReader,
};
use crate::cache::{
    is_storable, CacheEntry, CacheKey, CacheLimits, CacheMode, CacheState, HttpCache,
};
use crate::cancel::CancellationToken;
use crate::cors::{
    cors_check, cors_unsafe_header_names, evaluate_preflight, is_cors_safelisted_method,
    serialize_origin, PreflightCache, PreflightRequest,
};
use crate::dns::{DnsResolver, SystemDnsResolver};
use crate::error::NetworkError;
use crate::headers::HeaderMap;
use crate::http_date::now_unix;
use crate::id::NetworkRequestId;
use crate::metrics::{NetworkMetrics, PoolStats};
use crate::mime::MimeType;
use crate::policy::NetworkPolicy;
use crate::protocol::HttpProtocol;
use crate::redirect::{
    drop_body_after_redirect, is_redirect_status, method_after_redirect, same_origin, RedirectMode,
    RedirectRecord,
};
use crate::referrer::compute_referrer;
use crate::request::{CredentialsMode, HttpMethod, NetworkRequest, RequestMode};
use crate::response::{ContentRange, NetworkResponse, ResponseMeta, TlsInfo};
use crate::timing::{ms_since, NetworkTiming};
use crate::transport::{RawTransportResponse, ReqwestTransport, Transport, TransportConfig};
use crate::user_agent::{accept_encoding, accept_for, AXIOM_USER_AGENT, DEFAULT_ACCEPT_LANGUAGE};

/// What the cookie authority needs to apply SameSite and credential rules.
#[derive(Debug, Clone, Copy)]
pub struct CookieRequestContext<'a> {
    pub url: &'a Url,
    /// Top-level document URL (equals `url` for top-level navigations).
    pub top_level_url: &'a Url,
    pub is_top_level_navigation: bool,
    pub method: &'a HttpMethod,
    /// Document that initiated the request (`None` for browser-initiated navigations).
    pub initiator: Option<&'a Url>,
}

/// Cookie authority bridge (implemented by the browser's CookieService). The network
/// service never stores cookies itself.
pub trait CookieProvider: Send + Sync {
    fn cookie_header(&self, ctx: &CookieRequestContext<'_>) -> Option<String>;
    fn store_set_cookies(&self, ctx: &CookieRequestContext<'_>, set_cookies: &[String]);
}

pub struct NullCookieProvider;

impl CookieProvider for NullCookieProvider {
    fn cookie_header(&self, _ctx: &CookieRequestContext<'_>) -> Option<String> {
        None
    }
    fn store_set_cookies(&self, _ctx: &CookieRequestContext<'_>, _set_cookies: &[String]) {}
}

#[derive(Clone)]
pub struct NetworkServiceConfig {
    pub connect_timeout: Duration,
    /// Longest silence while waiting for response headers or the next body chunk.
    pub read_timeout: Option<Duration>,
    pub cache_limits: CacheLimits,
    /// Normal profiles keep the cache across `shutdown`; private profiles clear it.
    pub persistent_cache: bool,
    /// Directory for a disk-backed cache. Only used when `persistent_cache` is true;
    /// if it cannot be opened the service falls back to a memory cache.
    pub cache_dir: Option<PathBuf>,
    pub max_redirects: usize,
    /// Upper bound on the summed size of one response's header names and values.
    pub max_header_bytes: usize,
    /// Extra PEM trust anchors. Certificate verification is never disabled.
    pub extra_root_certs_pem: Vec<Vec<u8>>,
    pub http1_only: bool,
    pub log_capacity: usize,
    /// Profile label used in structured log lines (never a path or user data).
    pub label: String,
    /// Sent unless the request sets its own `User-Agent`.
    pub user_agent: String,
    /// Sent unless the request sets its own `Accept-Language`; empty sends none.
    pub accept_language: String,
}

impl Default for NetworkServiceConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            read_timeout: Some(Duration::from_secs(300)),
            cache_limits: CacheLimits::default(),
            persistent_cache: true,
            cache_dir: None,
            max_redirects: 20,
            max_header_bytes: 256 * 1024,
            extra_root_certs_pem: Vec::new(),
            http1_only: false,
            log_capacity: 500,
            label: "default".into(),
            user_agent: AXIOM_USER_AGENT.into(),
            accept_language: DEFAULT_ACCEPT_LANGUAGE.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogState {
    Pending,
    Receiving,
    Complete,
    Failed,
    Cancelled,
    /// The consumer stopped reading before the end of the body.
    Abandoned,
}

impl LogState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Receiving => "receiving",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Abandoned => "abandoned",
        }
    }
}

/// One redirect hop as shown in diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedRedirect {
    pub status: u16,
    pub from: String,
    pub to: String,
    pub method: String,
    pub cross_origin: bool,
}

/// DevTools / `axiom://network` entry. Headers are stored redacted; bodies never.
#[derive(Debug, Clone)]
pub struct NetworkLogEntry {
    pub id: NetworkRequestId,
    pub context_id: Option<u64>,
    pub method: String,
    pub url: String,
    pub final_url: Option<String>,
    pub status: Option<u16>,
    pub resource_type: &'static str,
    pub initiator: String,
    pub priority: &'static str,
    pub origin: Option<String>,
    /// The `Referer` actually sent on the first hop (after policy), if any.
    pub referrer: Option<String>,
    pub protocol: Option<HttpProtocol>,
    pub cache_state: Option<CacheState>,
    pub tls: Option<TlsInfo>,
    pub transferred_bytes: u64,
    pub decoded_bytes: u64,
    pub timing: NetworkTiming,
    pub redirects: Vec<LoggedRedirect>,
    pub request_headers: HeaderMap,
    pub response_headers: HeaderMap,
    pub state: LogState,
    pub error: Option<String>,
    /// Stable machine-readable error class ([`NetworkError::kind_name`]).
    pub error_kind: Option<&'static str>,
}

impl NetworkLogEntry {
    fn new(req: &NetworkRequest) -> Self {
        Self {
            id: req.id,
            context_id: req.context_id,
            method: req.method.as_str().to_string(),
            url: req.url.as_str(),
            final_url: None,
            status: None,
            resource_type: req.resource_type.as_str(),
            initiator: req.initiator.clone(),
            priority: req.priority.as_str(),
            origin: req.origin.as_ref().map(|o| o.origin()),
            referrer: None,
            protocol: None,
            cache_state: None,
            tls: None,
            transferred_bytes: 0,
            decoded_bytes: 0,
            timing: NetworkTiming {
                queued_ms: req.queued_ms,
                ..Default::default()
            },
            redirects: Vec::new(),
            request_headers: HeaderMap::new(),
            response_headers: HeaderMap::new(),
            state: LogState::Pending,
            error: None,
            error_kind: None,
        }
    }
}

struct LogStore {
    entries: Mutex<VecDeque<NetworkLogEntry>>,
    capacity: usize,
}

impl LogStore {
    fn push(&self, entry: NetworkLogEntry) {
        let mut g = self.entries.lock();
        g.push_back(entry);
        while g.len() > self.capacity {
            g.pop_front();
        }
    }

    fn update(&self, id: NetworkRequestId, f: impl FnOnce(&mut NetworkLogEntry)) {
        let mut g = self.entries.lock();
        if let Some(e) = g.iter_mut().rev().find(|e| e.id == id) {
            f(e);
        }
    }

    fn find(&self, id: NetworkRequestId) -> Option<NetworkLogEntry> {
        self.entries
            .lock()
            .iter()
            .rev()
            .find(|e| e.id == id)
            .cloned()
    }

    fn recent(&self, limit: usize) -> Vec<NetworkLogEntry> {
        let g = self.entries.lock();
        let skip = g.len().saturating_sub(limit);
        g.iter().skip(skip).cloned().collect()
    }
}

/// Largest preflight response body read before the connection is released.
const MAX_PREFLIGHT_BODY: u64 = 64 * 1024;

/// CORS state of one request across its redirect hops.
#[derive(Debug, Default)]
struct CorsState {
    /// Fetch response tainting "cors": a hop of a `Cors` request reached a URL that is
    /// cross-origin to the requester. Once set it stays set.
    tainted: bool,
    /// Fetch "tainted origin flag": the request's origin now serializes as `null`.
    origin_tainted: bool,
    /// [`cors_unsafe_header_names`] of the headers the requester set.
    unsafe_headers: Vec<String>,
}

impl CorsState {
    fn origin(&self, req: &NetworkRequest) -> String {
        serialize_origin(req.origin.as_ref(), self.origin_tainted)
    }

    fn credentials(req: &NetworkRequest) -> bool {
        req.credentials_mode == CredentialsMode::Include
    }

    /// Apply the CORS check to a response of a tainted hop (redirects included).
    fn check(&self, req: &NetworkRequest, headers: &HeaderMap) -> Result<(), NetworkError> {
        if !self.tainted {
            return Ok(());
        }
        cors_check(headers, &self.origin(req), Self::credentials(req))
            .map_err(|why| NetworkError::Cors(format!("{}: {why}", req.url.origin())))
    }
}

/// What the network hop contributed to a response served from the cache.
struct HopInfo {
    protocol: HttpProtocol,
    tls: Option<TlsInfo>,
    ttfb_ms: f64,
    set_cookies: Vec<String>,
}

pub struct NetworkService {
    transport: Arc<dyn Transport>,
    cache: Arc<HttpCache>,
    cookies: Arc<dyn CookieProvider>,
    policies: RwLock<Vec<Arc<dyn NetworkPolicy>>>,
    metrics: Arc<NetworkMetrics>,
    log: Arc<LogStore>,
    activity: Arc<ActivityHub>,
    preflights: PreflightCache,
    config: NetworkServiceConfig,
    shutting_down: AtomicBool,
}

impl NetworkService {
    /// Service with the system resolver.
    ///
    /// # Panics
    /// If the network runtime cannot start or `extra_root_certs_pem` contains invalid PEM;
    /// use [`NetworkService::try_new`] to handle those cases.
    pub fn new(config: NetworkServiceConfig) -> Self {
        Self::try_new(config, Arc::new(SystemDnsResolver)).expect("network service")
    }

    pub fn try_new(
        config: NetworkServiceConfig,
        dns: Arc<dyn DnsResolver>,
    ) -> Result<Self, NetworkError> {
        let transport = ReqwestTransport::new(
            &TransportConfig {
                connect_timeout: Some(config.connect_timeout),
                read_timeout: config.read_timeout,
                extra_root_certs_pem: config.extra_root_certs_pem.clone(),
                http1_only: config.http1_only,
            },
            dns,
        )?;
        Ok(Self::with_transport(config, Arc::new(transport)))
    }

    pub fn with_transport(config: NetworkServiceConfig, transport: Arc<dyn Transport>) -> Self {
        let cache = match (&config.cache_dir, config.persistent_cache) {
            (Some(dir), true) => match HttpCache::open(dir, config.cache_limits) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!(
                        target: "axiom_net",
                        "profile={} disk cache unavailable ({e}); using memory cache",
                        config.label
                    );
                    HttpCache::new(config.cache_limits, true)
                }
            },
            _ => HttpCache::new(config.cache_limits, config.persistent_cache),
        };
        Self {
            transport,
            cache: Arc::new(cache),
            cookies: Arc::new(NullCookieProvider),
            policies: RwLock::new(Vec::new()),
            metrics: Arc::new(NetworkMetrics::default()),
            log: Arc::new(LogStore {
                entries: Mutex::new(VecDeque::new()),
                capacity: config.log_capacity.max(1),
            }),
            activity: Arc::new(ActivityHub::default()),
            preflights: PreflightCache::new(),
            config,
            shutting_down: AtomicBool::new(false),
        }
    }

    pub fn with_cookies(mut self, cookies: Arc<dyn CookieProvider>) -> Self {
        self.cookies = cookies;
        self
    }

    pub fn with_cache(mut self, cache: Arc<HttpCache>) -> Self {
        self.cache = cache;
        self
    }

    pub fn add_policy(&self, policy: Arc<dyn NetworkPolicy>) {
        self.policies.write().push(policy);
    }

    pub fn cache(&self) -> &Arc<HttpCache> {
        &self.cache
    }

    /// The profile's CORS-preflight cache (memory only).
    pub fn preflight_cache(&self) -> &PreflightCache {
        &self.preflights
    }

    /// Forget cached responses and preflight results (clearing the profile's cache).
    pub fn clear_cache(&self) {
        self.cache.clear();
        self.preflights.clear();
    }

    pub fn metrics(&self) -> &NetworkMetrics {
        &self.metrics
    }

    pub fn config(&self) -> &NetworkServiceConfig {
        &self.config
    }

    pub fn pool_stats(&self) -> PoolStats {
        self.transport.pool_stats()
    }

    pub fn recent_log(&self, limit: usize) -> Vec<NetworkLogEntry> {
        self.log.recent(limit)
    }

    /// Log entry for one request, while it is still retained.
    pub fn log_entry(&self, id: NetworkRequestId) -> Option<NetworkLogEntry> {
        self.log.find(id)
    }

    /// Subscribe to the activity stream. A slow subscriber loses events instead of
    /// slowing requests down.
    pub fn subscribe_activity(&self, capacity: usize) -> Receiver<NetworkActivity> {
        self.activity.subscribe(capacity)
    }

    pub fn activity(&self) -> &ActivityHub {
        &self.activity
    }

    /// Stop accepting requests. Private (non-persistent) caches are cleared; disk caches
    /// persist their LRU state.
    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        self.preflights.clear();
        if self.cache.is_persistent() {
            self.cache.flush();
        } else {
            self.cache.clear();
        }
    }

    pub fn is_shut_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    pub fn execute_blocking(&self, req: NetworkRequest) -> Result<NetworkResponse, NetworkError> {
        self.execute_with_cancel(req, &CancellationToken::new())
    }

    /// Run a request to response headers. The body streams from the returned reader.
    /// Must not be called from inside an async runtime.
    pub fn execute_with_cancel(
        &self,
        mut req: NetworkRequest,
        cancel: &CancellationToken,
    ) -> Result<NetworkResponse, NetworkError> {
        if self.is_shut_down() {
            return Err(NetworkError::Cancelled);
        }
        self.metrics.requests.fetch_add(1, Ordering::Relaxed);
        let start = Instant::now();
        self.log.push(NetworkLogEntry::new(&req));
        let id = req.id;
        self.activity.emit(|| NetworkActivity::RequestStarted {
            id,
            context_id: req.context_id,
            method: req.method.as_str().to_string(),
            url: req.url.as_str(),
            resource_type: req.resource_type.as_str(),
            initiator: req.initiator.clone(),
        });
        // Internal schemes (axiom:, about:, data:, file:, …) are never sent to the network.
        let result = if req.url.scheme != "http" && req.url.scheme != "https" {
            Err(NetworkError::UnsupportedScheme(req.url.scheme.clone()))
        } else {
            self.run(&mut req, cancel, start)
        };
        if let Err(e) = &result {
            let counter = match e {
                NetworkError::Cancelled => &self.metrics.cancelled,
                NetworkError::Blocked(_) => &self.metrics.blocked,
                _ => &self.metrics.failed,
            };
            counter.fetch_add(1, Ordering::Relaxed);
            let state = if *e == NetworkError::Cancelled {
                LogState::Cancelled
            } else {
                LogState::Failed
            };
            let total_ms = ms_since(start);
            self.log.update(id, |entry| {
                entry.state = state;
                entry.error = Some(e.to_string());
                entry.error_kind = Some(e.kind_name());
                entry.timing.total_ms = Some(total_ms);
            });
            self.activity.emit(|| match e {
                NetworkError::Cancelled => NetworkActivity::RequestCanceled { id },
                _ => NetworkActivity::RequestFailed {
                    id,
                    error: e.clone(),
                },
            });
            OutcomeLog::new(&self.config.label, &req).emit(None, None, 0, total_ms, Some(e));
        }
        result
    }

    fn run(
        &self,
        req: &mut NetworkRequest,
        cancel: &CancellationToken,
        start: Instant,
    ) -> Result<NetworkResponse, NetworkError> {
        let mut chain: Vec<RedirectRecord> = Vec::new();
        // Fetch §4.5 step 8.22: author-supplied conditionals bypass the cache.
        if req.cache_mode == CacheMode::Default && has_conditional_headers(&req.headers) {
            req.cache_mode = CacheMode::NoStore;
        }
        // Computed before the service adds its own headers.
        let mut cors = CorsState {
            unsafe_headers: cors_unsafe_header_names(&req.headers),
            ..CorsState::default()
        };
        loop {
            cancel.check()?;
            if req.mode == RequestMode::Cors
                && !req
                    .origin
                    .as_ref()
                    .is_some_and(|o| same_origin(o, &req.url))
            {
                cors.tainted = true;
            }
            self.check_request_policies(req, chain.last())?;
            if cors.tainted {
                self.preflight_if_needed(req, &cors, cancel)?;
            }
            self.prepare_headers(req, &cors);
            let redacted = req.headers.redacted();
            let referer = req.headers.get("referer").map(str::to_string);
            let first_hop = chain.is_empty();
            self.activity.emit(|| NetworkActivity::RequestHeadersReady {
                id: req.id,
                url: req.url.as_str(),
                headers: redacted.clone(),
            });
            self.log.update(req.id, |e| {
                e.request_headers = redacted;
                if first_hop {
                    e.referrer = referer;
                }
            });

            let key = CacheKey::for_request(&req.method, &req.url);
            let consult_cache = req.method == HttpMethod::Get
                && !req.headers.contains("range")
                && !matches!(req.cache_mode, CacheMode::NoStore | CacheMode::Reload);
            let mut revalidating: Option<CacheEntry> = None;
            if consult_cache {
                match self.cache.lookup(&key, &req.headers) {
                    Some(entry) => {
                        let fresh = entry.is_fresh(Instant::now());
                        let serve = match req.cache_mode {
                            CacheMode::Default => fresh,
                            CacheMode::ForceCache | CacheMode::OnlyIfCached => {
                                fresh || !(entry.must_revalidate || entry.requires_validation)
                            }
                            _ => false,
                        };
                        if serve {
                            self.cache.record_hit();
                            self.metrics.cache_hits.fetch_add(1, Ordering::Relaxed);
                            let state = if fresh {
                                CacheState::Hit
                            } else {
                                CacheState::Stale
                            };
                            self.activity
                                .emit(|| NetworkActivity::CacheHit { id: req.id, state });
                            cors.check(req, &entry.headers)?;
                            if is_redirect_status(entry.status)
                                && req.redirect_mode == RedirectMode::Follow
                            {
                                self.follow_redirect(
                                    req,
                                    entry.status,
                                    &entry.headers,
                                    &mut chain,
                                    &mut cors,
                                )?;
                                continue;
                            }
                            return self.cached_response(req, entry, state, chain, start, None);
                        }
                        if req.cache_mode == CacheMode::OnlyIfCached {
                            return Err(NetworkError::Cache(
                                "only-if-cached: stored response requires revalidation".into(),
                            ));
                        }
                        if entry.has_validators() {
                            if let Some(etag) = entry.etag() {
                                req.headers.set("If-None-Match", etag);
                            }
                            if let Some(lm) = entry.last_modified() {
                                req.headers.set("If-Modified-Since", lm);
                            }
                            revalidating = Some(entry);
                        }
                    }
                    None if req.cache_mode == CacheMode::OnlyIfCached => {
                        return Err(NetworkError::Cache("only-if-cached: not in cache".into()));
                    }
                    None => {}
                }
                if revalidating.is_none() {
                    self.cache.record_miss();
                    self.metrics.cache_misses.fetch_add(1, Ordering::Relaxed);
                    self.activity
                        .emit(|| NetworkActivity::CacheMiss { id: req.id });
                }
            }

            self.metrics
                .network_requests
                .fetch_add(1, Ordering::Relaxed);
            let hop_start = Instant::now();
            let raw = self.transport.execute(req, cancel);
            if revalidating.is_some() {
                req.headers.remove("if-none-match");
                req.headers.remove("if-modified-since");
            }
            let raw = raw?;
            let ttfb_ms = ms_since(hop_start);
            let response_time = now_unix();
            let header_bytes: usize = raw
                .headers
                .iter()
                .map(|(k, vs)| vs.iter().map(|v| k.len() + v.len() + 4).sum::<usize>())
                .sum();
            if header_bytes > self.config.max_header_bytes {
                return Err(NetworkError::HeadersTooLarge {
                    limit: self.config.max_header_bytes,
                });
            }
            self.activity.emit(|| NetworkActivity::ResponseStarted {
                id: req.id,
                status: raw.status,
                protocol: raw.protocol,
            });

            let set_cookies: Vec<String> = raw.headers.get_all("set-cookie").to_vec();
            if !set_cookies.is_empty() && self.credentials_allowed(req, &cors) {
                self.cookies
                    .store_set_cookies(&cookie_context(req), &set_cookies);
            }

            if raw.status == 304 {
                if let Some(entry) = revalidating.take() {
                    drop(raw.body);
                    let updated = self
                        .cache
                        .freshen(&key, &req.headers, &raw.headers, response_time)
                        .unwrap_or(entry);
                    self.cache.record_revalidation();
                    self.metrics
                        .cache_revalidations
                        .fetch_add(1, Ordering::Relaxed);
                    self.activity.emit(|| NetworkActivity::CacheHit {
                        id: req.id,
                        state: CacheState::Revalidated,
                    });
                    cors.check(req, &updated.headers)?;
                    if is_redirect_status(updated.status)
                        && req.redirect_mode == RedirectMode::Follow
                    {
                        self.follow_redirect(
                            req,
                            updated.status,
                            &updated.headers,
                            &mut chain,
                            &mut cors,
                        )?;
                        continue;
                    }
                    let hop = HopInfo {
                        protocol: raw.protocol,
                        tls: raw.tls,
                        ttfb_ms,
                        set_cookies,
                    };
                    return self.cached_response(
                        req,
                        updated,
                        CacheState::Revalidated,
                        chain,
                        start,
                        Some(hop),
                    );
                }
            }

            // RFC 9111 §4.4: unsafe methods invalidate stored responses for the target.
            if !req.method.is_safe() && raw.status < 400 {
                self.cache
                    .invalidate(&CacheKey::for_request(&HttpMethod::Get, &req.url));
                for h in ["location", "content-location"] {
                    if let Some(u) = raw.headers.get(h).and_then(|l| req.url.join(l).ok()) {
                        if same_origin(&u, &req.url) {
                            self.cache
                                .invalidate(&CacheKey::for_request(&HttpMethod::Get, &u));
                        }
                    }
                }
            }

            // Fetch "HTTP fetch" step 4.4: before redirects are handled, so a foreign
            // server cannot bounce a CORS request onwards without opting in.
            cors.check(req, &raw.headers)?;

            if is_redirect_status(raw.status) {
                match req.redirect_mode {
                    RedirectMode::Follow => {
                        if req.cache_mode != CacheMode::NoStore {
                            self.cache.store_with_tls(
                                key,
                                &req.headers,
                                raw.status,
                                &raw.headers,
                                Vec::new(),
                                response_time,
                                raw.tls.clone(),
                            );
                        }
                        drop(raw.body);
                        self.follow_redirect(req, raw.status, &raw.headers, &mut chain, &mut cors)?;
                        continue;
                    }
                    RedirectMode::Error => {
                        return Err(NetworkError::Protocol(format!(
                            "redirect ({}) disallowed by redirect mode",
                            raw.status
                        )));
                    }
                    RedirectMode::Manual => {}
                }
            }

            return self.network_response(
                req,
                raw,
                set_cookies,
                chain,
                start,
                ttfb_ms,
                key,
                consult_cache,
                response_time,
            );
        }
    }

    fn follow_redirect(
        &self,
        req: &mut NetworkRequest,
        status: u16,
        headers: &HeaderMap,
        chain: &mut Vec<RedirectRecord>,
        cors: &mut CorsState,
    ) -> Result<(), NetworkError> {
        let location = headers
            .get("location")
            .ok_or_else(|| NetworkError::Protocol("redirect response without Location".into()))?;
        let mut next = req.url.join(location).map_err(|e| {
            // An absolute Location with a scheme we will not fetch (file:, ftp:, data:,
            // javascript:) is a scheme refusal, not a malformed response.
            match absolute_scheme(location) {
                Some(s) if s != "http" && s != "https" => NetworkError::UnsupportedScheme(s),
                _ => NetworkError::Protocol(format!("invalid redirect Location {location:?}: {e}")),
            }
        })?;
        if next.scheme != "http" && next.scheme != "https" {
            return Err(NetworkError::UnsupportedScheme(next.scheme.clone()));
        }
        if chain.len() >= self.config.max_redirects {
            // Redirects are followed up to the limit even when a hop repeats (cookies set
            // along the way may end a loop). Past the limit, a repeated hop — same from,
            // to, status and method — is reported as a loop.
            let strip = |u: &Url| {
                let mut u = u.clone();
                u.fragment = None;
                u
            };
            let (from, to) = (strip(&req.url), strip(&next));
            let repeats = chain.iter().any(|r| {
                r.status == status
                    && r.method_in == req.method
                    && strip(&r.from) == from
                    && strip(&r.to) == to
            });
            return Err(if repeats {
                NetworkError::RedirectLoop
            } else {
                NetworkError::TooManyRedirects {
                    limit: self.config.max_redirects,
                }
            });
        }
        if req.mode == RequestMode::SameOrigin
            && !req.origin.as_ref().is_some_and(|o| same_origin(o, &next))
        {
            return Err(NetworkError::Blocked(format!(
                "same-origin request redirected to cross-origin {}",
                next.origin()
            )));
        }
        // Fetch "HTTP-redirect fetch" step 13.
        if !same_origin(&req.url, &next)
            && !req
                .origin
                .as_ref()
                .is_some_and(|o| same_origin(o, &req.url))
        {
            cors.origin_tainted = true;
        }
        let method_out = method_after_redirect(status, &req.method);
        let drop_body = drop_body_after_redirect(status, &req.method, &method_out);
        if !drop_body && !req.body.is_empty() && !req.body.is_replayable() {
            return Err(NetworkError::Protocol(
                "cannot replay a streamed request body across a redirect".into(),
            ));
        }
        if next.fragment.is_none() {
            next.fragment = req.url.fragment.clone();
        }
        let cross_origin = !same_origin(&req.url, &next);
        let record = RedirectRecord {
            from: req.url.clone(),
            to: next.clone(),
            status,
            method_in: req.method.clone(),
            method_out: method_out.clone(),
            cross_origin,
        };
        if drop_body {
            req.body = RequestBody::Empty;
            for h in [
                "content-type",
                "content-length",
                "content-encoding",
                "content-language",
                "content-location",
            ] {
                req.headers.remove(h);
            }
        }
        if cross_origin {
            req.headers.remove("authorization");
            req.headers.remove("proxy-authorization");
        }
        if req.is_top_level_navigation() {
            req.top_level_url = Some(next.clone());
        }
        let logged = LoggedRedirect {
            status,
            from: req.url.as_str(),
            to: next.as_str(),
            method: req.method.as_str().to_string(),
            cross_origin,
        };
        self.activity.emit(|| NetworkActivity::Redirected {
            id: req.id,
            status,
            from: logged.from.clone(),
            to: logged.to.clone(),
            cross_origin,
        });
        self.log.update(req.id, |e| e.redirects.push(logged));
        req.url = next;
        req.method = method_out;
        chain.push(record);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn network_response(
        &self,
        req: &NetworkRequest,
        raw: RawTransportResponse,
        set_cookies: Vec<String>,
        chain: Vec<RedirectRecord>,
        start: Instant,
        ttfb_ms: f64,
        key: CacheKey,
        consulted_cache: bool,
        response_time: i64,
    ) -> Result<NetworkResponse, NetworkError> {
        let status = raw.status;
        let headers = raw.headers;
        let counters = BodyCounters::default();
        let codings = parse_content_encoding(headers.get("content-encoding"));
        let bodyless =
            req.method == HttpMethod::Head || matches!(status, 204 | 304) || status < 200;
        let wire = counting(raw.body, &counters);
        let decoded: Box<dyn Read + Send> = if bodyless || codings.is_empty() {
            wire
        } else {
            decoding(wire, &codings)?
        };
        let content_length = headers
            .get("content-length")
            .and_then(|v| v.trim().parse::<u64>().ok());
        let limits = self.cache.limits();
        let storable = req.cache_mode != CacheMode::NoStore
            && is_storable(&req.method, status, &req.headers, &headers)
            && !matches!(content_length, Some(l) if l > limits.max_entry_bytes as u64);
        let reader: Box<dyn Read + Send> = if storable {
            let cache = Arc::clone(&self.cache);
            let req_headers = req.headers.clone();
            let tls = raw.tls.clone();
            let mut stored_headers = headers.clone();
            // The cache holds the decoded body.
            stored_headers.remove("content-encoding");
            stored_headers.remove("content-length");
            Box::new(CacheTee::new(
                decoded,
                limits.max_entry_bytes,
                Arc::clone(&self.metrics.peak_cache_buffer_bytes),
                Box::new(move |body| {
                    cache.store_with_tls(
                        key,
                        &req_headers,
                        status,
                        &stored_headers,
                        body,
                        response_time,
                        tls,
                    );
                }),
            ))
        } else {
            decoded
        };

        let cache_state = if !consulted_cache {
            CacheState::Bypassed
        } else if storable {
            CacheState::Miss
        } else {
            CacheState::NotCacheable
        };
        let meta = ResponseMeta {
            request_id: req.id,
            final_url: req.url.clone(),
            status,
            reason: raw.reason,
            mime: headers.get("content-type").and_then(MimeType::parse),
            content_length,
            content_encoding: codings,
            content_range: if status == 206 {
                headers.get("content-range").and_then(ContentRange::parse)
            } else {
                None
            },
            protocol: raw.protocol,
            tls: raw.tls,
            redirect_chain: chain,
            cache_state,
            timing: NetworkTiming {
                queued_ms: req.queued_ms,
                ttfb_ms: Some(ttfb_ms),
                ..Default::default()
            },
            set_cookies,
            download: detect_download(&headers, &req.url),
            headers,
            counters: counters.clone(),
        };
        self.check_response_policies(req, &meta)?;
        let body = self.instrument(req, &meta, reader, &counters, start);
        Ok(NetworkResponse { meta, body })
    }

    fn cached_response(
        &self,
        req: &NetworkRequest,
        entry: CacheEntry,
        state: CacheState,
        chain: Vec<RedirectRecord>,
        start: Instant,
        hop: Option<HopInfo>,
    ) -> Result<NetworkResponse, NetworkError> {
        let counters = BodyCounters::default();
        let len = entry.body.len() as u64;
        let (protocol, tls, ttfb_ms, set_cookies) = match hop {
            Some(h) => (
                h.protocol,
                h.tls.or(entry.tls.clone()),
                Some(h.ttfb_ms),
                h.set_cookies,
            ),
            // Served without a network hop: the TLS facts are those recorded at store time.
            None => (HttpProtocol::Unknown, entry.tls.clone(), None, Vec::new()),
        };
        let meta = ResponseMeta {
            request_id: req.id,
            final_url: req.url.clone(),
            status: entry.status,
            reason: None,
            mime: entry.headers.get("content-type").and_then(MimeType::parse),
            content_length: Some(len),
            content_encoding: Vec::new(),
            content_range: None,
            protocol,
            tls,
            redirect_chain: chain,
            cache_state: state,
            timing: NetworkTiming {
                queued_ms: req.queued_ms,
                ttfb_ms,
                ..Default::default()
            },
            set_cookies,
            download: detect_download(&entry.headers, &req.url),
            headers: entry.headers,
            counters: counters.clone(),
        };
        self.check_response_policies(req, &meta)?;
        let reader = Box::new(SharedBytesReader::new(entry.body));
        let body = self.instrument(req, &meta, reader, &counters, start);
        Ok(NetworkResponse { meta, body })
    }

    fn instrument(
        &self,
        req: &NetworkRequest,
        meta: &ResponseMeta,
        reader: Box<dyn Read + Send>,
        counters: &BodyCounters,
        start: Instant,
    ) -> ResponseBodyReader {
        let headers_at = Instant::now();
        let id = req.id;
        let (status, protocol, cache_state) = (meta.status, meta.protocol, meta.cache_state);
        let final_url = meta.final_url.as_str();
        let response_headers = meta.headers.redacted();
        let ttfb = meta.timing.ttfb_ms;
        let tls = meta.tls.clone();
        self.activity
            .emit(|| NetworkActivity::ResponseHeadersReady {
                id,
                status,
                protocol,
                cache_state,
                headers: response_headers.clone(),
            });
        self.log.update(id, |e| {
            e.status = Some(status);
            e.protocol = Some(protocol);
            e.cache_state = Some(cache_state);
            e.tls = tls;
            e.final_url = Some(final_url);
            e.response_headers = response_headers;
            e.timing.ttfb_ms = ttfb;
            e.state = LogState::Receiving;
        });
        let log = Arc::clone(&self.log);
        let metrics = Arc::clone(&self.metrics);
        let activity = Arc::clone(&self.activity);
        let outcome_log = OutcomeLog::new(&self.config.label, req);
        let c = counters.clone();
        let finish = Box::new(move |outcome: BodyOutcome| {
            let (transferred, decoded) = (c.transferred(), c.decoded());
            metrics
                .bytes_transferred
                .fetch_add(transferred, Ordering::Relaxed);
            metrics.bytes_decoded.fetch_add(decoded, Ordering::Relaxed);
            let total_ms = ms_since(start);
            let failure = match &outcome {
                BodyOutcome::Failed(e) => Some(e.clone()),
                _ => None,
            };
            let (state, error) = match outcome {
                BodyOutcome::Complete => (LogState::Complete, None),
                BodyOutcome::Abandoned => (LogState::Abandoned, None),
                BodyOutcome::Failed(NetworkError::Cancelled) => {
                    metrics.cancelled.fetch_add(1, Ordering::Relaxed);
                    (
                        LogState::Cancelled,
                        Some(NetworkError::Cancelled.to_string()),
                    )
                }
                BodyOutcome::Failed(e) => {
                    metrics.failed.fetch_add(1, Ordering::Relaxed);
                    (LogState::Failed, Some(e.to_string()))
                }
            };
            let error_kind = failure.as_ref().map(NetworkError::kind_name);
            log.update(id, |e| {
                e.state = state;
                e.error = error;
                e.error_kind = error_kind;
                e.transferred_bytes = transferred;
                e.decoded_bytes = decoded;
                e.timing.download_ms = Some(ms_since(headers_at));
                e.timing.total_ms = Some(total_ms);
            });
            activity.emit(|| match &failure {
                None => NetworkActivity::RequestCompleted {
                    id,
                    transferred_bytes: transferred,
                    decoded_bytes: decoded,
                    total_ms,
                },
                Some(NetworkError::Cancelled) => NetworkActivity::RequestCanceled { id },
                Some(e) => NetworkActivity::RequestFailed {
                    id,
                    error: e.clone(),
                },
            });
            outcome_log.emit(
                Some(status),
                Some((protocol, cache_state)),
                transferred,
                total_ms,
                failure.as_ref(),
            );
        });
        let reader: Box<dyn Read + Send> = if self.activity.is_observed() {
            Box::new(ActivityReader {
                inner: reader,
                id,
                activity: Arc::clone(&self.activity),
            })
        } else {
            reader
        };
        ResponseBodyReader::from_reader(Box::new(Instrumented::new(reader, counters, finish)))
    }

    /// Fetch "includeCredentials": `same-origin` credentials stop once the response
    /// tainting is `cors`, even if a later hop returns to the requester's origin.
    fn credentials_allowed(&self, req: &NetworkRequest, cors: &CorsState) -> bool {
        match req.credentials_mode {
            CredentialsMode::Omit => false,
            CredentialsMode::Include => true,
            CredentialsMode::SameOrigin => {
                !cors.tainted && req.origin.as_ref().is_none_or(|o| same_origin(o, &req.url))
            }
        }
    }

    /// Fetch "CORS-preflight fetch" for the current hop, unless the request needs none or
    /// the preflight cache already approves it. The preflight is an `OPTIONS` request
    /// through this service (own log entry, no credentials, no redirects, no cache).
    fn preflight_if_needed(
        &self,
        req: &NetworkRequest,
        cors: &CorsState,
        cancel: &CancellationToken,
    ) -> Result<(), NetworkError> {
        let needed = req.use_cors_preflight
            || (req.unsafe_request
                && (!is_cors_safelisted_method(&req.method) || !cors.unsafe_headers.is_empty()));
        if !needed {
            return Ok(());
        }
        let origin = cors.origin(req);
        let ask = PreflightRequest {
            method: req.method.as_str().to_string(),
            unsafe_headers: cors.unsafe_headers.clone(),
            credentials: CorsState::credentials(req),
            forced: req.use_cors_preflight,
        };
        if self.preflights.allows(&origin, &req.url, &ask) {
            return Ok(());
        }
        let mut p = NetworkRequest::new(HttpMethod::Options, req.url.clone(), req.resource_type);
        p.headers.set("Accept", "*/*");
        p.headers
            .set("Access-Control-Request-Method", ask.method.clone());
        if !ask.unsafe_headers.is_empty() {
            p.headers.set(
                "Access-Control-Request-Headers",
                ask.unsafe_headers.join(","),
            );
        }
        p.headers.set("Origin", origin.clone());
        p.priority = req.priority;
        // `NoCors` keeps the preflight itself out of CORS processing; its response is
        // evaluated below against the actual request.
        p.mode = RequestMode::NoCors;
        p.redirect_mode = RedirectMode::Error;
        p.cache_mode = CacheMode::NoStore;
        p.credentials_mode = CredentialsMode::Omit;
        p.referrer = req.referrer.clone();
        p.referrer_policy = req.referrer_policy;
        p.origin = req.origin.clone();
        p.top_level_url = req.top_level_url.clone();
        p.initiator = "cors-preflight".into();
        p.context_id = req.context_id;
        p.request_timeout_ms = req.request_timeout_ms.or(p.request_timeout_ms);
        let resp = self.execute_with_cancel(p, cancel).map_err(|e| match e {
            NetworkError::Protocol(why) => {
                NetworkError::Cors(format!("preflight to {}: {why}", req.url.origin()))
            }
            other => other,
        })?;
        let (status, headers) = (resp.meta.status, resp.meta.headers.clone());
        // Drain a small body so the connection can be reused; a large one is abandoned.
        // The body plays no part in the decision, so a read error is not one either.
        let _ = resp.body.read_all(MAX_PREFLIGHT_BODY);
        let grant = evaluate_preflight(status, &headers, &origin, &ask).map_err(|why| {
            NetworkError::Cors(format!("preflight to {}: {why}", req.url.origin()))
        })?;
        self.preflights
            .store(&origin, &req.url, ask.credentials, &grant);
        Ok(())
    }

    fn prepare_headers(&self, req: &mut NetworkRequest, cors: &CorsState) {
        if !req.headers.contains("user-agent") {
            req.headers
                .set("User-Agent", self.config.user_agent.clone());
        }
        if !req.headers.contains("accept") {
            req.headers.set("Accept", accept_for(req.resource_type));
        }
        if !req.headers.contains("accept-language") && !self.config.accept_language.is_empty() {
            req.headers
                .set("Accept-Language", self.config.accept_language.clone());
        }
        if req.headers.contains("range") {
            req.headers.set("Accept-Encoding", "identity");
        } else {
            req.headers.set("Accept-Encoding", accept_encoding());
        }
        req.headers.remove("referer");
        if let Some(r) = compute_referrer(req.referrer_policy, req.referrer.as_deref(), &req.url) {
            req.headers.set("Referer", r);
        }
        if cors.tainted {
            req.headers.set("Origin", cors.origin(req));
        } else if cors.origin_tainted && req.headers.contains("origin") {
            req.headers.set("Origin", "null");
        }
        // Cookies only ever come from the cookie authority, recomputed for every hop.
        req.headers.remove("cookie");
        if self.credentials_allowed(req, cors) {
            if let Some(c) = self.cookies.cookie_header(&cookie_context(req)) {
                if !c.is_empty() {
                    req.headers.set("Cookie", c);
                }
            }
        }
    }

    fn check_request_policies(
        &self,
        req: &NetworkRequest,
        redirect: Option<&RedirectRecord>,
    ) -> Result<(), NetworkError> {
        for p in self.policies.read().iter() {
            p.check_request(req, redirect)
                .map_err(|why| NetworkError::Blocked(format!("{}: {why}", p.name())))?;
        }
        if let (Some(_), Some(check)) = (redirect, &req.redirect_check) {
            check.check(&req.url).map_err(NetworkError::Blocked)?;
        }
        Ok(())
    }

    fn check_response_policies(
        &self,
        req: &NetworkRequest,
        meta: &ResponseMeta,
    ) -> Result<(), NetworkError> {
        for p in self.policies.read().iter() {
            p.check_response(req, meta)
                .map_err(|why| NetworkError::Blocked(format!("{}: {why}", p.name())))?;
        }
        Ok(())
    }
}

/// Publishes `DataReceived` for each chunk handed to the consumer (decoded bytes).
struct ActivityReader {
    inner: Box<dyn Read + Send>,
    id: NetworkRequestId,
    activity: Arc<ActivityHub>,
}

impl Read for ActivityReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            let id = self.id;
            self.activity.emit(|| NetworkActivity::DataReceived {
                id,
                bytes: n as u64,
            });
        }
        Ok(n)
    }
}

/// Fields of the one structured log line written per request. URLs are reduced to the
/// host (paths and queries may carry secrets); bodies and headers are never logged.
#[derive(Clone)]
struct OutcomeLog {
    profile: String,
    id: NetworkRequestId,
    context_id: Option<u64>,
    method: String,
    host: String,
}

impl OutcomeLog {
    fn new(profile: &str, req: &NetworkRequest) -> Self {
        Self {
            profile: profile.to_string(),
            id: req.id,
            context_id: req.context_id,
            method: req.method.as_str().to_string(),
            host: req.url.host.clone(),
        }
    }

    fn emit(
        &self,
        status: Option<u16>,
        via: Option<(HttpProtocol, CacheState)>,
        bytes: u64,
        duration_ms: f64,
        error: Option<&NetworkError>,
    ) {
        let status = status.map_or_else(|| "-".to_string(), |s| s.to_string());
        let (protocol, cache) = via.map_or(("-", "-"), |(p, c)| (p.as_str(), c.as_str()));
        let context = self
            .context_id
            .map_or_else(|| "-".to_string(), |c| c.to_string());
        match error {
            None => log::info!(
                target: "axiom_net",
                "request_id={} context_id={context} profile_id={} method={} host={} status={status} protocol={protocol} cache_status={cache} bytes={bytes} duration_ms={duration_ms:.1}",
                self.id, self.profile, self.method, self.host
            ),
            Some(e) => log::info!(
                target: "axiom_net",
                "request_id={} context_id={context} profile_id={} method={} host={} status={status} protocol={protocol} cache_status={cache} bytes={bytes} duration_ms={duration_ms:.1} error={}",
                self.id, self.profile, self.method, self.host, e.kind_name()
            ),
        }
    }
}

fn cookie_context(req: &NetworkRequest) -> CookieRequestContext<'_> {
    CookieRequestContext {
        url: &req.url,
        top_level_url: req.top_level_url.as_ref().unwrap_or(&req.url),
        is_top_level_navigation: req.is_top_level_navigation(),
        method: &req.method,
        initiator: req.origin.as_ref(),
    }
}

/// Lower-cased scheme of an absolute URL reference (`scheme ":" ...`), if any.
fn absolute_scheme(reference: &str) -> Option<String> {
    let (scheme, _) = reference.trim().split_once(':')?;
    let mut chars = scheme.chars();
    let valid = chars.next()?.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then(|| scheme.to_ascii_lowercase())
}

fn has_conditional_headers(h: &HeaderMap) -> bool {
    [
        "if-none-match",
        "if-modified-since",
        "if-match",
        "if-unmodified-since",
        "if-range",
    ]
    .iter()
    .any(|n| h.contains(n))
}

/// Download handoff (foundation only — there is no download manager yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadCandidate {
    pub url: String,
    pub filename: Option<String>,
    pub mime: Option<String>,
    pub content_length: Option<u64>,
}

pub fn detect_download(headers: &HeaderMap, url: &Url) -> Option<DownloadCandidate> {
    let cd = headers.get("content-disposition")?;
    let mut parts = cd.split(';').map(str::trim);
    if !parts.next()?.eq_ignore_ascii_case("attachment") {
        return None;
    }
    let mut filename = None;
    let mut filename_ext = None;
    for p in parts {
        let Some((name, value)) = p.split_once('=') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "filename" => filename = Some(value.trim().trim_matches('"').to_string()),
            "filename*" => {
                // RFC 8187: charset'lang'percent-encoded
                let v = value.trim();
                if let Some(encoded) = v.splitn(3, '\'').nth(2) {
                    filename_ext = Some(percent_decode(encoded));
                }
            }
            _ => {}
        }
    }
    Some(DownloadCandidate {
        url: url.as_str(),
        filename: filename_ext.or(filename),
        mime: headers.get("content-type").map(str::to_string),
        content_length: headers.get("content-length").and_then(|s| s.parse().ok()),
    })
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
    fn download_detection() {
        let url = Url::parse("http://d.test/f").unwrap();
        let mut h = HeaderMap::new();
        h.set(
            "content-disposition",
            "attachment; filename=\"a.zip\"; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf",
        );
        let d = detect_download(&h, &url).unwrap();
        assert_eq!(d.filename.as_deref(), Some("résumé.pdf"));
        h.set("content-disposition", "inline; filename=x");
        assert!(detect_download(&h, &url).is_none());
    }
}
