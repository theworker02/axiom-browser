//! HTTP transport: async reqwest (hyper, rustls, HTTP/1.1 + HTTP/2 via ALPN) driven by a
//! runtime owned by the transport.
//!
//! * Callers stay synchronous: `execute` and body reads `block_on` the runtime from the
//!   caller's thread (never call them from inside an async context).
//! * Cancellation: every await races the request's [`CancellationToken`], so cancelling
//!   interrupts DNS, connect, TLS, waiting for headers, and body reads.
//! * DNS goes through the service's [`DnsResolver`].
//! * New connections are counted by a connector layer (`connections_opened`); reuse is not
//!   observable and is not reported.
//! * Content decoding is disabled here; the service decodes so transferred vs decoded byte
//!   counts are honest.
//! * TLS verification is always on; extra trust anchors can be added, never disabled.

use std::io::{self, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Buf;

use crate::body::{RequestBody, UploadItem, UploadSource};
use crate::cancel::CancellationToken;
use crate::dns::{DnsFailure, DnsResolver, ReqwestDnsAdapter};
use crate::error::{into_io, CertificateErrorKind, NetworkError};
use crate::headers::HeaderMap;
use crate::metrics::PoolStats;
use crate::protocol::HttpProtocol;
use crate::request::NetworkRequest;
use crate::response::{CertificateInfo, TlsInfo};

const UPLOAD_CHUNK: usize = 64 * 1024;
/// Reader uploads stop reading ahead once this much is queued for the connection.
const UPLOAD_READ_AHEAD: u64 = 4 * UPLOAD_CHUNK as u64;

pub trait Transport: Send + Sync {
    fn execute(
        &self,
        req: &NetworkRequest,
        cancel: &CancellationToken,
    ) -> Result<RawTransportResponse, NetworkError>;

    fn pool_stats(&self) -> PoolStats {
        PoolStats::default()
    }
}

pub struct RawTransportResponse {
    pub status: u16,
    pub reason: Option<String>,
    pub headers: HeaderMap,
    pub protocol: HttpProtocol,
    pub tls: Option<TlsInfo>,
    /// Encoded body bytes as received (no content decoding applied).
    pub body: Box<dyn Read + Send>,
}

#[derive(Clone, Default)]
pub struct TransportConfig {
    pub connect_timeout: Option<Duration>,
    /// Longest wait for the next piece of the response (headers or a body chunk).
    /// Not applied while a streamed request body is still being uploaded.
    pub read_timeout: Option<Duration>,
    /// Additional PEM trust anchors (e.g. an enterprise or test CA). Verification remains on.
    pub extra_root_certs_pem: Vec<Vec<u8>>,
    /// Disable HTTP/2 (diagnostics / tests).
    pub http1_only: bool,
}

struct RuntimeHolder {
    runtime: Option<tokio::runtime::Runtime>,
}

impl RuntimeHolder {
    fn block_on<F: std::future::Future>(&self, f: F) -> F::Output {
        self.runtime
            .as_ref()
            .expect("network runtime alive while referenced")
            .block_on(f)
    }

    /// Streaming request body. Reader sources are pumped on the blocking pool in
    /// `UPLOAD_CHUNK` pieces, at most `UPLOAD_READ_AHEAD` ahead of the connection;
    /// channel sources are forwarded as they arrive (their producer paces itself on
    /// `UploadSender::buffered`).
    fn upload_body(&self, source: UploadSource) -> reqwest::Body {
        let (rx, buffered) = match source {
            UploadSource::Channel { rx, buffered } => (rx, buffered),
            UploadSource::Reader(mut reader) => {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                let buffered = Arc::new(AtomicU64::new(0));
                let queued = Arc::clone(&buffered);
                let runtime = self
                    .runtime
                    .as_ref()
                    .expect("network runtime alive while referenced");
                runtime.spawn_blocking(move || {
                    let mut buf = vec![0u8; UPLOAD_CHUNK];
                    loop {
                        while queued.load(Ordering::SeqCst) >= UPLOAD_READ_AHEAD {
                            if tx.is_closed() {
                                return;
                            }
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        let item = match reader.read(&mut buf) {
                            Ok(0) => UploadItem::End,
                            Ok(n) => {
                                queued.fetch_add(n as u64, Ordering::SeqCst);
                                UploadItem::Data(buf[..n].to_vec())
                            }
                            Err(e) => UploadItem::Error(e.to_string()),
                        };
                        let last = !matches!(item, UploadItem::Data(_));
                        if tx.send(item).is_err() || last {
                            return;
                        }
                    }
                });
                (rx, buffered)
            }
        };
        let stream = futures_util::stream::unfold(Some(rx), move |state| {
            let buffered = Arc::clone(&buffered);
            async move {
                let mut rx = state?;
                match rx.recv().await {
                    Some(UploadItem::Data(bytes)) => {
                        buffered.fetch_sub(bytes.len() as u64, Ordering::SeqCst);
                        Some((Ok(bytes::Bytes::from(bytes)), Some(rx)))
                    }
                    Some(UploadItem::End) => None,
                    Some(UploadItem::Error(e)) => Some((Err(io::Error::other(e)), None)),
                    None => Some((Err(io::Error::other("request body stream aborted")), None)),
                }
            }
        });
        reqwest::Body::wrap_stream(stream)
    }
}

impl Drop for RuntimeHolder {
    fn drop(&mut self) {
        if let Some(rt) = self.runtime.take() {
            // Don't wait for blocking DNS lookups that may still be running.
            rt.shutdown_background();
        }
    }
}

pub struct ReqwestTransport {
    client: reqwest::Client,
    runtime: Arc<RuntimeHolder>,
    opened: Arc<AtomicU64>,
    read_timeout: Option<Duration>,
}

impl ReqwestTransport {
    pub fn new(config: &TransportConfig, dns: Arc<dyn DnsResolver>) -> Result<Self, NetworkError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(8)
            .thread_name("axiom-net-io")
            .enable_all()
            .build()
            .map_err(|e| NetworkError::Protocol(format!("network runtime: {e}")))?;
        let opened = Arc::new(AtomicU64::new(0));
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(Arc::new(ReqwestDnsAdapter { inner: dns }))
            .connector_layer(CountConnectionsLayer(Arc::clone(&opened)))
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(8)
            .tls_info(true);
        if let Some(t) = config.connect_timeout {
            builder = builder.connect_timeout(t);
        }
        for pem in &config.extra_root_certs_pem {
            let cert = reqwest::Certificate::from_pem(pem)
                .map_err(|e| NetworkError::Tls(format!("invalid extra root certificate: {e}")))?;
            builder = builder.add_root_certificate(cert);
        }
        if config.http1_only {
            builder = builder.http1_only();
        }
        let client = {
            let _guard = runtime.enter();
            builder
                .build()
                .map_err(|e| NetworkError::Protocol(format!("http client: {e}")))?
        };
        Ok(Self {
            client,
            runtime: Arc::new(RuntimeHolder {
                runtime: Some(runtime),
            }),
            opened,
            read_timeout: config.read_timeout,
        })
    }

    pub fn connections_opened(&self) -> u64 {
        self.opened.load(Ordering::Relaxed)
    }
}

impl Transport for ReqwestTransport {
    fn execute(
        &self,
        req: &NetworkRequest,
        cancel: &CancellationToken,
    ) -> Result<RawTransportResponse, NetworkError> {
        cancel.check()?;
        let method = reqwest::Method::from_bytes(req.method.as_str().as_bytes()).map_err(|_| {
            NetworkError::Protocol(format!("invalid method {}", req.method.as_str()))
        })?;
        let mut url = req.url.clone();
        url.fragment = None;
        let url =
            reqwest::Url::parse(&url.as_str()).map_err(|e| NetworkError::Url(e.to_string()))?;
        let https = url.scheme() == "https";
        let hostname = url.host_str().map(str::to_string);

        let mut builder = self.client.request(method, url);
        for (name, values) in req.headers.iter() {
            for v in values {
                builder = builder.header(name, v.as_str());
            }
        }
        match &req.body {
            RequestBody::Empty => {}
            RequestBody::Bytes(b) => builder = builder.body(b.clone()),
            RequestBody::Stream(s) => {
                let source = s.take_source()?;
                if let Some(len) = s.length {
                    builder = builder.header("content-length", len.to_string());
                }
                builder = builder.body(self.runtime.upload_body(source));
            }
        }
        let deadline = req
            .request_timeout_ms
            .map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));
        let header_idle = match req.body {
            RequestBody::Stream(_) => None,
            _ => self.read_timeout,
        };

        let response = self.runtime.block_on(async {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(NetworkError::Cancelled),
                r = with_timeouts(deadline, header_idle, builder.send()) => r.and_then(|r| r.map_err(|e| classify(&e))),
            }
        })?;

        let status = response.status().as_u16();
        let reason = response.status().canonical_reason().map(str::to_string);
        let protocol = match response.version() {
            reqwest::Version::HTTP_2 => HttpProtocol::Http2,
            reqwest::Version::HTTP_11 | reqwest::Version::HTTP_10 => HttpProtocol::Http11,
            reqwest::Version::HTTP_3 => HttpProtocol::Http3,
            _ => HttpProtocol::Unknown,
        };
        let mut headers = HeaderMap::new();
        for (name, value) in response.headers() {
            headers.append(
                name.as_str(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            );
        }
        let tls = https.then(|| TlsInfo {
            alpn: protocol,
            certificate_verified: true,
            version: None,
            cipher_suite: None,
            hostname,
            certificate: response
                .extensions()
                .get::<reqwest::tls::TlsInfo>()
                .and_then(|i| i.peer_certificate())
                .and_then(CertificateInfo::from_der),
        });
        let body = AsyncBodyReader {
            response: Some(response),
            runtime: Arc::clone(&self.runtime),
            cancel: cancel.clone(),
            deadline,
            idle: self.read_timeout,
            pending: bytes::Bytes::new(),
        };
        Ok(RawTransportResponse {
            status,
            reason,
            headers,
            protocol,
            tls,
            body: Box::new(body),
        })
    }

    fn pool_stats(&self) -> PoolStats {
        PoolStats {
            connections_opened: Some(self.connections_opened()),
            connections_reused: None,
            active_connections: None,
        }
    }
}

