//! Wave G — document loading, subresource pipeline and page lifecycle against the
//! controlled test site in `tests/fixtures/site` (served by `TestServer::spawn_site`).
//!
//! Correctness never depends on sleeps: ordering is asserted from the page's own event
//! log and the document event sequence; waits only poll until a condition holds.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axiom_browser::Browser;
use axiom_document::{BlockHostsPolicy, PolicyDirective};
use axiom_engine::{BrowsingContext, DocumentEvent, DocumentEventKind, ResourceDiagnostics};
use axiom_net::test_server::{TestResponse, TestServer};
use axiom_net::LogState;
use tempfile::{tempdir, TempDir};

const SITE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/site");
const IDLE: Duration = Duration::from_secs(15);

fn site() -> TestServer {
    TestServer::spawn_site(SITE)
}

fn browser(dir: &TempDir) -> Browser {
    Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap()
}

fn ctx(b: &mut Browser) -> &mut BrowsingContext {
    &mut b.window.tabs.active_tab_mut().context
}

fn eval_in(ctx: &mut BrowsingContext, src: &str) -> String {
    let js = ctx.page.js.as_mut().expect("page has js");
    js.eval(src)
        .map(|v| v.display)
        .unwrap_or_else(|e| format!("<{e}>"))
}

fn eval(b: &mut Browser, src: &str) -> String {
    eval_in(ctx(b), src)
}

fn log_in(ctx: &mut BrowsingContext) -> Vec<String> {
    let raw = eval_in(ctx, "JSON.stringify(log)");
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("log is not JSON ({e}): {raw}"))
}

fn log(b: &mut Browser) -> Vec<String> {
    log_in(ctx(b))
}

/// Navigate and run until the document is idle (load fired, nothing pending).
fn load(b: &mut Browser, url: &str) {
    b.navigate_resolved(url);
    assert!(ctx(b).run_until_idle(IDLE), "{url} never went idle");
}

fn resource(ctx: &BrowsingContext, suffix: &str) -> ResourceDiagnostics {
    ctx.resource_diagnostics()
        .into_iter()
        .find(|r| r.url.ends_with(suffix))
        .unwrap_or_else(|| {
            let urls: Vec<_> = ctx
                .resource_diagnostics()
                .into_iter()
                .map(|r| r.url)
                .collect();
            panic!("no resource ending with {suffix}; have {urls:?}")
        })
}

fn seq(events: &[DocumentEvent], kind: DocumentEventKind) -> u64 {
    events
        .iter()
        .find(|e| e.kind == kind)
        .unwrap_or_else(|| panic!("no {} event", kind.name()))
        .seq
}

fn resource_seq(events: &[DocumentEvent], kind: DocumentEventKind, r: &ResourceDiagnostics) -> u64 {
    events
        .iter()
        .find(|e| e.kind == kind && e.resource == Some(r.id))
        .unwrap_or_else(|| panic!("no {} event for {}", kind.name(), r.url))
        .seq
}

fn idx(log: &[String], item: &str) -> usize {
    log.iter()
        .position(|l| l == item)
        .unwrap_or_else(|| panic!("{item:?} missing from {log:?}"))
}

