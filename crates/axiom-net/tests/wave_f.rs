//! Wave F.1 — networking integration tests against a local recording HTTP server.
//!
//! Every behavioural claim is verified from the server side (what was actually sent)
//! rather than from client-side bookkeeping alone.

use std::io::Write;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axiom_net::test_server::{TestResponse, TestServer};
use axiom_net::{
    format_http_date, now_unix, CacheLimits, CacheMode, CacheState, CancellationToken,
    CookieProvider, CookieRequestContext, FixedDnsResolver, HostBlocklist, HttpMethod,
    HttpProtocol, NetworkError, NetworkEvent, NetworkRequest, NetworkRequestId, NetworkService,
    NetworkServiceConfig, ReferrerPolicy, RequestBody, RequestPriority, RequestScheduler,
    ResourceType, SchedulerConfig, StreamingBody, AXIOM_USER_AGENT,
};
use axiom_url::Url;
use crossbeam_channel::{bounded, Receiver};
use parking_lot::Mutex;

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

fn service() -> NetworkService {
    NetworkService::new(NetworkServiceConfig::default())
}

fn get(u: &str) -> NetworkRequest {
    NetworkRequest::get(url(u), ResourceType::Other)
}

/// Execute and read the whole body.
fn fetch(svc: &NetworkService, req: NetworkRequest) -> (axiom_net::ResponseMeta, Vec<u8>) {
    let resp = svc.execute_blocking(req).expect("request");
    let body = resp.body.read_all(u64::MAX).expect("body");
    (resp.meta, body)
}

fn date_now() -> String {
    format_http_date(now_unix())
}

// ---------------------------------------------------------------------------
// Connections and content coding
// ---------------------------------------------------------------------------

#[test]
fn keep_alive_reuses_one_connection() {
    let srv = TestServer::spawn(|_| {
        TestResponse::ok("hello", "text/plain").with_header("Cache-Control", "no-store")
    });
    let svc = service();
    for _ in 0..5 {
        let (meta, body) = fetch(&svc, get(&srv.url("/a")));
        assert_eq!(meta.status, 200);
        assert_eq!(body, b"hello");
        assert_eq!(meta.protocol, HttpProtocol::Http11);
    }
    assert_eq!(srv.request_count(), 5);
    assert_eq!(
        srv.connections(),
        1,
        "server saw more than one TCP connection"
    );
    assert_eq!(svc.pool_stats().connections_opened, Some(1));
    // Reuse is not observable through reqwest; it must not be reported as measured.
    assert_eq!(svc.pool_stats().connections_reused, None);
}

fn compress(coding: &str, data: &[u8]) -> Vec<u8> {
    match coding {
        "gzip" => {
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(data).unwrap();
            e.finish().unwrap()
        }
        "deflate" => {
            let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(data).unwrap();
            e.finish().unwrap()
        }
        "br" => {
            let mut w = brotli::CompressorWriter::new(Vec::new(), 4096, 5, 22);
            w.write_all(data).unwrap();
            w.into_inner()
        }
        _ => unreachable!(),
    }
}

#[test]
fn content_encodings_decode_with_honest_byte_counts() {
    let original: Vec<u8> = "The quick brown fox jumps over the lazy dog. "
        .repeat(500)
        .into_bytes();
    let orig = original.clone();
    let srv = TestServer::spawn(move |req| {
        let coding = req.path.trim_start_matches('/');
        TestResponse::ok(compress(coding, &orig), "text/plain")
            .with_header("Content-Encoding", coding)
            .with_header("Cache-Control", "no-store")
    });
    let svc = service();
    for coding in ["gzip", "deflate", "br"] {
        let wire_len = compress(coding, &original).len() as u64;
        let (meta, body) = fetch(&svc, get(&srv.url(&format!("/{coding}"))));
        assert_eq!(body, original, "{coding} decode mismatch");
        assert_eq!(meta.content_encoding, vec![coding.to_string()]);
        assert_eq!(meta.transferred_bytes(), wire_len, "{coding} transferred");
        assert_eq!(
            meta.decoded_bytes(),
            original.len() as u64,
            "{coding} decoded"
        );
        assert!(meta.transferred_bytes() < meta.decoded_bytes());
    }
    let ae = srv.requests()[0]
        .header("accept-encoding")
        .unwrap()
        .to_string();
    for c in ["gzip", "deflate", "br"] {
        assert!(ae.contains(c), "Accept-Encoding {ae:?} lacks {c}");
    }
    let m = svc.metrics().snapshot();
    assert!(m.bytes_transferred < m.bytes_decoded);
}

#[test]
fn corrupt_encoded_body_is_a_body_error() {
    let srv = TestServer::spawn(|_| {
        TestResponse::ok(b"definitely not gzip".to_vec(), "text/plain")
            .with_header("Content-Encoding", "gzip")
    });
    let svc = service();
    let resp = svc.execute_blocking(get(&srv.url("/"))).unwrap();
    let err = resp.body.read_all(u64::MAX).unwrap_err();
    assert!(matches!(err, NetworkError::Body(_)), "{err:?}");
}

// ---------------------------------------------------------------------------
// HTTP cache
// ---------------------------------------------------------------------------

