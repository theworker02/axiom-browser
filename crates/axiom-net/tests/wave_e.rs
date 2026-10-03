//! Phase 3 Wave E — Networking 2.0 integration tests against the local standard server:
//! redirect classification, structured errors, header limits, the activity stream, the
//! disk-backed per-profile cache and private-profile cleanup.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axiom_net::test_server::{TestResponse, TestServer};
use axiom_net::{
    CacheLimits, CacheState, CancellationToken, NetworkActivity, NetworkError, NetworkRequest,
    NetworkService, NetworkServiceConfig, ResourceType,
};
use axiom_url::Url;
use crossbeam_channel::Receiver;

fn get(u: &str) -> NetworkRequest {
    NetworkRequest::get(Url::parse(u).unwrap(), ResourceType::Other)
}

fn service_with(f: impl FnOnce(&mut NetworkServiceConfig)) -> NetworkService {
    let mut config = NetworkServiceConfig::default();
    f(&mut config);
    NetworkService::new(config)
}

fn fetch(svc: &NetworkService, req: NetworkRequest) -> (axiom_net::ResponseMeta, Vec<u8>) {
    let resp = svc.execute_blocking(req).expect("request");
    let body = resp.body.read_all(u64::MAX).expect("body");
    (resp.meta, body)
}

fn error_of(svc: &NetworkService, req: NetworkRequest) -> NetworkError {
    match svc.execute_blocking(req) {
        Ok(r) => panic!("expected an error, got status {}", r.meta.status),
        Err(e) => e,
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("axiom-wave-e-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn files_under(dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .map(|e| {
            let p = e.path();
            if p.is_dir() {
                files_under(&p)
            } else {
                1
            }
        })
        .sum()
}

fn drain(rx: &Receiver<NetworkActivity>) -> Vec<NetworkActivity> {
    rx.try_iter().collect()
}

// ---------------------------------------------------------------------------
// Redirects
// ---------------------------------------------------------------------------

#[test]
fn redirect_chain_is_followed_and_every_hop_is_logged() {
    let srv = TestServer::spawn_standard();
    let svc = service_with(|_| {});
    let req = get(&srv.url("/redirect-chain"));
    let id = req.id;
    let (meta, body) = fetch(&svc, req);
    assert_eq!(meta.status, 200);
    assert_eq!(body, b"ok");
    assert!(meta.final_url.as_str().ends_with("/ok"));
    let entry = svc.log_entry(id).expect("log entry");
    let hops: Vec<_> = entry.redirects.iter().map(|r| r.to.clone()).collect();
    assert_eq!(hops.len(), 6, "hops: {hops:?}");
    assert!(hops[0].ends_with("/redirect-chain/1"));
    assert!(hops[5].ends_with("/ok"));
    assert!(entry
        .redirects
        .iter()
        .all(|r| r.status == 302 && !r.cross_origin && r.method == "GET"));
    assert_eq!(srv.request_count(), 7);
}

#[test]
fn redirect_loop_is_classified_as_a_loop() {
    let srv = TestServer::spawn_standard();
    let svc = service_with(|c| c.max_redirects = 5);
    let req = get(&srv.url("/redirect-loop"));
    let id = req.id;
    let err = error_of(&svc, req);
    assert!(matches!(err, NetworkError::RedirectLoop), "{err:?}");
    assert_eq!(err.kind_name(), "redirect_loop");
    let entry = svc.log_entry(id).unwrap();
    assert_eq!(entry.error_kind, Some("redirect_loop"));
    // Bounded: the service stops at the limit instead of spinning.
    assert_eq!(srv.requests_for("/redirect-loop").len(), 6);
}

#[test]
fn distinct_hops_over_the_limit_are_too_many_redirects() {
    let srv = TestServer::spawn_standard();
    let svc = service_with(|c| c.max_redirects = 3);
    let err = error_of(&svc, get(&srv.url("/redirect-chain")));
    assert!(
        matches!(err, NetworkError::TooManyRedirects { limit: 3 }),
        "{err:?}"
    );
    assert_eq!(err.kind_name(), "too_many_redirects");
}

