//! Private HTTP cache (RFC 9111 subset).
//!
//! * Primary key: method + URL without fragment. `Vary` selects among stored variants by
//!   comparing the request header values captured at store time.
//! * Freshness: `max-age`, else `Expires − Date`, else a heuristic (10% of
//!   `Date − Last-Modified`, capped at one day) only when `Last-Modified` exists. `Age` and
//!   apparent age are subtracted.
//! * `no-store` is never stored; `no-cache` is stored but always revalidated;
//!   `must-revalidate` forbids serving stale even for `force-cache`. `private` is storable
//!   (this is a private cache). `Vary: *`, `206`, and range requests are never stored.
//! * Entries without freshness or validators are not stored (they could never be reused).
//! * Least-recently-used eviction under a total byte budget, plus a per-entry size cap.
//! * Storage: memory-only (private profiles, tests), or disk-backed via
//!   [`HttpCache::open`] (normal profiles). A disk-backed cache keeps metadata in memory
//!   and bodies on disk (see `disk_cache.rs`); it survives restarts. Private caches are
//!   cleared on shutdown and never touch the disk.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::disk_cache::{DiskStore, LoadedEntry};
use crate::headers::HeaderMap;
use crate::http_date::parse_http_date;
use crate::request::HttpMethod;
use crate::response::TlsInfo;
use axiom_url::Url;

const HEURISTIC_CAP: Duration = Duration::from_secs(24 * 3600);
const HEURISTIC_STATUSES: &[u16] = &[200, 203, 204, 206, 300, 301, 308, 404, 405, 410, 414, 501];
/// Cookies belong to the cookie store only: they are neither kept in memory entries nor
/// written to the disk cache, and a 304 cannot add them back.
const NEVER_STORED_HEADERS: &[&str] = &["set-cookie", "set-cookie2"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    Default,
    NoStore,
    Reload,
    NoCache,
    ForceCache,
    OnlyIfCached,
}

/// How the cache took part in a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheState {
    /// Looked up, not usable; the network response was stored.
    Miss,
    /// Served fresh from the cache without touching the network.
    Hit,
    /// Served from the cache although stale (`force-cache` / `only-if-cached`).
    Stale,
    /// A stored response was confirmed by `304 Not Modified` and served.
    Revalidated,
    /// The cache was not consulted (non-GET, range request, `no-store` / `reload` mode).
    Bypassed,
    /// Looked up, not usable, and the network response may not be stored.
    NotCacheable,
}

impl CacheState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Miss => "miss",
            Self::Hit => "hit",
            Self::Stale => "stale",
            Self::Revalidated => "revalidated",
            Self::Bypassed => "bypassed",
            Self::NotCacheable => "not_cacheable",
        }
    }

    /// The body came from the cache (possibly after a 304).
    pub fn served_from_cache(self) -> bool {
        matches!(self, Self::Hit | Self::Stale | Self::Revalidated)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub method: String,
    pub url: String,
}