#[test]
fn etag_revalidation_uses_304() {
    let srv = TestServer::spawn(|req| {
        if req.header("if-none-match") == Some("\"v1\"") {
            TestResponse::status(304, "").with_header("ETag", "\"v1\"")
        } else {
            TestResponse::ok("version-1", "text/plain")
                .with_header("ETag", "\"v1\"")
                .with_header("Cache-Control", "max-age=0")
        }
    });
    let svc = service();
    let (m1, b1) = fetch(&svc, get(&srv.url("/doc")));
    assert_eq!(m1.cache_state, CacheState::Miss);
    let (m2, b2) = fetch(&svc, get(&srv.url("/doc")));
    assert_eq!(m2.cache_state, CacheState::Revalidated);
    assert_eq!(m2.status, 200);
    assert_eq!(b1, b2);
    let reqs = srv.requests_for("/doc");
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].header("if-none-match"), None);
    assert_eq!(reqs[1].header("if-none-match"), Some("\"v1\""));
    assert_eq!(svc.cache().stats().revalidations, 1);
}

#[test]
fn last_modified_revalidation_with_no_cache() {
    let lm = format_http_date(now_unix() - 3600);
    let lm2 = lm.clone();
    let srv = TestServer::spawn(move |req| {
        if req.header("if-modified-since") == Some(lm2.as_str()) {
            TestResponse::status(304, "")
        } else {
            TestResponse::ok("body", "text/plain")
                .with_header("Last-Modified", &lm2)
                .with_header("Cache-Control", "no-cache")
        }
    });
    let svc = service();
    fetch(&svc, get(&srv.url("/r")));
    let (m, b) = fetch(&svc, get(&srv.url("/r")));
    assert_eq!(m.cache_state, CacheState::Revalidated);
    assert_eq!(b, b"body");
    assert_eq!(
        srv.requests_for("/r")[1].header("if-modified-since"),
        Some(lm.as_str())
    );
}

#[test]
fn fresh_max_age_is_served_without_network() {
    let srv = TestServer::spawn(|_| {
        TestResponse::ok("cached", "text/plain").with_header("Cache-Control", "max-age=60")
    });
    let svc = service();
    fetch(&svc, get(&srv.url("/f")));
    let (m, b) = fetch(&svc, get(&srv.url("/f")));
    assert_eq!(m.cache_state, CacheState::Hit);
    assert!(m.from_cache());
    assert_eq!(b, b"cached");
    assert_eq!(srv.request_count(), 1);
}

#[test]
fn vary_selects_the_matching_variant() {
    let srv = TestServer::spawn(|req| {
        let lang = req.header("accept-language").unwrap_or("none").to_string();
        TestResponse::ok(lang, "text/plain")
            .with_header("Cache-Control", "max-age=60")
            .with_header("Vary", "Accept-Language")
    });
    let svc = service();
    let with_lang = |l: &str| {
        let mut r = get(&srv.url("/v"));
        r.headers.set("Accept-Language", l);
        r
    };
    assert_eq!(fetch(&svc, with_lang("en")).1, b"en");
    assert_eq!(fetch(&svc, with_lang("fr")).1, b"fr");
    let (m, b) = fetch(&svc, with_lang("en"));
    assert_eq!((m.cache_state, b.as_slice()), (CacheState::Hit, &b"en"[..]));
    let (m, b) = fetch(&svc, with_lang("fr"));
    assert_eq!((m.cache_state, b.as_slice()), (CacheState::Hit, &b"fr"[..]));
    assert_eq!(srv.request_count(), 2);
}

#[test]
fn no_store_is_never_cached() {
    let srv = TestServer::spawn(|_| {
        TestResponse::ok("x", "text/plain").with_header("Cache-Control", "no-store, max-age=60")
    });
    let svc = service();
    fetch(&svc, get(&srv.url("/n")));
    let (m, _) = fetch(&svc, get(&srv.url("/n")));
    assert_eq!(m.cache_state, CacheState::NotCacheable);
    assert_eq!(srv.request_count(), 2);
    assert!(svc.cache().is_empty());
}

#[test]
fn expires_past_is_stale_and_future_is_fresh() {
    let srv = TestServer::spawn(|req| {
        let now = now_unix();
        let exp = if req.path == "/past" {
            now - 10
        } else {
            now + 60
        };
        TestResponse::ok("e", "text/plain")
            .with_header("Date", &format_http_date(now))
            .with_header("Expires", &format_http_date(exp))
    });
    let svc = service();
    for _ in 0..2 {
        fetch(&svc, get(&srv.url("/past")));
        fetch(&svc, get(&srv.url("/future")));
    }
    assert_eq!(srv.requests_for("/past").len(), 2);
    assert_eq!(srv.requests_for("/future").len(), 1);
}

#[test]
fn heuristic_freshness_requires_last_modified() {
    let srv = TestServer::spawn(|req| {
        let r = TestResponse::ok("h", "text/plain").with_header("Date", &date_now());
        if req.path == "/lm" {
            r.with_header("Last-Modified", &format_http_date(now_unix() - 10_000))
        } else {
            r
        }
    });
    let svc = service();
    for _ in 0..2 {
        fetch(&svc, get(&srv.url("/lm")));
        fetch(&svc, get(&srv.url("/plain")));
    }
    assert_eq!(
        srv.requests_for("/lm").len(),
        1,
        "heuristic freshness not applied"
    );
    assert_eq!(
        srv.requests_for("/plain").len(),
        2,
        "cached without any freshness"
    );
}

