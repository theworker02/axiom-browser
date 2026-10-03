//! Event-driven waiting and selective cancellation of the resource loader.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axiom_loader::{LoaderEvent, ResourceLoader, ResourceRequest};
use axiom_net::test_server::{TestResponse, TestServer};
use axiom_trace::TraceTimeline;
use axiom_url::Url;

fn server() -> TestServer {
    TestServer::spawn(|req| match req.path.as_str() {
        "/fast" => TestResponse::ok("fast", "text/plain"),
        "/slow" => TestResponse::ok("slow", "text/plain").delayed(400),
        _ => TestResponse::status(404, "missing"),
    })
}

#[test]
fn wait_next_wakes_on_the_first_event() {
    let srv = server();
    let loader = ResourceLoader::new(Arc::new(TraceTimeline::default()));
    let fast = loader.start(ResourceRequest::new(
        Url::parse(&srv.url("/fast")).unwrap(),
        axiom_loader::ResourceType::Other,
    ));
    let started = Instant::now();
    let mut done = None;
    while done.is_none() && started.elapsed() < Duration::from_secs(10) {
        for ev in loader.wait_next(Duration::from_secs(10)) {
            if let LoaderEvent::Completed(r) = ev {
                done = Some(r.id);
            }
        }
    }
    assert_eq!(done, Some(fast));
    assert!(!loader.is_pending(fast));
    // Nothing outstanding: returns empty after the timeout instead of blocking forever.
    assert!(loader.wait_next(Duration::from_millis(20)).is_empty());
}

#[test]
fn cancel_all_except_keeps_only_the_listed_requests() {
    let srv = server();
    let loader = ResourceLoader::new(Arc::new(TraceTimeline::default()));
    let url = |p: &str| Url::parse(&srv.url(p)).unwrap();
    let old_a = loader.start(ResourceRequest::new(
        url("/slow"),
        axiom_loader::ResourceType::Image,
    ));
    let old_b = loader.start(ResourceRequest::new(
        url("/slow"),
        axiom_loader::ResourceType::Script,
    ));
    let keep = loader.start(ResourceRequest::new(
        url("/fast"),
        axiom_loader::ResourceType::Document,
    ));
    let cancelled = loader.cancel_all_except(&[keep]);
    assert_eq!(cancelled.len(), 2);
    assert!(cancelled.contains(&old_a) && cancelled.contains(&old_b));
    assert!(loader.is_pending(keep));
    assert!(!loader.is_pending(old_a));
    let events = loader.wait_for(&[keep], Duration::from_secs(10));
    assert!(matches!(events.as_slice(), [LoaderEvent::Completed(r)] if r.id == keep));
    // Late events of the cancelled requests never surface.
    let late = loader.wait_next(Duration::from_millis(600));
    assert!(
        late.iter().all(|e| e.id() != old_a && e.id() != old_b),
        "{late:?}"
    );
}
