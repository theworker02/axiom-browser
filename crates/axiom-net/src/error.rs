//! Typed network errors.

use thiserror::Error;

/// Why a server certificate was rejected. Verification is never disabled; these only
/// classify the failure for error pages and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateErrorKind {
    Expired,
    NotYetValid,
    HostnameMismatch,
    UntrustedIssuer,
    Revoked,
    /// Bad signature, bad encoding, wrong key usage or another chain problem.
    InvalidChain,
}

impl CertificateErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::NotYetValid => "not_yet_valid",
            Self::HostnameMismatch => "hostname_mismatch",
            Self::UntrustedIssuer => "untrusted_issuer",
            Self::Revoked => "revoked",
            Self::InvalidChain => "invalid_chain",
        }
    }

    /// Short explanation for the trusted error page.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Expired => "The server's certificate has expired.",
            Self::NotYetValid => "The server's certificate is not valid yet.",
            Self::HostnameMismatch => "The server's certificate is for a different site.",
            Self::UntrustedIssuer => {
                "The server's certificate is not issued by a trusted authority."
            }
            Self::Revoked => "The server's certificate has been revoked.",
            Self::InvalidChain => "The server's certificate chain is invalid.",
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum NetworkError {
    #[error("dns: {0}")]
    Dns(String),
    /// Connection could not be established (reset, unreachable, …).
    #[error("connection: {0}")]
    Connection(String),
    /// The server actively refused the connection.
    #[error("connection refused: {0}")]
    ConnectionRefused(String),
    /// No connection within the configured connect timeout.
    #[error("connection timed out")]
    ConnectTimeout,
    /// TLS handshake failure other than certificate validation.
    #[error("tls: {0}")]
    Tls(String),
    /// The server certificate failed validation.
    #[error("certificate {}: {detail}", kind.as_str())]
    Certificate {
        kind: CertificateErrorKind,
        detail: String,
    },
    /// The whole-request deadline passed.
    #[error("timeout")]
    Timeout,
    /// No bytes arrived for the configured idle/read timeout.
    #[error("read timed out")]
    ReadTimeout,
    /// The same redirect was seen twice.
    #[error("redirect loop")]
    RedirectLoop,
    #[error("too many redirects (limit {limit})")]
    TooManyRedirects { limit: usize },
    #[error("cancelled")]
    Cancelled,
    /// Invalid or unexpected HTTP from the server (the "invalid response" class).
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("body: {0}")]
    Body(String),
    #[error("cache: {0}")]
    Cache(String),
    #[error("offline")]
    Offline,
    #[error("url: {0}")]
    Url(String),
    /// Only `http` and `https` reach the network stack.
    #[error("unsupported scheme: {0}")]
    UnsupportedScheme(String),
    #[error("http status {0}")]
    HttpStatus(u16),
    /// Rejected by a network or loader policy hook (content blocking, mixed content, …).
    #[error("blocked: {0}")]
    Blocked(String),
    /// A CORS check or preflight failed.
    #[error("cors: {0}")]
    Cors(String),
    /// Response body exceeded the per-request limit.
    #[error("response body exceeds limit of {limit} bytes")]
    TooLarge { limit: u64 },
    /// Response headers exceeded the configured limit.
    #[error("response headers exceed limit of {limit} bytes")]
    HeadersTooLarge { limit: usize },
}

impl NetworkError {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Dns(_) => "dns",
            Self::Connection(_) => "connection",
            Self::ConnectionRefused(_) => "connection_refused",
            Self::ConnectTimeout => "connect_timeout",
            Self::Tls(_) => "tls",
            Self::Certificate { .. } => "certificate",
            Self::Timeout => "timeout",
            Self::ReadTimeout => "read_timeout",
            Self::RedirectLoop => "redirect_loop",
            Self::TooManyRedirects { .. } => "too_many_redirects",
            Self::Cancelled => "cancelled",
            Self::Protocol(_) => "protocol",
            Self::Body(_) => "body",
            Self::Cache(_) => "cache",
            Self::Offline => "offline",
            Self::Url(_) => "url",
            Self::UnsupportedScheme(_) => "unsupported_scheme",
            Self::HttpStatus(_) => "http_status",
            Self::Blocked(_) => "blocked",
            Self::Cors(_) => "cors",
            Self::TooLarge { .. } => "too_large",
            Self::HeadersTooLarge { .. } => "headers_too_large",
        }
    }

    /// One-line, secret-free reason for error pages.
    pub fn summary(&self) -> &'static str {
        match self {
            Self::Dns(_) => "The server's address could not be found.",
            Self::Connection(_) => "The connection to the server failed.",
            Self::ConnectionRefused(_) => "The server refused the connection.",
            Self::ConnectTimeout => "The server took too long to accept the connection.",
            Self::Tls(_) => "A secure connection could not be established.",
            Self::Certificate { kind, .. } => kind.describe(),
            Self::Timeout => "The request took too long.",
            Self::ReadTimeout => "The server stopped sending data.",
            Self::RedirectLoop => "The page redirects in a loop.",
            Self::TooManyRedirects { .. } => "The page redirected too many times.",
            Self::Cancelled => "The request was cancelled.",
            Self::Protocol(_) => "The server sent an invalid response.",
            Self::Body(_) => "The response could not be read completely.",
            Self::Cache(_) => "The response is not available from the cache.",
            Self::Offline => "The browser is offline.",
            Self::Url(_) => "The address is not valid.",
            Self::UnsupportedScheme(_) => "This kind of address cannot be loaded from the network.",
            Self::HttpStatus(_) => "The server returned an error status.",
            Self::Blocked(_) => "The request was blocked by a browser policy.",
            Self::Cors(_) => "The server did not allow this cross-origin request.",
            Self::TooLarge { .. } => "The response is too large.",
            Self::HeadersTooLarge { .. } => "The server sent headers that are too large.",
        }
    }

    pub fn is_timeout(&self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::ConnectTimeout | Self::ReadTimeout
        )
    }
}

/// Wrap a [`NetworkError`] so it survives a trip through `std::io::Read`.
pub(crate) fn into_io(e: NetworkError) -> std::io::Error {
    std::io::Error::other(e)
}

/// Recover a [`NetworkError`] carried through `std::io::Error`, or classify the raw I/O error.
pub(crate) fn from_io(e: std::io::Error) -> NetworkError {
    if let Some(inner) = e.get_ref().and_then(|r| r.downcast_ref::<NetworkError>()) {
        return inner.clone();
    }
    NetworkError::Body(e.to_string())
}