#[test]
fn unsafe_method_invalidates_cached_get() {
    let srv = TestServer::spawn(|_| {
        TestResponse::ok("res", "text/plain").with_header("Cache-Control", "max-age=60")
    });
    let svc = service();
    fetch(&svc, get(&srv.url("/res")));
    let mut post =
        NetworkRequest::new(HttpMethod::Post, url(&srv.url("/res")), ResourceType::Fetch);
    post.body = RequestBody::Bytes(b"x".to_vec());
    fetch(&svc, post);
    let (m, _) = fetch(&svc, get(&srv.url("/res")));
    assert_eq!(m.cache_state, CacheState::Miss);
    let gets = srv
        .requests_for("/res")
        .into_iter()
        .filter(|r| r.method == "GET")
        .count();
    assert_eq!(gets, 2);
}

#[test]
fn only_if_cached_never_touches_the_network() {
    let srv = TestServer::spawn(|_| {
        TestResponse::ok("c", "text/plain").with_header("Cache-Control", "max-age=60")
    });
    let svc = service();
    let mut r = get(&srv.url("/o"));
    r.cache_mode = CacheMode::OnlyIfCached;
    assert!(matches!(
        svc.execute_blocking(r),
        Err(NetworkError::Cache(_))
    ));
    assert_eq!(srv.request_count(), 0);
    fetch(&svc, get(&srv.url("/o")));
    let mut r = get(&srv.url("/o"));
    r.cache_mode = CacheMode::OnlyIfCached;
    assert_eq!(fetch(&svc, r).0.cache_state, CacheState::Hit);
    assert_eq!(srv.request_count(), 1);
}

#[test]
fn reload_bypasses_fresh_cache() {
    let srv = TestServer::spawn(|_| {
        TestResponse::ok("c", "text/plain").with_header("Cache-Control", "max-age=60")
    });
    let svc = service();
    fetch(&svc, get(&srv.url("/r")));
    let mut r = get(&srv.url("/r"));
    r.cache_mode = CacheMode::Reload;
    assert_eq!(fetch(&svc, r).0.cache_state, CacheState::Bypassed);
    assert_eq!(srv.request_count(), 2);
}

#[test]
fn force_cache_serves_stale_entries_without_network() {
    let srv = TestServer::spawn(|_| {
        TestResponse::ok("old", "text/plain").with_header("Cache-Control", "max-age=0")
    });
    let svc = service();
    fetch(&svc, get(&srv.url("/f")));
    let mut r = get(&srv.url("/f"));
    r.cache_mode = CacheMode::ForceCache;
    let (meta, body) = fetch(&svc, r);
    assert_eq!(meta.cache_state, CacheState::Stale);
    assert!(meta.cache_state.served_from_cache());
    assert_eq!(body, b"old");
    assert_eq!(srv.request_count(), 1);
}

#[test]
fn no_store_mode_neither_reads_nor_writes_the_cache() {
    let srv = TestServer::spawn(|_| {
        TestResponse::ok("c", "text/plain").with_header("Cache-Control", "max-age=60")
    });
    let svc = service();
    fetch(&svc, get(&srv.url("/primed")));
    let mut r = get(&srv.url("/primed"));
    r.cache_mode = CacheMode::NoStore;
    assert_ne!(fetch(&svc, r).0.cache_state, CacheState::Hit);
    assert_eq!(srv.request_count(), 2);

    let mut r = get(&srv.url("/fresh"));
    r.cache_mode = CacheMode::NoStore;
    fetch(&svc, r);
    assert_eq!(
        fetch(&svc, get(&srv.url("/fresh"))).0.cache_state,
        CacheState::Miss
    );
    assert_eq!(srv.requests_for("/fresh").len(), 2);
}

#[test]
fn no_cache_mode_revalidates_fresh_entries() {
    let srv = TestServer::spawn(|req| {
        if req.header("if-none-match") == Some("\"v1\"") {
            TestResponse::status(304, "")
        } else {
            TestResponse::ok("body", "text/plain")
                .with_header("Cache-Control", "max-age=60")
                .with_header("ETag", "\"v1\"")
        }
    });
    let svc = service();
    fetch(&svc, get(&srv.url("/n")));
    let mut r = get(&srv.url("/n"));
    r.cache_mode = CacheMode::NoCache;
    let (meta, body) = fetch(&svc, r);
    assert_eq!(meta.cache_state, CacheState::Revalidated);
    assert_eq!(body, b"body");
    let reqs = srv.requests_for("/n");
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[1].header("if-none-match"), Some("\"v1\""));
}

#[test]
fn large_streamed_response_keeps_cache_buffer_bounded() {
    const MB: usize = 1024 * 1024;
    let srv = TestServer::spawn(|_| TestResponse::Chunked {
        status: 200,
        headers: vec![("Cache-Control".into(), "max-age=60".into())],
        chunks: (0..32).map(|i| vec![i as u8; MB]).collect(),
        delay_ms: 0,
    });
    let limits = CacheLimits::default();
    let svc = service();
    let resp = svc.execute_blocking(get(&srv.url("/big"))).unwrap();
    // Stream it; never hold the whole body.
    let mut total = 0usize;
    loop {
        let chunk = resp.body.read_chunk(64 * 1024).unwrap();
        if chunk.is_empty() {
            break;
        }
        total += chunk.len();
    }
    assert_eq!(total, 32 * MB);
    assert!(svc.cache().is_empty(), "over-limit body must not be cached");
    let peak = svc.metrics().snapshot().peak_cache_buffer_bytes;
    assert!(
        peak <= limits.max_entry_bytes as u64,
        "cache tee buffered {peak} bytes"
    );
}

