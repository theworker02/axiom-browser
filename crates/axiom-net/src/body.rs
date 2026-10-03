//! Request / response bodies and the response read pipeline.
//!
//! Response pipeline (each stage is a `std::io::Read`):
//!
//! ```text
//! transport chunks → CountingReader (transferred bytes)
//!                  → content decoder (gzip / deflate / br, ours — not the transport's)
//!                  → CacheTee (optional, bounded; abandons buffering past the entry cap)
//!                  → Instrumented (decoded bytes, completion → log / metrics)
//! ```
//!
//! Errors travel as `io::Error` wrapping [`NetworkError`] so cancellation / timeouts stay typed.

use std::io::{self, Cursor, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::error::{from_io, into_io, NetworkError};

/// Request body. `Stream` bodies are sent as they are produced (chunked on HTTP/1.1) and
/// can be sent only once.
#[derive(Debug, Clone, Default)]
pub enum RequestBody {
    #[default]
    Empty,
    Bytes(Vec<u8>),
    Stream(StreamingBody),
}

impl RequestBody {
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// Known length (`None` for streams without a declared length).
    pub fn len(&self) -> Option<u64> {
        match self {
            Self::Empty => Some(0),
            Self::Bytes(b) => Some(b.len() as u64),
            Self::Stream(s) => s.length,
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty) || self.len() == Some(0)
    }

    /// Streams can be sent once; replaying them across redirects is refused.
    pub fn is_replayable(&self) -> bool {
        !matches!(self, Self::Stream(_))
    }
}

/// One-shot upload stream.
#[derive(Clone)]
pub struct StreamingBody {
    source: Arc<Mutex<Option<UploadSource>>>,
    pub length: Option<u64>,
}

pub(crate) enum UploadSource {
    /// Blocking reader, pumped on the transport's blocking pool.
    Reader(Box<dyn Read + Send>),
    /// Chunks pushed by an [`UploadSender`] (never blocks the producer).
    Channel {
        rx: tokio::sync::mpsc::UnboundedReceiver<UploadItem>,
        buffered: Arc<AtomicU64>,
    },
}

pub(crate) enum UploadItem {
    Data(Vec<u8>),
    End,
    Error(String),
}

impl StreamingBody {
    pub fn new(reader: impl Read + Send + 'static, length: Option<u64>) -> Self {
        Self {
            source: Arc::new(Mutex::new(Some(UploadSource::Reader(Box::new(reader))))),
            length,
        }
    }

    /// A body fed chunk by chunk from another thread (e.g. a script's `ReadableStream`).
    /// Dropping the sender without [`UploadSender::close`] aborts the upload.
    pub fn channel() -> (UploadSender, Self) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let buffered = Arc::new(AtomicU64::new(0));
        let body = Self {
            source: Arc::new(Mutex::new(Some(UploadSource::Channel {
                rx,
                buffered: Arc::clone(&buffered),
            }))),
            length: None,
        };
        let sender = UploadSender {
            tx: Some(tx),
            buffered,
        };
        (sender, body)
    }

    pub(crate) fn take_source(&self) -> Result<UploadSource, NetworkError> {
        self.source
            .lock()
            .take()
            .ok_or_else(|| NetworkError::Body("request body stream already consumed".into()))
    }
}

impl std::fmt::Debug for StreamingBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamingBody")
            .field("length", &self.length)
            .finish()
    }
}

/// Producer half of [`StreamingBody::channel`].
pub struct UploadSender {
    tx: Option<tokio::sync::mpsc::UnboundedSender<UploadItem>>,
    buffered: Arc<AtomicU64>,
}

impl UploadSender {
    /// Queue a chunk. `Err` once the request is gone (failed, cancelled or finished).
    pub fn write(&self, bytes: Vec<u8>) -> Result<(), NetworkError> {
        let tx = self.tx.as_ref().ok_or(NetworkError::Cancelled)?;
        let len = bytes.len() as u64;
        self.buffered.fetch_add(len, Ordering::SeqCst);
        tx.send(UploadItem::Data(bytes)).map_err(|_| {
            self.buffered.fetch_sub(len, Ordering::SeqCst);
            NetworkError::Cancelled
        })
    }

