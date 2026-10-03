//! Resource loading for a browsing context, above the profile's [`RequestScheduler`].
//!
//! * Every document, stylesheet, script, image and font request goes through
//!   [`ResourceLoader::start`] → scheduler → network service (no private HTTP clients).
//! * Requests are asynchronous; [`ResourceLoader::poll`] is non-blocking and
//!   [`ResourceLoader::wait_for`] blocks only for the requests a caller must have (e.g.
//!   render-blocking CSS), while other requests keep streaming in the background.
//! * Each loader is one cancellation context: dropping it or calling
//!   [`ResourceLoader::cancel_all_for_context`] cancels its queued and in-flight requests.

mod svg;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axiom_trace::{TraceKind, TraceTimeline};
use axiom_url::{Url, UrlError};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender};
use parking_lot::Mutex;
use thiserror::Error;

pub use axiom_net::{
    is_redirect_status, same_origin, CacheMode, CacheState, CredentialsMode, DownloadCandidate,
    FlowControl, HeaderMap, HttpCache, HttpMethod, HttpProtocol, MimeType, NetworkError,
    NetworkRequestId as RequestId, NetworkTiming, RedirectCheck, RedirectMode, RedirectRecord,
    ReferrerPolicy, RequestBody, RequestMode, RequestPriority, ResourceType, StreamingBody,
    TlsInfo, UploadSender,
};
use axiom_net::{
    NetworkEvent, NetworkRequest, NetworkService, NetworkServiceConfig, RequestScheduler,
    ResponseMeta, SchedulerConfig,
};

static NEXT_CONTEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Events buffered between the scheduler and this loader (backpressure bound).
const EVENT_CHANNEL_CAPACITY: usize = 256;
const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Clone)]
pub struct ResourceRequest {
    pub id: RequestId,
    pub url: Url,
    pub method: HttpMethod,
    pub headers: HeaderMap,
    pub body: RequestBody,
    pub resource_type: ResourceType,
    pub priority: RequestPriority,
    pub initiator: String,
    /// Requesting document (referrer source and request origin).
    pub document_url: Option<Url>,
    /// Top-level document for SameSite decisions (defaults to `document_url`).
    pub top_level_url: Option<Url>,
    pub cache_mode: CacheMode,
    pub credentials_mode: CredentialsMode,
    pub referrer_policy: ReferrerPolicy,
    pub mode: RequestMode,
    pub redirect_mode: RedirectMode,
    /// Deliver the response as `Response` + `Chunk` events instead of one assembled body.
    pub stream_body: bool,
    /// Overrides the loader's per-type body limit.
    pub max_body_bytes: Option<u64>,
    /// Referrer source when it is not the requesting document (must be same-origin with
    /// it; the referrer policy still applies).
    pub referrer: Option<Url>,
    /// Consumer-driven backpressure for `stream_body` requests.
    pub flow: Option<FlowControl>,
    /// Whole-request deadline; `None` for none (e.g. script `fetch()`, which has its own
    /// cancellation).
    pub request_timeout_ms: Option<u64>,
    /// See [`NetworkRequest::unsafe_request`].
    pub unsafe_request: bool,
    /// See [`NetworkRequest::use_cors_preflight`].
    pub use_cors_preflight: bool,
    /// See [`NetworkRequest::redirect_check`].
    pub redirect_check: Option<RedirectCheck>,
}

impl ResourceRequest {
    pub fn new(url: Url, resource_type: ResourceType) -> Self {
        Self {
            id: RequestId::new(),
            url,
            method: HttpMethod::Get,
            headers: HeaderMap::new(),
            body: RequestBody::Empty,
            resource_type,
            priority: resource_type.default_priority(),
            initiator: resource_type.as_str().to_string(),
            document_url: None,
            top_level_url: None,
            cache_mode: CacheMode::Default,
            credentials_mode: CredentialsMode::Include,
            referrer_policy: ReferrerPolicy::default(),
            mode: if resource_type == ResourceType::Document {
                RequestMode::Navigate
            } else {
                RequestMode::NoCors
            },
            redirect_mode: RedirectMode::Follow,
            stream_body: false,
            max_body_bytes: None,
            referrer: None,
            flow: None,
            request_timeout_ms: Some(DEFAULT_REQUEST_TIMEOUT_MS),
            unsafe_request: false,
            use_cors_preflight: false,
            redirect_check: None,
        }
    }