#[test]
fn range_request_returns_206_and_is_not_cached() {
    let srv = TestServer::spawn(|req| match req.header("range") {
        Some("bytes=0-9") => TestResponse::status(206, "0123456789")
            .with_header("Content-Range", "bytes 0-9/100")
            .with_header("Cache-Control", "max-age=60"),
        _ => TestResponse::ok(vec![b'x'; 100], "application/octet-stream"),
    });
    let svc = service();
    let mut r = get(&srv.url("/range"));
    r.headers.set("Range", "bytes=0-9");
    let (m, b) = fetch(&svc, r);
    assert_eq!(m.status, 206);
    assert_eq!(b, b"0123456789");
    let cr = m.content_range.expect("content-range");
    assert_eq!((cr.start, cr.end, cr.complete_length), (0, 9, Some(100)));
    assert_eq!(m.cache_state, CacheState::Bypassed);
    assert!(svc.cache().is_empty());
    assert_eq!(
        srv.requests()[0].header("accept-encoding"),
        Some("identity")
    );
}

// ---------------------------------------------------------------------------
// Redirects and cookies
// ---------------------------------------------------------------------------

#[test]
fn redirect_method_and_body_matrix_verified_server_side() {
    let srv = TestServer::spawn(|req| {
        if let Some(code) = req.path.strip_prefix("/r") {
            TestResponse::redirect(code.parse().unwrap(), &format!("/target{code}"))
        } else {
            TestResponse::ok("done", "text/plain")
        }
    });
    let svc = service();
    for (code, method_out, body_kept) in [
        (301, "GET", false),
        (302, "GET", false),
        (303, "GET", false),
        (307, "POST", true),
        (308, "POST", true),
    ] {
        let mut post = NetworkRequest::new(
            HttpMethod::Post,
            url(&srv.url(&format!("/r{code}"))),
            ResourceType::Fetch,
        );
        post.headers.set("Content-Type", "text/plain");
        post.body = RequestBody::Bytes(b"payload".to_vec());
        let (meta, _) = fetch(&svc, post);
        assert_eq!(meta.status, 200);
        assert_eq!(meta.redirect_chain.len(), 1);
        assert_eq!(meta.redirect_chain[0].status, code);
        assert_eq!(meta.redirect_chain[0].method_out.as_str(), method_out);
        assert!(!meta.redirect_chain[0].cross_origin);

        let origin = srv.requests_for(&format!("/r{code}"));
        assert_eq!(origin[0].method, "POST");
        assert_eq!(origin[0].body, b"payload");
        let target = srv.requests_for(&format!("/target{code}"));
        assert_eq!(target.len(), 1, "{code}");
        assert_eq!(target[0].method, method_out, "{code}");
        if body_kept {
            assert_eq!(target[0].body, b"payload", "{code}");
            assert_eq!(target[0].header("content-type"), Some("text/plain"));
        } else {
            assert!(target[0].body.is_empty(), "{code} replayed body");
            assert_eq!(target[0].header("content-type"), None, "{code}");
        }
    }
}

#[test]
fn streamed_body_is_not_replayed_across_307() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/r307" => TestResponse::redirect(307, "/t307"),
        "/r303" => TestResponse::redirect(303, "/t303"),
        _ => TestResponse::ok("ok", "text/plain"),
    });
    let svc = service();
    let stream_post = |path: &str| {
        let mut r = NetworkRequest::new(HttpMethod::Post, url(&srv.url(path)), ResourceType::Fetch);
        r.body = RequestBody::Stream(StreamingBody::new(
            std::io::Cursor::new(b"abc".to_vec()),
            Some(3),
        ));
        r
    };
    let err = svc.execute_blocking(stream_post("/r307")).unwrap_err();
    assert!(matches!(err, NetworkError::Protocol(_)), "{err:?}");
    assert!(srv.requests_for("/t307").is_empty());
    // 303 drops the body, so following is fine.
    let (m, _) = fetch(&svc, stream_post("/r303"));
    assert_eq!(m.status, 200);
    assert_eq!(srv.requests_for("/t303")[0].method, "GET");
}

#[test]
fn redirect_limit_is_enforced() {
    let srv = TestServer::spawn(|_| TestResponse::redirect(302, "/loop"));
    let svc = service();
    let err = svc.execute_blocking(get(&srv.url("/loop"))).unwrap_err();
    assert_eq!(err, NetworkError::RedirectLoop);
    assert_eq!(srv.request_count(), svc.config().max_redirects + 1);
}

