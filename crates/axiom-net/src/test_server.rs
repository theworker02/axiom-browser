//! Deterministic local HTTP/1.1 server for tests and benchmarks (not a production server).
//!
//! Records every request (method, path, headers, body, connection id), supports
//! keep-alive, counts TCP connections and concurrent handlers, and can send slow chunked
//! streams (recording whether the client aborted) or raw malformed bytes.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;

#[derive(Debug, Clone)]
pub struct TestRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Chunks of a `Transfer-Encoding: chunked` request body (0 for fixed-length bodies).
    pub body_chunks: usize,
    pub connection_id: u64,
}

impl TestRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone)]
pub enum TestResponse {
    Fixed {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// Chunked transfer, sleeping `delay_ms` before each chunk.
    Chunked {
        status: u16,
        headers: Vec<(String, String)>,
        chunks: Vec<Vec<u8>>,
        delay_ms: u64,
    },
    /// Sleep, then send the inner response.
    Delayed {
        delay_ms: u64,
        response: Box<TestResponse>,
    },
    /// Bytes written verbatim, then the connection is closed.
    Raw(Vec<u8>),
    /// Close the connection without responding.
    Close,
}

impl TestResponse {
    pub fn ok(body: impl Into<Vec<u8>>, content_type: &str) -> Self {
        Self::Fixed {
            status: 200,
            headers: vec![("Content-Type".into(), content_type.into())],
            body: body.into(),
        }
    }