    /// Bytes queued but not yet handed to the connection.
    pub fn buffered(&self) -> u64 {
        self.buffered.load(Ordering::SeqCst)
    }

    /// Whether the request still consumes the body.
    pub fn is_open(&self) -> bool {
        self.tx.as_ref().is_some_and(|tx| !tx.is_closed())
    }

    /// End of body.
    pub fn close(mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(UploadItem::End);
        }
    }

    /// Fail the upload (and therefore the request).
    pub fn error(mut self, message: &str) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(UploadItem::Error(message.to_string()));
        }
    }
}

impl std::fmt::Debug for UploadSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UploadSender")
            .field("buffered", &self.buffered())
            .field("open", &self.is_open())
            .finish()
    }
}

/// Shared byte counters for one response.
#[derive(Debug, Clone, Default)]
pub struct BodyCounters {
    transferred: Arc<AtomicU64>,
    decoded: Arc<AtomicU64>,
}

impl BodyCounters {
    /// Encoded body bytes received from the network (HTTP framing and headers excluded).
    pub fn transferred(&self) -> u64 {
        self.transferred.load(Ordering::Relaxed)
    }

    /// Bytes delivered to the consumer after content decoding.
    pub fn decoded(&self) -> u64 {
        self.decoded.load(Ordering::Relaxed)
    }
}

/// Response body handed to consumers; reads pull from the network on demand.
pub struct ResponseBodyReader {
    inner: Arc<Mutex<Option<Box<dyn Read + Send>>>>,
}

impl ResponseBodyReader {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self::from_reader(Box::new(Cursor::new(bytes)))
    }

    pub fn from_reader(r: Box<dyn Read + Send>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Some(r))),
        }
    }

    /// Read up to `max` bytes; an empty vector means end of body.
    pub fn read_chunk(&self, max: usize) -> Result<Vec<u8>, NetworkError> {
        let mut guard = self.inner.lock();
        let Some(r) = guard.as_mut() else {
            return Ok(Vec::new());
        };
        let mut tmp = vec![0u8; max.max(1)];
        loop {
            match r.read(&mut tmp) {
                Ok(0) => {
                    *guard = None;
                    return Ok(Vec::new());
                }
                Ok(n) => {
                    tmp.truncate(n);
                    return Ok(tmp);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    *guard = None;
                    return Err(from_io(e));
                }
            }
        }
    }

    /// Read the whole body, failing with [`NetworkError::TooLarge`] past `limit` bytes.
    pub fn read_all(&self, limit: u64) -> Result<Vec<u8>, NetworkError> {
        let mut out = Vec::new();
        loop {
            let chunk = self.read_chunk(64 * 1024)?;
            if chunk.is_empty() {
                return Ok(out);
            }
            if (out.len() + chunk.len()) as u64 > limit {
                *self.inner.lock() = None;
                return Err(NetworkError::TooLarge { limit });
            }
            out.extend_from_slice(&chunk);
        }
    }

    /// Stop reading; the underlying connection is released / closed.
    pub fn abandon(&self) {
        *self.inner.lock() = None;
    }
}