#[test]
fn cross_origin_redirect_strips_authorization_and_records_metadata() {
    let b = TestServer::spawn(|_| TestResponse::ok("b", "text/plain"));
    let target = b.url("/landing");
    let a = TestServer::spawn(move |_| TestResponse::redirect(302, &target));
    let svc = service();
    let mut r = get(&a.url("/go"));
    r.headers.set("Authorization", "Bearer secret");
    let (m, body) = fetch(&svc, r);
    assert_eq!(body, b"b");
    assert_eq!(m.final_url.port, Some(b.port()));
    assert!(m.redirect_chain[0].cross_origin);
    assert_eq!(
        a.requests()[0].header("authorization"),
        Some("Bearer secret")
    );
    assert_eq!(b.requests()[0].header("authorization"), None);
}

#[derive(Default)]
struct MemCookies {
    jar: Mutex<Vec<String>>,
    seen: Mutex<Vec<(String, bool, String)>>,
}

impl CookieProvider for MemCookies {
    fn cookie_header(&self, ctx: &CookieRequestContext<'_>) -> Option<String> {
        self.seen.lock().push((
            ctx.url.path.clone(),
            ctx.is_top_level_navigation,
            ctx.top_level_url.path.clone(),
        ));
        let jar = self.jar.lock();
        (!jar.is_empty()).then(|| jar.join("; "))
    }

    fn store_set_cookies(&self, _ctx: &CookieRequestContext<'_>, set_cookies: &[String]) {
        let mut jar = self.jar.lock();
        for sc in set_cookies {
            jar.push(sc.split(';').next().unwrap().trim().to_string());
        }
    }
}

#[test]
fn cookies_are_stored_and_sent_on_each_redirect_hop() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/login" => TestResponse::redirect(302, "/home").with_header("Set-Cookie", "sid=1; Path=/"),
        _ => TestResponse::ok("<p>home</p>", "text/html"),
    });
    let cookies = Arc::new(MemCookies::default());
    let svc = service().with_cookies(cookies.clone());
    let mut r = NetworkRequest::get(url(&srv.url("/login")), ResourceType::Document);
    // A page-supplied Cookie header must never reach the wire.
    r.headers.set("Cookie", "forged=1");
    fetch(&svc, r);
    assert_eq!(srv.requests_for("/login")[0].header("cookie"), None);
    assert_eq!(srv.requests_for("/home")[0].header("cookie"), Some("sid=1"));
    // Top-level navigation: the top-level URL follows the redirect.
    let seen = cookies.seen.lock().clone();
    assert_eq!(seen[0], ("/login".into(), true, "/login".into()));
    assert_eq!(seen[1], ("/home".into(), true, "/home".into()));
}

#[test]
fn credentials_omit_sends_and_stores_no_cookies() {
    let srv =
        TestServer::spawn(|_| TestResponse::ok("x", "text/plain").with_header("Set-Cookie", "a=1"));
    let cookies = Arc::new(MemCookies::default());
    cookies.jar.lock().push("pre=1".into());
    let svc = service().with_cookies(cookies.clone());
    let mut r = get(&srv.url("/"));
    r.credentials_mode = axiom_net::CredentialsMode::Omit;
    fetch(&svc, r);
    assert_eq!(srv.requests()[0].header("cookie"), None);
    assert_eq!(cookies.jar.lock().as_slice(), ["pre=1".to_string()]);
}

#[test]
fn referer_follows_policy_and_user_agent_is_set() {
    let srv = TestServer::spawn(|_| TestResponse::ok("x", "text/plain"));
    let svc = service();
    let mut same = get(&srv.url("/a"));
    same.referrer = Some(srv.url("/page?q=1#frag"));
    fetch(&svc, same);
    // Loopback http is potentially trustworthy, so this is cross-origin, not a downgrade.
    let mut cross = get(&srv.url("/b"));
    cross.referrer = Some("https://secure.example/page?secret=1".into());
    fetch(&svc, cross);
    let mut none = get(&srv.url("/c"));
    none.referrer = Some(srv.url("/page"));
    none.referrer_policy = ReferrerPolicy::NoReferrer;
    fetch(&svc, none);

    assert_eq!(
        srv.requests_for("/a")[0].header("referer"),
        Some(srv.url("/page?q=1").as_str())
    );
    assert_eq!(
        srv.requests_for("/b")[0].header("referer"),
        Some("https://secure.example/")
    );
    assert_eq!(srv.requests_for("/c")[0].header("referer"), None);
    assert_eq!(
        srv.requests_for("/a")[0].header("user-agent"),
        Some(AXIOM_USER_AGENT)
    );
}

// ---------------------------------------------------------------------------
// Errors, DNS, policy, retries
// ---------------------------------------------------------------------------

#[test]
fn malformed_responses_are_errors_not_panics() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/garbage" => TestResponse::Raw(b"this is not http\r\n\r\n".to_vec()),
        "/badstatus" => TestResponse::Raw(b"HTTP/1.1 abc OK\r\n\r\n".to_vec()),
        "/truncated" => {
            TestResponse::Raw(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nshort".to_vec())
        }
        _ => TestResponse::Close,
    });
    let svc = service();
    for path in ["/garbage", "/badstatus", "/close"] {
        assert!(svc.execute_blocking(get(&srv.url(path))).is_err(), "{path}");
    }
    let resp = svc.execute_blocking(get(&srv.url("/truncated"))).unwrap();
    assert!(matches!(
        resp.body.read_all(u64::MAX),
        Err(NetworkError::Body(_))
    ));
    assert!(svc.metrics().snapshot().failed >= 4);
}