/// Race `fut` against the whole-request deadline (`Timeout`) and an idle limit for this
/// wait (`ReadTimeout`), whichever comes first.
async fn with_timeouts<F, T>(
    deadline: Option<tokio::time::Instant>,
    idle: Option<Duration>,
    fut: F,
) -> Result<T, NetworkError>
where
    F: std::future::Future<Output = T>,
{
    let idle_at = idle.map(|d| tokio::time::Instant::now() + d);
    let (at, error) = match (deadline, idle_at) {
        (Some(d), Some(i)) if i < d => (Some(i), NetworkError::ReadTimeout),
        (Some(d), _) => (Some(d), NetworkError::Timeout),
        (None, Some(i)) => (Some(i), NetworkError::ReadTimeout),
        (None, None) => (None, NetworkError::Timeout),
    };
    match at {
        Some(at) => tokio::time::timeout_at(at, fut).await.map_err(|_| error),
        None => Ok(fut.await),
    }
}

/// Pulls body chunks from the async response on demand (backpressure: nothing is read
/// until the consumer asks).
struct AsyncBodyReader {
    response: Option<reqwest::Response>,
    runtime: Arc<RuntimeHolder>,
    cancel: CancellationToken,
    deadline: Option<tokio::time::Instant>,
    idle: Option<Duration>,
    pending: bytes::Bytes,
}

