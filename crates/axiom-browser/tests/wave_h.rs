//! Wave H — CORS end to end: `fetch()` preflights and cross-origin redirects, and
//! subresource CORS (`crossorigin` on `script`/`img`/`link`, module scripts, fonts).
//! Two local servers on different ports are cross-origin to each other but share the
//! cookie jar (cookies ignore ports), so credential leaks show up on the wire.

use std::time::{Duration, Instant};

use axiom_browser::Browser;
use axiom_engine::ResourceDiagnostics;
use axiom_net::test_server::{TestRequest, TestResponse, TestServer};
use tempfile::{tempdir, TempDir};

fn browser(dir: &TempDir) -> Browser {
    Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap()
}

/// A page (setting a cookie) whose scripts record into `log` and set `done` on `load`.
fn page(head: &str, body: &str) -> TestResponse {
    let html = format!(
        "<!doctype html><html><head><script>var log = []; var done = false;\n\
         function fail(e) {{ log.push('error:' + (e && e.name) + ':' + (e && e.message)); done = true; }}\n\
         </script>{head}</head><body>{body}</body></html>"
    );
    TestResponse::ok(html, "text/html")
        .with_header("Cache-Control", "no-store")
        .with_header("Set-Cookie", "sid=s1; Path=/")
}

fn eval(browser: &mut Browser, src: &str) -> String {
    let tab = browser.window.tabs.active_tab_mut();
    let js = tab.context.page.js.as_mut().expect("page has js");
    js.eval(src)
        .map(|v| v.display)
        .unwrap_or_else(|e| format!("<{e}>"))
}

