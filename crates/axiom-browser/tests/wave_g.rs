//! Wave G — `fetch()` over the profile's ResourceLoader: Request/Response/Headers,
//! streaming bodies, AbortSignal cancellation, credentials and the CORS response rules.
//! Preflights and per-hop redirect checks (Wave H) are covered in `wave_h.rs`.

use std::time::{Duration, Instant};

use axiom_browser::Browser;
use axiom_net::test_server::{TestResponse, TestServer};
use axiom_net::LogState;
use tempfile::{tempdir, TempDir};

fn browser(dir: &TempDir) -> Browser {
    Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap()
}

/// A page whose script records results in `log` and sets `done` when finished.
fn page(script: &str) -> TestResponse {
    let html = format!(
        "<html><body><script>var log = []; var done = false;\n\
         function fail(e) {{ log.push('error:' + (e && e.name) + ':' + (e && e.message)); done = true; }}\n\
         {script}\n</script></body></html>"
    );
    TestResponse::ok(html, "text/html").with_header("Cache-Control", "no-store")
}

fn eval(browser: &mut Browser, src: &str) -> String {
    let tab = browser.window.tabs.active_tab_mut();
    let js = tab.context.page.js.as_mut().expect("page has js");
    js.eval(src)
        .map(|v| v.display)
        .unwrap_or_else(|e| format!("<{e}>"))
}

fn pump_until(browser: &mut Browser, cond: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        browser.window.tabs.active_tab_mut().context.tick();
        if eval(browser, cond) == "true" {
            return eval(browser, "JSON.stringify(log)");
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for `{cond}`; log = {}",
            eval(browser, "JSON.stringify(log)")
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn run(browser: &mut Browser) -> String {
    pump_until(browser, "done", Duration::from_secs(5))
}

#[test]
fn get_resolves_with_response_metadata_and_json_and_text_bodies() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/data.json" => {
            TestResponse::ok(r#"{"a": 7}"#, "application/json").with_header("X-Custom", "yes")
        }
        "/t.txt" => TestResponse::ok("plain text", "text/plain"),
        _ => page(
            r#"fetch('/data.json').then(function (r) {
                 log.push(r.status, r.ok, r.type, r.statusText, r.redirected,
                          r.headers.get('content-type'), r.headers.get('x-custom'),
                          r.url.endsWith('/data.json'), r.bodyUsed);
                 return r.json();
               }).then(function (j) {
                 log.push(j.a);
                 return fetch('t.txt');
               }).then(function (r) { return r.text(); })
                 .then(function (t) { log.push(t); done = true; }, fail);"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(
        run(&mut b),
        r#"[200,true,"basic","OK",false,"application/json","yes",true,false,7,"plain text"]"#
    );
    let req = &srv.requests_for("/data.json")[0];
    assert_eq!(req.method, "GET");
    assert_eq!(req.header("accept"), Some("*/*"));
    assert_eq!(req.header("referer"), Some(srv.url("/").as_str()));
    assert!(req.header("origin").is_none(), "GET must not carry Origin");
}

#[test]
fn post_sends_body_and_custom_headers_but_never_forbidden_ones() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/submit" => TestResponse::ok("thanks", "text/plain"),
        _ => page(
            r#"fetch('/submit', {
                 method: 'post',
                 body: 'hello body',
                 headers: { 'X-Test': '1', 'Content-Type': 'text/plain',
                            'Cookie': 'evil=1', 'Host': 'evil.example', 'Origin': 'http://evil.example' }
               }).then(function (r) { return r.text(); })
                 .then(function (t) { log.push(t); done = true; }, fail);"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(run(&mut b), r#"["thanks"]"#);
    let req = &srv.requests_for("/submit")[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.body, b"hello body");
    assert_eq!(req.header("x-test"), Some("1"));
    assert_eq!(req.header("content-type"), Some("text/plain"));
    assert!(req.header("cookie").is_none(), "script set a Cookie header");
    let origin = srv.url("");
    assert_eq!(req.header("origin"), Some(origin.as_str()));
    let host = format!("127.0.0.1:{}", srv.port());
    assert_eq!(req.header("host"), Some(host.as_str()));
}