    pub fn status(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self::Fixed {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn redirect(status: u16, location: &str) -> Self {
        Self::Fixed {
            status,
            headers: vec![("Location".into(), location.into())],
            body: Vec::new(),
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        match &mut self {
            Self::Fixed { headers, .. } | Self::Chunked { headers, .. } => {
                headers.push((name.into(), value.into()))
            }
            Self::Delayed { response, .. } => {
                let inner = std::mem::replace(response.as_mut(), Self::Close);
                **response = inner.with_header(name, value);
            }
            Self::Raw(_) | Self::Close => {}
        }
        self
    }

    pub fn delayed(self, delay_ms: u64) -> Self {
        Self::Delayed {
            delay_ms,
            response: Box::new(self),
        }
    }
}

type Handler = dyn Fn(&TestRequest) -> TestResponse + Send + Sync;

#[derive(Default)]
struct Stats {
    connections: AtomicU64,
    active: AtomicUsize,
    peak_active: AtomicUsize,
    streams_completed: AtomicU64,
    streams_aborted: AtomicU64,
    stream_bytes: AtomicU64,
    requests: Mutex<Vec<TestRequest>>,
}

pub struct TestServer {
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    stats: Arc<Stats>,
}

impl TestServer {
    pub fn spawn<F>(handler: F) -> Self
    where
        F: Fn(&TestRequest) -> TestResponse + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        listener.set_nonblocking(true).expect("nonblocking");
        let addr = listener.local_addr().expect("addr");
        let shutdown = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Stats::default());
        let handler: Arc<Handler> = Arc::new(handler);
        let (sd, st) = (Arc::clone(&shutdown), Arc::clone(&stats));
        thread::spawn(move || {
            while !sd.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let conn_id = st.connections.fetch_add(1, Ordering::SeqCst) + 1;
                        let (h, st2, sd2) =
                            (Arc::clone(&handler), Arc::clone(&st), Arc::clone(&sd));
                        thread::spawn(move || serve_connection(stream, conn_id, &*h, &st2, &sd2));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            addr,
            shutdown,
            stats,
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.addr.port(), path)
    }

    /// TCP connections accepted so far.
    pub fn connections(&self) -> u64 {
        self.stats.connections.load(Ordering::SeqCst)
    }

    pub fn requests(&self) -> Vec<TestRequest> {
        self.stats.requests.lock().clone()
    }

    pub fn requests_for(&self, path: &str) -> Vec<TestRequest> {
        self.requests()
            .into_iter()
            .filter(|r| r.path == path)
            .collect()
    }

    pub fn request_count(&self) -> usize {
        self.stats.requests.lock().len()
    }

    /// Most handlers running at the same time.
    pub fn peak_concurrency(&self) -> usize {
        self.stats.peak_active.load(Ordering::SeqCst)
    }

    pub fn streams_completed(&self) -> u64 {
        self.stats.streams_completed.load(Ordering::SeqCst)
    }

    /// Chunked responses the client stopped reading (write failed mid-stream).
    pub fn streams_aborted(&self) -> u64 {
        self.stats.streams_aborted.load(Ordering::SeqCst)
    }

    /// Payload bytes of chunked responses successfully written to sockets so far.
    pub fn stream_bytes_written(&self) -> u64 {
        self.stats.stream_bytes.load(Ordering::SeqCst)
    }
}

impl TestServer {
    /// Server with the named endpoints of [`standard_routes`].
    pub fn spawn_standard() -> Self {
        Self::spawn(standard_routes)
    }

    /// Controlled test site serving the fixture directory `root` (see [`site_routes`]).
    pub fn spawn_site(root: impl Into<std::path::PathBuf>) -> Self {
        let root = root.into();
        Self::spawn(move |req| site_routes(&root, req))
    }
}

/// A 1×1 opaque PNG.
pub fn png_1x1() -> Vec<u8> {
    const B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg";
    let value = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        _ => 63,
    };
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0u32);
    for c in B64.bytes() {
        acc = (acc << 6) | u32::from(value(c));
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// File-backed "controlled test site" used by document-loading tests and benchmarks.
///
/// * `/d/<ms>/<path>` serves `<path>` after `<ms>` milliseconds (relative URLs inside it
///   stay under the same delay prefix).
/// * `/status/<code>/...` answers `<code>`.
/// * Query strings are ignored for file lookup; `..` segments are refused.
/// * `{{ORIGIN}}` in HTML/CSS/JS is replaced by the server origin (from `Host`).
/// * `*.png` missing on disk is a generated 1×1 PNG, except `img/broken.png`, which is
///   non-image bytes labelled `image/png`; `*.woff2` missing on disk is a WOFF2 signature.
/// * `cookie.html` sets `site=1`; `cache/*` is fresh for an hour; `etag/*` and
///   `cache.html` are `no-cache` with `ETag: "v1"` and answer `If-None-Match` with 304.
///   Everything else is `no-store`.
pub fn site_routes(root: &std::path::Path, req: &TestRequest) -> TestResponse {
    let path = req.path.split('?').next().unwrap_or("/");
    if let Some((ms, tail)) = path.strip_prefix("/d/").and_then(|r| r.split_once('/')) {
        if let Ok(ms) = ms.parse::<u64>() {
            let inner = TestRequest {
                path: format!("/{tail}"),
                ..req.clone()
            };
            return site_routes(root, &inner).delayed(ms);
        }
    }
    if let Some(code) = path
        .strip_prefix("/status/")
        .and_then(|r| r.split('/').next())
        .and_then(|c| c.parse::<u16>().ok())
    {
        return TestResponse::status(code, format!("status {code}"));
    }
    let rel = match path.trim_start_matches('/') {
        "" => "index.html",
        rel => rel,
    };
    if rel.split('/').any(|s| s == "..") {
        return TestResponse::status(404, "not found");
    }
    let ext = rel.rsplit('.').next().unwrap_or("");
    let mime = match ext {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css",
        "js" => "text/javascript",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    };
    let etag = rel.starts_with("etag/") || rel == "cache.html";
    if etag && req.header("if-none-match") == Some("\"v1\"") {
        return TestResponse::status(304, Vec::new()).with_header("ETag", "\"v1\"");
    }
    let body = match std::fs::read(root.join(rel)) {
        Ok(bytes) if matches!(ext, "html" | "css" | "js") => {
            let origin = req
                .header("host")
                .map(|h| format!("http://{h}"))
                .unwrap_or_default();
            String::from_utf8_lossy(&bytes)
                .replace("{{ORIGIN}}", &origin)
                .into_bytes()
        }
        Ok(bytes) => bytes,
        Err(_) if rel == "img/broken.png" => b"this is not an image".to_vec(),
        Err(_) if ext == "png" => png_1x1(),
        Err(_) if ext == "woff2" => {
            let mut font = b"wOF2".to_vec();
            font.resize(64, 0);
            font
        }
        Err(_) => return TestResponse::status(404, "not found"),
    };
    let mut resp = TestResponse::ok(body, mime);
    resp = if rel.starts_with("cache/") {
        resp.with_header("Cache-Control", "max-age=3600")
    } else if etag {
        resp.with_header("Cache-Control", "no-cache")
            .with_header("ETag", "\"v1\"")
    } else {
        resp.with_header("Cache-Control", "no-store")
    };
    if rel == "cookie.html" {
        resp = resp.with_header("Set-Cookie", "site=1; Path=/");
    }
    resp
}

/// Size of the `/download` payload.
pub const DOWNLOAD_LEN: usize = 256 * 1024;
/// `Content-Disposition` file name sent by `/download` — deliberately hostile.
pub const DOWNLOAD_FILENAME: &str = "../../evil name.bin";
const DOWNLOAD_ETAG: &str = "\"dl-1\"";
const LAST_MODIFIED: &str = "Tue, 01 Jan 2030 00:00:00 GMT";

/// Deterministic `/download` body.
pub fn download_body() -> Vec<u8> {
    (0..DOWNLOAD_LEN).map(|i| (i % 251) as u8).collect()
}

/// Named endpoints shared by tests and benchmarks:
///
/// `/ok`, `/redirect`, `/redirect-chain` (5 hops), `/redirect-loop`, `/cache/max-age`,
/// `/cache/etag`, `/cache/last-modified`, `/compressed` (gzip), `/slow` (300 ms),
/// `/stream` (64 KiB chunked), `/download` and `/download/slow` (ranges + ETag),
/// `/set-cookie`, `/echo-cookie`, `/status/<code>`. Anything else is 404.
pub fn standard_routes(req: &TestRequest) -> TestResponse {
    let path = req.path.split('?').next().unwrap_or("");
    match path {
        "/ok" => TestResponse::ok("ok", "text/plain"),
        "/redirect" => TestResponse::redirect(302, "/ok"),
        "/redirect-chain" => TestResponse::redirect(302, "/redirect-chain/1"),
        "/redirect-loop" => TestResponse::redirect(302, "/redirect-loop"),
        "/cache/max-age" => {
            TestResponse::ok("fresh", "text/plain").with_header("Cache-Control", "max-age=60")
        }
        "/cache/etag" => {
            if req.header("if-none-match") == Some("\"v1\"") {
                TestResponse::status(304, Vec::new()).with_header("ETag", "\"v1\"")
            } else {
                TestResponse::ok("etag body", "text/plain")
                    .with_header("ETag", "\"v1\"")
                    .with_header("Cache-Control", "no-cache")
            }
        }
        "/cache/last-modified" => {
            if req.header("if-modified-since") == Some(LAST_MODIFIED) {
                TestResponse::status(304, Vec::new())
            } else {
                TestResponse::ok("lm body", "text/plain")
                    .with_header("Last-Modified", LAST_MODIFIED)
                    .with_header("Cache-Control", "no-cache")
            }
        }
        "/compressed" => {
            use std::io::Write as _;
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            let _ = e.write_all("compressible text ".repeat(4096).as_bytes());
            TestResponse::ok(e.finish().unwrap_or_default(), "text/plain")
                .with_header("Content-Encoding", "gzip")
        }
        "/slow" => TestResponse::ok("slow", "text/plain").delayed(300),
        "/stream" => TestResponse::Chunked {
            status: 200,
            headers: vec![("Content-Type".into(), "application/octet-stream".into())],
            chunks: vec![vec![7u8; 4096]; 16],
            delay_ms: 5,
        },
        "/download" => download_response(req, false),
        "/download/slow" => download_response(req, true),
        "/set-cookie" => {
            TestResponse::ok("set", "text/plain").with_header("Set-Cookie", "session=abc; Path=/")
        }
        "/echo-cookie" => {
            TestResponse::ok(req.header("cookie").unwrap_or("").to_string(), "text/plain")
        }
        p => {
            if let Some(n) = p.strip_prefix("/redirect-chain/") {
                let n: u32 = n.parse().unwrap_or(5);
                return if n >= 5 {
                    TestResponse::redirect(302, "/ok")
                } else {
                    TestResponse::redirect(302, &format!("/redirect-chain/{}", n + 1))
                };
            }
            if let Some(code) = p.strip_prefix("/status/").and_then(|c| c.parse().ok()) {
                return TestResponse::status(code, format!("status {code}"));
            }
            TestResponse::status(404, "not found")
        }
    }
}

fn download_response(req: &TestRequest, slow: bool) -> TestResponse {
    let body = download_body();
    let with_headers = |r: TestResponse| {
        r.with_header("Content-Type", "application/octet-stream")
            .with_header(
                "Content-Disposition",
                &format!("attachment; filename=\"{DOWNLOAD_FILENAME}\""),
            )
            .with_header("Accept-Ranges", "bytes")
            .with_header("ETag", DOWNLOAD_ETAG)
    };
    let range_start = req
        .header("range")
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.strip_suffix('-'))
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|&n| n < body.len());
    let if_range_ok = req.header("if-range").is_none_or(|v| v == DOWNLOAD_ETAG);
    if let (Some(start), true) = (range_start, if_range_ok) {
        let range = format!("bytes {start}-{}/{}", body.len() - 1, body.len());
        return with_headers(TestResponse::Fixed {
            status: 206,
            headers: vec![("Content-Range".into(), range)],
            body: body[start..].to_vec(),
        });
    }
    if slow {
        with_headers(TestResponse::Chunked {
            status: 200,
            headers: Vec::new(),
            chunks: body.chunks(4096).map(<[u8]>::to_vec).collect(),
            delay_ms: 20,
        })
    } else {
        with_headers(TestResponse::Fixed {
            status: 200,
            headers: Vec::new(),
            body,
        })
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

fn serve_connection(
    stream: TcpStream,
    conn_id: u64,
    handler: &Handler,
    stats: &Stats,
    shutdown: &AtomicBool,
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
    let mut write = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut reader = BufReader::new(stream);
    let mut idle = 0u32;
    loop {
        if shutdown.load(Ordering::SeqCst) {
            return;
        }
        let req = match read_request(&mut reader, conn_id) {
            Ok(Some(r)) => r,
            Ok(None) => return,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                idle += 1;
                if idle > 100 {
                    return;
                }
                continue;
            }
            Err(_) => return,
        };
        idle = 0;
        let client_close = req
            .header("connection")
            .is_some_and(|v| v.eq_ignore_ascii_case("close"));
        stats.requests.lock().push(req.clone());
        let now = stats.active.fetch_add(1, Ordering::SeqCst) + 1;
        stats.peak_active.fetch_max(now, Ordering::SeqCst);
        let response = handler(&req);
        let keep = write_response(&mut write, &req, response, stats) && !client_close;
        stats.active.fetch_sub(1, Ordering::SeqCst);
        if !keep {
            let _ = write.shutdown(Shutdown::Both);
            return;
        }
    }
}

fn read_request(
    reader: &mut BufReader<TcpStream>,
    conn_id: u64,
) -> std::io::Result<Option<TestRequest>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let _ = reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(5)));
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 {
            return Ok(None);
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let chunked = headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked")
    });
    let mut body_chunks = 0;
    let body = if chunked {
        let mut body = Vec::new();
        loop {
            let mut size_line = String::new();
            if reader.read_line(&mut size_line)? == 0 {
                return Ok(None);
            }
            let hex = size_line.trim().split(';').next().unwrap_or("");
            let size = usize::from_str_radix(hex, 16)
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "chunk size"))?;
            if size == 0 {
                // Trailers until the empty line.
                loop {
                    let mut t = String::new();
                    if reader.read_line(&mut t)? == 0 || t.trim_end().is_empty() {
                        break;
                    }
                }
                break body;
            }
            let start = body.len();
            body.resize(start + size, 0);
            reader.read_exact(&mut body[start..])?;
            let mut crlf = [0u8; 2];
            reader.read_exact(&mut crlf)?;
            body_chunks += 1;
        }
    } else {
        let len = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body)?;
        body
    };
    let _ = reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(100)));
    Ok(Some(TestRequest {
        method,
        path,
        headers,
        body,
        body_chunks,
        connection_id: conn_id,
    }))
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