    pub fn document(url: Url) -> Self {
        let mut r = Self::new(url, ResourceType::Document);
        r.initiator = "navigation".into();
        r
    }

    pub fn stylesheet(url: Url) -> Self {
        Self::new(url, ResourceType::Stylesheet)
    }

    pub fn script(url: Url) -> Self {
        Self::new(url, ResourceType::Script)
    }

    pub fn image(url: Url) -> Self {
        Self::new(url, ResourceType::Image)
    }

    pub fn font(url: Url) -> Self {
        Self::new(url, ResourceType::Font)
    }

    pub fn from_document(mut self, document_url: &Url) -> Self {
        self.document_url = Some(document_url.clone());
        self
    }

    pub fn with_cache_mode(mut self, mode: CacheMode) -> Self {
        self.cache_mode = mode;
        self
    }

    pub fn with_priority(mut self, priority: RequestPriority) -> Self {
        self.priority = priority;
        self
    }
}

#[derive(Debug, Clone)]
pub struct ResourceResponse {
    pub id: RequestId,
    pub resource_type: ResourceType,
    /// Final URL after redirects.
    pub url: Url,
    pub status: u16,
    /// Reason phrase as received (empty for HTTP/2 and cached responses).
    pub status_text: String,
    pub headers: HeaderMap,
    pub mime: Option<MimeType>,
    /// Empty for `stream_body` requests (the body arrived as `Chunk` events).
    pub body: Vec<u8>,
    pub cache_state: CacheState,
    pub protocol: HttpProtocol,
    /// Facts about the validated TLS connection (`None` for plain HTTP).
    pub tls: Option<TlsInfo>,
    pub transferred_bytes: u64,
    pub decoded_bytes: u64,
    pub redirect_chain: Vec<RedirectRecord>,
    pub download: Option<DownloadCandidate>,
    pub timing: NetworkTiming,
}

impl ResourceResponse {
    pub fn from_cache(&self) -> bool {
        self.cache_state.served_from_cache()
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Body decoded with the declared charset (UTF-8 / BOM / windows-1252).
    pub fn text(&self) -> String {
        axiom_net::decode_text(
            &self.body,
            self.mime.as_ref().and_then(|m| m.charset.as_deref()),
        )
    }
}

#[derive(Debug, Error)]
pub enum LoaderError {
    #[error(transparent)]
    Url(#[from] UrlError),
    #[error(transparent)]
    Network(#[from] NetworkError),
    #[error("decode: {0}")]
    Decode(String),
}

impl LoaderError {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Url(_) => "url",
            Self::Network(e) => e.kind_name(),
            Self::Decode(_) => "decode",
        }
    }
}

#[derive(Debug)]
pub enum LoaderEvent {
    /// Headers of a `stream_body` request (body empty); `Chunk`s and then `Completed` or
    /// `Failed` follow.
    Response(Box<ResourceResponse>),
    /// Decoded body bytes of a `stream_body` request, in order.
    Chunk {
        id: RequestId,
        bytes: Vec<u8>,
    },
    Completed(Box<ResourceResponse>),
    Failed {
        id: RequestId,
        resource_type: ResourceType,
        url: Url,
        error: NetworkError,
    },
}

impl LoaderEvent {
    pub fn id(&self) -> RequestId {
        match self {
            Self::Response(r) | Self::Completed(r) => r.id,
            Self::Chunk { id, .. } | Self::Failed { id, .. } => *id,
        }
    }

    /// `Completed` / `Failed` end a request; `Response` / `Chunk` are streaming progress.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed(_) | Self::Failed { .. })
    }
}

/// Loader-level policy, checked before a request reaches the network (e.g. per-document
/// content blocking). Network-level policies live on the `NetworkService`.
pub trait RequestPolicy: Send + Sync {
    fn name(&self) -> &str;
    fn check(&self, request: &ResourceRequest) -> Result<(), String>;
}

/// Maximum decoded body size per resource type.
#[derive(Debug, Clone, Copy)]
pub struct LoaderLimits {
    pub document: u64,
    pub stylesheet: u64,
    pub script: u64,
    pub image: u64,
    pub font: u64,
    pub other: u64,
}