#[test]
fn http_errors_resolve_but_network_errors_reject_without_details() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/missing" => TestResponse::status(404, "nope"),
        "/broken" => TestResponse::Close,
        _ => page(
            r#"fetch('/missing').then(function (r) {
                 log.push(r.status, r.ok);
                 return fetch('/broken');
               }).then(function () { log.push('resolved'); done = true; },
                       function (e) { log.push(e instanceof TypeError, e.message); done = true; });"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(run(&mut b), r#"[404,false,true,"Failed to fetch"]"#);
}

#[test]
fn credentials_modes_use_the_profile_cookie_jar_and_hide_set_cookie() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/login" => {
            TestResponse::ok("ok", "text/plain").with_header("Set-Cookie", "sid=secret123; Path=/")
        }
        "/a" | "/b" | "/c" => TestResponse::ok("x", "text/plain"),
        _ => page(
            r#"fetch('/login').then(function (r) {
                 log.push(r.headers.get('set-cookie'), r.headers.has('set-cookie'),
                          r.headers.getSetCookie().length);
                 return fetch('/a');
               }).then(function () { return fetch('/b', { credentials: 'omit' }); })
                 .then(function () { return fetch('/c', { credentials: 'include' }); })
                 .then(function () { done = true; }, fail);"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(run(&mut b), "[null,false,0]");
    assert_eq!(
        srv.requests_for("/a")[0].header("cookie"),
        Some("sid=secret123")
    );
    assert!(srv.requests_for("/b")[0].header("cookie").is_none());
    assert_eq!(
        srv.requests_for("/c")[0].header("cookie"),
        Some("sid=secret123")
    );
}

#[test]
fn same_origin_credentials_are_port_aware_for_cross_origin_no_cors() {
    // Cookies ignore ports, so the other server shares 127.0.0.1's jar but not its origin.
    let other = TestServer::spawn(|_| TestResponse::ok("x", "text/plain"));
    let (plain, with_creds) = (other.url("/plain"), other.url("/creds"));
    let script = format!(
        r#"fetch('/login').then(function () {{
             return fetch('{plain}', {{ mode: 'no-cors' }});
           }}).then(function () {{
             return fetch('{with_creds}', {{ mode: 'no-cors', credentials: 'include' }});
           }}).then(function () {{ done = true; }}, fail);"#
    );
    let srv = TestServer::spawn(move |req| match req.path.as_str() {
        "/login" => {
            TestResponse::ok("ok", "text/plain").with_header("Set-Cookie", "sid=s1; Path=/")
        }
        _ => page(&script),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    run(&mut b);
    assert!(other.requests_for("/plain")[0].header("cookie").is_none());
    assert_eq!(
        other.requests_for("/creds")[0].header("cookie"),
        Some("sid=s1")
    );
}

#[test]
fn cross_origin_cors_without_allow_origin_rejects_and_no_cors_is_opaque() {
    let other = TestServer::spawn(|_| {
        TestResponse::ok("secret payload", "text/plain").with_header("X-Secret", "1")
    });
    let target = other.url("/x");
    let script = format!(
        r#"var target = '{target}';
           fetch(target).then(function () {{ log.push('cors resolved'); }},
                              function (e) {{ log.push(e.name, e.message); }})
           .then(function () {{ return fetch(target, {{ headers: {{ 'X-Custom': '1' }} }}); }})
           .then(function () {{ log.push('preflighted resolved'); }},
                 function (e) {{ log.push(e.name, e.message); }})
           .then(function () {{ return fetch(target, {{ mode: 'same-origin' }}); }})
           .then(function () {{ log.push('same-origin resolved'); }},
                 function (e) {{ log.push(e.name); }})
           .then(function () {{ return fetch(target, {{ mode: 'no-cors' }}); }})
           .then(function (r) {{
             log.push(r.type, r.status, r.ok, r.statusText, r.url, r.headers.has('x-secret'));
             return r.text();
           }}).then(function (t) {{ log.push(t); done = true; }}, fail);"#
    );
    let srv = TestServer::spawn(move |_| page(&script));
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(
        run(&mut b),
        r#"["TypeError","Failed to fetch","TypeError","Failed to fetch","TypeError","opaque",0,false,"","",false,""]"#
    );
    // The simple CORS request, the failed preflight and the no-cors request were sent;
    // the preflighted request itself and the same-origin one never left the browser.
    let sent = other.requests_for("/x");
    let methods: Vec<&str> = sent.iter().map(|r| r.method.as_str()).collect();
    assert_eq!(methods, ["GET", "OPTIONS", "GET"]);
    let origin = srv.url("");
    assert_eq!(sent[0].header("origin"), Some(origin.trim_end_matches('/')));
    assert_eq!(
        sent[1].header("access-control-request-headers"),
        Some("x-custom")
    );
    assert!(sent.iter().all(|r| r.header("x-custom").is_none()));
    assert!(sent.iter().all(|r| r.header("cookie").is_none()));
}

#[test]
fn cross_origin_cors_with_allow_origin_exposes_only_safelisted_and_listed_headers() {
    let other = TestServer::spawn(|req| {
        let origin = req.header("origin").unwrap_or("").to_string();
        TestResponse::ok(r#"{"n": 3}"#, "application/json")
            .with_header("Access-Control-Allow-Origin", &origin)
            .with_header("Access-Control-Expose-Headers", "X-Total")
            .with_header("X-Total", "3")
            .with_header("X-Secret", "1")
    });
    let target = other.url("/api");
    let script = format!(
        r#"fetch('{target}').then(function (r) {{
             log.push(r.type, r.status, r.headers.get('content-type'), r.headers.get('x-total'),
                      r.headers.has('x-secret'), r.headers.has('access-control-allow-origin'));
             return r.json();
           }}).then(function (j) {{ log.push(j.n); done = true; }}, fail);"#
    );
    let srv = TestServer::spawn(move |_| page(&script));
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(
        run(&mut b),
        r#"["cors",200,"application/json","3",false,false,3]"#
    );
}

