//! Wave H — CORS in the network service: preflights, the per-profile preflight cache,
//! per-hop CORS checks on redirects and origin tainting. Each claim is verified from the
//! server side (what was actually sent). Two local servers on different ports are
//! cross-origin to each other.

use std::sync::Arc;

use axiom_net::test_server::{TestRequest, TestResponse, TestServer};
use axiom_net::{
    CookieProvider, CookieRequestContext, CredentialsMode, HttpMethod, NetworkError,
    NetworkRequest, NetworkService, NetworkServiceConfig, RequestMode, ResourceType,
};
use axiom_url::Url;

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

fn origin_of(srv: &TestServer) -> String {
    srv.url("").trim_end_matches('/').to_string()
}

/// Cookies for every request, so a leak onto a preflight or anonymous request shows up.
struct AlwaysCookie;

impl CookieProvider for AlwaysCookie {
    fn cookie_header(&self, _ctx: &CookieRequestContext<'_>) -> Option<String> {
        Some("sid=secret".into())
    }
    fn store_set_cookies(&self, _ctx: &CookieRequestContext<'_>, _set_cookies: &[String]) {}
}

fn service() -> NetworkService {
    NetworkService::new(NetworkServiceConfig::default()).with_cookies(Arc::new(AlwaysCookie))
}

/// A script-style `fetch()` request from `page` to `target`.
fn cors_request(method: HttpMethod, target: &str, page: &str) -> NetworkRequest {
    let mut r = NetworkRequest::new(method, url(target), ResourceType::Fetch);
    r.mode = RequestMode::Cors;
    r.credentials_mode = CredentialsMode::SameOrigin;
    r.origin = Some(url(page));
    r.unsafe_request = true;
    r
}

fn run(svc: &NetworkService, req: NetworkRequest) -> Result<Vec<u8>, NetworkError> {
    let resp = svc.execute_blocking(req)?;
    Ok(resp.body.read_all(u64::MAX).expect("body"))
}

fn methods(reqs: &[TestRequest]) -> Vec<&str> {
    reqs.iter().map(|r| r.method.as_str()).collect()
}

/// Grants `PUT` with `X-Custom` to the requesting origin for `max_age` seconds.
fn permissive(req: &TestRequest, max_age: &str) -> TestResponse {
    let origin = req.header("origin").unwrap_or("").to_string();
    if req.method == "OPTIONS" {
        TestResponse::status(204, "")
            .with_header("Access-Control-Allow-Origin", &origin)
            .with_header("Access-Control-Allow-Methods", "PUT")
            .with_header("Access-Control-Allow-Headers", "X-Custom")
            .with_header("Access-Control-Max-Age", max_age)
    } else {
        TestResponse::ok("done", "text/plain")
            .with_header("Access-Control-Allow-Origin", &origin)
            .with_header("Cache-Control", "no-store")
    }
}

#[test]
fn preflight_precedes_a_non_simple_request_and_is_cached_per_profile() {
    let page = TestServer::spawn(|_| TestResponse::ok("", "text/plain"));
    let api = TestServer::spawn(|req| permissive(req, "60"));
    let svc = service();
    let put = || {
        let mut r = cors_request(HttpMethod::Put, &api.url("/api"), &page.url("/"));
        r.headers.set("X-Custom", "1");
        r
    };
    assert_eq!(run(&svc, put()).unwrap(), b"done");

    let sent = api.requests_for("/api");
    assert_eq!(methods(&sent), ["OPTIONS", "PUT"]);
    let (pre, actual) = (&sent[0], &sent[1]);
    let origin = origin_of(&page);
    assert_eq!(pre.header("access-control-request-method"), Some("PUT"));
    assert_eq!(
        pre.header("access-control-request-headers"),
        Some("x-custom")
    );
    assert_eq!(pre.header("origin"), Some(origin.as_str()));
    assert!(
        pre.header("x-custom").is_none(),
        "preflight carried the header"
    );
    assert!(pre.header("cookie").is_none(), "preflight carried cookies");
    assert_eq!(actual.header("x-custom"), Some("1"));
    assert_eq!(actual.header("origin"), Some(origin.as_str()));
    assert!(
        actual.header("cookie").is_none(),
        "same-origin credentials leaked"
    );

    // The preflight is its own entry on the network log.
    let log = svc.recent_log(10);
    assert!(log
        .iter()
        .any(|e| e.method == "OPTIONS" && e.initiator == "cors-preflight"));

    // Cached (one entry for the method, one for the header): the second request goes
    // straight out.
    assert_eq!(svc.preflight_cache().len(), 2);
    run(&svc, put()).unwrap();
    assert_eq!(
        methods(&api.requests_for("/api")),
        ["OPTIONS", "PUT", "PUT"]
    );

    // Clearing the profile's cache forgets it; so does another profile never share it.
    let other_profile = service();
    run(&other_profile, put()).unwrap();
    assert_eq!(api.requests_for("/api").len(), 5);
    svc.clear_cache();
    assert!(svc.preflight_cache().is_empty());
    run(&svc, put()).unwrap();
    assert_eq!(
        methods(&api.requests_for("/api")),
        ["OPTIONS", "PUT", "PUT", "OPTIONS", "PUT", "OPTIONS", "PUT"]
    );
}

#[test]
fn max_age_zero_is_not_cached() {
    let page = TestServer::spawn(|_| TestResponse::ok("", "text/plain"));
    let api = TestServer::spawn(|req| permissive(req, "0"));
    let svc = service();
    for _ in 0..2 {
        run(
            &svc,
            cors_request(HttpMethod::Put, &api.url("/api"), &page.url("/")),
        )
        .unwrap();
    }
    assert_eq!(
        methods(&api.requests_for("/api")),
        ["OPTIONS", "PUT", "OPTIONS", "PUT"]
    );
    assert!(svc.preflight_cache().is_empty());
}

#[test]
fn a_failing_preflight_sends_no_bytes_of_the_actual_request() {
    let page = TestServer::spawn(|_| TestResponse::ok("", "text/plain"));
    let api = TestServer::spawn(|req| {
        let origin = req.header("origin").unwrap_or("").to_string();
        match req.path.as_str() {
            // No Access-Control-Allow-Origin at all.
            "/none" => TestResponse::status(204, ""),
            // Allows the method but not the header.
            "/method-only" => TestResponse::status(204, "")
                .with_header("Access-Control-Allow-Origin", &origin)
                .with_header("Access-Control-Allow-Methods", "PUT"),
            // A redirected preflight is a network error.
            "/moved" => TestResponse::redirect(307, "/none"),
            // Not a 2xx.
            _ => TestResponse::status(403, "no")
                .with_header("Access-Control-Allow-Origin", &origin)
                .with_header("Access-Control-Allow-Methods", "PUT")
                .with_header("Access-Control-Allow-Headers", "X-Custom"),
        }
    });
    let svc = service();
    for path in ["/none", "/method-only", "/moved", "/forbidden"] {
        let mut r = cors_request(HttpMethod::Put, &api.url(path), &page.url("/"));
        r.headers.set("X-Custom", "1");
        r.body = axiom_net::RequestBody::Bytes(b"payload".to_vec());
        match run(&svc, r) {
            Err(NetworkError::Cors(_)) => {}
            other => panic!("{path}: expected a CORS error, got {other:?}"),
        }
        assert_eq!(methods(&api.requests_for(path)), ["OPTIONS"], "{path}");
        assert!(api.requests_for(path)[0].body.is_empty());
    }
    assert!(methods(&api.requests()).iter().all(|m| *m == "OPTIONS"));
    assert!(svc.preflight_cache().is_empty());
}

#[test]
fn credentialed_requests_need_allow_credentials_and_an_exact_origin() {
    let page = TestServer::spawn(|_| TestResponse::ok("", "text/plain"));
    let api = TestServer::spawn(|req| {
        let origin = req.header("origin").unwrap_or("").to_string();
        let allow = match req.path.as_str() {
            "/wildcard" => "*".to_string(),
            _ => origin,
        };
        let mut resp = if req.method == "OPTIONS" {
            TestResponse::status(204, "").with_header("Access-Control-Allow-Methods", "PUT")
        } else {
            TestResponse::ok("done", "text/plain")
        };
        resp = resp.with_header("Access-Control-Allow-Origin", &allow);
        if req.path != "/no-creds" {
            resp = resp.with_header("Access-Control-Allow-Credentials", "true");
        }
        resp
    });
    let svc = service();
    let request = |path: &str| {
        let mut r = cors_request(HttpMethod::Put, &api.url(path), &page.url("/"));
        r.credentials_mode = CredentialsMode::Include;
        r
    };
    assert!(matches!(
        run(&svc, request("/no-creds")),
        Err(NetworkError::Cors(_))
    ));
    assert!(matches!(
        run(&svc, request("/wildcard")),
        Err(NetworkError::Cors(_))
    ));
    assert_eq!(methods(&api.requests_for("/no-creds")), ["OPTIONS"]);
    assert_eq!(methods(&api.requests_for("/wildcard")), ["OPTIONS"]);

    assert_eq!(run(&svc, request("/ok")).unwrap(), b"done");
    let sent = api.requests_for("/ok");
    assert_eq!(methods(&sent), ["OPTIONS", "PUT"]);
    assert!(
        sent[0].header("cookie").is_none(),
        "preflight carried cookies"
    );
    assert_eq!(sent[1].header("cookie"), Some("sid=secret"));
}

#[test]
fn simple_and_same_origin_requests_are_not_preflighted() {
    let api = TestServer::spawn(|req| {
        let mut resp = TestResponse::ok("x", "text/plain").with_header("Cache-Control", "no-store");
        if req.path == "/allowed" {
            let origin = req.header("origin").unwrap_or("").to_string();
            resp = resp.with_header("Access-Control-Allow-Origin", &origin);
        }
        resp
    });
    let page = TestServer::spawn(|_| TestResponse::ok("", "text/plain"));
    let svc = service();

    // A simple cross-origin GET goes out once, and fails its CORS check without ACAO.
    let mut simple = cors_request(HttpMethod::Get, &api.url("/denied"), &page.url("/"));
    simple.headers.set("Content-Type", "text/plain");
    assert!(matches!(run(&svc, simple), Err(NetworkError::Cors(_))));
    assert_eq!(methods(&api.requests_for("/denied")), ["GET"]);
    let ok = cors_request(HttpMethod::Post, &api.url("/allowed"), &page.url("/"));
    assert_eq!(run(&svc, ok).unwrap(), b"x");
    assert_eq!(methods(&api.requests_for("/allowed")), ["POST"]);

    // Same-origin: no preflight, no CORS check, cookies allowed.
    let mut same = cors_request(HttpMethod::Put, &api.url("/same"), &api.url("/"));
    same.headers.set("X-Custom", "1");
    run(&svc, same).unwrap();
    let sent = api.requests_for("/same");
    assert_eq!(methods(&sent), ["PUT"]);
    assert_eq!(sent[0].header("cookie"), Some("sid=secret"));
}

#[test]
fn the_use_cors_preflight_flag_forces_a_preflight_for_a_simple_request() {
    let page = TestServer::spawn(|_| TestResponse::ok("", "text/plain"));
    let api = TestServer::spawn(|req| {
        let origin = req.header("origin").unwrap_or("").to_string();
        let resp = if req.method == "OPTIONS" {
            TestResponse::status(204, "")
        } else {
            TestResponse::ok("ok", "text/plain")
        };
        resp.with_header("Access-Control-Allow-Origin", &origin)
    });
    let mut r = cors_request(HttpMethod::Post, &api.url("/stream"), &page.url("/"));
    r.use_cors_preflight = true;
    r.headers.set("Content-Type", "text/plain");
    assert_eq!(run(&service(), r).unwrap(), b"ok");
    let sent = api.requests_for("/stream");
    assert_eq!(methods(&sent), ["OPTIONS", "POST"]);
    assert_eq!(
        sent[0].header("access-control-request-method"),
        Some("POST")
    );
    assert!(sent[0].header("access-control-request-headers").is_none());
}

#[test]
fn cross_origin_redirects_are_checked_per_hop_and_taint_the_origin() {
    let page = TestServer::spawn(|_| TestResponse::ok("", "text/plain"));
    let page_origin = origin_of(&page);
    let end = TestServer::spawn(|req| {
        let allow = match req.path.as_str() {
            // Allows only the page's real origin, which is no longer the one sent.
            "/strict" => "http://unused.invalid",
            _ => "null",
        };
        TestResponse::ok("landed", "text/plain")
            .with_header("Access-Control-Allow-Origin", allow)
            .with_header("Cache-Control", "no-store")
    });
    let (to_ok, to_strict) = (end.url("/ok"), end.url("/strict"));
    let allowed = page_origin.clone();
    let hop = TestServer::spawn(move |req| match req.path.as_str() {
        "/no-acao" => TestResponse::redirect(302, &to_ok),
        "/strict" => TestResponse::redirect(302, &to_strict)
            .with_header("Access-Control-Allow-Origin", &allowed),
        _ => {
            TestResponse::redirect(302, &to_ok).with_header("Access-Control-Allow-Origin", &allowed)
        }
    });
    let svc = service();
    let get = |path: &str| cors_request(HttpMethod::Get, &hop.url(path), &page.url("/"));

    // Followed: the redirect passed its CORS check; the next hop sees `Origin: null`.
    assert_eq!(run(&svc, get("/start")).unwrap(), b"landed");
    let first = &hop.requests_for("/start")[0];
    assert_eq!(first.header("origin"), Some(page_origin.as_str()));
    let landed = &end.requests_for("/ok")[0];
    assert_eq!(landed.header("origin"), Some("null"));
    assert!(landed.header("cookie").is_none());

    // A redirect without Access-Control-Allow-Origin fails before the next hop.
    let before = end.requests_for("/ok").len();
    assert!(matches!(
        run(&svc, get("/no-acao")),
        Err(NetworkError::Cors(_))
    ));
    assert_eq!(end.requests_for("/ok").len(), before);

    // After tainting, the final response must allow `null`, not the page's origin.
    assert!(matches!(
        run(&svc, get("/strict")),
        Err(NetworkError::Cors(_))
    ));
    assert_eq!(end.requests_for("/strict").len(), 1);
}

#[test]
fn same_origin_mode_still_refuses_cross_origin_redirects() {
    let other = TestServer::spawn(|_| TestResponse::ok("x", "text/plain"));
    let target = other.url("/x");
    let srv = TestServer::spawn(move |_| TestResponse::redirect(302, &target));
    let mut r = cors_request(HttpMethod::Get, &srv.url("/r"), &srv.url("/"));
    r.mode = RequestMode::SameOrigin;
    assert!(matches!(run(&service(), r), Err(NetworkError::Blocked(_))));
    assert_eq!(other.request_count(), 0);
}