/// Tick until `cond` holds (polling only; the assertion is about state, not timing).
fn pump_until(ctx: &mut BrowsingContext, what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + IDLE;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        ctx.tick();
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn script_order_parser_blocking_inline_and_defer() {
    let srv = site();
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    load(&mut b, &srv.url("/order.html"));
    assert_eq!(
        log(&mut b),
        [
            "blocking-1:false",
            "blocking-2",
            "inline:true:false:loading",
            "readystatechange:interactive",
            "defer-1:interactive:true",
            "defer-2",
            "defer-3",
            "DOMContentLoaded:interactive",
            "readystatechange:complete",
            "load:complete",
        ]
    );
    let c = ctx(&mut b);
    // The defer scripts really completed in reverse order.
    let events = c.document_events();
    let registry = c.resource_registry().unwrap();
    let arrived = |suffix: &str| {
        registry
            .scripts()
            .find(|r| r.url.ends_with(suffix))
            .and_then(|r| r.response_at)
            .unwrap_or_else(|| panic!("{suffix} has no response time"))
    };
    assert!(
        arrived("/defer-3.js") < arrived("/defer-1.js"),
        "defer fixture did not invert completion order"
    );
    let d1 = resource(c, "/defer-1.js");
    assert_eq!(d1.script_kind, Some("defer"));
    assert!(
        resource_seq(&events, DocumentEventKind::ResourceReady, &d1)
            < seq(&events, DocumentEventKind::DomContentLoadedFired)
    );
    assert_eq!(
        resource(c, "/blocking-1.js").script_kind,
        Some("parser-blocking")
    );
    // DOMContentLoaded does not wait for the slow image; load does.
    let img = resource(c, "/slow.png");
    let dcl = seq(&events, DocumentEventKind::DomContentLoadedFired);
    assert!(dcl < resource_seq(&events, DocumentEventKind::ResourceReady, &img));
    assert!(
        resource_seq(&events, DocumentEventKind::ResourceReady, &img)
            < seq(&events, DocumentEventKind::LoadFired)
    );
    assert!(img.load_blocking);
}

#[test]
fn html_streams_into_the_parser_before_the_body_completes() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/stream.html" => {
            let mut chunks: Vec<Vec<u8>> = vec![
                b"<!DOCTYPE html><html><head><meta charset=\"windows-1252\">\
                <title>Streaming</title><link rel=\"stylesheet\" href=\"/leaf.css\"></head>\
                <body><p id=\"early\">early</p>"
                    .to_vec(),
            ];
            chunks.extend((0..5).map(|i| format!("<p>filler {i}</p>").into_bytes()));
            chunks.push(b"<p id=\"late\">caf\xE9</p></body></html>".to_vec());
            TestResponse::Chunked {
                status: 200,
                headers: vec![
                    ("Content-Type".into(), "text/html".into()),
                    ("Cache-Control".into(), "no-store".into()),
                ],
                chunks,
                delay_ms: 150,
            }
        }
        "/leaf.css" => TestResponse::ok("#early { color: rgb(3, 3, 3); }", "text/css"),
        _ => TestResponse::status(404, "not found"),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    let c = ctx(&mut b);
    c.start_navigation(&srv.url("/stream.html"));
    pump_until(c, "the stylesheet request", || {
        !srv.requests_for("/leaf.css").is_empty()
    });
    // The document committed and parsed its first chunk while the body is still arriving.
    assert_eq!(
        srv.streams_completed(),
        0,
        "body finished before subresource discovery"
    );
    assert!(c.document_info().unwrap().url.ends_with("/stream.html"));
    assert_eq!(c.document_diagnostics().unwrap().ready_state, "loading");
    assert_eq!(
        eval_in(c, "document.getElementById('early') !== null"),
        "true"
    );
    assert!(c.run_until_idle(IDLE));
    assert_eq!(srv.streams_completed(), 1);
    assert_eq!(
        eval_in(c, "document.getElementById('late').textContent"),
        "caf\u{e9}"
    );
    let d = c.document_diagnostics().unwrap();
    assert_eq!(d.encoding.as_deref(), Some("windows-1252 (meta)"));
    assert!(d.load_fired);
    assert_eq!(resource(c, "/leaf.css").state, "READY");
}

#[test]
fn lifecycle_event_order_is_recorded() {
    let srv = site();
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    load(&mut b, &srv.url("/order.html"));
    let c = ctx(&mut b);
    let events = c.document_events();
    let order = [
        DocumentEventKind::DocumentCreated,
        DocumentEventKind::ParsingStarted,
        DocumentEventKind::ParsingCompleted,
        DocumentEventKind::DomContentLoadedFired,
        DocumentEventKind::LoadFired,
    ];
    let seqs: Vec<u64> = order.iter().map(|k| seq(&events, *k)).collect();
    assert!(
        seqs.windows(2).all(|w| w[0] < w[1]),
        "out of order: {seqs:?}"
    );
    for kind in [
        DocumentEventKind::DomContentLoadedFired,
        DocumentEventKind::LoadFired,
    ] {
        assert_eq!(
            events.iter().filter(|e| e.kind == kind).count(),
            1,
            "{} fired twice",
            kind.name()
        );
    }
    let d = c.document_diagnostics().unwrap();
    assert!(d.dom_content_loaded_fired && d.load_fired && !d.canceled);
    assert_eq!(d.ready_state, "complete");
    assert_eq!(d.kind, "network");
    assert_eq!(d.pending_resources, 0);
    let timeline: std::collections::HashMap<_, _> = d.timeline.iter().cloned().collect();
    let dcl = timeline["dom_content_loaded"].expect("DCL measured");
    let load = timeline["load"].expect("load measured");
    assert!(dcl <= load);
    assert_eq!(c.page.shared.ready_state.get().as_str(), "complete");
}

