//! Wave G completion — Blob/File/FormData/URLSearchParams bodies, streaming uploads,
//! response backpressure, Subresource Integrity, unhandled rejections, keepalive, custom
//! referrers and the headless engine's event loop.

use std::time::{Duration, Instant};

use axiom_browser::Browser;
use axiom_engine::{Engine, NavigateOptions};
use axiom_net::test_server::{TestResponse, TestServer};
use axiom_net::LogState;
use axiom_paint::DisplayCommand;
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
        browser.window.tabs.tick_active();
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
    pump_until(browser, "done", Duration::from_secs(10))
}

fn pump_while(b: &mut Browser, what: &str, mut busy: impl FnMut(&Browser) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while busy(b) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        b.window.tabs.tick_active();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_log_state(b: &mut Browser, path: &str, state: LogState, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let found = b
            .network()
            .recent_log(50)
            .into_iter()
            .any(|e| e.url.ends_with(path) && e.state == state);
        if found {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{path} never reached {state:?} in the network log"
        );
        b.window.tabs.tick_active();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn urlsearchparams_formdata_and_blob_request_bodies() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/form" | "/multi" | "/blob" => TestResponse::ok("ok", "text/plain"),
        _ => page(
            r#"var params = new URLSearchParams('?b=x y&a=1');
               params.append('c', 'é&');
               params.sort();
               log.push(params.toString(), params.get('b'), params.has('a', '1'), params.size);
               var fd = new FormData();
               fd.append('field', 'line1\nline2');
               fd.append('upload', new Blob(['file-bytes'], { type: 'text/plain' }), 'f.txt');
               log.push(fd.get('upload') instanceof File, fd.get('upload').name, fd.getAll('field').length);
               fetch('/form', { method: 'POST', body: params })
                 .then(function () { return fetch('/multi', { method: 'POST', body: fd }); })
                 .then(function () {
                   return fetch('/blob', { method: 'POST', body: new Blob(['{"k":1}'], { type: 'Application/JSON' }) });
                 })
                 .then(function () { done = true; }, fail);"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(
        run(&mut b),
        r#"["a=1&b=x+y&c=%C3%A9%26","x y",true,3,true,"f.txt",1]"#
    );

    let form = &srv.requests_for("/form")[0];
    assert_eq!(form.body, b"a=1&b=x+y&c=%C3%A9%26");
    assert_eq!(
        form.header("content-type"),
        Some("application/x-www-form-urlencoded;charset=UTF-8")
    );

    let multi = &srv.requests_for("/multi")[0];
    let ct = multi.header("content-type").unwrap();
    let boundary = ct
        .strip_prefix("multipart/form-data; boundary=")
        .expect("multipart content type");
    let body = String::from_utf8(multi.body.clone()).unwrap();
    assert_eq!(
        body,
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"field\"\r\n\r\nline1\r\nline2\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"upload\"; filename=\"f.txt\"\r\n\
             Content-Type: text/plain\r\n\r\nfile-bytes\r\n--{boundary}--\r\n"
        )
    );

    let blob = &srv.requests_for("/blob")[0];
    assert_eq!(blob.body, br#"{"k":1}"#);
    assert_eq!(blob.header("content-type"), Some("application/json"));
}

#[test]
fn blob_and_form_data_body_readers() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/img" => TestResponse::ok(vec![1u8, 2, 3, 4, 5], "image/png"),
        "/q" => TestResponse::ok("x=1&y=a+b&x=2", "application/x-www-form-urlencoded"),
        _ => page(
            r#"fetch('/img').then(function (r) { return r.blob(); })
               .then(function (blob) {
                 log.push(blob.size, blob.type, blob instanceof Blob);
                 return blob.slice(1, -1).arrayBuffer();
               }).then(function (buf) {
                 log.push(Array.from(new Uint8Array(buf)).join(','));
                 return fetch('/q');
               }).then(function (r) { return r.formData(); })
               .then(function (fd) {
                 log.push(fd.getAll('x').join('|'), fd.get('y'));
                 var out = new FormData();
                 out.append('name', 'value');
                 out.append('file', new File(['hello'], 'h.txt', { type: 'text/plain' }));
                 return new Response(out).formData();
               }).then(function (fd) {
                 var f = fd.get('file');
                 log.push(fd.get('name'), f.name, f.type, f.size);
                 return f.text();
               }).then(function (t) {
                 log.push(t);
                 return new Response('plain', { headers: { 'content-type': 'text/plain' } }).formData();
               }).then(function () { log.push('parsed'); done = true; },
                       function (e) { log.push(e.name); done = true; });"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(
        run(&mut b),
        r#"[5,"image/png",true,"2,3,4","1|2","a b","value","h.txt","text/plain",5,"hello","TypeError"]"#
    );
}