impl Read for AsyncBodyReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if !self.pending.is_empty() {
                let n = self.pending.len().min(buf.len());
                buf[..n].copy_from_slice(&self.pending[..n]);
                self.pending.advance(n);
                return Ok(n);
            }
            let Some(resp) = self.response.as_mut() else {
                return Ok(0);
            };
            let cancel = &self.cancel;
            let (deadline, idle) = (self.deadline, self.idle);
            let next = self.runtime.block_on(async {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => Err(NetworkError::Cancelled),
                    r = with_timeouts(deadline, idle, resp.chunk()) => r.and_then(|r| r.map_err(|e| classify(&e))),
                }
            });
            match next {
                Ok(Some(chunk)) => self.pending = chunk,
                Ok(None) => {
                    self.response = None;
                    return Ok(0);
                }
                Err(e) => {
                    // Dropping the response closes (does not pool) the connection.
                    self.response = None;
                    let e = match e {
                        NetworkError::Protocol(m) => NetworkError::Body(m),
                        other => other,
                    };
                    return Err(into_io(e));
                }
            }
        }
    }
}

type DynError = dyn std::error::Error + 'static;

/// Find an error of type `T` anywhere in the chain. `io::Error::source()` skips its own
/// payload, so the payload is searched explicitly.
fn find_in_chain<T: std::error::Error + 'static>(err: &DynError) -> Option<&T> {
    if let Some(t) = err.downcast_ref::<T>() {
        return Some(t);
    }
    if let Some(inner) = err.downcast_ref::<io::Error>().and_then(|io| io.get_ref()) {
        if let Some(t) = find_in_chain::<T>(inner) {
            return Some(t);
        }
    }
    err.source().and_then(find_in_chain::<T>)
}