#[test]
fn async_scripts_run_when_ready_and_do_not_block_dom_content_loaded() {
    let srv = site();
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    load(&mut b, &srv.url("/async.html"));
    let log = log(&mut b);
    assert!(idx(&log, "async-fast") < idx(&log, "async-slow"), "{log:?}");
    assert!(
        idx(&log, "DOMContentLoaded:interactive") < idx(&log, "async-slow"),
        "{log:?}"
    );
    assert!(
        idx(&log, "async-slow") < idx(&log, "load:complete"),
        "{log:?}"
    );
    let c = ctx(&mut b);
    assert_eq!(resource(c, "/async-slow.js").script_kind, Some("async"));
    assert!(resource(c, "/async-slow.js").load_blocking);
}

#[test]
fn stylesheets_imports_fonts_and_failures() {
    let srv = site();
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    load(&mut b, &srv.url("/styles.html"));
    // `@import` and `@font-face` URLs resolve against the stylesheet, not the document.
    for path in [
        "/d/300/css/parts/child.css",
        "/d/300/css/leaf.css",
        "/d/300/fonts/fixture.ttf",
    ] {
        assert_eq!(srv.requests_for(path).len(), 1, "{path}");
    }
    assert!(
        srv.requests_for("/d/300/fonts/unused.woff2").is_empty(),
        "unused font fetched"
    );
    // The listed WOFF2 is corrupt, so the TrueType source after it is fetched instead.
    assert_eq!(srv.requests_for("/d/300/fonts/fixture.woff2").len(), 1);
    assert!(srv.requests_for("/fonts/fixture.ttf").is_empty());
    let c = ctx(&mut b);
    let woff2 = resource(c, "/fixture.woff2");
    assert_eq!(woff2.state, "FAILED");
    assert!(woff2.failure.as_deref().unwrap().contains("WOFF2"));
    let css = c.page.author_css();
    let (leaf, child, main) = (
        css.find("rgb(7, 8, 9)").expect("leaf"),
        css.find("rgb(4, 5, 6)").expect("child"),
        css.find("rgb(1, 2, 3)").expect("main"),
    );
    assert!(leaf < child && child < main, "import order wrong");
    // The inline script waited for the pending stylesheet.
    let events = c.document_events();
    let main_sheet = resource(c, "/d/300/css/main.css");
    assert!(main_sheet.render_blocking);
    assert!(
        resource_seq(&events, DocumentEventKind::ResourceReady, &main_sheet)
            < seq(&events, DocumentEventKind::ScriptExecutionStarted)
    );
    let child_sheet = resource(c, "/parts/child.css");
    assert_eq!(child_sheet.initiator, "stylesheet");
    assert_eq!(child_sheet.state, "READY");
    let font = resource(c, "/fixture.ttf");
    assert_eq!(font.kind, "font");
    assert_eq!(font.state, "READY");
    // A 404 stylesheet fails in isolation.
    let missing = resource(c, "/css/missing.css");
    assert_eq!(missing.state, "FAILED");
    assert_eq!(missing.error_class, Some("stylesheet"));
    assert!(missing.failure.as_deref().unwrap().contains("http-status"));
    assert_eq!(eval_in(c, "scriptRan"), "true");
    assert!(c.document_diagnostics().unwrap().load_fired);
}

