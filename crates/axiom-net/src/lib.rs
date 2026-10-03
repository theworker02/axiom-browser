//! Axiom networking platform.
//!
//! Path: ResourceLoader (axiom-loader) → [`RequestScheduler`] → [`NetworkService`] →
//! [`Transport`]. Cookie policy stays outside this crate behind [`CookieProvider`]
//! (implemented by the browser's CookieService). One service, scheduler and cache exist per
//! browser profile and are shared by its tabs.

mod activity;
mod body;
mod cache;
mod cancel;
mod cors;
mod disk_cache;
mod dns;
mod encoding;
mod error;
mod flow;
mod headers;
mod http_date;
mod id;
mod metrics;
mod mime;
mod policy;
mod priority;
mod protocol;
mod redirect;
mod referrer;
mod request;
mod resource_type;
mod response;
mod scheduler;
mod service;
mod timing;
mod transport;
mod user_agent;

#[doc(hidden)]
pub mod test_server;

pub use activity::{ActivityHub, NetworkActivity};
pub use body::{
    parse_content_encoding, BodyCounters, BodyOutcome, RequestBody, ResponseBodyReader,
    StreamingBody, UploadSender,
};
pub use cache::{
    freshness_lifetime, is_storable, parse_vary, CacheDirectives, CacheEntry, CacheKey,
    CacheLimits, CacheMode, CacheState, CacheStats, HttpCache,
};
pub use cancel::CancellationToken;
pub use cors::{
    cors_check, cors_unsafe_header_names, evaluate_preflight, is_cors_safelisted_method,
    is_cors_safelisted_request_header, is_forbidden_request_header,
    is_no_cors_safelisted_request_header, origin_tainted, serialize_origin, PreflightCache,
    PreflightGrant, PreflightRequest, DEFAULT_PREFLIGHT_AGE, MAX_PREFLIGHT_AGE,
};
pub use dns::{DnsResolver, FixedDnsResolver, SystemDnsResolver};
pub use encoding::{
    prescan_meta_charset, EncodingSource, TextDecoder, TextEncoding, META_PRESCAN_BYTES,
};
pub use error::{CertificateErrorKind, NetworkError};
pub use flow::FlowControl;
pub use headers::HeaderMap;
pub use http_date::{format_http_date, now_unix, parse_http_date};
pub use id::NetworkRequestId;
pub use metrics::{MetricsSnapshot, NetworkMetrics, PoolStats};
pub use mime::{decode_text, MimeType};
pub use policy::{HostBlocklist, NetworkPolicy};
pub use priority::RequestPriority;
pub use protocol::HttpProtocol;
pub use redirect::{is_redirect_status, same_origin, RedirectMode, RedirectRecord};
pub use referrer::{compute_referrer, is_potentially_trustworthy, ReferrerPolicy};
pub use request::{CredentialsMode, HttpMethod, NetworkRequest, RedirectCheck, RequestMode};
pub use resource_type::ResourceType;
pub use response::{CertificateInfo, ContentRange, NetworkResponse, ResponseMeta, TlsInfo};
pub use scheduler::{
    effective_priority, NetworkEvent, RequestScheduler, SchedulerConfig, SchedulerStats,
};
pub use service::{
    detect_download, CookieProvider, CookieRequestContext, DownloadCandidate, LogState,
    LoggedRedirect, NetworkLogEntry, NetworkService, NetworkServiceConfig, NullCookieProvider,
};
pub use timing::NetworkTiming;
pub use transport::{RawTransportResponse, ReqwestTransport, Transport, TransportConfig};
pub use user_agent::{accept_encoding, accept_for, AXIOM_USER_AGENT, DEFAULT_ACCEPT_LANGUAGE};