#[test]
fn unsupported_schemes_are_rejected_before_and_during_redirects() {
    for u in ["file:///tmp/x.txt", "ftp://example.com/x"] {
        assert!(Url::parse(u).is_err(), "{u} must not be a network URL");
    }
    let svc = service_with(|_| {});
    // Internal pages are addressable URLs but never go to the network.
    let err = error_of(&svc, get("axiom://settings/"));
    assert!(matches!(err, NetworkError::UnsupportedScheme(_)), "{err:?}");
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/to-file" => TestResponse::redirect(302, "file:///etc/passwd"),
        "/to-internal" => TestResponse::redirect(302, "axiom://settings"),
        _ => TestResponse::ok("x", "text/plain"),
    });
    for path in ["/to-file", "/to-internal"] {
        let err = error_of(&svc, get(&srv.url(path)));
        assert!(
            matches!(err, NetworkError::UnsupportedScheme(_)),
            "{path}: {err:?}"
        );
        assert_eq!(err.kind_name(), "unsupported_scheme");
    }
    assert_eq!(srv.request_count(), 2, "nothing followed past the guard");
}

// ---------------------------------------------------------------------------
// Structured errors and limits
// ---------------------------------------------------------------------------

#[test]
fn oversized_response_headers_are_rejected() {
    let big = "v".repeat(4096);
    let srv =
        TestServer::spawn(move |_| TestResponse::ok("x", "text/plain").with_header("X-Big", &big));
    let svc = service_with(|c| c.max_header_bytes = 1024);
    let req = get(&srv.url("/"));
    let id = req.id;
    let err = error_of(&svc, req);
    assert!(
        matches!(err, NetworkError::HeadersTooLarge { limit: 1024 }),
        "{err:?}"
    );
    assert_eq!(
        svc.log_entry(id).unwrap().error_kind,
        Some("headers_too_large")
    );
    // The default limit accepts the same response.
    let (meta, _) = fetch(&service_with(|_| {}), get(&srv.url("/")));
    assert_eq!(meta.status, 200);
}

#[test]
fn closed_port_is_connection_refused() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let svc = service_with(|c| c.connect_timeout = Duration::from_secs(10));
    let err = error_of(&svc, get(&format!("http://127.0.0.1:{port}/")));
    assert!(matches!(err, NetworkError::ConnectionRefused(_)), "{err:?}");
    assert_eq!(err.kind_name(), "connection_refused");
}

#[test]
fn silent_server_hits_the_read_timeout() {
    let srv = TestServer::spawn_standard();
    let svc = service_with(|c| c.read_timeout = Some(Duration::from_millis(100)));
    let req = get(&srv.url("/slow"));
    let id = req.id;
    let err = error_of(&svc, req);
    assert!(matches!(err, NetworkError::ReadTimeout), "{err:?}");
    assert_eq!(err.kind_name(), "read_timeout");
    assert_eq!(svc.log_entry(id).unwrap().error_kind, Some("read_timeout"));
    // A generous idle timeout lets the same endpoint complete.
    let (meta, body) = fetch(&service_with(|_| {}), get(&srv.url("/slow")));
    assert_eq!((meta.status, body.as_slice()), (200, b"slow".as_slice()));
}

#[test]
fn standard_endpoints_serve_compressed_and_streamed_bodies() {
    let srv = TestServer::spawn_standard();
    let svc = service_with(|_| {});
    let (meta, body) = fetch(&svc, get(&srv.url("/compressed")));
    assert_eq!(meta.status, 200);
    assert_eq!(body.len(), "compressible text ".len() * 4096);
    let (meta, body) = fetch(&svc, get(&srv.url("/stream")));
    assert_eq!(meta.status, 200);
    assert_eq!(body, vec![7u8; 16 * 4096]);
    let (meta, _) = fetch(&svc, get(&srv.url("/status/500")));
    assert_eq!(meta.status, 500);
}

// ---------------------------------------------------------------------------
// Activity stream
// ---------------------------------------------------------------------------