#[test]
fn streaming_request_body_is_sent_as_it_is_produced() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/upload" => TestResponse::ok(format!("got {}", req.body.len()), "text/plain"),
        _ => page(
            r#"function throws(f) { try { f(); return 'ok'; } catch (e) { return e.name; } }
               function makeStream(parts) {
                 var i = 0, enc = new TextEncoder();
                 return new ReadableStream({
                   pull: function (c) {
                     return new Promise(function (resolve) {
                       setTimeout(function () {
                         if (i < parts.length) c.enqueue(enc.encode(parts[i++])); else c.close();
                         resolve();
                       }, 20);
                     });
                   }
                 });
               }
               log.push(throws(function () { new Request('/upload', { method: 'POST', body: makeStream([]) }); }));
               log.push(throws(function () {
                 new Request('/upload', { method: 'POST', body: makeStream([]), duplex: 'half', keepalive: true });
               }));
               log.push(throws(function () {
                 new Request('/upload', { method: 'POST', body: makeStream([]), duplex: 'half', mode: 'no-cors' });
               }));
               fetch('/upload', { method: 'POST', body: makeStream(['alpha-', 'beta-', 'gamma']), duplex: 'half' })
                 .then(function (r) { return r.text(); })
                 .then(function (t) { log.push(t); done = true; }, fail);"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(
        run(&mut b),
        r#"["TypeError","TypeError","TypeError","got 16"]"#
    );
    let req = &srv.requests_for("/upload")[0];
    assert_eq!(req.body, b"alpha-beta-gamma");
    assert_eq!(req.header("transfer-encoding"), Some("chunked"));
    assert!(
        req.body_chunks > 1,
        "body arrived in {} chunk(s): it was buffered, not streamed",
        req.body_chunks
    );
}

