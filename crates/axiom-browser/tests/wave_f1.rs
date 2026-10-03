//! Wave F.1 — browser-level networking: one network service, cache and scheduler per
//! profile, shared by its tabs; subresources through the loader; tab-scoped cancellation.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axiom_browser::Browser;
use axiom_net::test_server::{TestRequest, TestResponse, TestServer};
use axiom_net::LogState;
use tempfile::tempdir;

const FIXTURE: &str = include_str!("../../../tests/fixtures/network/subresources.html");
const PNG_1X1: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

fn png() -> Vec<u8> {
    axiom_loader::parse_data_url(PNG_1X1)
        .expect("png data url")
        .1
}

fn normal_browser(dir: &tempfile::TempDir) -> Browser {
    Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap()
}

fn html(body: &str) -> TestResponse {
    TestResponse::ok(body.to_string(), "text/html").with_header("Cache-Control", "no-store")
}

fn delay_of(req: &TestRequest, prefix: &str, default_ms: u64) -> u64 {
    req.path
        .strip_prefix(prefix)
        .and_then(|rest| rest.split('.').next())
        .and_then(|n| n.parse::<u64>().ok())
        .map(|n| (5 - n.min(5)) * 60)
        .unwrap_or(default_ms)
}

#[test]
fn tabs_share_the_profile_scheduler_and_cache() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/style.css" => TestResponse::ok("p { color: rgb(1, 2, 3); }", "text/css")
            .with_header("Cache-Control", "max-age=600"),
        _ => html(
            r#"<html><head><link rel="stylesheet" href="/style.css"></head><body><p>x</p></body></html>"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut browser = normal_browser(&dir);
    browser.navigate_resolved(&srv.url("/page"));
    browser.new_tab();
    browser.navigate_resolved(&srv.url("/page"));

    for tab in browser.window.tabs.tabs() {
        assert!(
            Arc::ptr_eq(tab.context.loader.scheduler(), browser.scheduler()),
            "tab is not on the profile scheduler"
        );
        assert!(tab.context.page.author_css().contains("rgb(1, 2, 3)"));
    }
    assert_eq!(srv.requests_for("/page").len(), 2);
    assert_eq!(
        srv.requests_for("/style.css").len(),
        1,
        "second tab did not reuse the profile cache"
    );
    assert!(browser.network().cache().stats().hits >= 1);
}

#[test]
fn private_profile_cache_is_separate_and_cleared_on_close() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/style.css" => TestResponse::ok("p { color: red; }", "text/css")
            .with_header("Cache-Control", "max-age=600"),
        _ => html(
            r#"<html><head><link rel="stylesheet" href="/style.css"></head><body></body></html>"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut normal = normal_browser(&dir);
    let mut private = Browser::new_private(800, 600).unwrap();
    normal.navigate_resolved(&srv.url("/page"));
    private.navigate_resolved(&srv.url("/page"));
    assert_eq!(
        srv.requests_for("/style.css").len(),
        2,
        "private reused the normal cache"
    );
    assert!(!Arc::ptr_eq(normal.network(), private.network()));
    assert!(normal.network().cache().is_persistent());
    assert!(!private.network().cache().is_persistent());

    let private_net = Arc::clone(private.network());
    drop(private);
    assert!(private_net.is_shut_down());
    assert!(
        private_net.cache().is_empty(),
        "private cache survived close"
    );

    let normal_net = Arc::clone(normal.network());
    drop(normal);
    assert!(!normal_net.cache().is_empty());
}