/// Returns whether the connection may be kept alive.
fn write_response(w: &mut TcpStream, req: &TestRequest, resp: TestResponse, stats: &Stats) -> bool {
    let head_only = req.method.eq_ignore_ascii_case("HEAD");
    match resp {
        TestResponse::Delayed { delay_ms, response } => {
            thread::sleep(Duration::from_millis(delay_ms));
            write_response(w, req, *response, stats)
        }
        TestResponse::Close => false,
        TestResponse::Raw(bytes) => {
            let _ = w.write_all(&bytes);
            let _ = w.flush();
            false
        }
        TestResponse::Fixed {
            status,
            headers,
            body,
        } => {
            let mut out = format!("HTTP/1.1 {status} {}\r\n", reason(status));
            let has_len = headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-length"));
            for (k, v) in &headers {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            if !has_len && status != 304 && status != 204 {
                out.push_str(&format!("Content-Length: {}\r\n", body.len()));
            }
            out.push_str("Connection: keep-alive\r\n\r\n");
            let mut bytes = out.into_bytes();
            if !head_only && status != 304 && status != 204 {
                bytes.extend_from_slice(&body);
            }
            w.write_all(&bytes).and_then(|_| w.flush()).is_ok()
        }
        TestResponse::Chunked {
            status,
            headers,
            chunks,
            delay_ms,
        } => {
            let mut out = format!("HTTP/1.1 {status} {}\r\n", reason(status));
            for (k, v) in &headers {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            out.push_str("Transfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n");
            if w.write_all(out.as_bytes()).and_then(|_| w.flush()).is_err() {
                stats.streams_aborted.fetch_add(1, Ordering::SeqCst);
                return false;
            }
            if head_only {
                return true;
            }
            for c in chunks {
                if delay_ms > 0 {
                    thread::sleep(Duration::from_millis(delay_ms));
                }
                let mut frame = format!("{:x}\r\n", c.len()).into_bytes();
                frame.extend_from_slice(&c);
                frame.extend_from_slice(b"\r\n");
                if w.write_all(&frame).and_then(|_| w.flush()).is_err() {
                    stats.streams_aborted.fetch_add(1, Ordering::SeqCst);
                    return false;
                }
                stats
                    .stream_bytes
                    .fetch_add(c.len() as u64, Ordering::SeqCst);
            }
            if w.write_all(b"0\r\n\r\n").and_then(|_| w.flush()).is_err() {
                stats.streams_aborted.fetch_add(1, Ordering::SeqCst);
                return false;
            }
            stats.streams_completed.fetch_add(1, Ordering::SeqCst);
            true
        }
    }
}