#[test]
fn large_streaming_upload_is_paced_and_arrives_intact() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/upload" => {
            let sum: u64 = req.body.iter().map(|&b| u64::from(b)).sum();
            TestResponse::ok(format!("{} {sum}", req.body.len()), "text/plain")
        }
        _ => page(
            r#"var sent = 0, CHUNK = 16384, TOTAL = 1024 * 1024;
               var stream = new ReadableStream({
                 pull: function (c) {
                   if (sent >= TOTAL) { c.close(); return; }
                   var chunk = new Uint8Array(CHUNK);
                   for (var i = 0; i < CHUNK; i++) chunk[i] = (sent + i) & 0xff;
                   sent += CHUNK;
                   c.enqueue(chunk);
                 }
               });
               fetch('/upload', { method: 'POST', body: stream, duplex: 'half' })
                 .then(function (r) { return r.text(); })
                 .then(function (t) { log.push(t); done = true; }, fail);"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    let total: u64 = 1024 * 1024;
    let sum: u64 = (0..total).map(|i| i & 0xff).sum();
    assert_eq!(
        pump_until(&mut b, "done", Duration::from_secs(30)),
        format!(r#"["{total} {sum}"]"#)
    );
}

#[test]
fn an_erroring_upload_stream_fails_the_fetch() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/upload" => TestResponse::ok("unexpected", "text/plain"),
        _ => page(
            r#"var n = 0;
               var stream = new ReadableStream({
                 pull: function (c) {
                   if (n++ === 0) c.enqueue(new TextEncoder().encode('partial'));
                   else c.error(new Error('producer failed'));
                 }
               });
               fetch('/upload', { method: 'POST', body: stream, duplex: 'half' })
                 .then(function () { log.push('resolved'); done = true; },
                       function (e) { log.push(e.name); done = true; });"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(run(&mut b), r#"["TypeError"]"#);
}

#[test]
fn response_backpressure_pauses_the_network_until_script_reads() {
    const CHUNK: usize = 64 * 1024;
    const CHUNKS: usize = 256; // 16 MiB
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/big" => TestResponse::Chunked {
            status: 200,
            headers: vec![("Content-Type".into(), "application/octet-stream".into())],
            chunks: (0..CHUNKS).map(|_| vec![b'x'; CHUNK]).collect(),
            delay_ms: 0,
        },
        _ => page(
            r#"var reader = null, total = 0;
               fetch('/big').then(function (r) { reader = r.body.getReader(); }, fail);
               function drain() {
                 return reader.read().then(function (res) {
                   if (res.done) { log.push(total); done = true; return; }
                   total += res.value.byteLength;
                   return drain();
                 });
               }"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    pump_until(&mut b, "reader !== null", Duration::from_secs(5));

    // Script holds the body without reading: the network must stop well short of the end.
    let stall = Instant::now() + Duration::from_millis(600);
    while Instant::now() < stall {
        b.window.tabs.tick_active();
        std::thread::sleep(Duration::from_millis(5));
    }
    let written = srv.stream_bytes_written();
    let total = (CHUNK * CHUNKS) as u64;
    assert_eq!(srv.streams_completed(), 0, "the whole body was pulled in");
    assert!(
        written < total / 2,
        "server wrote {written} of {total} bytes while script was not reading"
    );

    eval(&mut b, "drain().catch(fail); 0");
    assert_eq!(
        pump_until(&mut b, "done", Duration::from_secs(60)),
        format!("[{total}]")
    );
    assert_eq!(srv.streams_completed(), 1);
}

#[test]
fn integrity_is_verified_before_the_response_is_revealed() {
    // printf 'alert(1)' | openssl dgst -sha256 -binary | base64
    const GOOD: &str = "sha256-bhHHL3z2vDgxUt0W3dWQOrprscmda2Y5pLsLg4GF+pI=";
    let other = TestServer::spawn(|_| TestResponse::ok("alert(1)", "text/javascript"));
    let cross = other.url("/s.js");
    let script = format!(
        r#"var good = '{GOOD}';
           fetch('/s.js', {{ integrity: good }}).then(function (r) {{ return r.text(); }})
           .then(function (t) {{
             log.push(t);
             return fetch('/s.js', {{ integrity: 'sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=' }});
           }}).then(function () {{ log.push('bad digest resolved'); }}, function (e) {{ log.push(e.name); }})
           .then(function () {{ return fetch('/s.js', {{ integrity: 'md5-whatever' }}); }})
           .then(function (r) {{ log.push(r.status); return fetch('{cross}', {{ mode: 'no-cors', integrity: good }}); }})
           .then(function () {{ log.push('opaque resolved'); }}, function (e) {{ log.push(e.name); }})
           .then(function () {{ return fetch('data:text/plain,alert(1)', {{ integrity: good }}); }})
           .then(function (r) {{ return r.text(); }})
           .then(function (t) {{
             log.push(t);
             return fetch('data:text/plain,alert(2)', {{ integrity: good }});
           }}).then(function () {{ log.push('bad data resolved'); done = true; }},
                    function (e) {{ log.push(e.name); done = true; }});"#
    );
    let srv = TestServer::spawn(move |req| match req.path.as_str() {
        "/s.js" => TestResponse::ok("alert(1)", "text/javascript"),
        _ => page(&script),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(
        run(&mut b),
        r#"["alert(1)","TypeError",200,"TypeError","alert(1)","TypeError"]"#
    );
}

#[test]
fn unhandled_rejections_fire_events_and_late_handlers_fire_rejectionhandled() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/broken" => TestResponse::Close,
        _ => page(
            r#"function onUnhandled(e) {
                 log.push('unhandled', e.reason && e.reason.message, e.promise instanceof Promise,
                          e.cancelable, e.type);
                 e.preventDefault();
               }
               window.addEventListener('unhandledrejection', onUnhandled);
               window.onrejectionhandled = function (e) {
                 log.push('handled', e.reason.message);
                 fetch('/broken');
               };
               Promise.reject(new Error('handled in time')).catch(function () {});
               var late = Promise.reject(new Error('late'));
               setTimeout(function () { late.catch(function () {}); }, 20);
               window.addEventListener('unhandledrejection', function (e) {
                 if (e.reason instanceof TypeError) { log.push('fetch', e.reason.message); done = true; }
               });"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(
        run(&mut b),
        r#"["unhandled","late",true,true,"unhandledrejection","handled","late","unhandled","Failed to fetch",true,true,"unhandledrejection","fetch","Failed to fetch"]"#
    );

    // Without preventDefault the rejection is reported on the console.
    let tab = b.window.tabs.active_tab_mut();
    let js = tab.context.page.js.as_mut().unwrap();
    js.eval(
        "removeEventListener('unhandledrejection', onUnhandled); \
         Promise.reject(new RangeError('nobody listens')); 0",
    )
    .unwrap();
    let console = js.take_console();
    assert!(
        console
            .iter()
            .any(|m| m == "Uncaught (in promise) RangeError: nobody listens"),
        "console: {console:?}"
    );
}

#[test]
fn handlers_attached_while_pending_suppress_unhandled_rejection() {
    let srv = TestServer::spawn(|_| {
        page(
            r#"window.addEventListener('unhandledrejection', function (e) {
                 log.push(e.reason.message);
                 e.preventDefault();
                 if (e.reason.message === 'real') done = true;
               });
               (async function () { await Promise.resolve(); throw new Error('after await'); })()
                 .catch(function () {});
               Promise.resolve().then(function () { throw new Error('then throw'); })
                 .catch(function () {});
               new Promise(function (_, rej) { setTimeout(function () { rej(new Error('timer')); }, 0); })
                 .finally(function () {}).catch(function () {});
               Promise.all([new Promise(function (_, rej) { setTimeout(function () { rej(new Error('all')); }, 0); })])
                 .catch(function () {});
               (async function () {
                 try {
                   await new Promise(function (_, rej) { setTimeout(function () { rej(new Error('awaited')); }, 0); });
                 } catch (e) {}
               })();
               new Promise(function (_, rej) { setTimeout(function () { rej(new Error('real')); }, 0); });"#,
        )
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(run(&mut b), r#"["real"]"#);
}

#[test]
fn keepalive_fetch_survives_navigation() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/beacon" | "/plain" => TestResponse::ok("ack", "text/plain").delayed(400),
        "/next" => page("done = true;"),
        _ => page(
            r#"fetch('/beacon', { method: 'POST', body: 'bye', keepalive: true });
               fetch('/plain', { method: 'POST', body: 'cancel me' });"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while srv.requests_for("/beacon").is_empty() || srv.requests_for("/plain").is_empty() {
        assert!(Instant::now() < deadline, "fetches never started");
        b.window.tabs.tick_active();
        std::thread::sleep(Duration::from_millis(5));
    }
    b.navigate_resolved(&srv.url("/next"));
    assert_eq!(b.window.tabs.active_tab().context.keepalive_in_flight(), 1);
    wait_for_log_state(
        &mut b,
        "/beacon",
        LogState::Complete,
        Duration::from_secs(5),
    );
    wait_for_log_state(
        &mut b,
        "/plain",
        LogState::Cancelled,
        Duration::from_secs(5),
    );
    assert_eq!(srv.requests_for("/beacon")[0].body, b"bye");
    pump_while(&mut b, "the keepalive load to finish", |b| {
        b.window.tabs.active_tab().context.keepalive_in_flight() > 0
    });
}

#[test]
fn keepalive_fetch_survives_closing_its_tab() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/beacon" => TestResponse::ok("ack", "text/plain").delayed(400),
        _ => page("fetch('/beacon', { method: 'POST', body: 'closing', keepalive: true });"),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.new_tab();
    b.navigate_resolved(&srv.url("/"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while srv.requests_for("/beacon").is_empty() {
        assert!(Instant::now() < deadline, "beacon never started");
        b.window.tabs.tick_active();
        std::thread::sleep(Duration::from_millis(5));
    }
    b.close_tab();
    assert_eq!(b.window.tabs.detached_keepalive_count(), 1);
    wait_for_log_state(
        &mut b,
        "/beacon",
        LogState::Complete,
        Duration::from_secs(5),
    );
    pump_while(&mut b, "the detached keepalive load to finish", |b| {
        b.window.tabs.detached_keepalive_count() > 0
    });
}

#[test]
fn keepalive_body_quota_is_enforced() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/k" => TestResponse::ok("ack", "text/plain").delayed(300),
        _ => page(
            r#"var body40k = new Uint8Array(40 * 1024);
               function send() {
                 return fetch('/k', { method: 'POST', body: body40k, keepalive: true })
                   .then(function (r) { return 'ok ' + r.status; }, function (e) { return e.name; });
               }
               var over = fetch('/k', { method: 'POST', body: new Uint8Array(65 * 1024), keepalive: true })
                 .then(function () { return 'resolved'; }, function (e) { return e.name; });
               Promise.all([over, send(), send()]).then(function (r) {
                 log.push(r[0], r[1], r[2]);
                 return send();
               }).then(function (r) { log.push(r); done = true; }, fail);"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    // 65 KiB is over the quota outright; the second 40 KiB request would exceed it while
    // the first is in flight; once that finishes there is room again.
    assert_eq!(
        run(&mut b),
        r#"["TypeError","ok 200","TypeError","ok 200"]"#
    );
    assert_eq!(srv.requests_for("/k").len(), 2);
}

#[test]
fn custom_referrer_urls_apply_only_when_same_origin() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/r1" | "/r2" | "/r3" => TestResponse::ok("x", "text/plain"),
        _ => page(
            r#"var same = new Request('/x', { referrer: '/some/page?x=1' });
               var cross = new Request('/x', { referrer: 'http://evil.test/' });
               log.push(same.referrer.endsWith('/some/page?x=1'), cross.referrer,
                        new Request('/x', { referrer: '' }).referrer);
               fetch('/r1', { referrer: '/some/page?x=1' })
                 .then(function () { return fetch('/r2', { referrer: 'http://evil.test/' }); })
                 .then(function () { return fetch('/r3', { referrer: '' }); })
                 .then(function () { done = true; }, fail);"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(run(&mut b), r#"[true,"about:client",""]"#);
    assert_eq!(
        srv.requests_for("/r1")[0].header("referer"),
        Some(srv.url("/some/page?x=1").as_str())
    );
    assert_eq!(
        srv.requests_for("/r2")[0].header("referer"),
        Some(srv.url("/").as_str())
    );
    assert!(srv.requests_for("/r3")[0].header("referer").is_none());
}

#[test]
fn headless_engine_runs_fetch_and_settles_before_the_frame() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/data" => TestResponse::ok("fetched-by-script", "text/plain").delayed(100),
        _ => TestResponse::ok(
            "<html><head><title>Headless</title></head><body><p id=out>waiting</p><script>\
             fetch('/data').then(function (r) { return r.text(); })\
             .then(function (t) { document.getElementById('out').textContent = t; });\
             </script></body></html>",
            "text/html",
        ),
    });
    let page = Engine::new()
        .navigate(&srv.url("/"), NavigateOptions::default())
        .expect("navigate");
    assert_eq!(page.title, "Headless");
    assert!(page.timings.settled);
    assert!(page.timings.net_ms > 0.0);
    assert!(page.timings.script_ms > 0.0);
    let texts: Vec<&str> = page
        .display_list
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("fetched-by-script")),
        "frame was taken before the fetch finished: {texts:?}"
    );
    assert_eq!(srv.requests_for("/data").len(), 1);
}

#[test]
fn headless_engine_reports_a_settle_timeout() {
    let srv = TestServer::spawn(|_| {
        TestResponse::ok(
            "<html><body><script>setTimeout(function () {}, 60000);</script></body></html>",
            "text/html",
        )
    });
    let page = Engine::new()
        .navigate(
            &srv.url("/"),
            NavigateOptions {
                settle_timeout: Duration::from_millis(100),
                ..NavigateOptions::default()
            },
        )
        .expect("navigate");
    assert!(!page.timings.settled);
    assert!(page.timings.report().contains("settle timeout"));
}