fn chain_has_io_kind(err: &DynError, kind: io::ErrorKind) -> bool {
    if let Some(io) = err.downcast_ref::<io::Error>() {
        if io.kind() == kind {
            return true;
        }
        if let Some(inner) = io.get_ref() {
            if chain_has_io_kind(inner, kind) {
                return true;
            }
        }
    }
    err.source().is_some_and(|s| chain_has_io_kind(s, kind))
}

fn certificate_kind(e: &rustls::CertificateError) -> CertificateErrorKind {
    use rustls::CertificateError as C;
    match e {
        C::Expired | C::ExpiredContext { .. } => CertificateErrorKind::Expired,
        C::NotValidYet | C::NotValidYetContext { .. } => CertificateErrorKind::NotYetValid,
        C::NotValidForName | C::NotValidForNameContext { .. } => {
            CertificateErrorKind::HostnameMismatch
        }
        C::UnknownIssuer => CertificateErrorKind::UntrustedIssuer,
        C::Revoked => CertificateErrorKind::Revoked,
        _ => CertificateErrorKind::InvalidChain,
    }
}

/// Map a reqwest error to a typed error by walking the source chain.
fn classify(e: &reqwest::Error) -> NetworkError {
    if let Some(d) = find_in_chain::<DnsFailure>(e) {
        return NetworkError::Dns(d.0.clone());
    }
    let mut chain = Vec::new();
    let mut cur: Option<&DynError> = Some(e);
    while let Some(err) = cur {
        chain.push(err.to_string());
        cur = err.source();
    }
    chain.dedup();
    let msg = chain.join(": ");
    if let Some(tls) = find_in_chain::<rustls::Error>(e) {
        return match tls {
            rustls::Error::InvalidCertificate(cert) => NetworkError::Certificate {
                kind: certificate_kind(cert),
                detail: msg,
            },
            _ => NetworkError::Tls(msg),
        };
    }
    if e.is_timeout() {
        return if e.is_connect() {
            NetworkError::ConnectTimeout
        } else {
            NetworkError::Timeout
        };
    }
    if e.is_connect() {
        let refused = chain_has_io_kind(e, io::ErrorKind::ConnectionRefused);
        return if refused {
            NetworkError::ConnectionRefused(msg)
        } else {
            NetworkError::Connection(msg)
        };
    }
    if e.is_body() || e.is_decode() {
        return NetworkError::Body(msg);
    }
    NetworkError::Protocol(msg)
}

#[derive(Clone)]
struct CountConnectionsLayer(Arc<AtomicU64>);

impl<S> tower_layer::Layer<S> for CountConnectionsLayer {
    type Service = CountConnections<S>;
    fn layer(&self, inner: S) -> Self::Service {
        CountConnections {
            inner,
            opened: Arc::clone(&self.0),
        }
    }
}

#[derive(Clone)]
struct CountConnections<S> {
    inner: S,
    opened: Arc<AtomicU64>,
}

impl<S, R> tower_service::Service<R> for CountConnections<S>
where
    S: tower_service::Service<R>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: R) -> Self::Future {
        self.opened.fetch_add(1, Ordering::Relaxed);
        self.inner.call(req)
    }
}