#[test]
fn post_is_not_retried_after_connection_failure() {
    let srv = TestServer::spawn(|_| TestResponse::Close);
    let svc = service();
    let mut post = NetworkRequest::new(
        HttpMethod::Post,
        url(&srv.url("/submit")),
        ResourceType::Fetch,
    );
    post.body = RequestBody::Bytes(b"order=1".to_vec());
    assert!(svc.execute_blocking(post).is_err());
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        srv.requests_for("/submit").len(),
        1,
        "POST was submitted twice"
    );
}

#[test]
fn injected_resolver_is_used_and_failures_are_typed() {
    let srv = TestServer::spawn(|_| TestResponse::ok("dns", "text/plain"));
    let dns = FixedDnsResolver::new().with("axiom.test", IpAddr::V4(Ipv4Addr::LOCALHOST));
    let svc = NetworkService::try_new(NetworkServiceConfig::default(), Arc::new(dns)).unwrap();
    let (m, b) = fetch(&svc, get(&format!("http://axiom.test:{}/x", srv.port())));
    assert_eq!((m.status, b.as_slice()), (200, &b"dns"[..]));
    assert_eq!(
        srv.requests()[0].header("host"),
        Some(format!("axiom.test:{}", srv.port()).as_str())
    );
    let err = svc
        .execute_blocking(get("http://unknown.test/"))
        .unwrap_err();
    assert!(matches!(err, NetworkError::Dns(_)), "{err:?}");
}

#[test]
fn policy_blocks_requests_and_redirect_targets() {
    let srv = TestServer::spawn(|_| TestResponse::redirect(302, "http://ads.blocked.test/x"));
    let svc = service();
    svc.add_policy(Arc::new(HostBlocklist::new(["blocked.test"])));
    let err = svc
        .execute_blocking(get("http://blocked.test/"))
        .unwrap_err();
    assert!(matches!(err, NetworkError::Blocked(_)), "{err:?}");
    let err = svc.execute_blocking(get(&srv.url("/go"))).unwrap_err();
    assert!(matches!(err, NetworkError::Blocked(_)), "{err:?}");
    assert_eq!(srv.request_count(), 1);
    assert_eq!(svc.metrics().snapshot().blocked, 2);
}

#[test]
fn request_timeout_is_enforced() {
    let srv = TestServer::spawn(|_| TestResponse::ok("late", "text/plain").delayed(2_000));
    let svc = service();
    let mut r = get(&srv.url("/slow"));
    r.request_timeout_ms = Some(200);
    let t = Instant::now();
    assert_eq!(svc.execute_blocking(r).unwrap_err(), NetworkError::Timeout);
    assert!(t.elapsed() < Duration::from_millis(1_500));
}

// ---------------------------------------------------------------------------
// Cancellation, streaming, scheduler
// ---------------------------------------------------------------------------

#[test]
fn cancel_mid_transfer_interrupts_read_and_aborts_stream() {
    let srv = TestServer::spawn(|_| TestResponse::Chunked {
        status: 200,
        headers: vec![("Content-Type".into(), "application/octet-stream".into())],
        chunks: (0..100).map(|_| vec![7u8; 16 * 1024]).collect(),
        delay_ms: 20,
    });
    let svc = service();
    let cancel = CancellationToken::new();
    let resp = svc
        .execute_with_cancel(get(&srv.url("/stream")), &cancel)
        .unwrap();
    assert!(!resp.body.read_chunk(16 * 1024).unwrap().is_empty());
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        c2.cancel();
    });
    let t = Instant::now();
    let err = loop {
        match resp.body.read_chunk(16 * 1024) {
            Ok(c) if c.is_empty() => panic!("stream finished despite cancel"),
            Ok(_) => continue,
            Err(e) => break e,
        }
    };
    assert_eq!(err, NetworkError::Cancelled);
    assert!(
        t.elapsed() < Duration::from_millis(1_000),
        "cancel latency {:?}",
        t.elapsed()
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while srv.streams_aborted() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        srv.streams_aborted(),
        1,
        "server kept streaming after cancel"
    );
    assert_eq!(srv.streams_completed(), 0);
}

fn collect_until_final(rx: &Receiver<NetworkEvent>, ids: &[NetworkRequestId]) -> Vec<NetworkEvent> {
    let mut finals = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while finals.len() < ids.len() && Instant::now() < deadline {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(50)) {
            if matches!(
                ev,
                NetworkEvent::Complete { .. } | NetworkEvent::Failed { .. }
            ) {
                finals.push(ev);
            }
        }
    }
    finals
}

#[test]
fn scheduler_bounds_concurrency() {
    let srv = TestServer::spawn(|_| {
        TestResponse::ok("x", "text/plain")
            .with_header("Cache-Control", "no-store")
            .delayed(150)
    });
    let svc = Arc::new(service());
    let sched = RequestScheduler::new(
        Arc::clone(&svc),
        SchedulerConfig {
            max_concurrent: 3,
            ..SchedulerConfig::default()
        },
    );
    let (tx, rx) = bounded(64);
    let ids: Vec<_> = (0..9)
        .map(|i| {
            let r = get(&srv.url(&format!("/c{i}")));
            let id = r.id;
            sched.enqueue(r, tx.clone()).unwrap();
            id
        })
        .collect();
    let finals = collect_until_final(&rx, &ids);
    assert_eq!(finals.len(), 9);
    assert!(finals
        .iter()
        .all(|e| matches!(e, NetworkEvent::Complete { .. })));
    assert!(
        srv.peak_concurrency() <= 3,
        "server peak {}",
        srv.peak_concurrency()
    );
    assert!(
        srv.peak_concurrency() >= 2,
        "requests did not run in parallel"
    );
    // Workers count a completion after delivering its final event.
    let deadline = Instant::now() + Duration::from_secs(5);
    while sched.stats().completed < 9 && Instant::now() < deadline {
        std::thread::yield_now();
    }
    let stats = sched.stats();
    assert!(stats.peak_in_flight <= 3);
    assert_eq!(stats.completed, 9);
}