#[test]
fn base_href_resolves_css_script_and_images() {
    let srv = site();
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    load(&mut b, &srv.url("/base.html"));
    for path in [
        "/assets/deep/rel.css",
        "/assets/up.js",
        "/img/abs.png",
        "/img/absolute.png",
        "/assets/deep/sub/relative.png",
    ] {
        assert_eq!(srv.requests_for(path).len(), 1, "{path}");
    }
    assert_eq!(eval(&mut b, "upLoaded"), "true");
    assert_eq!(ctx(&mut b).page.render().images.len(), 3);
}

#[test]
fn images_track_size_failures_and_lazy_priority() {
    let srv = site();
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    load(&mut b, &srv.url("/images.html"));
    let log = log(&mut b);
    for item in ["ok:load", "missing:error", "broken:error", "inline:load"] {
        idx(&log, item);
    }
    assert!(idx(&log, "ok:load") < idx(&log, "load:complete"));
    let c = ctx(&mut b);
    let missing = resource(c, "/status/404/missing.png");
    assert_eq!(
        (missing.state, missing.error_class),
        ("FAILED", Some("image"))
    );
    assert!(missing.failure.unwrap().contains("http-status"));
    let broken = resource(c, "/img/broken.png");
    assert_eq!(broken.state, "FAILED");
    assert!(broken.failure.unwrap().contains("decode"));
    let lazy = resource(c, "/img/lazy.png");
    assert_eq!(lazy.priority, "low");
    assert!(!lazy.load_blocking);
    let registry = c.resource_registry().unwrap();
    let ok = registry
        .images()
        .find(|r| r.url.ends_with("/img/ok.png"))
        .unwrap();
    let info = ok.image.expect("intrinsic size recorded");
    assert_eq!((info.width, info.height, info.decoded), (1, 1, true));
    // ok, lazy and the data: image are painted; the failures are not.
    assert_eq!(c.page.render().images.len(), 3);
    assert_eq!(c.document_diagnostics().unwrap().failed_resources, 2);
}

#[test]
fn dynamic_insertion_goes_through_the_loader() {
    let srv = site();
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    load(&mut b, &srv.url("/dynamic.html"));
    let log = log(&mut b);
    assert!(
        idx(&log, "inserted-inline:true") < idx(&log, "after-insert"),
        "{log:?}"
    );
    let ran = log
        .iter()
        .position(|l| l.starts_with("dynamic-script:") && l != "dynamic-script:load")
        .expect("dynamic script ran");
    assert!(ran < idx(&log, "dynamic-script:load"));
    assert!(
        idx(&log, "dynamic-script:load") < idx(&log, "load:complete"),
        "{log:?}"
    );
    assert!(
        idx(&log, "dynamic-img:load") < idx(&log, "load:complete"),
        "{log:?}"
    );
    let c = ctx(&mut b);
    let script = resource(c, "/js/dynamic.js");
    assert_eq!(
        (script.script_kind, script.initiator, script.state),
        (Some("dynamic"), "script", "READY")
    );
    assert_eq!(resource(c, "/css/dynamic.css").initiator, "script");
    assert!(c.page.author_css().contains("rgb(11, 22, 33)"));
    assert_eq!(resource(c, "/img/dynamic.png").state, "READY");
    assert_eq!(c.page.render().images.len(), 1);
    for path in ["/js/dynamic.js", "/css/dynamic.css", "/img/dynamic.png"] {
        assert_eq!(srv.requests_for(path).len(), 1, "{path}");
    }
}