#[test]
fn cross_origin_redirects_are_cors_checked_and_same_origin_ones_followed() {
    let other = TestServer::spawn(|_| TestResponse::ok("other", "text/plain"));
    let hop_target = other.url("/landing");
    let srv = TestServer::spawn(move |req| match req.path.as_str() {
        "/hop" => TestResponse::redirect(302, &hop_target),
        "/local" => TestResponse::redirect(301, "/final"),
        "/final" => TestResponse::ok("final", "text/plain"),
        _ => page(
            r#"fetch('/local').then(function (r) {
                 log.push(r.redirected, r.url.endsWith('/final'));
                 return r.text();
               }).then(function (t) {
                 log.push(t);
                 return fetch('/hop');
               }).then(function () { log.push('hop resolved'); done = true; },
                       function (e) { log.push(e.name); done = true; });"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(run(&mut b), r#"[true,true,"final","TypeError"]"#);
    // The hop is followed as a CORS request, then rejected: no Access-Control-Allow-Origin.
    let hop = other.requests_for("/landing");
    assert_eq!(hop.len(), 1);
    let origin = srv.url("");
    assert_eq!(hop[0].header("origin"), Some(origin.trim_end_matches('/')));
}

#[test]
fn redirect_error_and_manual_modes() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/r" => TestResponse::redirect(302, "/final"),
        "/final" => TestResponse::ok("final", "text/plain"),
        _ => page(
            r#"fetch('/r', { redirect: 'error' }).then(function () { log.push('resolved'); },
                                                     function (e) { log.push(e.name); })
               .then(function () { return fetch('/r', { redirect: 'manual' }); })
               .then(function (r) {
                 log.push(r.type, r.status, r.headers.has('location'));
                 done = true;
               }, fail);"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(run(&mut b), r#"["TypeError","opaqueredirect",0,false]"#);
    assert!(
        srv.requests_for("/final").is_empty(),
        "redirect followed in error/manual mode"
    );
}

#[test]
fn data_urls_and_unsupported_schemes() {
    let srv = TestServer::spawn(|_| {
        page(
            r#"fetch('data:text/plain;charset=utf-8,hello%20world').then(function (r) {
                 log.push(r.status, r.type, r.headers.get('content-type'));
                 return r.text();
               }).then(function (t) {
                 log.push(t);
                 return fetch('file:///etc/passwd');
               }).then(function () { log.push('file resolved'); done = true; },
                       function (e) { log.push(e.name); done = true; });"#,
        )
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(
        run(&mut b),
        r#"[200,"basic","text/plain;charset=utf-8","hello world","TypeError"]"#
    );
}