#[test]
fn scheduler_orders_by_priority_then_ages() {
    let srv = TestServer::spawn(|req| {
        let r = TestResponse::ok("x", "text/plain").with_header("Cache-Control", "no-store");
        if req.path == "/blocker" {
            r.delayed(300)
        } else {
            r
        }
    });
    let svc = Arc::new(service());
    let one = |aging: Duration| {
        RequestScheduler::new(
            Arc::clone(&svc),
            SchedulerConfig {
                max_concurrent: 1,
                aging_interval: aging,
                ..SchedulerConfig::default()
            },
        )
    };
    let enqueue = |s: &RequestScheduler, tx, path: &str, p: RequestPriority| {
        let mut r = get(&srv.url(path));
        r.priority = p;
        let id = r.id;
        s.enqueue(r, tx).unwrap();
        id
    };

    // No meaningful aging: High requests overtake an earlier VeryLow one.
    let sched = one(Duration::from_secs(60));
    let (tx, rx) = bounded(64);
    let ids = vec![
        enqueue(&sched, tx.clone(), "/blocker", RequestPriority::VeryHigh),
        enqueue(&sched, tx.clone(), "/low", RequestPriority::VeryLow),
        enqueue(&sched, tx.clone(), "/high1", RequestPriority::High),
        enqueue(&sched, tx.clone(), "/high2", RequestPriority::High),
    ];
    assert_eq!(collect_until_final(&rx, &ids).len(), 4);
    let order: Vec<_> = srv.requests().iter().map(|r| r.path.clone()).collect();
    assert_eq!(order, ["/blocker", "/high1", "/high2", "/low"]);

    // Fast aging: a long-waiting VeryLow request is not starved by newer High ones.
    let sched = one(Duration::from_millis(50));
    let (tx, rx) = bounded(64);
    let mut ids = vec![
        enqueue(&sched, tx.clone(), "/blocker", RequestPriority::VeryHigh),
        enqueue(&sched, tx.clone(), "/old-low", RequestPriority::VeryLow),
    ];
    std::thread::sleep(Duration::from_millis(250));
    ids.push(enqueue(
        &sched,
        tx.clone(),
        "/new-high",
        RequestPriority::High,
    ));
    assert_eq!(collect_until_final(&rx, &ids).len(), 3);
    let order: Vec<_> = srv
        .requests()
        .iter()
        .skip(4)
        .map(|r| r.path.clone())
        .collect();
    assert_eq!(order, ["/blocker", "/old-low", "/new-high"]);
}

#[test]
fn cancel_context_only_cancels_that_context() {
    let srv = TestServer::spawn(|_| {
        TestResponse::ok("x", "text/plain")
            .with_header("Cache-Control", "no-store")
            .delayed(300)
    });
    let svc = Arc::new(service());
    let sched = RequestScheduler::new(Arc::clone(&svc), SchedulerConfig::default());
    let (tx_a, rx_a) = bounded(64);
    let (tx_b, rx_b) = bounded(64);
    let mut ids_a = Vec::new();
    let mut ids_b = Vec::new();
    for i in 0..3 {
        let mut r = get(&srv.url(&format!("/a{i}")));
        r.context_id = Some(1);
        ids_a.push(r.id);
        sched.enqueue(r, tx_a.clone()).unwrap();
        let mut r = get(&srv.url(&format!("/b{i}")));
        r.context_id = Some(2);
        ids_b.push(r.id);
        sched.enqueue(r, tx_b.clone()).unwrap();
    }
    std::thread::sleep(Duration::from_millis(50));
    sched.cancel_context(1);
    let a = collect_until_final(&rx_a, &ids_a);
    let b = collect_until_final(&rx_b, &ids_b);
    assert_eq!(a.len(), 3);
    assert!(a.iter().all(|e| matches!(
        e,
        NetworkEvent::Failed {
            error: NetworkError::Cancelled,
            ..
        }
    )));
    assert_eq!(b.len(), 3);
    assert!(b.iter().all(|e| matches!(e, NetworkEvent::Complete { .. })));
    assert_eq!(sched.active_for_context(1), 0);
}