impl CacheKey {
    pub fn for_request(method: &HttpMethod, url: &Url) -> Self {
        let mut u = url.clone();
        u.fragment = None;
        Self {
            method: method.as_str().to_string(),
            url: u.as_str(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheDirectives {
    pub no_store: bool,
    pub no_cache: bool,
    pub must_revalidate: bool,
    pub private: bool,
    pub public: bool,
    pub max_age: Option<u64>,
}

impl CacheDirectives {
    pub fn parse(headers: &HeaderMap) -> Self {
        let mut d = Self::default();
        for value in headers.get_all("cache-control") {
            for part in value.split(',') {
                let part = part.trim();
                let (name, arg) = match part.split_once('=') {
                    Some((n, a)) => (n.trim(), Some(a.trim().trim_matches('"'))),
                    None => (part, None),
                };
                match name.to_ascii_lowercase().as_str() {
                    "no-store" => d.no_store = true,
                    "no-cache" => d.no_cache = true,
                    "must-revalidate" | "proxy-revalidate" => d.must_revalidate = true,
                    "private" => d.private = true,
                    "public" => d.public = true,
                    "max-age" => {
                        // Invalid max-age values make the response stale (RFC 9111 §4.2.1).
                        d.max_age = Some(arg.and_then(|a| a.parse().ok()).unwrap_or(0));
                    }
                    _ => {}
                }
            }
        }
        d
    }
}

/// Lower-cased `Vary` field names; `None` for `Vary: *`.
pub fn parse_vary(headers: &HeaderMap) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for value in headers.get_all("vary") {
        for f in value.split(',') {
            let f = f.trim().to_ascii_lowercase();
            if f == "*" {
                return None;
            }
            if !f.is_empty() && !out.contains(&f) {
                out.push(f);
            }
        }
    }
    Some(out)
}

/// Freshness lifetime in seconds (explicit, or heuristic when allowed).
pub fn freshness_lifetime(status: u16, headers: &HeaderMap, response_time_unix: i64) -> u64 {
    let d = CacheDirectives::parse(headers);
    if let Some(ma) = d.max_age {
        return ma;
    }
    let date = headers
        .get("date")
        .and_then(parse_http_date)
        .unwrap_or(response_time_unix);
    if let Some(expires) = headers.get("expires") {
        // Invalid Expires (e.g. "0") means already expired.
        return match parse_http_date(expires) {
            Some(exp) => (exp - date).max(0) as u64,
            None => 0,
        };
    }
    if HEURISTIC_STATUSES.contains(&status) {
        if let Some(lm) = headers.get("last-modified").and_then(parse_http_date) {
            let since = (date - lm).max(0) as u64;
            return (since / 10).min(HEURISTIC_CAP.as_secs());
        }
    }
    0
}

/// Current age at receipt in seconds (RFC 9111 §4.2.3, ignoring request delay).
pub fn initial_age(headers: &HeaderMap, response_time_unix: i64) -> u64 {
    let age_value = headers
        .get("age")
        .and_then(|a| a.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let apparent = headers
        .get("date")
        .and_then(parse_http_date)
        .map(|date| (response_time_unix - date).max(0) as u64)
        .unwrap_or(0);
    age_value.max(apparent)
}

fn has_validators(headers: &HeaderMap) -> bool {
    headers.contains("etag") || headers.contains("last-modified")
}

/// Whether a final response may be stored at all.
pub fn is_storable(
    method: &HttpMethod,
    status: u16,
    req_headers: &HeaderMap,
    resp_headers: &HeaderMap,
) -> bool {
    if *method != HttpMethod::Get || status == 206 || req_headers.contains("range") {
        return false;
    }
    if CacheDirectives::parse(req_headers).no_store {
        return false;
    }
    let d = CacheDirectives::parse(resp_headers);
    if d.no_store || parse_vary(resp_headers).is_none() {
        return false;
    }
    let explicit = d.max_age.is_some() || resp_headers.contains("expires");
    let heuristic = HEURISTIC_STATUSES.contains(&status) && resp_headers.contains("last-modified");
    let understood = explicit || heuristic || d.no_cache;
    understood && (has_validators(resp_headers) || explicit || heuristic)
}

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub status: u16,
    pub headers: HeaderMap,
    /// The body (loaded from disk for a disk-backed cache).
    pub body: Arc<Vec<u8>>,
    pub vary: Vec<String>,
    vary_values: Vec<Option<String>>,
    pub stored_at: Instant,
    pub fresh_until: Instant,
    /// `no-cache`: must be revalidated before every reuse.
    pub requires_validation: bool,
    pub must_revalidate: bool,
    /// TLS facts of the connection the response was received on.
    pub tls: Option<TlsInfo>,
    last_access: u64,
    /// Wall-clock last access (persisted for LRU order across restarts).
    accessed_unix_ms: i64,
    /// Disk object name and body length when the body lives on disk.
    disk: Option<(String, usize)>,
    access_dirty: bool,
}

impl CacheEntry {
    pub fn etag(&self) -> Option<&str> {
        self.headers.get("etag")
    }

    pub fn last_modified(&self) -> Option<&str> {
        self.headers.get("last-modified")
    }

    pub fn has_validators(&self) -> bool {
        has_validators(&self.headers)
    }

    pub fn is_fresh(&self, now: Instant) -> bool {
        !self.requires_validation && now < self.fresh_until
    }

    fn body_len(&self) -> usize {
        self.disk.as_ref().map_or(self.body.len(), |(_, len)| *len)
    }

    fn size(&self) -> usize {
        let headers: usize = self
            .headers
            .iter()
            .map(|(k, v)| k.len() + v.iter().map(|s| s.len()).sum::<usize>())
            .sum();
        self.body_len() + headers + 128
    }

    pub(crate) fn vary_values(&self) -> &[Option<String>] {
        &self.vary_values
    }

    pub(crate) fn accessed_unix_ms(&self) -> i64 {
        self.accessed_unix_ms
    }

    pub(crate) fn from_disk(loaded: LoadedEntry, tick: u64) -> Self {
        let now = Instant::now();
        let remaining_ms = loaded.fresh_until_unix_ms - now_unix_ms();
        let fresh_until = if remaining_ms >= 0 {
            now + Duration::from_millis(remaining_ms as u64)
        } else {
            now.checked_sub(Duration::from_millis(remaining_ms.unsigned_abs()))
                .unwrap_or(now)
        };
        Self {
            status: loaded.status,
            headers: loaded.headers,
            body: Arc::new(Vec::new()),
            vary: loaded.vary,
            vary_values: loaded.vary_values,
            stored_at: now,
            fresh_until,
            requires_validation: loaded.requires_validation,
            must_revalidate: loaded.must_revalidate,
            tls: loaded.tls,
            last_access: tick,
            accessed_unix_ms: loaded.accessed_unix_ms,
            disk: Some((loaded.object, loaded.body_len)),
            access_dirty: false,
        }
    }

    /// Wall-clock end of freshness, for persistence.
    pub(crate) fn fresh_until_unix_ms(&self) -> i64 {
        let now = Instant::now();
        let base = now_unix_ms();
        if self.fresh_until >= now {
            base + (self.fresh_until - now).as_millis() as i64
        } else {
            base - (now - self.fresh_until).as_millis() as i64
        }
    }

    fn matches(&self, req_headers: &HeaderMap) -> bool {
        self.vary
            .iter()
            .zip(&self.vary_values)
            .all(|(name, stored)| req_headers.get(name).map(normalize_vary_value) == *stored)
    }
}

fn normalize_vary_value(v: &str) -> String {
    v.split(',').map(|p| p.trim()).collect::<Vec<_>>().join(",")
}

pub(crate) fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy)]
pub struct CacheLimits {
    pub max_total_bytes: usize,
    pub max_entry_bytes: usize,
}

impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: 64 * 1024 * 1024,
            max_entry_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: usize,
    pub hits: u64,
    pub misses: u64,
    pub revalidations: u64,
    pub evictions: u64,
    pub stores: u64,
    /// Disk reads or writes that failed (the affected entry is dropped, never served).
    pub disk_errors: u64,
    pub disk_backed: bool,
}

