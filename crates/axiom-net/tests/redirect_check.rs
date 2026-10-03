//! Wave I — the per-request redirect check (how the document's Content Security Policy
//! guards redirects): it sees every redirect target, never the original URL, and a
//! refusal stops the chain before the target is contacted.

use std::sync::{Arc, Mutex};

use axiom_net::test_server::{TestResponse, TestServer};
use axiom_net::{
    HttpMethod, NetworkError, NetworkRequest, NetworkService, NetworkServiceConfig, RedirectCheck,
    ResourceType,
};
use axiom_url::Url;

fn request(target: &str, check: RedirectCheck) -> NetworkRequest {
    let mut r = NetworkRequest::new(
        HttpMethod::Get,
        Url::parse(target).unwrap(),
        ResourceType::Fetch,
    );
    r.redirect_check = Some(check);
    r
}

#[test]
fn refused_redirect_targets_are_never_contacted() {
    let other = TestServer::spawn(|_| TestResponse::ok("x", "text/plain"));
    let target = other.url("/x");
    let srv = TestServer::spawn(move |req| match req.path.as_str() {
        "/hop" => TestResponse::redirect(302, "/next"),
        "/next" => TestResponse::redirect(302, &target),
        _ => TestResponse::ok("unexpected", "text/plain"),
    });
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let other_port = other.port();
    let check = RedirectCheck::new(move |url: &Url| {
        log.lock().unwrap().push(url.path.clone());
        if url.port == Some(other_port) {
            Err("refused".to_string())
        } else {
            Ok(())
        }
    });

    let svc = NetworkService::new(NetworkServiceConfig::default());
    let result = svc.execute_blocking(request(&srv.url("/hop"), check));
    match result {
        Err(NetworkError::Blocked(why)) => assert_eq!(why, "refused"),
        other => panic!(
            "expected a blocked redirect, got {:?}",
            other.map(|r| r.status)
        ),
    }
    assert_eq!(*seen.lock().unwrap(), ["/next", "/x"]);
    assert_eq!(other.request_count(), 0);
    assert_eq!(srv.requests_for("/next").len(), 1);
}

#[test]
fn accepted_redirects_complete_normally() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/hop" => TestResponse::redirect(302, "/done"),
        _ => TestResponse::ok("done", "text/plain"),
    });
    let svc = NetworkService::new(NetworkServiceConfig::default());
    let check = RedirectCheck::new(|_| Ok(()));
    let resp = svc
        .execute_blocking(request(&srv.url("/hop"), check))
        .unwrap();
    assert_eq!(resp.body.read_all(u64::MAX).unwrap(), b"done");
}