impl Default for LoaderLimits {
    fn default() -> Self {
        const MB: u64 = 1024 * 1024;
        Self {
            document: 32 * MB,
            stylesheet: 8 * MB,
            script: 16 * MB,
            image: 32 * MB,
            font: 16 * MB,
            other: 64 * MB,
        }
    }
}

impl LoaderLimits {
    pub fn for_type(&self, t: ResourceType) -> u64 {
        match t {
            ResourceType::Document => self.document,
            ResourceType::Stylesheet => self.stylesheet,
            ResourceType::Script => self.script,
            ResourceType::Image => self.image,
            ResourceType::Font => self.font,
            _ => self.other,
        }
    }
}

struct Pending {
    url: Url,
    resource_type: ResourceType,
    started: Instant,
    stream: bool,
    meta: Option<Box<ResponseMeta>>,
    body: Vec<u8>,
}

pub struct ResourceLoader {
    // Dropped first so blocked scheduler workers see a disconnected channel immediately.
    rx: Receiver<NetworkEvent>,
    tx: Sender<NetworkEvent>,
    pending: Mutex<HashMap<RequestId, Pending>>,
    ready: Mutex<VecDeque<LoaderEvent>>,
    policies: Vec<Arc<dyn RequestPolicy>>,
    limits: LoaderLimits,
    context_id: u64,
    scheduler: Arc<RequestScheduler>,
    pub trace: Arc<TraceTimeline>,
}

impl ResourceLoader {
    /// Standalone loader with its own network service and scheduler (tools, tests, the
    /// Phase 1 `Engine`). Browser tabs use [`ResourceLoader::shared`] instead.
    pub fn new(trace: Arc<TraceTimeline>) -> Self {
        let service = Arc::new(NetworkService::new(NetworkServiceConfig::default()));
        let scheduler = Arc::new(RequestScheduler::new(service, SchedulerConfig::default()));
        Self::shared(scheduler, trace)
    }

    /// Loader on a profile-owned scheduler (and therefore its service, cache and cookies).
    pub fn shared(scheduler: Arc<RequestScheduler>, trace: Arc<TraceTimeline>) -> Self {
        let (tx, rx) = bounded(EVENT_CHANNEL_CAPACITY);
        Self {
            rx,
            tx,
            pending: Mutex::new(HashMap::new()),
            ready: Mutex::new(VecDeque::new()),
            policies: Vec::new(),
            limits: LoaderLimits::default(),
            context_id: NEXT_CONTEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            scheduler,
            trace,
        }
    }

    pub fn context_id(&self) -> u64 {
        self.context_id
    }

    pub fn scheduler(&self) -> &Arc<RequestScheduler> {
        &self.scheduler
    }

    pub fn service(&self) -> &Arc<NetworkService> {
        self.scheduler.service()
    }

    pub fn cache(&self) -> &Arc<HttpCache> {
        self.service().cache()
    }

    pub fn add_policy(&mut self, policy: Arc<dyn RequestPolicy>) {
        self.policies.push(policy);
    }

    pub fn set_limits(&mut self, limits: LoaderLimits) {
        self.limits = limits;
    }

    pub fn limits(&self) -> LoaderLimits {
        self.limits
    }

    /// Requests started and not yet completed / failed / cancelled.
    pub fn pending_count(&self) -> usize {
        self.pending.lock().len()
    }

    /// Queue a request. Its outcome arrives through [`poll`](Self::poll) or
    /// [`wait_for`](Self::wait_for).
    pub fn start(&self, request: ResourceRequest) -> RequestId {
        let id = request.id;
        let fail = |error: NetworkError| {
            self.ready.lock().push_back(LoaderEvent::Failed {
                id,
                resource_type: request.resource_type,
                url: request.url.clone(),
                error,
            });
        };
        for p in &self.policies {
            if let Err(why) = p.check(&request) {
                fail(NetworkError::Blocked(format!("{}: {why}", p.name())));
                return id;
            }
        }
        let net = self.to_network_request(&request);
        self.pending.lock().insert(
            id,
            Pending {
                url: request.url.clone(),
                resource_type: request.resource_type,
                started: Instant::now(),
                stream: request.stream_body,
                meta: None,
                body: Vec::new(),
            },
        );
        if let Err(e) = self.scheduler.enqueue(net, self.tx.clone()) {
            self.pending.lock().remove(&id);
            fail(e);
        }
        id
    }