#[test]
fn closing_a_tab_cancels_only_its_requests() {
    let png = png();
    let srv = TestServer::spawn(move |req| {
        if req.path.starts_with("/slow") {
            TestResponse::ok(png.clone(), "image/png").delayed(3_000)
        } else if req.path == "/quick.png" {
            TestResponse::ok(png.clone(), "image/png").delayed(300)
        } else if req.path == "/a" {
            html(
                r#"<html><body><img src="/slow0.png"><img src="/slow1.png"><img src="/slow2.png"></body></html>"#,
            )
        } else {
            html(r#"<html><body><img src="/quick.png"></body></html>"#)
        }
    });
    let dir = tempdir().unwrap();
    let mut browser = normal_browser(&dir);
    browser.navigate_resolved(&srv.url("/a"));
    let ctx_a = browser.window.tabs.active_tab().context.loader.context_id();
    assert_eq!(
        browser
            .window
            .tabs
            .active_tab()
            .context
            .pending_subresources(),
        3
    );

    browser.new_tab();
    browser.navigate_resolved(&srv.url("/b"));
    browser.select_tab(0);
    let t = Instant::now();
    browser.close_tab();

    // Tab A's in-flight requests are interrupted promptly…
    while browser.scheduler().active_for_context(ctx_a) > 0 {
        assert!(
            t.elapsed() < Duration::from_secs(1),
            "tab A requests still running"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let cancelled = browser
        .network()
        .recent_log(50)
        .into_iter()
        .filter(|e| e.url.contains("/slow") && e.state == LogState::Cancelled)
        .count();
    assert_eq!(cancelled, 3);

    // …while tab B's image still completes and is decoded.
    let tab_b = browser.window.tabs.active_tab_mut();
    assert!(tab_b.context.wait_for_subresources(Duration::from_secs(5)));
    assert_eq!(tab_b.context.page.render().images.len(), 1);
}

#[test]
fn switching_tabs_keeps_background_loads_alive() {
    let png = png();
    let srv = TestServer::spawn(move |req| {
        if req.path == "/slow.png" {
            TestResponse::ok(png.clone(), "image/png").delayed(300)
        } else {
            html(r#"<html><body><img src="/slow.png"></body></html>"#)
        }
    });
    let dir = tempdir().unwrap();
    let mut browser = normal_browser(&dir);
    browser.navigate_resolved(&srv.url("/a"));
    assert_eq!(
        browser
            .window
            .tabs
            .active_tab()
            .context
            .pending_subresources(),
        1
    );

    browser.new_tab();
    browser.select_tab(0);
    browser.select_tab(1);
    browser.select_tab(0);

    let tab_a = browser.window.tabs.active_tab_mut();
    assert!(tab_a.context.wait_for_subresources(Duration::from_secs(5)));
    assert_eq!(tab_a.context.page.render().images.len(), 1);
    assert_eq!(srv.requests_for("/slow.png").len(), 1);
}

#[test]
fn fixture_subresources_load_concurrently_through_the_loader() {
    let png = png();
    let srv = TestServer::spawn(move |req| {
        let p = req.path.as_str();
        if p == "/" {
            html(FIXTURE)
        } else if let Some(rest) = p.strip_prefix("/css/") {
            let n = rest.trim_end_matches(".css");
            TestResponse::ok(format!("#out {{ margin-left: {n}px; }}"), "text/css").delayed(150)
        } else if p.starts_with("/js/") {
            // Later scripts arrive first; execution must still follow document order.
            let n = p.trim_start_matches("/js/").trim_end_matches(".js");
            TestResponse::ok(
                format!("var order = order || []; order.push({n});"),
                "text/javascript",
            )
            .delayed(delay_of(req, "/js/", 0))
        } else if p.starts_with("/img/") {
            TestResponse::ok(png.clone(), "image/png").delayed(700)
        } else {
            TestResponse::status(404, "")
        }
    });
    let dir = tempdir().unwrap();
    let mut browser = normal_browser(&dir);

    let t = Instant::now();
    browser.navigate_resolved(&srv.url("/"));
    let nav = t.elapsed();

    let tab = browser.window.tabs.active_tab_mut();
    assert!(tab.context.last_error.is_none());
    let out = {
        let doc = tab.context.page.document.borrow();
        let node = doc.query_selector("#out").unwrap();
        doc.text_content(node)
    };
    assert_eq!(out.trim(), "0,1,2,3,4", "blocking scripts ran out of order");
    let css = tab.context.page.author_css();
    let (i0, i1, i2) = (
        css.find("margin-left: 0px").unwrap(),
        css.find("margin-left: 1px").unwrap(),
        css.find("margin-left: 2px").unwrap(),
    );
    assert!(i0 < i1 && i1 < i2, "stylesheets not in document order");
    assert!(
        css.find("rgb(9, 9, 9)").unwrap() > i2,
        "inline style before earlier links"
    );
    assert!(srv.requests_for("/css/alternate.css").is_empty());

    // Images do not block the initial document commit; a wall-clock threshold is
    // intentionally avoided because Windows CI can schedule the main thread late.
    // Pending images plus a committed framebuffer prove the lifecycle boundary
    // without making the concurrency contract depend on runner speed.
    let _initial_commit_elapsed = nav;
    assert!(tab.context.pending_subresources() > 0);
    assert!(
        tab.context.page.framebuffer.is_some(),
        "no first paint before images"
    );
    assert!(tab.context.wait_for_subresources(Duration::from_secs(10)));
    assert_eq!(tab.context.page.render().images.len(), 10);

    let limit = browser.scheduler().config().max_concurrent;
    assert!(
        srv.peak_concurrency() > 1,
        "subresources were fetched serially"
    );
    assert!(srv.peak_concurrency() <= limit);
    assert!(browser.scheduler().stats().peak_in_flight <= limit);

    // Subresource requests carry the document as referrer and a type-specific Accept.
    let css0 = &srv.requests_for("/css/0.css")[0];
    assert_eq!(css0.header("referer"), Some(srv.url("/").as_str()));
    assert!(css0.header("accept").unwrap().contains("text/css"));
}

#[test]
fn stylesheet_with_wrong_mime_is_ignored() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/evil.css" => TestResponse::ok("p { color: rgb(6, 6, 6); }", "text/html"),
        _ => html(
            r#"<html><head><link rel="stylesheet" href="/evil.css"></head><body></body></html>"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut browser = normal_browser(&dir);
    browser.navigate_resolved(&srv.url("/"));
    let tab = browser.window.tabs.active_tab();
    assert!(!tab.context.page.author_css().contains("rgb(6, 6, 6)"));
}

#[test]
fn navigation_error_shows_trusted_page_without_server_bytes() {
    let srv = TestServer::spawn(|_| {
        TestResponse::Raw(
            b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\n<script>evil()</script>".to_vec(),
        )
    });
    let dir = tempdir().unwrap();
    let mut browser = normal_browser(&dir);
    browser.navigate_resolved(&srv.url("/"));
    let tab = browser.window.tabs.active_tab();
    assert!(tab.context.last_error.is_some());
    assert_eq!(tab.context.page.url, "axiom://network-error");
    let doc = tab.context.page.document.borrow();
    let body = doc.body().map(|b| doc.text_content(b)).unwrap_or_default();
    assert!(
        !body.contains("evil()"),
        "raw server bytes reached the error page"
    );
}

#[test]
fn network_page_uses_profile_data_and_hides_secrets() {
    let srv = TestServer::spawn(|_| {
        html("<html><body>hi</body></html>").with_header("Set-Cookie", "sid=supersecret; Path=/")
    });
    let dir = tempdir().unwrap();
    let mut browser = normal_browser(&dir);
    browser.navigate_resolved(&format!("{}?token=abc123", srv.url("/page")));
    browser.navigate_resolved(&srv.url("/page"));
    assert_eq!(srv.requests()[1].header("cookie"), Some("sid=supersecret"));
    browser.refresh_internal_pages();
    let page = browser
        .internals
        .resolve("axiom://network")
        .expect("network page")
        .html
        .clone();
    assert!(page.contains("/page"));
    assert!(!page.contains("supersecret"));
    assert!(
        !page.contains("abc123"),
        "query string shown on axiom://network"
    );
    assert!(page.contains("persistent=true"));
    assert!(
        page.contains("n/a"),
        "unmeasured values must be shown as n/a"
    );
}