#[test]
fn abort_before_and_during_a_request_cancels_the_network_load() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/never" => TestResponse::ok("x", "text/plain"),
        "/slow" => TestResponse::ok("late", "text/plain").delayed(3_000),
        _ => page(
            r#"var early = new AbortController();
               early.abort();
               fetch('/never', { signal: early.signal }).then(function () { log.push('resolved'); },
                 function (e) { log.push(e.name, e instanceof DOMException); })
               .then(function () {
                 var c = new AbortController();
                 var p = fetch('/slow', { signal: c.signal });
                 setTimeout(function () { c.abort(); }, 100);
                 return p;
               }).then(function () { log.push('slow resolved'); done = true; },
                       function (e) { log.push(e.name, e.code); done = true; });"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    let t = Instant::now();
    assert_eq!(run(&mut b), r#"["AbortError",true,"AbortError",20]"#);
    assert!(t.elapsed() < Duration::from_secs(2), "abort was not prompt");
    assert!(srv.requests_for("/never").is_empty());
    wait_for_cancelled(&b, "/slow");
}

/// The network service records cancellation asynchronously on its worker.
fn wait_for_cancelled(b: &Browser, path: &str) {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let cancelled = b
            .network()
            .recent_log(20)
            .into_iter()
            .any(|e| e.url.ends_with(path) && e.state == LogState::Cancelled);
        if cancelled {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{path} was not cancelled in the network service"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn abort_signal_timeout_rejects_with_timeout_error() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/slow" => TestResponse::ok("late", "text/plain").delayed(3_000),
        _ => page(
            r#"fetch('/slow', { signal: AbortSignal.timeout(50) })
                 .then(function () { log.push('resolved'); done = true; },
                       function (e) { log.push(e.name); done = true; });"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(run(&mut b), r#"["TimeoutError"]"#);
}

#[test]
fn response_body_streams_chunks_before_the_transfer_completes() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/stream" => TestResponse::Chunked {
            status: 200,
            headers: vec![("Content-Type".into(), "text/plain".into())],
            chunks: vec![b"one,".to_vec(), b"two,".to_vec(), b"three".to_vec()],
            delay_ms: 200,
        },
        _ => page(
            r#"var first = false, text = '';
               fetch('/stream').then(function (r) {
                 var reader = r.body.getReader(), dec = new TextDecoder();
                 function step() {
                   return reader.read().then(function (res) {
                     if (res.done) { log.push(text); done = true; return; }
                     first = true;
                     text += dec.decode(res.value, { stream: true });
                     return step();
                   });
                 }
                 return step();
               }).catch(fail);"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    pump_until(&mut b, "first", Duration::from_secs(5));
    assert_eq!(
        srv.streams_completed(),
        0,
        "first chunk only reached script after the whole body"
    );
    assert_eq!(run(&mut b), r#"["one,two,three"]"#);
}

#[test]
fn cancelling_the_body_stream_aborts_the_transfer() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/stream" => TestResponse::Chunked {
            status: 200,
            headers: vec![("Content-Type".into(), "text/plain".into())],
            chunks: (0..40).map(|i| format!("chunk{i};").into_bytes()).collect(),
            delay_ms: 50,
        },
        _ => page(
            r#"fetch('/stream').then(function (r) {
                 var reader = r.body.getReader();
                 return reader.read().then(function (res) {
                   log.push(res.done);
                   return reader.cancel();
                 });
               }).then(function () { done = true; }, fail);"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(run(&mut b), "[false]");
    let deadline = Instant::now() + Duration::from_secs(5);
    while srv.streams_aborted() == 0 {
        assert!(Instant::now() < deadline, "server kept streaming");
        b.window.tabs.active_tab_mut().context.tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(srv.streams_completed(), 0);
}

#[test]
fn body_is_single_use_and_clone_tees_it() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/t" => TestResponse::ok("abc", "text/plain"),
        _ => page(
            r#"fetch('/t').then(function (r) {
                 var copy = r.clone();
                 return Promise.all([r.text(), copy.text()]).then(function (texts) {
                   log.push(texts[0], texts[1], r.bodyUsed);
                   return r.text();
                 });
               }).then(function () { log.push('reread'); done = true; },
                       function (e) { log.push(e.name); done = true; });"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(run(&mut b), r#"["abc","abc",true,"TypeError"]"#);
}