#[test]
fn activity_stream_reports_a_redirected_request_in_order() {
    let srv = TestServer::spawn_standard();
    let svc = service_with(|_| {});
    let rx = svc.subscribe_activity(256);
    let mut req = get(&srv.url("/redirect"));
    req.headers.set("Authorization", "Bearer secret-token");
    let id = req.id;
    fetch(&svc, req);
    let events: Vec<_> = drain(&rx).into_iter().filter(|e| e.id() == id).collect();
    let names: Vec<_> = events.iter().map(|e| e.name()).collect();
    let pos = |n: &str| {
        names
            .iter()
            .position(|x| *x == n)
            .unwrap_or_else(|| panic!("{n} missing from {names:?}"))
    };
    assert_eq!(names.first(), Some(&"RequestStarted"), "{names:?}");
    assert_eq!(names.last(), Some(&"RequestCompleted"), "{names:?}");
    assert!(pos("CacheMiss") < pos("ResponseStarted"));
    assert!(pos("ResponseStarted") < pos("Redirected"));
    assert!(pos("Redirected") < pos("ResponseHeadersReady"));
    assert!(pos("ResponseHeadersReady") < pos("DataReceived"));
    assert_eq!(
        names
            .iter()
            .filter(|n| **n == "RequestHeadersReady")
            .count(),
        2,
        "one per hop: {names:?}"
    );
    for e in &events {
        if let NetworkActivity::RequestHeadersReady { headers, .. } = e {
            if let Some(v) = headers.get("authorization") {
                assert_eq!(v, "<redacted>");
            }
            assert!(!format!("{headers:?}").contains("secret-token"));
        }
    }
}

#[test]
fn activity_stream_reports_cache_hits_and_cancellation() {
    let srv = TestServer::spawn_standard();
    let svc = service_with(|_| {});
    fetch(&svc, get(&srv.url("/cache/max-age")));
    let rx = svc.subscribe_activity(64);
    let req = get(&srv.url("/cache/max-age"));
    let id = req.id;
    fetch(&svc, req);
    let events: Vec<_> = drain(&rx).into_iter().filter(|e| e.id() == id).collect();
    assert!(events.iter().any(|e| matches!(
        e,
        NetworkActivity::CacheHit {
            state: CacheState::Hit,
            ..
        }
    )));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, NetworkActivity::ResponseStarted { .. })),
        "a fresh hit must not touch the network"
    );
    assert_eq!(srv.requests_for("/cache/max-age").len(), 1);

    let cancel = CancellationToken::new();
    cancel.cancel();
    let req = get(&srv.url("/ok"));
    let id = req.id;
    let result = svc.execute_with_cancel(req, &cancel);
    assert!(
        matches!(result, Err(NetworkError::Cancelled)),
        "pre-cancelled request was not refused"
    );
    assert!(drain(&rx)
        .iter()
        .any(|e| matches!(e, NetworkActivity::RequestCanceled { id: i } if *i == id)));
}

#[test]
fn slow_subscriber_drops_events_instead_of_blocking() {
    let srv = TestServer::spawn_standard();
    let svc = service_with(|_| {});
    let rx = svc.subscribe_activity(2);
    for _ in 0..5 {
        fetch(&svc, get(&srv.url("/ok")));
    }
    assert_eq!(drain(&rx).len(), 2);
    assert!(svc.activity().dropped() > 0);
}

// ---------------------------------------------------------------------------
// Per-profile disk cache
// ---------------------------------------------------------------------------

fn disk_service(dir: &Path) -> NetworkService {
    service_with(|c| {
        c.persistent_cache = true;
        c.cache_dir = Some(dir.to_path_buf());
        c.cache_limits = CacheLimits {
            max_total_bytes: 1024 * 1024,
            max_entry_bytes: 256 * 1024,
        };
    })
}