    fn to_network_request(&self, r: &ResourceRequest) -> NetworkRequest {
        let mut n = NetworkRequest::new(r.method.clone(), r.url.clone(), r.resource_type);
        n.id = r.id;
        n.headers = r.headers.clone();
        n.body = r.body.clone();
        n.priority = r.priority;
        n.cache_mode = r.cache_mode;
        n.credentials_mode = r.credentials_mode;
        n.referrer_policy = r.referrer_policy;
        n.mode = r.mode;
        n.redirect_mode = r.redirect_mode;
        n.referrer = r
            .referrer
            .as_ref()
            .or(r.document_url.as_ref())
            .map(|u| u.as_str());
        n.origin = r.document_url.clone();
        n.flow = r.flow.clone();
        n.request_timeout_ms = r.request_timeout_ms;
        n.unsafe_request = r.unsafe_request;
        n.use_cors_preflight = r.use_cors_preflight;
        n.redirect_check = r.redirect_check.clone();
        n.top_level_url = if r.resource_type == ResourceType::Document {
            Some(r.url.clone())
        } else {
            r.top_level_url.clone().or_else(|| r.document_url.clone())
        };
        n.initiator = r.initiator.clone();
        n.context_id = Some(self.context_id);
        n.max_body_bytes = Some(
            r.max_body_bytes
                .unwrap_or_else(|| self.limits.for_type(r.resource_type)),
        );
        n
    }

    /// Non-blocking: every event that is ready now.
    pub fn poll(&self) -> Vec<LoaderEvent> {
        let mut out: Vec<LoaderEvent> = self.ready.lock().drain(..).collect();
        while let Ok(ev) = self.rx.try_recv() {
            if let Some(done) = self.handle(ev) {
                out.push(done);
            }
        }
        out
    }