fn run(browser: &mut Browser) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        browser.window.tabs.tick_active();
        if eval(browser, "done") == "true" {
            return eval(browser, "JSON.stringify(log)");
        }
        assert!(
            Instant::now() < deadline,
            "timed out; log = {}",
            eval(browser, "JSON.stringify(log)")
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn origin_of(srv: &TestServer) -> String {
    srv.url("").trim_end_matches('/').to_string()
}

fn only(srv: &TestServer, path: &str) -> TestRequest {
    let reqs = srv.requests_for(path);
    assert_eq!(reqs.len(), 1, "{path} requested {} times", reqs.len());
    reqs.into_iter().next().unwrap()
}

fn resource(b: &mut Browser, suffix: &str) -> ResourceDiagnostics {
    let ctx = &b.window.tabs.active_tab_mut().context;
    ctx.resource_diagnostics()
        .into_iter()
        .find(|r| r.url.ends_with(suffix))
        .unwrap_or_else(|| panic!("no resource ending with {suffix}"))
}

fn js(body: &str) -> TestResponse {
    TestResponse::ok(body, "text/javascript").with_header("Cache-Control", "no-store")
}

/// Found by live_smoke on Google results: recursion through native callbacks runs on the
/// Rust stack. It must stop at the engine's recursion limit well within a quarter of the
/// 8 MiB main-thread stack the binaries are linked with (see `build.rs`).
#[test]
fn runaway_recursion_through_native_callbacks_stops_at_the_limit() {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let srv = TestServer::spawn(|_| {
                TestResponse::ok(
                    "<script>var depth = 0;\n\
                     function f() { depth++; [1].forEach(f); }\n\
                     f();</script>",
                    "text/html",
                )
            });
            let dir = tempdir().unwrap();
            let mut b = browser(&dir);
            b.navigate_resolved(&srv.url("/"));
            let depth: u32 = eval(&mut b, "depth").parse().unwrap();
            assert!((100..=512).contains(&depth), "depth {depth}");
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn fetch_preflights_non_simple_requests_on_the_profile_network_service() {
    let api = TestServer::spawn(|req| {
        let origin = req.header("origin").unwrap_or("").to_string();
        if req.method == "OPTIONS" {
            TestResponse::status(204, "")
                .with_header("Access-Control-Allow-Origin", &origin)
                .with_header("Access-Control-Allow-Methods", "PUT, DELETE")
                .with_header("Access-Control-Allow-Headers", "x-custom, content-type")
                .with_header("Access-Control-Max-Age", "600")
        } else {
            TestResponse::ok("stored", "text/plain")
                .with_header("Access-Control-Allow-Origin", &origin)
        }
    });
    let target = api.url("/item");
    let script = format!(
        r#"<script>
             var opts = {{ method: 'PUT', body: 'data',
                          headers: {{ 'X-Custom': '1', 'Content-Type': 'application/json' }} }};
             fetch('{target}', opts).then(function (r) {{
               log.push(r.type, r.status);
               return r.text();
             }}).then(function (t) {{
               log.push(t);
               return fetch('{target}', opts);
             }}).then(function (r) {{ log.push(r.status); done = true; }}, fail);
           </script>"#
    );
    let srv = TestServer::spawn(move |_| page(&script, ""));
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(run(&mut b), r#"["cors",200,"stored",200]"#);

    // One preflight (the second request hit the preflight cache), then the requests.
    let sent = api.requests_for("/item");
    let methods: Vec<&str> = sent.iter().map(|r| r.method.as_str()).collect();
    assert_eq!(methods, ["OPTIONS", "PUT", "PUT"]);
    let origin = origin_of(&srv);
    assert_eq!(sent[0].header("origin"), Some(origin.as_str()));
    assert_eq!(sent[0].header("access-control-request-method"), Some("PUT"));
    assert_eq!(
        sent[0].header("access-control-request-headers"),
        Some("content-type,x-custom")
    );
    assert!(sent[0].body.is_empty());
    assert_eq!(sent[1].body, b"data");
    assert!(sent.iter().all(|r| r.header("cookie").is_none()));

    // The preflight is visible on the profile's network log, tied to the tab.
    let log = b.network().recent_log(50);
    let pre = log
        .iter()
        .find(|e| e.method == "OPTIONS")
        .expect("preflight logged");
    assert_eq!(pre.initiator, "cors-preflight");
    assert!(pre.context_id.is_some());

    // Clearing the profile's cache forgets the preflight.
    assert!(!b.network().preflight_cache().is_empty());
    b.clear_cache();
    assert!(b.network().preflight_cache().is_empty());
}

#[test]
fn fetch_follows_cross_origin_redirects_with_a_tainted_origin() {
    let end = TestServer::spawn(|req| {
        TestResponse::ok("landed", "text/plain").with_header(
            "Access-Control-Allow-Origin",
            req.header("origin").unwrap_or(""),
        )
    });
    let landing = end.url("/end");
    let hop = TestServer::spawn(move |req| {
        TestResponse::redirect(302, &landing).with_header(
            "Access-Control-Allow-Origin",
            req.header("origin").unwrap_or(""),
        )
    });
    let target = hop.url("/go");
    let script = format!(
        r#"<script>
             fetch('{target}').then(function (r) {{
               log.push(r.type, r.redirected, r.url.endsWith('/end'));
               return r.text();
             }}).then(function (t) {{ log.push(t); done = true; }}, fail);
           </script>"#
    );
    let srv = TestServer::spawn(move |_| page(&script, ""));
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(run(&mut b), r#"["cors",true,true,"landed"]"#);
    let origin = origin_of(&srv);
    assert_eq!(only(&hop, "/go").header("origin"), Some(origin.as_str()));
    // Cross-origin to cross-origin: the origin is tainted and serializes to `null`.
    assert_eq!(only(&end, "/end").header("origin"), Some("null"));
}

#[test]
fn crossorigin_scripts_and_module_scripts_need_cors_approval() {
    let other = TestServer::spawn(|req| {
        let body = format!("log.push('{}:ran');", req.path.trim_start_matches('/'));
        let resp = js(&body);
        if req.path.starts_with("/open") {
            resp.with_header("Access-Control-Allow-Origin", "*")
        } else {
            resp
        }
    });
    let o = origin_of(&other);
    let head = format!(
        r#"<script crossorigin src="{o}/open.js" onerror="log.push('open:error')"></script>
           <script crossorigin="anonymous" src="{o}/closed.js" onerror="log.push('closed:error')"></script>
           <script src="{o}/plain.js"></script>
           <script type="module" src="{o}/open-mod.js"></script>
           <script type="module" src="{o}/closed-mod.js" onerror="log.push('closed-mod:error')"></script>
           <script>window.addEventListener('load', function () {{ done = true; }});</script>"#
    );
    let srv = TestServer::spawn(move |_| page(&head, ""));
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    let log: Vec<String> = serde_json::from_str(&run(&mut b)).unwrap();
    for want in [
        "open.js:ran",
        "closed:error",
        "plain.js:ran",
        "open-mod.js:ran",
        "closed-mod:error",
    ] {
        assert!(log.iter().any(|e| e == want), "missing {want}: {log:?}");
    }
    for never in ["open:error", "closed.js:ran", "closed-mod.js:ran"] {
        assert!(
            !log.iter().any(|e| e == never),
            "unexpected {never}: {log:?}"
        );
    }

    let origin = origin_of(&srv);
    // `crossorigin` and module scripts: cors mode, credentials only same-origin.
    for path in ["/open.js", "/closed.js", "/open-mod.js", "/closed-mod.js"] {
        let r = only(&other, path);
        assert_eq!(r.header("origin"), Some(origin.as_str()), "{path}");
        assert!(r.header("cookie").is_none(), "{path} carried cookies");
    }
    // A plain classic script stays no-cors: no Origin, cookies included.
    let plain = only(&other, "/plain.js");
    assert!(plain.header("origin").is_none());
    assert_eq!(plain.header("cookie"), Some("sid=s1"));
    let closed = resource(&mut b, "/closed.js");
    assert_eq!(closed.state, "FAILED");
    assert!(
        closed.failure.as_deref().unwrap_or("").contains("cors:"),
        "{:?}",
        closed.failure
    );
}

#[test]
fn crossorigin_images_stylesheets_and_fonts() {
    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="red"/></svg>"#;
    let other = TestServer::spawn(|req| {
        let origin = req.header("origin").unwrap_or("").to_string();
        let resp = match req.path.rsplit('.').next() {
            Some("svg") => TestResponse::ok(SVG, "image/svg+xml"),
            Some("css") => TestResponse::ok("p { color: blue; }", "text/css"),
            _ => TestResponse::ok("not a font", "font/ttf"),
        };
        let resp = resp.with_header("Cache-Control", "no-store");
        match req.path.as_str() {
            "/creds.svg" => resp
                .with_header("Access-Control-Allow-Origin", &origin)
                .with_header("Access-Control-Allow-Credentials", "true"),
            p if p.starts_with("/open") => resp.with_header("Access-Control-Allow-Origin", "*"),
            _ => resp,
        }
    });
    let o = origin_of(&other);
    let head = format!(
        r#"<link rel="stylesheet" crossorigin href="{o}/open.css">
           <link rel="stylesheet" crossorigin href="{o}/closed.css">
           <link rel="stylesheet" href="{o}/plain.css">
           <style>@font-face {{ font-family: Remote; src: url({o}/closed-font.ttf); }}
                  p {{ font-family: Remote, sans-serif; }}</style>
           <script>window.addEventListener('load', function () {{ done = true; }});</script>"#
    );
    let body = format!(
        r#"<p>text</p>
           <img crossorigin src="{o}/open.svg">
           <img crossorigin="anonymous" src="{o}/closed.svg">
           <img src="{o}/plain.svg">
           <img crossorigin="use-credentials" src="{o}/creds.svg">"#
    );
    let srv = TestServer::spawn(move |_| page(&head, &body));
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    run(&mut b);

    for (suffix, state) in [
        ("/open.svg", "READY"),
        ("/closed.svg", "FAILED"),
        ("/plain.svg", "READY"),
        ("/creds.svg", "READY"),
        ("/open.css", "READY"),
        ("/closed.css", "FAILED"),
        ("/plain.css", "READY"),
        ("/closed-font.ttf", "FAILED"),
    ] {
        let r = resource(&mut b, suffix);
        assert_eq!(r.state, state, "{suffix}: {:?}", r.failure);
        if state == "FAILED" {
            let why = r.failure.unwrap_or_default();
            assert!(why.contains("cors:"), "{suffix}: {why}");
        }
    }

    let origin = origin_of(&srv);
    for path in [
        "/open.svg",
        "/closed.svg",
        "/open.css",
        "/closed.css",
        "/closed-font.ttf",
    ] {
        let r = only(&other, path);
        assert_eq!(r.header("origin"), Some(origin.as_str()), "{path}");
        assert!(r.header("cookie").is_none(), "{path} carried cookies");
    }
    for path in ["/plain.svg", "/plain.css"] {
        let r = only(&other, path);
        assert!(r.header("origin").is_none(), "{path}");
        assert_eq!(r.header("cookie"), Some("sid=s1"), "{path}");
    }
    // `use-credentials`: cors mode with cookies.
    let creds = only(&other, "/creds.svg");
    assert_eq!(creds.header("origin"), Some(origin.as_str()));
    assert_eq!(creds.header("cookie"), Some("sid=s1"));
}