impl Clone for ResponseBodyReader {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// Reader over a shared cached body (no copy per hit).
pub(crate) struct SharedBytesReader {
    data: Arc<Vec<u8>>,
    pos: usize,
}

impl SharedBytesReader {
    pub(crate) fn new(data: Arc<Vec<u8>>) -> Self {
        Self { data, pos: 0 }
    }
}

impl Read for SharedBytesReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = (self.data.len() - self.pos).min(buf.len());
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

pub(crate) struct CountingReader<R> {
    inner: R,
    counter: Arc<AtomicU64>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.counter.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

pub(crate) fn counting(
    inner: Box<dyn Read + Send>,
    counters: &BodyCounters,
) -> Box<dyn Read + Send> {
    Box::new(CountingReader {
        inner,
        counter: Arc::clone(&counters.transferred),
    })
}

/// Parsed `Content-Encoding` codings in application order.
pub fn parse_content_encoding(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or("")
        .split(',')
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| !c.is_empty() && c != "identity")
        .collect()
}

pub(crate) fn is_supported_coding(c: &str) -> bool {
    matches!(c, "gzip" | "x-gzip" | "deflate" | "br")
}

/// Wrap `inner` with decoders undoing `codings` (last applied is removed first).
pub(crate) fn decoding(
    mut inner: Box<dyn Read + Send>,
    codings: &[String],
) -> Result<Box<dyn Read + Send>, NetworkError> {
    for c in codings.iter().rev() {
        if !is_supported_coding(c) {
            return Err(NetworkError::Protocol(format!(
                "unsupported content-encoding: {c}"
            )));
        }
        inner = Box::new(LazyDecoder {
            coding: c.clone(),
            state: LazyState::Pending(Some(inner)),
        });
    }
    Ok(inner)
}

struct PeekReader {
    head: Vec<u8>,
    pos: usize,
    inner: Box<dyn Read + Send>,
}

impl PeekReader {
    fn fill(&mut self, n: usize) -> io::Result<()> {
        let mut tmp = [0u8; 16];
        while self.head.len() < n {
            let want = (n - self.head.len()).min(tmp.len());
            match self.inner.read(&mut tmp[..want]) {
                Ok(0) => break,
                Ok(k) => self.head.extend_from_slice(&tmp[..k]),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

impl Read for PeekReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos < self.head.len() {
            let n = (self.head.len() - self.pos).min(buf.len());
            buf[..n].copy_from_slice(&self.head[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
        self.inner.read(buf)
    }
}

enum LazyState {
    Pending(Option<Box<dyn Read + Send>>),
    Active(Box<dyn Read + Send>),
    Empty,
}

/// Decoder chosen on first read: tolerates empty bodies and sniffs zlib vs raw deflate.
struct LazyDecoder {
    coding: String,
    state: LazyState,
}

impl Read for LazyDecoder {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            match &mut self.state {
                LazyState::Empty => return Ok(0),
                LazyState::Active(r) => {
                    return r.read(buf).map_err(|e| {
                        if e.get_ref().is_some_and(|r| r.is::<NetworkError>()) {
                            e
                        } else {
                            into_io(NetworkError::Body(format!(
                                "{} decoding failed: {e}",
                                self.coding
                            )))
                        }
                    });
                }
                LazyState::Pending(inner) => {
                    let inner = inner.take().expect("pending decoder input");
                    let mut peek = PeekReader {
                        head: Vec::new(),
                        pos: 0,
                        inner,
                    };
                    peek.fill(2)?;
                    if peek.head.is_empty() {
                        self.state = LazyState::Empty;
                        continue;
                    }
                    let decoder: Box<dyn Read + Send> = match self.coding.as_str() {
                        "gzip" | "x-gzip" => Box::new(flate2::read::MultiGzDecoder::new(peek)),
                        "deflate" => {
                            let h = &peek.head;
                            let zlib = h.len() == 2
                                && (h[0] & 0x0F) == 8
                                && ((h[0] as u16) << 8 | h[1] as u16).is_multiple_of(31);
                            if zlib {
                                Box::new(flate2::read::ZlibDecoder::new(peek))
                            } else {
                                Box::new(flate2::read::DeflateDecoder::new(peek))
                            }
                        }
                        "br" => Box::new(brotli_decompressor::Decompressor::new(peek, 4096)),
                        other => {
                            return Err(into_io(NetworkError::Protocol(format!(
                                "unsupported content-encoding: {other}"
                            ))))
                        }
                    };
                    self.state = LazyState::Active(decoder);
                }
            }
        }
    }
}

type TeeComplete = Box<dyn FnOnce(Vec<u8>) + Send>;

/// Copies the decoded body for the cache while the consumer streams it. Buffering stops
/// (and the entry is skipped) once `limit` is exceeded, so memory stays bounded.
pub(crate) struct CacheTee {
    inner: Box<dyn Read + Send>,
    buf: Option<Vec<u8>>,
    limit: usize,
    peak: Arc<AtomicU64>,
    on_complete: Option<TeeComplete>,
}

impl CacheTee {
    pub(crate) fn new(
        inner: Box<dyn Read + Send>,
        limit: usize,
        peak: Arc<AtomicU64>,
        on_complete: TeeComplete,
    ) -> Self {
        Self {
            inner,
            buf: Some(Vec::new()),
            limit,
            peak,
            on_complete: Some(on_complete),
        }
    }
}

impl Read for CacheTee {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = match self.inner.read(out) {
            Ok(n) => n,
            Err(e) => {
                self.buf = None;
                self.on_complete = None;
                return Err(e);
            }
        };
        if n == 0 {
            if let (Some(buf), Some(done)) = (self.buf.take(), self.on_complete.take()) {
                done(buf);
            }
            return Ok(0);
        }
        if let Some(buf) = self.buf.as_mut() {
            if buf.len() + n > self.limit {
                self.buf = None;
                self.on_complete = None;
            } else {
                buf.extend_from_slice(&out[..n]);
                self.peak.fetch_max(buf.len() as u64, Ordering::Relaxed);
            }
        }
        Ok(n)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyOutcome {
    Complete,
    Failed(NetworkError),
    /// The consumer dropped the body before the end.
    Abandoned,
}

type FinishFn = Box<dyn FnOnce(BodyOutcome) + Send>;

/// Outermost stage: decoded byte count and exactly-once completion reporting.
pub(crate) struct Instrumented {
    inner: Box<dyn Read + Send>,
    decoded: Arc<AtomicU64>,
    finish: Option<FinishFn>,
}

impl Instrumented {
    pub(crate) fn new(
        inner: Box<dyn Read + Send>,
        counters: &BodyCounters,
        finish: FinishFn,
    ) -> Self {
        Self {
            inner,
            decoded: Arc::clone(&counters.decoded),
            finish: Some(finish),
        }
    }

    fn done(&mut self, outcome: BodyOutcome) {
        if let Some(f) = self.finish.take() {
            f(outcome);
        }
    }
}

impl Read for Instrumented {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        match self.inner.read(buf) {
            Ok(0) => {
                self.done(BodyOutcome::Complete);
                Ok(0)
            }
            Ok(n) => {
                self.decoded.fetch_add(n as u64, Ordering::Relaxed);
                Ok(n)
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => Err(e),
            Err(e) => {
                let err = from_io(e);
                self.done(BodyOutcome::Failed(err.clone()));
                Err(into_io(err))
            }
        }
    }
}

impl Drop for Instrumented {
    fn drop(&mut self) {
        self.done(BodyOutcome::Abandoned);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn decode_all(data: Vec<u8>, coding: &str) -> Result<Vec<u8>, NetworkError> {
        let r = decoding(Box::new(Cursor::new(data)), &[coding.to_string()])?;
        ResponseBodyReader::from_reader(r).read_all(1 << 20)
    }

    #[test]
    fn gzip_deflate_raw_deflate_and_empty() {
        let text = b"hello hello hello axiom".to_vec();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&text).unwrap();
        assert_eq!(decode_all(gz.finish().unwrap(), "gzip").unwrap(), text);

        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&text).unwrap();
        assert_eq!(decode_all(z.finish().unwrap(), "deflate").unwrap(), text);

        let mut raw =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        raw.write_all(&text).unwrap();
        assert_eq!(decode_all(raw.finish().unwrap(), "deflate").unwrap(), text);

        assert_eq!(decode_all(Vec::new(), "gzip").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn corrupt_stream_is_typed_body_error() {
        let err = decode_all(b"\x1f\x8bnot really gzip".to_vec(), "gzip").unwrap_err();
        assert!(
            matches!(err, NetworkError::Body(ref m) if m.contains("gzip")),
            "{err:?}"
        );
        assert!(decoding(Box::new(Cursor::new(vec![])), &["zstd".into()]).is_err());
    }

    #[test]
    fn tee_abandons_past_limit() {
        let stored = Arc::new(Mutex::new(None));
        let s2 = Arc::clone(&stored);
        let peak = Arc::new(AtomicU64::new(0));
        let tee = CacheTee::new(
            Box::new(Cursor::new(vec![7u8; 5000])),
            1000,
            Arc::clone(&peak),
            Box::new(move |b| *s2.lock() = Some(b)),
        );
        let body = ResponseBodyReader::from_reader(Box::new(tee))
            .read_all(1 << 20)
            .unwrap();
        assert_eq!(body.len(), 5000);
        assert!(stored.lock().is_none());
        assert!(peak.load(Ordering::Relaxed) <= 1000);
    }
}