    /// Block until each of `ids` completes or fails (or `timeout` passes — remaining ones
    /// are cancelled and reported as timeouts). Events for other requests are kept for
    /// the next [`poll`](Self::poll).
    pub fn wait_for(&self, ids: &[RequestId], timeout: Duration) -> Vec<LoaderEvent> {
        let deadline = Instant::now() + timeout;
        let mut remaining: HashSet<RequestId> = ids.iter().copied().collect();
        let mut out = Vec::new();
        {
            let mut ready = self.ready.lock();
            let mut keep = VecDeque::new();
            for ev in ready.drain(..) {
                if ev.is_terminal() && remaining.remove(&ev.id()) {
                    out.push(ev);
                } else {
                    keep.push_back(ev);
                }
            }
            *ready = keep;
        }
        while !remaining.is_empty() {
            let now = Instant::now();
            if now >= deadline {
                for id in remaining.drain() {
                    self.scheduler.cancel(id);
                    if let Some(p) = self.pending.lock().remove(&id) {
                        out.push(LoaderEvent::Failed {
                            id,
                            resource_type: p.resource_type,
                            url: p.url,
                            error: NetworkError::Timeout,
                        });
                    }
                }
                break;
            }
            match self.rx.recv_timeout(deadline - now) {
                Ok(ev) => {
                    if let Some(done) = self.handle(ev) {
                        if done.is_terminal() && remaining.remove(&done.id()) {
                            out.push(done);
                        } else {
                            self.ready.lock().push_back(done);
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            // Requests that are no longer pending (cancelled elsewhere) cannot complete.
            let pending = self.pending.lock();
            remaining.retain(|id| pending.contains_key(id));
        }
        out
    }

    /// Block until at least one event is ready (or `timeout` passes), then return every
    /// ready event. Event-driven: wakes on the next network event, never polls on a timer.
    pub fn wait_next(&self, timeout: Duration) -> Vec<LoaderEvent> {
        let deadline = Instant::now() + timeout;
        loop {
            let out = self.poll();
            if !out.is_empty() {
                return out;
            }
            let now = Instant::now();
            if now >= deadline {
                return out;
            }
            match self.rx.recv_timeout(deadline - now) {
                Ok(ev) => {
                    if let Some(done) = self.handle(ev) {
                        self.ready.lock().push_back(done);
                    }
                }
                Err(RecvTimeoutError::Timeout) => return Vec::new(),
                Err(RecvTimeoutError::Disconnected) => return self.poll(),
            }
        }
    }

    pub fn is_pending(&self, id: RequestId) -> bool {
        self.pending.lock().contains_key(&id)
    }

    /// Cancel every request of this context except `keep` (document replacement: the new
    /// navigation survives, the old document's subresources do not). Buffered events of
    /// cancelled requests are discarded; late network events for them are ignored.
    pub fn cancel_all_except(&self, keep: &[RequestId]) -> Vec<RequestId> {
        let cancelled: Vec<RequestId> = {
            let mut pending = self.pending.lock();
            let ids: Vec<RequestId> = pending
                .keys()
                .copied()
                .filter(|id| !keep.contains(id))
                .collect();
            for id in &ids {
                pending.remove(id);
            }
            ids
        };
        for id in &cancelled {
            self.scheduler.cancel(*id);
        }
        self.ready.lock().retain(|ev| keep.contains(&ev.id()));
        cancelled
    }

    /// Start a request and wait for it.
    pub fn fetch_sync(
        &self,
        request: ResourceRequest,
        timeout: Duration,
    ) -> Result<ResourceResponse, LoaderError> {
        let id = self.start(request);
        match self.wait_for(&[id], timeout).pop() {
            Some(LoaderEvent::Completed(r)) => Ok(*r),
            Some(LoaderEvent::Failed { error, .. }) => Err(error.into()),
            Some(LoaderEvent::Response(_) | LoaderEvent::Chunk { .. }) | None => {
                Err(NetworkError::Cancelled.into())
            }
        }
    }

    /// Cancel one request; a `Failed(Cancelled)` event is reported.
    pub fn cancel(&self, id: RequestId) {
        self.scheduler.cancel(id);
        if let Some(p) = self.pending.lock().remove(&id) {
            self.ready.lock().push_back(LoaderEvent::Failed {
                id,
                resource_type: p.resource_type,
                url: p.url,
                error: NetworkError::Cancelled,
            });
        }
    }

    /// Cancel everything this context started (navigation, stop, tab close). Outstanding
    /// events for those requests are discarded.
    pub fn cancel_all_for_context(&self) {
        self.scheduler.cancel_context(self.context_id);
        self.pending.lock().clear();
        self.ready.lock().clear();
        while self.rx.try_recv().is_ok() {}
    }

    fn handle(&self, ev: NetworkEvent) -> Option<LoaderEvent> {
        let mut pending = self.pending.lock();
        match ev {
            NetworkEvent::Response { id, meta } => {
                let p = pending.get_mut(&id)?;
                if p.resource_type == ResourceType::Document && !p.stream && meta.download.is_some()
                {
                    // A download is handed to the download manager; the document request
                    // stops at headers instead of buffering the file.
                    let p = pending.remove(&id)?;
                    drop(pending);
                    self.scheduler.cancel(id);
                    return Some(LoaderEvent::Completed(Box::new(response_from_meta(
                        id,
                        p.resource_type,
                        &meta,
                        Vec::new(),
                    ))));
                }
                let head = p
                    .stream
                    .then(|| response_from_meta(id, p.resource_type, &meta, Vec::new()));
                p.meta = Some(meta);
                head.map(|r| LoaderEvent::Response(Box::new(r)))
            }
            NetworkEvent::Data { id, bytes } => {
                let p = pending.get_mut(&id)?;
                if p.stream {
                    return Some(LoaderEvent::Chunk { id, bytes });
                }
                p.body.extend_from_slice(&bytes);
                None
            }
            NetworkEvent::Complete { id } => {
                let p = pending.remove(&id)?;
                drop(pending);
                let Some(meta) = p.meta else {
                    return Some(LoaderEvent::Failed {
                        id,
                        resource_type: p.resource_type,
                        url: p.url,
                        error: NetworkError::Protocol("completed without response".into()),
                    });
                };
                self.trace.record(
                    TraceKind::Network,
                    format!("{} {}", p.resource_type.as_str(), meta.final_url),
                    p.started.elapsed(),
                    Some(format!("{} {}", meta.status, meta.cache_state.as_str())),
                );
                Some(LoaderEvent::Completed(Box::new(response_from_meta(
                    id,
                    p.resource_type,
                    &meta,
                    p.body,
                ))))
            }
            NetworkEvent::Failed { id, error } => {
                let p = pending.remove(&id)?;
                Some(LoaderEvent::Failed {
                    id,
                    resource_type: p.resource_type,
                    url: p.url,
                    error,
                })
            }
        }
    }
}

fn response_from_meta(
    id: RequestId,
    resource_type: ResourceType,
    meta: &ResponseMeta,
    body: Vec<u8>,
) -> ResourceResponse {
    ResourceResponse {
        id,
        resource_type,
        transferred_bytes: meta.transferred_bytes(),
        decoded_bytes: meta.decoded_bytes(),
        url: meta.final_url.clone(),
        status: meta.status,
        status_text: meta.reason.clone().unwrap_or_default(),
        headers: meta.headers.clone(),
        mime: meta.mime.clone(),
        body,
        cache_state: meta.cache_state,
        protocol: meta.protocol,
        tls: meta.tls.clone(),
        redirect_chain: meta.redirect_chain.clone(),
        download: meta.download.clone(),
        timing: meta.timing.clone(),
    }
}

impl Drop for ResourceLoader {
    fn drop(&mut self) {
        self.scheduler.cancel_context(self.context_id);
    }
}

/// Decode image bytes into RGBA8 + intrinsic size.
pub fn decode_image(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), LoaderError> {
    if image::guess_format(bytes).is_err() && svg::looks_like_svg(bytes) {
        return svg::decode_svg(bytes).map_err(|e| LoaderError::Decode(format!("SVG: {e}")));
    }
    let img = image::load_from_memory(bytes).map_err(|e| LoaderError::Decode(e.to_string()))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok((w, h, rgba.into_raw()))
}

/// data: URL support (text and base64).
pub fn parse_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    let rest = url.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let data = percent_decode(data.split('#').next().unwrap_or(""));
    let mut params: Vec<&str> = meta.split(';').map(str::trim).collect();
    let base64 = params
        .last()
        .is_some_and(|p| p.eq_ignore_ascii_case("base64"));
    if base64 {
        params.pop();
    }
    let mime = if params.first().is_none_or(|t| t.is_empty()) {
        "text/plain;charset=US-ASCII".to_string()
    } else {
        params.join(";")
    };
    if base64 {
        let text = std::str::from_utf8(&data).ok()?;
        Some((mime, decode_base64(text)?))
    } else {
        Some((mime, data))
    }
}

fn percent_decode(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for &c in input.as_bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let val = TABLE.iter().position(|&x| x == c)? as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{decode_image, parse_data_url};

    #[test]
    fn favicon_formats_decode() {
        let img = image::RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([x as u8 * 80, y as u8 * 90, 7, 255])
        });
        for format in [image::ImageFormat::Ico, image::ImageFormat::Bmp] {
            let mut bytes = std::io::Cursor::new(Vec::new());
            img.write_to(&mut bytes, format).unwrap();
            let (w, h, rgba) = decode_image(bytes.get_ref()).unwrap();
            assert_eq!((w, h), (3, 2), "{format:?}");
            assert_eq!(&rgba[..4], &[0, 0, 7, 255], "{format:?}");
            assert_eq!(&rgba[rgba.len() - 4..], &[160, 90, 7, 255], "{format:?}");
        }
    }

    #[test]
    fn data_urls_are_percent_decoded_and_keep_mime_parameters() {
        assert_eq!(
            parse_data_url("data:text/plain;charset=utf-8,hello%20world%21"),
            Some(("text/plain;charset=utf-8".into(), b"hello world!".to_vec()))
        );
        assert_eq!(
            parse_data_url("data:,a%zzb"),
            Some(("text/plain;charset=US-ASCII".into(), b"a%zzb".to_vec()))
        );
        assert_eq!(
            parse_data_url("data:text/html;base64,PGI%2BeDwvYj4="),
            Some(("text/html".into(), b"<b>x</b>".to_vec()))
        );
        assert_eq!(parse_data_url("data:text/plain;base64,@@@"), None);
        assert_eq!(parse_data_url("data:no-comma"), None);
    }
}