#[test]
fn obsolete_document_callbacks_never_touch_the_new_document() {
    let srv = site();
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    let start = Instant::now();
    ctx(&mut b).start_navigation(&srv.url("/race-a.html"));
    pump_until(ctx(&mut b), "race-a.js request", || {
        !srv.requests_for("/d/800/js/race-a.js").is_empty()
    });
    let c = ctx(&mut b);
    assert!(c.document_info().unwrap().url.ends_with("/race-a.html"));
    let a_doc = c.document_id().unwrap();
    assert!(c.pending_subresources() >= 2);

    // Navigate away while A's stylesheet, script, image and timer are all outstanding.
    b.navigate_resolved(&srv.url("/race-b.html"));
    let c = ctx(&mut b);
    assert_ne!(c.document_id(), Some(a_doc));
    // Give A's delayed responses (800 ms) and timer (300 ms) every chance to arrive.
    let until = start + Duration::from_millis(1500);
    pump_until(c, "A's delayed responses", || Instant::now() >= until);
    assert!(c.run_until_idle(IDLE));

    assert_eq!(c.page.title, "Race B");
    assert_eq!(
        eval_in(c, "document.getElementById('from-a') === null"),
        "true"
    );
    assert_eq!(eval_in(c, "document.getElementById('b') !== null"), "true");
    assert_eq!(eval_in(c, "typeof window.raceAScript"), "undefined");
    assert_eq!(eval_in(c, "typeof window.aTimer"), "undefined");
    assert!(
        !c.page.author_css().contains("rgb(200, 0, 0)"),
        "A's stylesheet applied to B"
    );
    assert_eq!(c.page.render().images.len(), 1, "A's image reached B");
    assert!(c.document_diagnostics().unwrap().load_fired);
    assert!(c
        .resource_diagnostics()
        .iter()
        .all(|r| !r.url.contains("race-a")));

    let retired = c
        .retired_documents()
        .iter()
        .find(|r| r.diagnostics.document == a_doc)
        .expect("A kept as a retired snapshot");
    assert!(retired.diagnostics.canceled);
    assert!(!retired.diagnostics.load_fired);
    for suffix in ["/race-a.css", "/race-a.js"] {
        let r = retired
            .resources
            .iter()
            .find(|r| r.url.ends_with(suffix))
            .unwrap();
        assert_eq!(r.state, "CANCELED", "{suffix}");
    }
    assert!(retired
        .resources
        .iter()
        .filter(|r| r.url.ends_with("/race-a.png"))
        .all(|r| r.state == "CANCELED"));
    assert!(retired
        .events
        .iter()
        .any(|e| e.kind == DocumentEventKind::DocumentCanceled));
    let net = b.network().recent_log(100);
    for suffix in ["/race-a.css", "/race-a.js"] {
        let e = net.iter().rev().find(|e| e.url.ends_with(suffix)).unwrap();
        assert_eq!(e.state, LogState::Cancelled, "{suffix}");
    }
}

#[test]
fn stop_cancels_pending_resources_and_finishes_the_document() {
    let srv = site();
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/order.html"));
    let c = ctx(&mut b);
    // DOMContentLoaded already fired; the 1.5 s image is still loading.
    assert!(c.document_diagnostics().unwrap().dom_content_loaded_fired);
    let img = resource(c, "/slow.png");
    if img.state != "READY" {
        c.stop_loading();
        assert_eq!(resource(c, "/slow.png").state, "CANCELED");
        assert_eq!(c.pending_subresources(), 0);
    }
    assert!(c.run_until_idle(IDLE));
    assert!(c.document_diagnostics().unwrap().load_fired);
}

#[test]
fn tabs_load_documents_concurrently_and_independently() {
    let srv = site();
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.new_tab();
    let tabs = b.window.tabs.tabs_mut();
    assert_eq!(tabs.len(), 2);
    tabs[0].context.start_navigation(&srv.url("/order.html"));
    tabs[1].context.start_navigation(&srv.url("/async.html"));
    let deadline = Instant::now() + IDLE;
    loop {
        let tabs = b.window.tabs.tabs_mut();
        let mut idle = true;
        for t in tabs.iter_mut() {
            t.context.tick();
            idle &= t.context.is_idle();
        }
        if idle {
            break;
        }
        assert!(Instant::now() < deadline, "tabs never went idle");
        std::thread::sleep(Duration::from_millis(2));
    }
    let tabs = b.window.tabs.tabs_mut();
    let order = log_in(&mut tabs[0].context);
    assert_eq!(order.last().map(String::as_str), Some("load:complete"));
    assert_eq!(
        idx(&order, "defer-1:interactive:true") + 2,
        idx(&order, "defer-3")
    );
    let async_log = log_in(&mut tabs[1].context);
    idx(&async_log, "async-slow");
    assert!(
        !async_log.iter().any(|l| l.starts_with("defer")),
        "tab logs mixed"
    );
    let (i0, i1) = (
        tabs[0].context.document_info().unwrap().clone(),
        tabs[1].context.document_info().unwrap().clone(),
    );
    assert_ne!(i0.id, i1.id);
    assert_ne!(i0.browsing_context, i1.browsing_context);
    assert_eq!(i0.tab, Some(tabs[0].id.0));
    assert_eq!(i1.tab, Some(tabs[1].id.0));
    assert!(i0.profile.is_some());
    assert_eq!(i0.profile, i1.profile);
    assert_eq!(i0.origin, i1.origin);
}

