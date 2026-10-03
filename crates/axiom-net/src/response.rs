use crate::body::{BodyCounters, ResponseBodyReader};
use crate::cache::CacheState;
use crate::error::NetworkError;
use crate::headers::HeaderMap;
use crate::id::NetworkRequestId;
use crate::mime::MimeType;
use crate::protocol::HttpProtocol;
use crate::redirect::RedirectRecord;
use crate::service::DownloadCandidate;
use crate::timing::NetworkTiming;
use axiom_url::Url;

/// TLS facts the transport can actually observe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsInfo {
    /// Negotiated application protocol (derived from the response HTTP version).
    pub alpn: HttpProtocol,
    /// The certificate chain was validated against the trust store (verification is never
    /// disabled, so any completed HTTPS response implies this).
    pub certificate_verified: bool,
    /// TLS protocol version — not exposed by the transport, so reported as unavailable.
    pub version: Option<String>,
    /// Negotiated cipher suite — not exposed by the transport, so reported as unavailable.
    pub cipher_suite: Option<String>,
    /// Host name the certificate was validated for.
    pub hostname: Option<String>,
    /// The server's leaf certificate, as presented on the connection.
    pub certificate: Option<CertificateInfo>,
}

/// Parsed leaf certificate. Informational only: validation happens in rustls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateInfo {
    pub subject: String,
    pub issuer: String,
    pub subject_alt_names: Vec<String>,
    pub serial_hex: String,
    pub not_before_unix: i64,
    pub not_after_unix: i64,
    /// SHA-256 over the DER encoding, as colon-free upper-case hex.
    pub sha256_fingerprint: String,
}

impl CertificateInfo {
    /// Parse a DER certificate. `None` if it cannot be parsed (the connection was still
    /// validated by rustls; only the display metadata is missing).
    pub fn from_der(der: &[u8]) -> Option<Self> {
        use x509_parser::extensions::GeneralName;
        let (_, cert) = x509_parser::parse_x509_certificate(der).ok()?;
        let subject_alt_names = cert
            .subject_alternative_name()
            .ok()
            .flatten()
            .map(|san| {
                san.value
                    .general_names
                    .iter()
                    .filter_map(|n| match n {
                        GeneralName::DNSName(d) => Some((*d).to_string()),
                        GeneralName::IPAddress(ip) => match ip.len() {
                            4 => Some(
                                std::net::Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3]).to_string(),
                            ),
                            16 => {
                                let mut b = [0u8; 16];
                                b.copy_from_slice(ip);
                                Some(std::net::Ipv6Addr::from(b).to_string())
                            }
                            _ => None,
                        },
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let digest = ring::digest::digest(&ring::digest::SHA256, der);
        Some(Self {
            subject: cert.subject().to_string(),
            issuer: cert.issuer().to_string(),
            subject_alt_names,
            serial_hex: cert.raw_serial_as_string().replace(':', "").to_uppercase(),
            not_before_unix: cert.validity().not_before.timestamp(),
            not_after_unix: cert.validity().not_after.timestamp(),
            sha256_fingerprint: digest.as_ref().iter().map(|b| format!("{b:02X}")).collect(),
        })
    }
}

/// Parsed `Content-Range: bytes start-end/complete`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentRange {
    pub start: u64,
    pub end: u64,
    pub complete_length: Option<u64>,
}

impl ContentRange {
    pub fn parse(value: &str) -> Option<Self> {
        let rest = value.trim().strip_prefix("bytes")?.trim_start();
        let (range, complete) = rest.split_once('/')?;
        let (start, end) = range.trim().split_once('-')?;
        let start: u64 = start.trim().parse().ok()?;
        let end: u64 = end.trim().parse().ok()?;
        if end < start {
            return None;
        }
        let complete_length = match complete.trim() {
            "*" => None,
            n => {
                let n: u64 = n.parse().ok()?;
                if end >= n {
                    return None;
                }
                Some(n)
            }
        };
        Some(Self {
            start,
            end,
            complete_length,
        })
    }

    pub fn len(&self) -> u64 {
        self.end - self.start + 1
    }

    pub fn is_empty(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone)]
pub struct ResponseMeta {
    pub request_id: NetworkRequestId,
    pub final_url: Url,
    pub status: u16,
    pub reason: Option<String>,
    pub headers: HeaderMap,
    pub mime: Option<MimeType>,
    /// `Content-Length` as sent (encoded length when a content-coding is applied).
    pub content_length: Option<u64>,
    pub content_encoding: Vec<String>,
    pub content_range: Option<ContentRange>,
    pub protocol: HttpProtocol,
    pub tls: Option<TlsInfo>,
    pub redirect_chain: Vec<RedirectRecord>,
    pub cache_state: CacheState,
    pub timing: NetworkTiming,
    pub set_cookies: Vec<String>,
    pub download: Option<DownloadCandidate>,
    /// Live byte counters; final once the body has been read to the end.
    pub counters: BodyCounters,
}

impl ResponseMeta {
    pub fn transferred_bytes(&self) -> u64 {
        self.counters.transferred()
    }

    pub fn decoded_bytes(&self) -> u64 {
        self.counters.decoded()
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn from_cache(&self) -> bool {
        matches!(self.cache_state, CacheState::Hit | CacheState::Revalidated)
    }
}

pub struct NetworkResponse {
    pub meta: ResponseMeta,
    pub body: ResponseBodyReader,
}

impl NetworkResponse {
    pub fn final_url(&self) -> &Url {
        &self.meta.final_url
    }

    pub fn status(&self) -> u16 {
        self.meta.status
    }

    /// Read the whole body (64 MiB cap).
    pub fn body_bytes(&self) -> Result<Vec<u8>, NetworkError> {
        self.body.read_all(64 * 1024 * 1024)
    }

    pub fn into_parts(self) -> (ResponseMeta, ResponseBodyReader) {
        (self.meta, self.body)
    }
}

impl std::fmt::Debug for NetworkResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkResponse")
            .field("status", &self.meta.status)
            .field("final_url", &self.meta.final_url.as_str())
            .field("cache_state", &self.meta.cache_state)
            .finish()
    }
}

impl std::ops::Deref for NetworkResponse {
    type Target = ResponseMeta;
    fn deref(&self) -> &Self::Target {
        &self.meta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range_parsing() {
        let r = ContentRange::parse("bytes 0-99/1000").unwrap();
        assert_eq!(
            (r.start, r.end, r.complete_length, r.len()),
            (0, 99, Some(1000), 100)
        );
        assert_eq!(
            ContentRange::parse("bytes 5-9/*").unwrap().complete_length,
            None
        );
        assert!(ContentRange::parse("bytes 9-5/10").is_none());
        assert!(ContentRange::parse("bytes 0-10/10").is_none());
        assert!(ContentRange::parse("items 0-1/2").is_none());
    }
}