#[test]
fn persistent_profile_cache_survives_restart() {
    let srv = TestServer::spawn_standard();
    let dir = TempDir::new("persist");
    {
        let svc = disk_service(dir.path());
        assert!(svc.cache().is_disk_backed());
        let (meta, _) = fetch(&svc, get(&srv.url("/cache/max-age")));
        assert_eq!(meta.cache_state, CacheState::Miss);
        svc.shutdown();
    }
    assert!(
        files_under(dir.path()) >= 2,
        "body + index expected on disk"
    );
    let svc = disk_service(dir.path());
    let (meta, body) = fetch(&svc, get(&srv.url("/cache/max-age")));
    assert_eq!(meta.cache_state, CacheState::Hit);
    assert_eq!(body, b"fresh");
    assert_eq!(srv.requests_for("/cache/max-age").len(), 1);
    // A revalidated entry is also restored from disk.
    fetch(&svc, get(&srv.url("/cache/etag")));
    svc.shutdown();
    drop(svc);
    let svc = disk_service(dir.path());
    let (meta, body) = fetch(&svc, get(&srv.url("/cache/etag")));
    assert_eq!(meta.cache_state, CacheState::Revalidated);
    assert_eq!(body, b"etag body");
    let reval = srv.requests_for("/cache/etag");
    assert_eq!(
        reval.last().unwrap().header("if-none-match"),
        Some("\"v1\"")
    );
}

#[test]
fn profiles_do_not_share_disk_caches() {
    let srv = TestServer::spawn_standard();
    let a_dir = TempDir::new("profile-a");
    let b_dir = TempDir::new("profile-b");
    let a = disk_service(a_dir.path());
    let b = disk_service(b_dir.path());
    fetch(&a, get(&srv.url("/cache/max-age")));
    let (meta, _) = fetch(&b, get(&srv.url("/cache/max-age")));
    assert_eq!(meta.cache_state, CacheState::Miss);
    assert_eq!(srv.requests_for("/cache/max-age").len(), 2);
    a.cache().clear();
    let (meta, _) = fetch(&b, get(&srv.url("/cache/max-age")));
    assert_eq!(
        meta.cache_state,
        CacheState::Hit,
        "clearing A must not touch B"
    );
}

#[test]
fn private_profile_cache_never_touches_disk_and_is_gone_after_shutdown() {
    let srv = TestServer::spawn_standard();
    let dir = TempDir::new("private");
    let svc = service_with(|c| {
        c.persistent_cache = false;
        // Even if a directory is configured, a private cache stays in memory.
        c.cache_dir = Some(dir.path().to_path_buf());
    });
    assert!(!svc.cache().is_disk_backed());
    fetch(&svc, get(&srv.url("/cache/max-age")));
    assert_eq!(svc.cache().stats().entries, 1);
    assert_eq!(files_under(dir.path()), 0);
    svc.shutdown();
    assert_eq!(svc.cache().stats().entries, 0);
    assert_eq!(files_under(dir.path()), 0);
}

#[test]
fn log_entry_carries_request_detail_without_secrets() {
    let srv = TestServer::spawn_standard();
    let svc = service_with(|_| {});
    let mut req = NetworkRequest::get(
        Url::parse(&srv.url("/redirect?token=abc")).unwrap(),
        ResourceType::Image,
    );
    req.referrer = Some(srv.url("/page?session=xyz"));
    req.initiator = "img".into();
    req.headers.set("Cookie", "forged=1");
    req.headers.set("Authorization", "Basic c2VjcmV0");
    let id = req.id;
    fetch(&svc, req);
    let e = svc.log_entry(id).unwrap();
    assert_eq!(e.resource_type, "image");
    assert_eq!(e.priority, "medium");
    assert_eq!(e.initiator, "img");
    assert_eq!(e.redirects.len(), 1);
    assert_eq!(e.cache_state, Some(CacheState::NotCacheable));
    assert!(e.tls.is_none(), "plain http has no TLS info");
    assert_eq!(e.request_headers.get("authorization"), Some("<redacted>"));
    assert!(e
        .request_headers
        .get("cookie")
        .is_none_or(|v| v == "<redacted>"));
    assert!(e.timing.total_ms.is_some());
    assert!(
        e.timing.dns_ms.is_none(),
        "DNS is not measured by this transport"
    );
}
