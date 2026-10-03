/// Per-request timing. Only phases that are actually measured are `Some`.
///
/// The transport (reqwest/hyper) does not expose DNS, connect, TLS or request-sent
/// boundaries, so those stay `None` and are displayed as unavailable — never estimated.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetworkTiming {
    /// Time spent waiting in the scheduler queue.
    pub queued_ms: Option<f64>,
    pub dns_ms: Option<f64>,
    pub connect_ms: Option<f64>,
    pub tls_ms: Option<f64>,
    pub request_sent_ms: Option<f64>,
    /// From dispatching the final hop to receiving its response headers.
    pub ttfb_ms: Option<f64>,
    /// From response headers to end of body.
    pub download_ms: Option<f64>,
    /// From service entry to end of body (or failure).
    pub total_ms: Option<f64>,
}

pub(crate) fn ms_since(t: std::time::Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}