#[test]
fn subresource_requests_carry_cookies_from_the_cookie_service() {
    let srv = site();
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    load(&mut b, &srv.url("/cookie.html"));
    assert!(srv.requests_for("/cookie.html")[0]
        .header("cookie")
        .is_none());
    for path in ["/css/leaf.css", "/js/blocking-2.js", "/img/cookie.png"] {
        let req = &srv.requests_for(path)[0];
        assert_eq!(req.header("cookie"), Some("site=1"), "{path}");
        let referer = srv.url("/cookie.html");
        assert_eq!(req.header("referer"), Some(referer.as_str()), "{path}");
    }
}

#[test]
fn private_profile_documents_do_not_share_state() {
    let srv = site();
    let mut p = Browser::new_private(800, 600).unwrap();
    load(&mut p, &srv.url("/cookie.html"));
    let c = ctx(&mut p);
    assert_eq!(c.document_diagnostics().unwrap().kind, "network");
    assert!(c.document_info().unwrap().profile.is_some());
    assert_eq!(
        srv.requests_for("/css/leaf.css")[0].header("cookie"),
        Some("site=1")
    );
    drop(p);
    // A new private session starts empty: the first session's cookie is gone.
    let mut p2 = Browser::new_private(800, 600).unwrap();
    load(&mut p2, &srv.url("/cookie.html"));
    assert!(srv.requests_for("/cookie.html")[1]
        .header("cookie")
        .is_none());
}

#[test]
fn cache_cold_warm_and_reload() {
    let srv = site();
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    load(&mut b, &srv.url("/cache.html"));
    let c = ctx(&mut b);
    let first = c.document_id().unwrap();
    assert_eq!(resource(c, "/cache/fresh.css").cache, Some("miss"));
    assert_eq!(resource(c, "/etag/style.css").cache, Some("miss"));

    load(&mut b, &srv.url("/cache.html"));
    let c = ctx(&mut b);
    let second = c.document_id().unwrap();
    assert_ne!(first, second);
    assert_eq!(resource(c, "/cache/fresh.css").cache, Some("hit"));
    assert_eq!(resource(c, "/etag/style.css").cache, Some("revalidated"));
    assert_eq!(srv.requests_for("/cache/fresh.css").len(), 1);
    let etag = srv.requests_for("/etag/style.css");
    assert_eq!(etag.len(), 2);
    assert_eq!(etag[1].header("if-none-match"), Some("\"v1\""));
    assert!(c.page.author_css().contains("rgb(1, 1, 1)"));
    assert!(c.page.author_css().contains("rgb(2, 2, 2)"));

    let before = c.history.len();
    c.reload();
    assert!(c.run_until_idle(IDLE));
    assert_ne!(c.document_id(), Some(second));
    assert_eq!(c.history.len(), before, "reload added a history entry");
    let doc = srv.requests_for("/cache.html");
    assert_eq!(doc.last().unwrap().header("if-none-match"), Some("\"v1\""));
    assert!(c.document_diagnostics().unwrap().load_fired);
}

#[test]
fn script_errors_are_recorded_and_do_not_stop_the_page() {
    let srv = site();
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    load(&mut b, &srv.url("/errors.html"));
    // `document.write` is unsupported: ignored, and the page keeps running.
    assert_eq!(
        log(&mut b),
        [
            "boom-start",
            "after-boom",
            "written:false",
            "readystatechange:interactive",
            "DOMContentLoaded:interactive",
            "readystatechange:complete",
            "load:complete",
        ]
    );
    let c = ctx(&mut b);
    let boom = resource(c, "/js/boom.js");
    assert_eq!((boom.state, boom.error_class), ("FAILED", Some("script")));
    assert!(boom.failure.as_deref().unwrap().contains("execution"));
    let errors = c.script_errors();
    let external = errors
        .iter()
        .find(|e| e.resource == Some(boom.id))
        .expect("boom.js error recorded");
    assert!(external.label.contains("boom.js"), "{}", external.label);
    assert!(external.message.contains("boom"), "{}", external.message);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("notDefinedAnywhere")),
        "inline error not recorded: {errors:?}"
    );
    // The module script is fetched; its 404 fails the resource without a script error.
    let module = resource(c, "/js/module.js");
    assert_eq!(module.script_kind, Some("module"));
    assert_eq!(module.state, "FAILED");
    assert!(module.failure.as_deref().unwrap().contains("404"));
    assert_eq!(srv.requests_for("/js/module.js").len(), 1);
    assert!(c.document_diagnostics().unwrap().script_errors >= 2);
    assert!(c.document_diagnostics().unwrap().load_fired);
}