#[test]
fn promise_reactions_run_as_microtasks_before_timers() {
    let srv = TestServer::spawn(|_| {
        page(
            r#"setTimeout(function () { log.push('timer'); done = true; }, 0);
               Promise.resolve().then(function () { log.push('micro'); });
               new Response('local body').text().then(function (t) { log.push(t); });
               log.push('sync');"#,
        )
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(run(&mut b), r#"["sync","micro","local body","timer"]"#);
}

#[test]
fn request_headers_and_response_objects_validate_input() {
    let srv = TestServer::spawn(|_| {
        page(
            r#"function throws(f) { try { f(); return 'ok'; } catch (e) { return e.name; } }
               var h = new Headers([['B', '2'], ['a', '1']]);
               h.append('a', '3');
               var keys = [];
               h.forEach(function (v, k) { keys.push(k + '=' + v); });
               log.push(keys.join('&'));
               log.push(throws(function () { new Headers({ 'bad name': 'x' }); }));
               log.push(throws(function () { new Request('/x', { method: 'GET', body: 'b' }); }));
               log.push(throws(function () { new Request('/x', { method: 'TRACE' }); }));
               log.push(throws(function () { new Request('/x', { mode: 'navigate' }); }));
               log.push(throws(function () { new Response('x', { status: 99 }); }));
               log.push(throws(function () { Response.error().headers.set('a', 'b'); }));
               var req = new Request('rel/path?q=1', { method: 'put' });
               log.push(req.method, req.url.endsWith('/rel/path?q=1'), req.mode, req.credentials);
               log.push(Response.error().type, Response.redirect('/x', 301).status);
               Response.json({ k: 1 }).json().then(function (j) { log.push(j.k); done = true; }, fail);"#,
        )
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(
        run(&mut b),
        r#"["a=1, 3&b=2","TypeError","TypeError","TypeError","TypeError","RangeError","TypeError","PUT",true,"cors","same-origin","error",301,1]"#
    );
}

#[test]
fn cache_modes_use_the_profile_http_cache() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/c" => {
            TestResponse::ok("cached", "text/plain").with_header("Cache-Control", "max-age=600")
        }
        _ => page(
            r#"function get(opts) { return fetch('/c', opts).then(function (r) { return r.text(); }); }
               get({}).then(function () { return get({}); })
                 .then(function () { return get({ cache: 'force-cache' }); })
                 .then(function () { return get({ cache: 'reload' }); })
                 .then(function () { return get({ cache: 'no-store' }); })
                 .then(function (t) { log.push(t); done = true; }, fail);"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(run(&mut b), r#"["cached"]"#);
    // default (miss), default (hit), force-cache (hit), reload (network), no-store (network)
    assert_eq!(srv.requests_for("/c").len(), 3);
}

#[test]
fn navigation_cancels_in_flight_fetches() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/slow" => TestResponse::ok("late", "text/plain").delayed(3_000),
        "/next" => page("done = true;"),
        _ => page("fetch('/slow').then(function () { log.push('resolved'); });"),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while srv.requests_for("/slow").is_empty() {
        assert!(Instant::now() < deadline, "fetch never started");
        b.window.tabs.active_tab_mut().context.tick();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(b.window.tabs.active_tab().context.pending_fetch_count(), 1);
    b.navigate_resolved(&srv.url("/next"));
    assert_eq!(b.window.tabs.active_tab().context.pending_fetch_count(), 0);
    let ctx = b.window.tabs.active_tab().context.loader.context_id();
    let deadline = Instant::now() + Duration::from_secs(1);
    while b.scheduler().active_for_context(ctx) > 0 {
        assert!(Instant::now() < deadline, "fetch survived navigation");
        std::thread::sleep(Duration::from_millis(5));
    }
    wait_for_cancelled(&b, "/slow");
}

#[test]
fn fetches_appear_on_axiom_network() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/api/items" => TestResponse::ok("[]", "application/json"),
        _ => page("fetch('/api/items?token=hidden').then(function () { done = true; }, fail);"),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    run(&mut b);
    b.refresh_internal_pages();
    let html = b
        .internals
        .resolve("axiom://network")
        .expect("network page")
        .html
        .clone();
    assert!(html.contains("/api/items"));
    assert!(html.contains("fetch"));
    assert!(
        !html.contains("hidden"),
        "query string shown on axiom://network"
    );
}