#[derive(Default)]
struct Inner {
    map: HashMap<CacheKey, Vec<CacheEntry>>,
    bytes: usize,
}

pub struct HttpCache {
    inner: Mutex<Inner>,
    limits: CacheLimits,
    persistent: bool,
    disk: Option<DiskStore>,
    clock: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
    revalidations: AtomicU64,
    evictions: AtomicU64,
    stores: AtomicU64,
    disk_errors: AtomicU64,
}

impl HttpCache {
    /// Memory cache. `persistent` marks a normal-profile cache that survives service
    /// shutdown; private caches (`false`) are cleared by it.
    pub fn new(limits: CacheLimits, persistent: bool) -> Self {
        Self::build(limits, persistent, None)
    }

    fn build(limits: CacheLimits, persistent: bool, disk: Option<DiskStore>) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            limits,
            persistent,
            disk,
            clock: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            revalidations: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            stores: AtomicU64::new(0),
            disk_errors: AtomicU64::new(0),
        }
    }

    /// Memory-only cache (private browsing); cleared on service shutdown.
    pub fn memory_only(limits: CacheLimits) -> Self {
        Self::new(limits, false)
    }

    /// Disk-backed cache rooted at `dir` (created if missing). Entries written by an
    /// earlier session are loaded; corrupt, partial or orphaned files are removed.
    pub fn open(dir: &Path, limits: CacheLimits) -> std::io::Result<Self> {
        let (disk, loaded) = DiskStore::open(dir)?;
        let cache = Self::build(limits, true, Some(disk));
        let mut victims = Vec::new();
        {
            let mut g = cache.inner.lock();
            let mut loaded = loaded;
            // Oldest first, so ticks reproduce the previous LRU order.
            loaded.sort_by_key(|e| e.accessed_unix_ms);
            for entry in loaded {
                let key = entry.key.clone();
                let e = CacheEntry::from_disk(entry, cache.tick());
                let size = e.size();
                if e.body_len() > limits.max_entry_bytes {
                    victims.extend(e.disk.clone().map(|(name, _)| name));
                    continue;
                }
                g.bytes += size;
                g.map.entry(key).or_default().push(e);
            }
            victims.extend(cache.evict_to_budget(&mut g, None));
        }
        cache.delete_objects(victims);
        Ok(cache)
    }

    pub fn limits(&self) -> CacheLimits {
        self.limits
    }

    pub fn is_persistent(&self) -> bool {
        self.persistent
    }

    pub fn is_disk_backed(&self) -> bool {
        self.disk.is_some()
    }

    /// Remove every entry (and, for a disk-backed cache, every file).
    pub fn clear(&self) {
        {
            let mut g = self.inner.lock();
            g.map.clear();
            g.bytes = 0;
        }
        if let Some(disk) = &self.disk {
            if let Err(e) = disk.clear() {
                self.disk_errors.fetch_add(1, Ordering::Relaxed);
                log::warn!(target: "axiom_net", "http cache clear failed: {e}");
            }
        }
    }

    /// Persist last-access times so LRU order survives a restart.
    pub fn flush(&self) {
        let Some(disk) = &self.disk else { return };
        let dirty: Vec<(CacheKey, CacheEntry)> = {
            let mut g = self.inner.lock();
            let mut out = Vec::new();
            for (k, variants) in g.map.iter_mut() {
                for e in variants.iter_mut().filter(|e| e.access_dirty) {
                    e.access_dirty = false;
                    out.push((k.clone(), e.clone()));
                }
            }
            out
        };
        for (k, e) in dirty {
            if let Some((name, _)) = &e.disk {
                if disk.write_meta(name, &k, &e).is_err() {
                    self.disk_errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    pub fn stats(&self) -> CacheStats {
        let g = self.inner.lock();
        CacheStats {
            entries: g.map.values().map(|v| v.len()).sum(),
            bytes: g.bytes,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            revalidations: self.revalidations.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            stores: self.stores.load(Ordering::Relaxed),
            disk_errors: self.disk_errors.load(Ordering::Relaxed),
            disk_backed: self.disk.is_some(),
        }
    }

    fn delete_objects(&self, names: Vec<String>) {
        if let Some(disk) = &self.disk {
            for name in names {
                disk.remove(&name);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.stats().entries
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_revalidation(&self) {
        self.revalidations.fetch_add(1, Ordering::Relaxed);
    }

    fn tick(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::Relaxed)
    }

    /// Variant matching the request's `Vary`-nominated headers. For a disk-backed cache
    /// the body is read from disk; an unreadable body drops the entry.
    pub fn lookup(&self, key: &CacheKey, req_headers: &HeaderMap) -> Option<CacheEntry> {
        let tick = self.tick();
        let mut entry = {
            let mut g = self.inner.lock();
            let variants = g.map.get_mut(key)?;
            let entry = variants.iter_mut().find(|e| e.matches(req_headers))?;
            entry.last_access = tick;
            entry.accessed_unix_ms = now_unix_ms();
            entry.access_dirty = true;
            entry.clone()
        };
        if let (Some(disk), Some((name, len))) = (&self.disk, &entry.disk) {
            match disk.read_body(name, *len) {
                Ok(body) => entry.body = Arc::new(body),
                Err(e) => {
                    log::warn!(target: "axiom_net", "http cache object {name} unreadable: {e}");
                    self.disk_errors.fetch_add(1, Ordering::Relaxed);
                    self.remove_variant(key, name);
                    return None;
                }
            }
        }
        Some(entry)
    }

    fn remove_variant(&self, key: &CacheKey, object: &str) {
        let removed = {
            let mut g = self.inner.lock();
            let Some(variants) = g.map.get_mut(key) else {
                return;
            };
            let Some(pos) = variants
                .iter()
                .position(|e| e.disk.as_ref().is_some_and(|(n, _)| n == object))
            else {
                return;
            };
            let size = variants.remove(pos).size();
            if variants.is_empty() {
                g.map.remove(key);
            }
            g.bytes -= size;
            object.to_string()
        };
        self.delete_objects(vec![removed]);
    }

    /// Store a complete response. Returns `false` when it is not storable or too large.
    pub fn store(
        &self,
        key: CacheKey,
        req_headers: &HeaderMap,
        status: u16,
        resp_headers: &HeaderMap,
        body: Vec<u8>,
        response_time_unix: i64,
    ) -> bool {
        self.store_with_tls(
            key,
            req_headers,
            status,
            resp_headers,
            body,
            response_time_unix,
            None,
        )
    }

    /// [`store`](Self::store), keeping the TLS facts of the connection with the entry.
    #[allow(clippy::too_many_arguments)]
    pub fn store_with_tls(
        &self,
        key: CacheKey,
        req_headers: &HeaderMap,
        status: u16,
        resp_headers: &HeaderMap,
        body: Vec<u8>,
        response_time_unix: i64,
        tls: Option<TlsInfo>,
    ) -> bool {
        let method = HttpMethod::parse(&key.method).unwrap_or(HttpMethod::Get);
        if !is_storable(&method, status, req_headers, resp_headers) {
            return false;
        }
        let Some(vary) = parse_vary(resp_headers) else {
            return false;
        };
        let vary_values = vary
            .iter()
            .map(|h| req_headers.get(h).map(normalize_vary_value))
            .collect();
        let d = CacheDirectives::parse(resp_headers);
        let now = Instant::now();
        let mut headers = resp_headers.clone();
        for name in NEVER_STORED_HEADERS {
            headers.remove(name);
        }
        let mut entry = CacheEntry {
            status,
            headers,
            body: Arc::new(body),
            vary,
            vary_values,
            stored_at: now,
            fresh_until: now + remaining_freshness(status, resp_headers, response_time_unix),
            requires_validation: d.no_cache,
            must_revalidate: d.must_revalidate,
            tls,
            last_access: self.tick(),
            accessed_unix_ms: now_unix_ms(),
            disk: None,
            access_dirty: false,
        };
        let size = entry.size();
        if size > self.limits.max_entry_bytes || size > self.limits.max_total_bytes {
            return false;
        }
        if let Some(disk) = &self.disk {
            match disk.write(&key, &entry) {
                Ok(name) => {
                    entry.disk = Some((name, entry.body.len()));
                    entry.body = Arc::new(Vec::new());
                }
                Err(e) => {
                    log::warn!(target: "axiom_net", "http cache write failed: {e}");
                    self.disk_errors.fetch_add(1, Ordering::Relaxed);
                    return false;
                }
            }
        }
        let mut stale_objects = Vec::new();
        {
            let mut g = self.inner.lock();
            let variants = g.map.entry(key.clone()).or_default();
            let mut freed = 0;
            if let Some(pos) = variants
                .iter()
                .position(|e| e.vary == entry.vary && e.vary_values == entry.vary_values)
            {
                let old = variants.remove(pos);
                freed = old.size();
                stale_objects.extend(old.disk.map(|(name, _)| name));
            }
            variants.push(entry);
            g.bytes = g.bytes - freed + size;
            stale_objects.extend(self.evict_to_budget(&mut g, Some(&key)));
        }
        self.delete_objects(stale_objects);
        self.stores.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Apply a `304 Not Modified` to the stored variant (RFC 9111 §4.3.4).
    pub fn freshen(
        &self,
        key: &CacheKey,
        req_headers: &HeaderMap,
        not_modified: &HeaderMap,
        response_time_unix: i64,
    ) -> Option<CacheEntry> {
        let tick = self.tick();
        let updated =
            self.freshen_in_memory(key, req_headers, not_modified, response_time_unix, tick)?;
        let mut out = updated.clone();
        if let (Some(disk), Some((name, len))) = (&self.disk, &updated.disk) {
            if disk.write_meta(name, key, &updated).is_err() {
                self.disk_errors.fetch_add(1, Ordering::Relaxed);
            }
            match disk.read_body(name, *len) {
                Ok(body) => out.body = Arc::new(body),
                Err(_) => {
                    self.disk_errors.fetch_add(1, Ordering::Relaxed);
                    self.remove_variant(key, name);
                    return None;
                }
            }
        }
        Some(out)
    }

    fn freshen_in_memory(
        &self,
        key: &CacheKey,
        req_headers: &HeaderMap,
        not_modified: &HeaderMap,
        response_time_unix: i64,
        tick: u64,
    ) -> Option<CacheEntry> {
        let mut g = self.inner.lock();
        let entry = g
            .map
            .get_mut(key)?
            .iter_mut()
            .find(|e| e.matches(req_headers))?;
        for (name, values) in not_modified.iter() {
            if matches!(
                name,
                "content-length" | "content-encoding" | "transfer-encoding"
            ) || NEVER_STORED_HEADERS.contains(&name)
            {
                continue;
            }
            entry.headers.remove(name);
            for v in values {
                entry.headers.append(name, v.clone());
            }
        }
        let d = CacheDirectives::parse(&entry.headers);
        let now = Instant::now();
        entry.fresh_until =
            now + remaining_freshness(entry.status, &entry.headers, response_time_unix);
        entry.requires_validation = d.no_cache;
        entry.must_revalidate = d.must_revalidate;
        entry.last_access = tick;
        entry.accessed_unix_ms = now_unix_ms();
        Some(entry.clone())
    }

    pub fn invalidate(&self, key: &CacheKey) {
        let removed = {
            let mut g = self.inner.lock();
            match g.map.remove(key) {
                Some(removed) => {
                    let freed: usize = removed.iter().map(|e| e.size()).sum();
                    g.bytes -= freed;
                    removed
                }
                None => return,
            }
        };
        self.delete_objects(
            removed
                .into_iter()
                .filter_map(|e| e.disk.map(|(n, _)| n))
                .collect(),
        );
    }

    /// Evict least-recently-used variants until the budget holds. Returns the disk objects
    /// to delete (outside the lock).
    fn evict_to_budget(&self, g: &mut Inner, keep: Option<&CacheKey>) -> Vec<String> {
        let mut objects = Vec::new();
        while g.bytes > self.limits.max_total_bytes {
            let only_one = g.map.len() == 1;
            let victim = g
                .map
                .iter()
                .flat_map(|(k, v)| {
                    v.iter()
                        .enumerate()
                        .map(move |(i, e)| (k, i, e.last_access))
                })
                .filter(|(k, _, _)| Some(*k) != keep || only_one)
                .min_by_key(|(_, _, a)| *a)
                .map(|(k, i, _)| (k.clone(), i));
            let Some((k, i)) = victim else { break };
            let variants = g.map.get_mut(&k).expect("victim exists");
            let removed = variants.remove(i);
            if variants.is_empty() {
                g.map.remove(&k);
            }
            g.bytes -= removed.size();
            objects.extend(removed.disk.map(|(name, _)| name));
            self.evictions.fetch_add(1, Ordering::Relaxed);
        }
        objects
    }
}

impl Drop for HttpCache {
    fn drop(&mut self) {
        self.flush();
    }
}

fn remaining_freshness(status: u16, headers: &HeaderMap, response_time_unix: i64) -> Duration {
    let lifetime = freshness_lifetime(status, headers, response_time_unix);
    let age = initial_age(headers, response_time_unix);
    Duration::from_secs(lifetime.saturating_sub(age))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_date::{format_http_date, now_unix};

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(k, *v);
        }
        h
    }

    fn key(path: &str) -> CacheKey {
        CacheKey::for_request(
            &HttpMethod::Get,
            &Url::parse(&format!("http://cache.test{path}")).unwrap(),
        )
    }

    fn cache() -> HttpCache {
        HttpCache::new(CacheLimits::default(), false)
    }

    #[test]
    fn fragment_is_not_part_of_key() {
        let a = CacheKey::for_request(
            &HttpMethod::Get,
            &Url::parse("http://x.test/a#one").unwrap(),
        );
        let b = CacheKey::for_request(
            &HttpMethod::Get,
            &Url::parse("http://x.test/a#two").unwrap(),
        );
        assert_eq!(a, b);
    }

    #[test]
    fn max_age_and_age_header() {
        let now = now_unix();
        let h = headers(&[("cache-control", "max-age=100"), ("age", "40")]);
        assert_eq!(freshness_lifetime(200, &h, now), 100);
        assert_eq!(initial_age(&h, now), 40);
    }

    #[test]
    fn expires_minus_date() {
        let now = now_unix();
        let h = headers(&[
            ("date", &format_http_date(now)),
            ("expires", &format_http_date(now + 300)),
        ]);
        assert_eq!(freshness_lifetime(200, &h, now), 300);
        let invalid = headers(&[("expires", "0")]);
        assert_eq!(freshness_lifetime(200, &invalid, now), 0);
    }

    #[test]
    fn heuristic_needs_last_modified() {
        let now = now_unix();
        let h = headers(&[
            ("date", &format_http_date(now)),
            ("last-modified", &format_http_date(now - 1000)),
        ]);
        assert_eq!(freshness_lifetime(200, &h, now), 100);
        let capped = headers(&[
            ("date", &format_http_date(now)),
            ("last-modified", &format_http_date(now - 100 * 86_400)),
        ]);
        assert_eq!(freshness_lifetime(200, &capped, now), 86_400);
        assert_eq!(freshness_lifetime(200, &HeaderMap::new(), now), 0);
    }

    #[test]
    fn storability_rules() {
        let get = HttpMethod::Get;
        let none = HeaderMap::new();
        assert!(is_storable(
            &get,
            200,
            &none,
            &headers(&[("cache-control", "max-age=60")])
        ));
        assert!(is_storable(
            &get,
            200,
            &none,
            &headers(&[("cache-control", "private, max-age=60")])
        ));
        assert!(!is_storable(
            &get,
            200,
            &none,
            &headers(&[("cache-control", "no-store, max-age=60")])
        ));
        assert!(!is_storable(
            &get,
            200,
            &none,
            &headers(&[("cache-control", "max-age=60"), ("vary", "*")])
        ));
        assert!(!is_storable(
            &get,
            206,
            &none,
            &headers(&[("cache-control", "max-age=60")])
        ));
        assert!(!is_storable(
            &get,
            200,
            &headers(&[("range", "bytes=0-1")]),
            &headers(&[("cache-control", "max-age=60")])
        ));
        assert!(!is_storable(
            &HttpMethod::Post,
            200,
            &none,
            &headers(&[("cache-control", "max-age=60")])
        ));
        // No freshness and no validators: useless to store.
        assert!(!is_storable(&get, 200, &none, &none));
        // no-cache with a validator is stored (for revalidation).
        assert!(is_storable(
            &get,
            200,
            &none,
            &headers(&[("cache-control", "no-cache"), ("etag", "\"v\"")])
        ));
    }

    #[test]
    fn vary_selects_variant() {
        let c = cache();
        let resp = headers(&[("cache-control", "max-age=60"), ("vary", "Accept-Language")]);
        let en = headers(&[("accept-language", "en")]);
        let fr = headers(&[("accept-language", "fr")]);
        assert!(c.store(key("/v"), &en, 200, &resp, b"hello".to_vec(), now_unix()));
        assert!(c.store(key("/v"), &fr, 200, &resp, b"bonjour".to_vec(), now_unix()));
        assert_eq!(c.lookup(&key("/v"), &en).unwrap().body.as_slice(), b"hello");
        assert_eq!(
            c.lookup(&key("/v"), &fr).unwrap().body.as_slice(),
            b"bonjour"
        );
        assert!(c
            .lookup(&key("/v"), &headers(&[("accept-language", "de")]))
            .is_none());
        assert!(c.lookup(&key("/v"), &HeaderMap::new()).is_none());
    }

    #[test]
    fn no_cache_requires_validation_and_freshen_updates() {
        let c = cache();
        let resp = headers(&[("cache-control", "no-cache"), ("etag", "\"1\"")]);
        assert!(c.store(
            key("/n"),
            &HeaderMap::new(),
            200,
            &resp,
            b"x".to_vec(),
            now_unix()
        ));
        let e = c.lookup(&key("/n"), &HeaderMap::new()).unwrap();
        assert!(!e.is_fresh(Instant::now()));
        let fresh = c
            .freshen(
                &key("/n"),
                &HeaderMap::new(),
                &headers(&[("cache-control", "max-age=60"), ("etag", "\"1\"")]),
                now_unix(),
            )
            .unwrap();
        assert!(fresh.is_fresh(Instant::now()));
        assert_eq!(fresh.body.as_slice(), b"x");
    }

    #[test]
    fn lru_eviction_and_entry_cap() {
        let c = HttpCache::new(
            CacheLimits {
                max_total_bytes: 3000,
                max_entry_bytes: 1500,
            },
            false,
        );
        let resp = headers(&[("cache-control", "max-age=60")]);
        let none = HeaderMap::new();
        assert!(!c.store(key("/big"), &none, 200, &resp, vec![0; 2000], now_unix()));
        assert!(c.store(key("/a"), &none, 200, &resp, vec![0; 1000], now_unix()));
        assert!(c.store(key("/b"), &none, 200, &resp, vec![0; 1000], now_unix()));
        // Touch /a so /b becomes least recently used.
        assert!(c.lookup(&key("/a"), &none).is_some());
        assert!(c.store(key("/c"), &none, 200, &resp, vec![0; 1000], now_unix()));
        assert!(c.lookup(&key("/a"), &none).is_some());
        assert!(c.lookup(&key("/b"), &none).is_none());
        assert!(c.lookup(&key("/c"), &none).is_some());
        assert!(c.stats().evictions >= 1);
        assert!(c.stats().bytes <= 3000);
    }

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static N: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "axiom-cache-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&p);
            Self(p)
        }

        fn files(&self, sub: &str) -> usize {
            std::fs::read_dir(self.0.join(sub))
                .map(|d| d.count())
                .unwrap_or(0)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn set_cookie_is_never_stored_in_memory_or_on_disk() {
        let dir = TempDir::new("cookies");
        let cache = HttpCache::open(&dir.0, CacheLimits::default()).unwrap();
        let none = HeaderMap::new();
        let resp = headers(&[
            ("cache-control", "no-cache"),
            ("etag", "\"1\""),
            ("set-cookie", "sid=topsecret"),
        ]);
        assert!(cache.store(key("/c"), &none, 200, &resp, b"x".to_vec(), now_unix()));
        let not_modified = headers(&[("set-cookie", "sid=rotated-secret")]);
        let fresh = cache
            .freshen(&key("/c"), &none, &not_modified, now_unix())
            .unwrap();
        assert!(!fresh.headers.contains("set-cookie"));
        assert!(!cache
            .lookup(&key("/c"), &none)
            .unwrap()
            .headers
            .contains("set-cookie"));
        for sub in ["index", "objects"] {
            for f in std::fs::read_dir(dir.0.join(sub)).unwrap().flatten() {
                let bytes = std::fs::read(f.path()).unwrap();
                let text = String::from_utf8_lossy(&bytes);
                assert!(!text.contains("secret"), "cookie persisted in {sub}");
            }
        }
    }

    #[test]
    fn disk_cache_survives_reopen_with_vary_and_tls() {
        let dir = TempDir::new("reopen");
        let resp = headers(&[
            ("cache-control", "max-age=600"),
            ("vary", "Accept-Language"),
        ]);
        let en = headers(&[("accept-language", "en")]);
        let tls = TlsInfo {
            alpn: crate::protocol::HttpProtocol::Http2,
            certificate_verified: true,
            version: None,
            cipher_suite: None,
            hostname: Some("cache.test".into()),
            certificate: None,
        };
        {
            let c = HttpCache::open(&dir.0, CacheLimits::default()).unwrap();
            assert!(c.is_disk_backed());
            assert!(c.store_with_tls(
                key("/d"),
                &en,
                200,
                &resp,
                b"persisted".to_vec(),
                now_unix(),
                Some(tls),
            ));
            // Bodies live on disk, not in memory.
            assert_eq!(dir.files("objects"), 1);
            assert_eq!(dir.files("index"), 1);
        }
        let c = HttpCache::open(&dir.0, CacheLimits::default()).unwrap();
        assert_eq!(c.len(), 1);
        let e = c.lookup(&key("/d"), &en).unwrap();
        assert_eq!(e.body.as_slice(), b"persisted");
        assert!(e.is_fresh(Instant::now()));
        let tls = e.tls.expect("tls kept");
        assert_eq!(tls.hostname.as_deref(), Some("cache.test"));
        assert!(c
            .lookup(&key("/d"), &headers(&[("accept-language", "fr")]))
            .is_none());
    }

    #[test]
    fn disk_cache_drops_corrupt_and_orphaned_files() {
        let dir = TempDir::new("corrupt");
        let resp = headers(&[("cache-control", "max-age=600")]);
        let none = HeaderMap::new();
        {
            let c = HttpCache::open(&dir.0, CacheLimits::default()).unwrap();
            assert!(c.store(key("/a"), &none, 200, &resp, b"aaaa".to_vec(), now_unix()));
            assert!(c.store(key("/b"), &none, 200, &resp, b"bbbb".to_vec(), now_unix()));
        }
        // Truncate one body, add an orphan body, a stray temp file and a garbage record.
        let objects: Vec<_> = std::fs::read_dir(dir.0.join("objects"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        std::fs::write(&objects[0], b"x").unwrap();
        std::fs::write(dir.0.join("objects").join("orphan.body"), b"zz").unwrap();
        std::fs::write(dir.0.join("index").join("half.tmp"), b"{").unwrap();
        std::fs::write(dir.0.join("index").join("junk.json"), b"not json").unwrap();

        let c = HttpCache::open(&dir.0, CacheLimits::default()).unwrap();
        assert_eq!(c.len(), 1, "only the intact entry is loaded");
        assert_eq!(dir.files("objects"), 1);
        assert_eq!(dir.files("index"), 1);
        let a = c.lookup(&key("/a"), &none).map(|e| e.body.to_vec());
        let b = c.lookup(&key("/b"), &none).map(|e| e.body.to_vec());
        assert!(a == Some(b"aaaa".to_vec()) || b == Some(b"bbbb".to_vec()));
    }

    #[test]
    fn disk_cache_eviction_and_invalidation_delete_files() {
        let dir = TempDir::new("evict");
        let c = HttpCache::open(
            &dir.0,
            CacheLimits {
                max_total_bytes: 3000,
                max_entry_bytes: 1500,
            },
        )
        .unwrap();
        let resp = headers(&[("cache-control", "max-age=60")]);
        let none = HeaderMap::new();
        assert!(c.store(key("/a"), &none, 200, &resp, vec![1; 1000], now_unix()));
        assert!(c.store(key("/b"), &none, 200, &resp, vec![2; 1000], now_unix()));
        assert!(c.lookup(&key("/a"), &none).is_some());
        assert!(c.store(key("/c"), &none, 200, &resp, vec![3; 1000], now_unix()));
        assert!(c.lookup(&key("/b"), &none).is_none());
        assert_eq!(dir.files("objects"), 2);
        c.invalidate(&key("/a"));
        assert_eq!(dir.files("objects"), 1);
        assert_eq!(dir.files("index"), 1);
        // Replacing an entry replaces its files.
        assert!(c.store(key("/c"), &none, 200, &resp, vec![4; 10], now_unix()));
        assert_eq!(dir.files("objects"), 1);
        assert_eq!(
            c.lookup(&key("/c"), &none).unwrap().body.as_slice(),
            &[4; 10]
        );
        c.clear();
        assert_eq!(dir.files("objects"), 0);
        assert_eq!(dir.files("index"), 0);
    }

    #[test]
    fn disk_cache_keeps_lru_order_across_restart() {
        let dir = TempDir::new("lru");
        let limits = CacheLimits {
            max_total_bytes: 3000,
            max_entry_bytes: 1500,
        };
        let resp = headers(&[("cache-control", "max-age=600")]);
        let none = HeaderMap::new();
        {
            let c = HttpCache::open(&dir.0, limits).unwrap();
            assert!(c.store(key("/old"), &none, 200, &resp, vec![0; 1000], now_unix()));
            std::thread::sleep(Duration::from_millis(5));
            assert!(c.store(key("/new"), &none, 200, &resp, vec![0; 1000], now_unix()));
            std::thread::sleep(Duration::from_millis(5));
            // Touch /old: it becomes most recently used; flushed on drop.
            assert!(c.lookup(&key("/old"), &none).is_some());
        }
        let c = HttpCache::open(&dir.0, limits).unwrap();
        assert!(c.store(key("/third"), &none, 200, &resp, vec![0; 1000], now_unix()));
        assert!(c.lookup(&key("/old"), &none).is_some());
        assert!(c.lookup(&key("/new"), &none).is_none());
    }

    #[test]
    fn memory_cache_never_touches_disk_and_freshen_rewrites_meta() {
        let c = cache();
        assert!(!c.is_disk_backed());
        assert!(!c.stats().disk_backed);

        let dir = TempDir::new("freshen");
        let d = HttpCache::open(&dir.0, CacheLimits::default()).unwrap();
        let resp = headers(&[("cache-control", "no-cache"), ("etag", "\"1\"")]);
        assert!(d.store(
            key("/f"),
            &HeaderMap::new(),
            200,
            &resp,
            b"v".to_vec(),
            now_unix()
        ));
        let fresh = d
            .freshen(
                &key("/f"),
                &HeaderMap::new(),
                &headers(&[("cache-control", "max-age=600"), ("etag", "\"1\"")]),
                now_unix(),
            )
            .unwrap();
        assert_eq!(fresh.body.as_slice(), b"v");
        drop(d);
        let d = HttpCache::open(&dir.0, CacheLimits::default()).unwrap();
        assert!(d
            .lookup(&key("/f"), &HeaderMap::new())
            .unwrap()
            .is_fresh(Instant::now()));
    }
}