#[test]
fn content_policy_blocks_before_any_request() {
    let srv = site();
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    ctx(&mut b).set_content_policy(Arc::new(BlockHostsPolicy {
        directive: PolicyDirective::ScriptSrc,
        hosts: vec!["127.0.0.1".into()],
    }));
    load(&mut b, &srv.url("/cookie.html"));
    assert!(srv.requests_for("/js/blocking-2.js").is_empty());
    let c = ctx(&mut b);
    let script = resource(c, "/js/blocking-2.js");
    assert_eq!(
        (script.state, script.error_class),
        ("FAILED", Some("policy-rejection"))
    );
    assert_eq!(resource(c, "/css/leaf.css").state, "READY");
    assert_eq!(resource(c, "/img/cookie.png").state, "READY");
    let d = c.document_diagnostics().unwrap();
    assert_eq!(d.failed_resources, 1);
    assert!(d.load_fired);
}

#[test]
fn document_diagnostics_page_redacts_queries_and_internal_pages_load_nothing() {
    let srv = site();
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    load(&mut b, &srv.url("/order.html?token=s3cr3t"));
    b.refresh_internal_pages();
    let html = b
        .internals
        .resolve("axiom://document")
        .unwrap()
        .html
        .clone();
    assert!(html.contains("Current document"));
    assert!(html.contains("/js/blocking-1.js"));
    assert!(html.contains("DOMContentLoaded"));
    assert!(!html.contains("s3cr3t"), "query string leaked");

    let network_doc = ctx(&mut b).document_id();
    b.navigate_resolved("axiom://document");
    let c = ctx(&mut b);
    assert_eq!(c.document_diagnostics().unwrap().kind, "internal");
    assert!(c.resource_registry().unwrap().is_empty());
    assert!(c.document_diagnostics().unwrap().load_fired);
    assert!(c
        .retired_documents()
        .iter()
        .any(|r| Some(r.diagnostics.document) == network_doc));
}

#[test]
fn internal_pages_never_fetch_web_resources() {
    let srv = site();
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    let html = format!(
        "<html><head><link rel=\"stylesheet\" href=\"{0}\"><script src=\"{1}\"></script></head>\
         <body><img src=\"{2}\"><script>var s = document.createElement('img'); \
         s.setAttribute('src', '{2}?dyn'); document.body.appendChild(s);</script></body></html>",
        srv.url("/css/leaf.css"),
        srv.url("/js/blocking-2.js"),
        srv.url("/img/ok.png"),
    );
    let c = ctx(&mut b);
    c.load_local_html("axiom://test-internal", &html, true)
        .unwrap();
    assert!(c.run_until_idle(IDLE));
    assert_eq!(
        srv.request_count(),
        0,
        "an internal page reached the network"
    );
    assert_eq!(c.document_diagnostics().unwrap().kind, "internal");
    assert!(c.resource_registry().unwrap().is_empty());
}

#[test]
fn local_file_documents_load_relative_resources_from_disk() {
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    let c = ctx(&mut b);
    c.navigate(&format!("{SITE}/local.html"));
    assert!(c.run_until_idle(IDLE));
    assert_eq!(c.document_diagnostics().unwrap().kind, "file");
    assert_eq!(resource(c, "leaf.css").state, "READY");
    assert!(c.page.author_css().contains("rgb(7, 8, 9)"));
    assert!(c.document_diagnostics().unwrap().load_fired);
}