#[test]
fn scheduler_streams_data_events_in_chunks() {
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let p2 = payload.clone();
    let srv = TestServer::spawn(move |_| TestResponse::ok(p2.clone(), "application/octet-stream"));
    let svc = Arc::new(service());
    let sched = RequestScheduler::new(
        Arc::clone(&svc),
        SchedulerConfig {
            chunk_size: 16 * 1024,
            ..SchedulerConfig::default()
        },
    );
    let (tx, rx) = bounded(4);
    sched.enqueue(get(&srv.url("/s")), tx).unwrap();
    let mut got = Vec::new();
    let mut data_events = 0;
    let mut saw_response = false;
    loop {
        match rx.recv_timeout(Duration::from_secs(10)).expect("event") {
            NetworkEvent::Response { meta, .. } => {
                assert!(!saw_response);
                saw_response = true;
                assert_eq!(meta.status, 200);
            }
            NetworkEvent::Data { bytes, .. } => {
                assert!(saw_response);
                assert!(bytes.len() <= 16 * 1024);
                data_events += 1;
                got.extend(bytes);
            }
            NetworkEvent::Complete { .. } => break,
            NetworkEvent::Failed { error, .. } => panic!("{error}"),
        }
    }
    assert_eq!(got, payload);
    assert!(data_events >= 200_000 / (16 * 1024));
}

#[test]
fn max_body_bytes_enforced_with_and_without_content_length() {
    let srv = TestServer::spawn(|req| {
        if req.path == "/chunked" {
            TestResponse::Chunked {
                status: 200,
                headers: vec![],
                chunks: (0..10).map(|_| vec![1u8; 64 * 1024]).collect(),
                delay_ms: 0,
            }
        } else {
            TestResponse::ok(vec![1u8; 640 * 1024], "application/octet-stream")
        }
    });
    let svc = Arc::new(service());
    let sched = RequestScheduler::new(Arc::clone(&svc), SchedulerConfig::default());
    for path in ["/chunked", "/sized"] {
        let (tx, rx) = bounded(64);
        let mut r = get(&srv.url(path));
        r.max_body_bytes = Some(100_000);
        let id = r.id;
        sched.enqueue(r, tx).unwrap();
        let finals = collect_until_final(&rx, &[id]);
        assert!(
            matches!(
                finals.as_slice(),
                [NetworkEvent::Failed {
                    error: NetworkError::TooLarge { limit: 100_000 },
                    ..
                }]
            ),
            "{path}: {finals:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Observability and lifecycle
// ---------------------------------------------------------------------------

#[test]
fn timing_reports_only_measured_phases() {
    let srv = TestServer::spawn(|_| TestResponse::ok("t", "text/plain"));
    let svc = service();
    let (m, _) = fetch(&svc, get(&srv.url("/t")));
    assert!(m.timing.ttfb_ms.is_some());
    assert_eq!(m.timing.dns_ms, None);
    assert_eq!(m.timing.connect_ms, None);
    assert_eq!(m.timing.tls_ms, None);
    assert_eq!(m.timing.request_sent_ms, None);
    let log = svc.recent_log(1);
    assert!(log[0].timing.total_ms.is_some());
    assert!(log[0].timing.download_ms.is_some());
}

#[test]
fn network_log_redacts_credentials() {
    let srv = TestServer::spawn(|_| {
        TestResponse::ok("x", "text/plain").with_header("Set-Cookie", "session=topsecret")
    });
    let cookies = Arc::new(MemCookies::default());
    cookies.jar.lock().push("sid=hunter2".into());
    let svc = service().with_cookies(cookies);
    let mut r = get(&srv.url("/l"));
    r.headers.set("Authorization", "Bearer abc");
    fetch(&svc, r);
    let e = &svc.recent_log(1)[0];
    assert_eq!(e.request_headers.get("cookie"), Some("<redacted>"));
    assert_eq!(e.request_headers.get("authorization"), Some("<redacted>"));
    assert_eq!(e.response_headers.get("set-cookie"), Some("<redacted>"));
    let dump = format!("{e:?}");
    for secret in ["hunter2", "topsecret", "Bearer abc"] {
        assert!(!dump.contains(secret), "log leaked {secret}");
    }
    // ...while the wire still carried the real values.
    assert_eq!(srv.requests()[0].header("cookie"), Some("sid=hunter2"));
}

#[test]
fn shutdown_clears_private_cache_but_keeps_persistent() {
    let srv = TestServer::spawn(|_| {
        TestResponse::ok("c", "text/plain").with_header("Cache-Control", "max-age=60")
    });
    for persistent in [false, true] {
        let svc = NetworkService::new(NetworkServiceConfig {
            persistent_cache: persistent,
            ..NetworkServiceConfig::default()
        });
        fetch(&svc, get(&srv.url(&format!("/p{persistent}"))));
        assert_eq!(svc.cache().len(), 1);
        svc.shutdown();
        assert_eq!(svc.cache().len(), usize::from(persistent));
        assert_eq!(
            svc.execute_blocking(get(&srv.url("/after"))).unwrap_err(),
            NetworkError::Cancelled
        );
    }
    assert!(srv.requests_for("/after").is_empty());
}

#[test]
fn separate_services_do_not_share_cache() {
    let srv = TestServer::spawn(|_| {
        TestResponse::ok("c", "text/plain").with_header("Cache-Control", "max-age=60")
    });
    let normal = service();
    let private = NetworkService::new(NetworkServiceConfig {
        persistent_cache: false,
        ..NetworkServiceConfig::default()
    });
    fetch(&normal, get(&srv.url("/shared")));
    let (m, _) = fetch(&private, get(&srv.url("/shared")));
    assert_eq!(m.cache_state, CacheState::Miss);
    assert_eq!(srv.request_count(), 2);
}
