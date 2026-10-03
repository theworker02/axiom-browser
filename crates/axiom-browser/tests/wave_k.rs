//! Wave K: web platform APIs pages use without asking. The performance timeline, base64,
//! `crypto`, `structuredClone`, `dataset`, the `CSS` namespace, focus, `MessageChannel`
//! and `postMessage`, DOM traversal, `XMLHttpRequest`, the History API, navigating
//! through `location`, and shadow trees.

use std::time::Duration;

use axiom_browser::Browser;
use axiom_engine::BrowsingContext;
use axiom_net::test_server::{TestResponse, TestServer};
use tempfile::{tempdir, TempDir};

const PAGE: &str = "<!doctype html><html><head><style>\
    body { margin: 0 } #tall { height: 3000px } #target { height: 10px }\
    </style></head><body>\
    <div id=box data-user-id=7><input id=field><span id=text>t</span></div>\
    <a id=jump href=#target>jump</a>\
    <div id=tall></div><div id=target></div>\
    </body></html>";

fn browser(dir: &TempDir) -> Browser {
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    ctx(&mut b).set_chrome_height(0);
    b
}

fn ctx(b: &mut Browser) -> &mut BrowsingContext {
    &mut b.window.tabs.active_tab_mut().context
}

fn eval(b: &mut Browser, src: &str) -> String {
    let js = ctx(b).page.js.as_mut().expect("page has js");
    js.eval(src)
        .map(|v| v.display)
        .unwrap_or_else(|e| format!("<{e}>"))
}

fn idle(b: &mut Browser) {
    assert!(
        ctx(b).run_until_idle(Duration::from_secs(15)),
        "page never went idle"
    );
}

fn open(b: &mut Browser, url: &str) {
    b.navigate_resolved(url);
    idle(b);
}

fn serve() -> TestServer {
    TestServer::spawn(|req| match req.path.as_str() {
        "/data.json" => TestResponse::ok(r#"{"items":[1,2,3]}"#, "application/json")
            .with_header("x-custom", "yes"),
        "/missing" => TestResponse::status(404, "gone"),
        "/legacy" => TestResponse::ok(
            &b"<html><head><title>legacy</title></head><body>caf\xe9</body></html>"[..],
            "text/html; charset=windows-1252",
        )
        .with_header("last-modified", "Tue, 15 Nov 1994 12:45:26 GMT"),
        _ => TestResponse::ok(PAGE, "text/html"),
    })
}

#[test]
fn synchronous_web_apis() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv.url("/"));
    let cases = [
        ("btoa('hello')", "aGVsbG8="),
        ("atob(' aGVs bG8 ')", "hello"),
        (
            "try { atob('a'); 'no' } catch (e) { e.name }",
            "InvalidCharacterError",
        ),
        (
            "try { btoa('\\u0100'); 'no' } catch (e) { e.name }",
            "InvalidCharacterError",
        ),
        ("crypto.getRandomValues(new Uint8Array(16)).length", "16"),
        (
            "/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(crypto.randomUUID())",
            "true",
        ),
        (
            "try { crypto.getRandomValues(new Float32Array(1)); 'no' } catch (e) { e.name }",
            "TypeMismatchError",
        ),
        (
            "var o = { m: new Map([[1, 'a']]), d: new Date(5) }; o.self = o;\
             var c = structuredClone(o); [c !== o, c.self === c, c.m.get(1), c.d.getTime()].join()",
            "true,true,a,5",
        ),
        (
            "try { structuredClone(function f() {}); 'no' } catch (e) { e.name }",
            "DataCloneError",
        ),
        ("box.dataset.userId", "7"),
        (
            "box.dataset.fooBar = 'x'; box.getAttribute('data-foo-bar')",
            "x",
        ),
        ("delete box.dataset.fooBar; box.hasAttribute('data-foo-bar')", "false"),
        ("CSS.escape('1a b')", "\\31 a\\ b"),
        ("CSS.supports('display', 'grid')", "true"),
        ("CSS.supports('(display: flex)')", "true"),
        ("CSS.supports('display', 'nonsense')", "false"),
        ("document.visibilityState + ' ' + document.hidden", "visible false"),
        ("field.focus(); document.activeElement === field", "true"),
        ("text.focus(); document.activeElement === field", "true"),
        ("field.blur(); document.activeElement === document.body", "true"),
        (
            "performance.mark('a'); performance.mark('b');\
             var m = performance.measure('ab', 'a', 'b');\
             [m.entryType, m.duration >= 0, performance.getEntriesByName('a').length].join()",
            "measure,true,1",
        ),
        (
            "performance.clearMarks('a'); performance.getEntriesByType('mark').map(e => e.name).join()",
            "b",
        ),
        (
            "var w = document.createTreeWalker(box, NodeFilter.SHOW_ELEMENT); var names = [];\
             while (w.nextNode()) names.push(w.currentNode.tagName); names.join()",
            "INPUT,SPAN",
        ),
        (
            "var it = document.createNodeIterator(box, NodeFilter.SHOW_TEXT); it.nextNode().data",
            "t",
        ),
        // Loopback is potentially trustworthy.
        ("window.origin === location.origin && isSecureContext", "true"),
    ];
    for (src, want) in cases {
        assert_eq!(eval(&mut b, src), want, "{src}");
    }
}

#[test]
fn intl_formats_like_chrome_en_us() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv.url("/"));
    eval(&mut b, "window.t = Date.UTC(2026, 8, 29, 15, 4, 5, 67)");
    let cases = [
        ("new Intl.NumberFormat().format(1234567.891)", "1,234,567.891"),
        ("(1234.5).toLocaleString()", "1,234.5"),
        ("(-0).toLocaleString()", "-0"),
        (
            "new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD' }).format(-1234.5)",
            "-$1,234.50",
        ),
        (
            "(3).toLocaleString('de-DE', { style: 'currency', currency: 'EUR' })",
            "\u{20ac}3.00",
        ),
        (
            "(5).toLocaleString('en', { style: 'currency', currency: 'JPY' })",
            "\u{a5}5",
        ),
        (
            "(-2).toLocaleString('en', { style: 'currency', currency: 'USD', currencySign: 'accounting' })",
            "($2.00)",
        ),
        ("(0.256).toLocaleString('en', { style: 'percent' })", "26%"),
        (
            "[1234, 12345, 123456, 999999, 1234567].map(n => n.toLocaleString('en', { notation: 'compact' })).join(' ')",
            "1.2K 12K 123K 1M 1.2M",
        ),
        (
            "(1234567).toLocaleString('en', { notation: 'compact', compactDisplay: 'long' })",
            "1.2 million",
        ),
        (
            "(1.005).toLocaleString('en', { maximumFractionDigits: 2 })",
            "1.01",
        ),
        (
            "(5).toLocaleString('en', { minimumFractionDigits: 2 })",
            "5.00",
        ),
        (
            "(0.000123456).toLocaleString('en', { maximumSignificantDigits: 3 })",
            "0.000123",
        ),
        (
            "(5).toLocaleString('en', { style: 'unit', unit: 'kilobyte' })",
            "5 kB",
        ),
        (
            "(1).toLocaleString('en', { style: 'unit', unit: 'day', unitDisplay: 'long' })",
            "1 day",
        ),
        (
            "(50).toLocaleString('en', { style: 'unit', unit: 'kilometer-per-hour' })",
            "50 km/h",
        ),
        ("(5).toLocaleString('en', { signDisplay: 'always' })", "+5"),
        (
            "(12345.678).toLocaleString('en', { notation: 'scientific' })",
            "1.235E4",
        ),
        ("12345678901234567890n.toLocaleString()", "12,345,678,901,234,567,890"),
        (
            "new Intl.NumberFormat().formatToParts(-1234.5).map(p => p.type).join()",
            "minusSign,integer,group,integer,decimal,fraction",
        ),
        ("new Intl.DateTimeFormat('en-US', { timeZone: 'UTC' }).format(t)", "9/29/2026"),
        (
            "new Date(t).toLocaleString('en-US', { timeZone: 'UTC' })",
            "9/29/2026, 3:04:05\u{202f}PM",
        ),
        (
            "new Date(t).toLocaleDateString(undefined, { timeZone: 'UTC', dateStyle: 'full' })",
            "Tuesday, September 29, 2026",
        ),
        (
            "new Date(t).toLocaleString('en', { timeZone: 'UTC', dateStyle: 'medium', timeStyle: 'short' })",
            "Sep 29, 2026, 3:04\u{202f}PM",
        ),
        (
            "new Date(t).toLocaleString('en', { timeZone: 'UTC', dateStyle: 'long', timeStyle: 'long' })",
            "September 29, 2026 at 3:04:05\u{202f}PM UTC",
        ),
        (
            "new Date(t).toLocaleDateString('en', { timeZone: 'UTC', month: 'short', day: 'numeric' })",
            "Sep 29",
        ),
        (
            "new Date(t).toLocaleDateString('en', { timeZone: 'UTC', weekday: 'short', year: 'numeric', month: 'long', day: 'numeric' })",
            "Tue, September 29, 2026",
        ),
        (
            "new Date(t).toLocaleTimeString('en', { timeZone: 'UTC', hour: '2-digit', minute: '2-digit', hour12: false })",
            "15:04",
        ),
        (
            "new Date(t).toLocaleTimeString('en', { timeZone: 'Etc/GMT-2', hour: 'numeric' })",
            "5\u{202f}PM",
        ),
        (
            "new Intl.DateTimeFormat('en', { timeZone: '+05:30', timeZoneName: 'short' }).format(t)",
            "9/29/2026, GMT+5:30",
        ),
        (
            "new Date(t).toLocaleString('en', { timeZone: '+05:30', timeZoneName: 'short' })",
            "9/29/2026, 8:34:05\u{202f}PM GMT+5:30",
        ),
        (
            "new Intl.DateTimeFormat('en', { timeZone: 'UTC', minute: '2-digit', second: '2-digit', fractionalSecondDigits: 2 }).format(t)",
            "04:05.06",
        ),
        (
            "new Intl.DateTimeFormat('en', { timeZone: 'UTC', hour: 'numeric', minute: 'numeric' }).formatToParts(t).map(p => p.type).join()",
            "hour,literal,minute,literal,dayPeriod",
        ),
        ("new Date(NaN).toLocaleString()", "Invalid Date"),
        (
            "try { new Intl.DateTimeFormat('en', { timeZone: 'Not a zone!' }); 'no' } catch (e) { e.name }",
            "RangeError",
        ),
        (
            "try { new Intl.DateTimeFormat('en', { dateStyle: 'short', year: 'numeric' }); 'no' } catch (e) { e.name }",
            "TypeError",
        ),
        (
            "var o = new Intl.DateTimeFormat('fr', { timeZone: 'UTC', hour: 'numeric' }).resolvedOptions();\
             [o.locale, o.timeZone, o.hourCycle, o.hour12].join()",
            "en-US,UTC,h12,true",
        ),
        (
            "var r = new Intl.RelativeTimeFormat('en');\
             [r.format(-1, 'day'), r.format(3, 'hours'), r.format(1000, 'years')].join('|')",
            "1 day ago|in 3 hours|in 1,000 years",
        ),
        (
            "var r = new Intl.RelativeTimeFormat('en', { numeric: 'auto' });\
             [r.format(-1, 'day'), r.format(0, 'second'), r.format(1, 'week'), r.format(-2, 'day')].join('|')",
            "yesterday|now|next week|2 days ago",
        ),
        (
            "new Intl.RelativeTimeFormat('en', { style: 'short' }).format(-5, 'minute')",
            "5 min. ago",
        ),
        (
            "new Intl.ListFormat('en').format(['a', 'b', 'c'])",
            "a, b, and c",
        ),
        (
            "new Intl.ListFormat('en', { type: 'disjunction' }).format(['a', 'b'])",
            "a or b",
        ),
        (
            "var p = new Intl.PluralRules('en'); var q = new Intl.PluralRules('en', { type: 'ordinal' });\
             [p.select(1), p.select(2), p.select(1.5), q.select(1), q.select(22), q.select(13), q.select(103)].join()",
            "one,other,other,one,two,other,few",
        ),
        (
            "['b', 'a', 'B', 'A', '\\u00e4'].sort(new Intl.Collator().compare).join()",
            "a,A,\u{e4},b,B",
        ),
        (
            "['10', '9', '1'].sort(new Intl.Collator(undefined, { numeric: true }).compare).join()",
            "1,9,10",
        ),
        ("'a'.localeCompare('B') + ' ' + 'a'.localeCompare('a')", "-1 0"),
        (
            "'\u{e9}'.localeCompare('e', undefined, { sensitivity: 'base' })",
            "0",
        ),
        (
            "[...new Intl.Segmenter('en', { granularity: 'word' }).segment(\"Hi, you're 3.5!\")]\
               .filter(s => s.isWordLike).map(s => s.segment).join('|')",
            "Hi|you're|3.5",
        ),
        (
            "[...new Intl.Segmenter().segment('e\\u0301\\ud83d\\udc4d\\ud83c\\udffdx')].length",
            "3",
        ),
        (
            "Intl.DateTimeFormat.supportedLocalesOf(['fr', 'en-gb']).join()",
            "en-GB",
        ),
        ("new Intl.Locale('EN-latn-us').baseName", "en-Latn-US"),
        (
            "new Intl.Locale('en', { hourCycle: 'h23' }).toString()",
            "en-u-hc-h23",
        ),
        (
            "new Intl.DisplayNames('en', { type: 'region' }).of('de')",
            "Germany",
        ),
        ("Intl.getCanonicalLocales('EN-us').join()", "en-US"),
    ];
    for (src, want) in cases {
        assert_eq!(eval(&mut b, src), want, "{src}");
    }
}

#[test]
fn messaging_and_performance_observer_deliver_asynchronously() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv.url("/"));
    eval(
        &mut b,
        "window.log = [];\
         var ch = new MessageChannel();\
         ch.port2.onmessage = function (e) { log.push('port:' + e.data.n); };\
         ch.port1.postMessage({ n: 1 });\
         addEventListener('message', function (e) { log.push('window:' + e.data + ':' + (e.origin === location.origin)); });\
         postMessage('hi', '*');\
         postMessage('dropped', 'https://elsewhere.example');\
         new PerformanceObserver(function (list) {\
           log.push('observer:' + list.getEntries().map(e => e.name).join('+'));\
         }).observe({ type: 'mark' });\
         performance.mark('m1');\
         log.push('sync');",
    );
    idle(&mut b);
    let log = eval(&mut b, "log.slice().sort().join()");
    assert_eq!(log, "observer:m1,port:1,sync,window:hi:true");
    assert_eq!(eval(&mut b, "log[0]"), "sync");
}

#[test]
fn xml_http_request_runs_on_fetch() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv.url("/"));
    eval(
        &mut b,
        "window.log = [];\
         var x = new XMLHttpRequest();\
         x.onreadystatechange = function () { log.push('rs' + x.readyState); };\
         x.onload = function () { log.push('load:' + x.status + ':' + x.response.items.length\
             + ':' + x.getResponseHeader('X-Custom') + ':' + x.responseURL.endsWith('/data.json')); };\
         x.onloadend = function () { log.push('loadend'); };\
         x.open('GET', '/data.json');\
         x.responseType = 'json';\
         x.send();\
         var y = new XMLHttpRequest();\
         y.onload = function () { log.push('missing:' + y.status + ':' + y.responseText); };\
         y.open('GET', '/missing');\
         y.send();\
         var z = new XMLHttpRequest();\
         z.onabort = function () { log.push('abort'); };\
         z.open('GET', '/data.json'); z.send(); z.abort();",
    );
    idle(&mut b);
    let log = eval(&mut b, "log.join()");
    assert!(log.starts_with("rs1,abort"), "{log}");
    assert!(log.contains("rs2,rs3"), "{log}");
    assert!(log.contains("rs4,load:200:3:yes:true,loadend"), "{log}");
    assert!(log.contains("missing:404:gone"), "{log}");
    assert_eq!(
        eval(
            &mut b,
            "var s = new XMLHttpRequest(); s.open('GET', '/data.json', false);\
             try { s.send(); 'no' } catch (e) { e.name }"
        ),
        "NetworkError"
    );
}

#[test]
fn push_state_updates_url_and_traversal_fires_popstate() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv.url("/"));
    let base = srv.url("");
    // The tab's first entry is its start page.
    let start = ctx(&mut b).history.len();
    assert_eq!(eval(&mut b, "history.length"), start.to_string());
    assert_eq!(eval(&mut b, "history.state"), "null");
    eval(
        &mut b,
        "window.pops = [];\
         addEventListener('popstate', function (e) { pops.push(JSON.stringify(e.state) + '@' + location.pathname); });\
         history.pushState({ page: 1 }, '', '/one?x=1');",
    );
    assert_eq!(
        eval(&mut b, "location.pathname + location.search"),
        "/one?x=1"
    );
    assert_eq!(eval(&mut b, "history.length"), (start + 1).to_string());
    assert_eq!(eval(&mut b, "history.state.page"), "1");
    eval(&mut b, "history.pushState({ page: 2 }, '', 'two')");
    eval(&mut b, "history.replaceState({ page: 22 }, '')");
    idle(&mut b);
    assert_eq!(ctx(&mut b).page.url, format!("{base}/two"));
    assert_eq!(ctx(&mut b).history.len(), start + 2);
    assert_eq!(
        eval(&mut b, "history.length + ':' + history.state.page"),
        format!("{}:22", start + 2)
    );

    assert_eq!(
        eval(
            &mut b,
            "try { history.pushState(null, '', 'https://elsewhere.example/'); 'no' } catch (e) { e.name }"
        ),
        "SecurityError"
    );
    assert_eq!(
        eval(
            &mut b,
            "try { history.pushState(function () {}, ''); 'no' } catch (e) { e.name }"
        ),
        "DataCloneError"
    );
    assert_eq!(
        eval(
            &mut b,
            "try { history.pushState({}); 'no' } catch (e) { e.name }"
        ),
        "TypeError"
    );

    eval(&mut b, "history.back()");
    idle(&mut b);
    assert_eq!(eval(&mut b, "pops.join()"), "{\"page\":1}@/one");
    assert_eq!(ctx(&mut b).page.url, format!("{base}/one?x=1"));
    eval(&mut b, "history.go(-1)");
    idle(&mut b);
    eval(&mut b, "history.go(2)");
    idle(&mut b);
    assert_eq!(
        eval(&mut b, "pops.join(' ')"),
        "{\"page\":1}@/one null@/ {\"page\":22}@/two"
    );
    assert_eq!(eval(&mut b, "history.state.page"), "22");
    // Out-of-range traversals are ignored.
    eval(&mut b, "history.forward()");
    idle(&mut b);
    assert_eq!(eval(&mut b, "pops.length"), "3");
}

#[test]
fn fragment_navigation_scrolls_and_fires_hashchange() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    let url = srv.url("/");
    open(&mut b, &url);
    let start = ctx(&mut b).history.len();
    eval(
        &mut b,
        "window.hashes = [];\
         addEventListener('hashchange', function (e) {\
           hashes.push(e.oldURL.split('/').pop() + '>' + e.newURL.split('/').pop());\
         });\
         location.hash = 'target';",
    );
    idle(&mut b);
    assert_eq!(ctx(&mut b).page.url, format!("{url}#target"));
    assert_eq!(eval(&mut b, "hashes.join()"), ">#target");
    assert!(ctx(&mut b).page.scroll_y > 2000.0);
    assert_eq!(ctx(&mut b).history.len(), start + 1);
    // The document stayed: its script state survives.
    assert_eq!(eval(&mut b, "typeof hashes"), "object");

    // Same hash again: no new entry, no event.
    eval(&mut b, "location.hash = '#target'");
    idle(&mut b);
    assert_eq!(ctx(&mut b).history.len(), start + 1);

    ctx(&mut b).back();
    idle(&mut b);
    assert_eq!(ctx(&mut b).page.url, url);
    assert_eq!(eval(&mut b, "hashes.join()"), ">#target,#target>");
    assert_eq!(eval(&mut b, "location.hash"), "");

    // Link activation to a fragment of this document is a fragment navigation too.
    let jump = {
        let doc = ctx(&mut b).page.document.borrow();
        doc.get_element_by_id("jump").unwrap()
    };
    ctx(&mut b).click_node(jump);
    idle(&mut b);
    assert_eq!(ctx(&mut b).page.url, format!("{url}#target"));
    assert_eq!(ctx(&mut b).history.len(), start + 1);
    assert_eq!(eval(&mut b, "hashes.join()"), ">#target,#target>,>#target");
}

#[test]
fn location_navigates_the_document() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv.url("/"));
    let start = ctx(&mut b).history.len();
    eval(&mut b, "window.marker = 1; location.assign('/next')");
    idle(&mut b);
    assert_eq!(ctx(&mut b).page.url, srv.url("/next"));
    assert_eq!(eval(&mut b, "typeof marker"), "undefined");
    assert_eq!(ctx(&mut b).history.len(), start + 1);

    eval(&mut b, "location.replace('/replaced')");
    idle(&mut b);
    assert_eq!(ctx(&mut b).page.url, srv.url("/replaced"));
    assert_eq!(ctx(&mut b).history.len(), start + 1);

    eval(&mut b, "location.search = 'q=1'");
    idle(&mut b);
    assert_eq!(ctx(&mut b).page.url, srv.url("/replaced?q=1"));

    eval(&mut b, "window.location = '/assigned'");
    idle(&mut b);
    assert_eq!(ctx(&mut b).page.url, srv.url("/assigned"));

    assert_eq!(
        eval(
            &mut b,
            "try { location.href = 'http://['; 'no' } catch (e) { e.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        eval(
            &mut b,
            "try { location.assign('http://['); 'no' } catch (e) { e.name }"
        ),
        "SyntaxError"
    );
    eval(&mut b, "location.href = 'javascript:window.ran = 1'");
    idle(&mut b);
    assert_eq!(ctx(&mut b).page.url, srv.url("/assigned"));
    assert_eq!(
        eval(&mut b, "Object.prototype.toString.call(location) + ' ' + String(location).endsWith('/assigned')"),
        "[object Location] true"
    );

    // Back to a pushState entry of a document that is gone: loads it again.
    eval(&mut b, "history.pushState({ kept: true }, '', '/pushed')");
    eval(&mut b, "location.assign('/after')");
    idle(&mut b);
    ctx(&mut b).back();
    idle(&mut b);
    assert_eq!(ctx(&mut b).page.url, srv.url("/pushed"));
    assert_eq!(eval(&mut b, "history.state"), "null");

    eval(&mut b, "window.marker = 2; location.reload()");
    idle(&mut b);
    assert_eq!(eval(&mut b, "typeof marker"), "undefined");
}

#[test]
fn document_metadata_referrer_and_domain() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv.url("/"));
    assert_eq!(
        eval(
            &mut b,
            "[JSON.stringify(document.referrer), document.contentType, document.characterSet,\n\
             document.charset, document.compatMode, document.domain, document.designMode].join(' ')"
        ),
        r#""" text/html UTF-8 UTF-8 CSS1Compat 127.0.0.1 off"#
    );
    assert_eq!(
        eval(
            &mut b,
            r"/^\d\d\/\d\d\/\d{4} \d\d:\d\d:\d\d$/.test(document.lastModified)"
        ),
        "true"
    );
    assert_eq!(
        eval(&mut b, "document.domain = '127.0.0.1'; document.domain"),
        "127.0.0.1"
    );
    assert_eq!(
        eval(
            &mut b,
            "try { document.domain = 'example.com'; 'no' } catch (e) { e.name }"
        ),
        "SecurityError"
    );
    assert_eq!(
        eval(
            &mut b,
            "document.dir = 'RTL'; document.dir + ' ' + document.documentElement.getAttribute('dir')"
        ),
        "rtl RTL"
    );
    assert_eq!(
        eval(
            &mut b,
            "var i = document.createElement('input'); i.setAttribute('name', 'q');\n\
             document.body.append(i); var l = document.getElementsByName('q'); var n = l.length;\n\
             i.remove(); [n, l.length, l instanceof NodeList].join(' ')"
        ),
        "1 0 true"
    );

    // A script navigation sends the document as referrer (same-origin: the full URL).
    eval(&mut b, "location.assign('/legacy')");
    idle(&mut b);
    assert_eq!(eval(&mut b, "document.referrer"), srv.url("/"));
    assert_eq!(
        eval(
            &mut b,
            "[document.characterSet, document.compatMode, document.body.textContent].join(' ')"
        ),
        "windows-1252 BackCompat café"
    );
    assert_eq!(
        eval(&mut b, "document.lastModified.indexOf('/1994 ') > 0"),
        "true"
    );
}

#[test]
fn additional_documents_dom_parser_and_xml_serializer() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv.url("/"));
    let cases = [
        (
            "var d = document.implementation.createHTMLDocument('T');\n\
             [d instanceof HTMLDocument, d.title, d.body.nodeName, d.head.nodeName,\n\
              d.documentElement.nodeName, d.doctype.name, d.URL, d.readyState, d.contentType,\n\
              d.compatMode, String(d.defaultView), d.cookie === '', d !== document].join(' ')",
            "true T BODY HEAD HTML html about:blank complete text/html CSS1Compat null true true",
        ),
        (
            "d.title = 'New'; d.body.innerHTML = '<b>x</b>';\n\
             [d.title, d.querySelector('title').textContent, d.body.firstChild.ownerDocument === d].join(' ')",
            "New New true",
        ),
        // A node belongs to the document whose tree holds it, or that created or last
        // held its detached root.
        (
            "var p = d.createElement('p'); var r = [p.ownerDocument === d];\n\
             d.body.appendChild(p); r.push(p.ownerDocument === d, p.isConnected);\n\
             p.remove(); r.push(p.ownerDocument === d);\n\
             document.body.appendChild(p); r.push(p.ownerDocument === document, p.isConnected);\n\
             p.remove(); r.push(p.ownerDocument === document);\n\
             r.push(d.adoptNode(document.createElement('q')).ownerDocument === d,\n\
               d.importNode(document.body, false).ownerDocument === d,\n\
               document.body.ownerDocument === document, String(d.ownerDocument));\n\
             r.join(' ')",
            "true true false true true true true true true true null",
        ),
        (
            "var n = new Document(); var e = n.createElement('Foo');\n\
             var s = document.implementation.createDocument('http://www.w3.org/2000/svg', 'svg', null);\n\
             [Object.getPrototypeOf(n) === Document.prototype, n.contentType, e.localName,\n\
              String(e.namespaceURI), s instanceof XMLDocument, s.contentType,\n\
              s.documentElement.namespaceURI].join(' ')",
            "true application/xml Foo null true image/svg+xml http://www.w3.org/2000/svg",
        ),
        (
            "try { Object.getOwnPropertyDescriptor(Document.prototype, 'title').get.call({}); 'no' }\n\
             catch (e) { e.message }",
            "Illegal invocation",
        ),
        // Parsed HTML is inert: its scripts never run and its images never load.
        (
            "var h = new DOMParser().parseFromString('<p id=x>hi</p><script>window.ran = 1<\\/script>' +\n\
               '<img src=/nope.png onerror=\"window.ran = 2\">', 'text/html');\n\
             [h instanceof HTMLDocument, h.getElementById('x').textContent, h.scripts.length,\n\
              h.compatMode, h.URL === document.URL, h.body.firstChild.ownerDocument === h].join(' ')",
            "true hi 1 BackCompat true true",
        ),
        (
            "document.body.appendChild(h.querySelector('script')); typeof window.ran",
            "undefined",
        ),
        (
            "var x = new DOMParser().parseFromString(\n\
               '<r xmlns:a=\"urn:a\"><a:i n=\"1\">t</a:i><![CDATA[<c>]]></r>', 'application/xml');\n\
             var i = x.documentElement.firstChild;\n\
             [x instanceof XMLDocument, x.contentType, x.documentElement.nodeName, i.nodeName,\n\
              i.localName, i.prefix, i.namespaceURI, i.getAttribute('n'), x.documentElement.textContent,\n\
              x.documentElement.lastChild.nodeType, x.getElementsByTagName('parsererror').length].join(' ')",
            "true application/xml r a:i i a urn:a 1 t<c> 4 0",
        ),
        (
            "var bad = new DOMParser().parseFromString('<a><b></a>', 'text/xml');\n\
             [bad.documentElement.localName, bad.documentElement.namespaceURI,\n\
              bad.getElementsByTagName('parsererror').length].join(' ')",
            "parsererror http://www.mozilla.org/newlayout/xml/parsererror.xml 1",
        ),
        (
            "try { new DOMParser().parseFromString('', 'text/plain'); 'no' } catch (e) { e.name }",
            "TypeError",
        ),
        (
            "new XMLSerializer().serializeToString(x)",
            r#"<r xmlns:a="urn:a"><a:i n="1">t</a:i><![CDATA[<c>]]></r>"#,
        ),
        (
            "var div = document.createElement('div');\n\
             div.innerHTML = '<br><span title=\"a&quot;b\">x &amp; y</span><svg><circle r=\"1\"/></svg><!--c-->';\n\
             new XMLSerializer().serializeToString(div)",
            r#"<div xmlns="http://www.w3.org/1999/xhtml"><br /><span title="a&quot;b">x &amp; y</span><svg xmlns="http://www.w3.org/2000/svg"><circle r="1"/></svg><!--c--></div>"#,
        ),
        (
            "new XMLSerializer().serializeToString(new DOMParser().parseFromString(\n\
               '<svg xmlns=\"http://www.w3.org/2000/svg\"><g/></svg>', 'image/svg+xml'))",
            r#"<svg xmlns="http://www.w3.org/2000/svg"><g/></svg>"#,
        ),
    ];
    for (src, expected) in cases {
        assert_eq!(eval(&mut b, src), expected, "{src}");
    }
    idle(&mut b);
    assert_eq!(eval(&mut b, "typeof window.ran"), "undefined");
}

const SHADOW_PAGE: &str = "<!doctype html><html><head><style>\
    body { margin: 0 } p { color: rgb(255, 0, 0) } .light { font-size: 10px }\
    </style></head><body>\
    <div id=host><span class=light slot=title id=titled>T</span><b id=plain>B</b><i id=orphan slot=none>I</i></div>\
    <x-card id=card></x-card>\
    </body></html>";

#[test]
fn shadow_trees_scope_style_render_slots_and_retarget_events() {
    let dir = tempdir().unwrap();
    let srv = TestServer::spawn(|_| TestResponse::ok(SHADOW_PAGE, "text/html"));
    let mut b = browser(&dir);
    open(&mut b, &srv.url("/"));
    eval(
        &mut b,
        "window.root = host.attachShadow({ mode: 'open' });\
         root.innerHTML = '<style>:host { display: block; height: 80px } p { color: rgb(0, 128, 0) }\
           ::slotted(span) { font-size: 20px; font-style: italic } ::slotted(*) { display: block; height: 7px }</style>\
           <p id=inner>in</p><slot name=title></slot><slot id=rest>fallback</slot>';",
    );
    idle(&mut b);
    let cases = [
        (
            "[root instanceof ShadowRoot, root instanceof DocumentFragment, root.mode, root.host === host,\
              host.shadowRoot === root, String(root), root.nodeName, root.parentNode, root.delegatesFocus,\
              root.slotAssignment].join()",
            "true,true,open,true,true,[object ShadowRoot],#document-fragment,,false,named",
        ),
        (
            "var inner = root.getElementById('inner');\
             [document.getElementById('inner'), document.querySelector('p'), inner.isConnected,\
              inner.getRootNode() === root, inner.getRootNode({ composed: true }) === document,\
              inner.ownerDocument === document].join()",
            ",,true,true,true,true",
        ),
        // Outer rules stop at the shadow boundary; shadow rules and ::slotted apply, but
        // lose to outer normal declarations (CSS Scoping §3.3).
        ("getComputedStyle(inner).color", "rgb(0, 128, 0)"),
        ("getComputedStyle(titled).fontStyle", "italic"),
        ("getComputedStyle(titled).fontSize", "10px"),
        ("host.getBoundingClientRect().height", "80"),
        ("titled.getBoundingClientRect().height", "7"),
        // A light child no slot takes is not rendered.
        ("orphan.getBoundingClientRect().height", "0"),
        (
            "var slots = root.querySelectorAll('slot');\
             [slots[0].assignedNodes().map(n => n.id).join('+'), slots[1].assignedElements().map(n => n.id).join('+'),\
              titled.assignedSlot === slots[0], orphan.assignedSlot, slots[0].name, titled.slot].join()",
            "titled,plain,true,,title,title",
        ),
        (
            "try { host.attachShadow({ mode: 'open' }); 'no' } catch (e) { e.name }",
            "NotSupportedError",
        ),
        (
            "try { document.createElement('a').attachShadow({ mode: 'open' }); 'no' } catch (e) { e.name }",
            "NotSupportedError",
        ),
        (
            "try { document.createElement('div').attachShadow({ mode: 'sideways' }); 'no' } catch (e) { e.name }",
            "TypeError",
        ),
        (
            "var closedHost = document.createElement('section'); var closed = closedHost.attachShadow({ mode: 'closed' });\
             [closedHost.shadowRoot, closed.mode].join()",
            ",closed",
        ),
    ];
    for (src, want) in cases {
        assert_eq!(eval(&mut b, src), want, "{src}");
    }

    // Events: targets are retargeted to the host outside the tree; non-composed events
    // stop at the shadow root; slotted nodes bubble through their slot.
    assert_eq!(
        eval(
            &mut b,
            "window.log = [];\
             function note(where) { return function (e) { log.push(where + ':' + (e.target.id || e.target.nodeName)); }; }\
             document.body.addEventListener('ping', note('body'));\
             root.addEventListener('ping', note('root'));\
             host.addEventListener('ping', note('host'));\
             inner.dispatchEvent(new Event('ping', { bubbles: true, composed: true }));\
             inner.dispatchEvent(new Event('ping', { bubbles: true }));\
             log.join(' ')"
        ),
        "root:inner host:host body:host root:inner"
    );
    assert_eq!(
        eval(
            &mut b,
            "var path;\
             inner.addEventListener('probe', function (e) { path = e.composedPath().map(n => n.id || n.nodeName || String(n)).join('>'); });\
             var ev = new Event('probe', { bubbles: true, composed: true }); inner.dispatchEvent(ev);\
             [path, ev.target === host, ev.composedPath().length].join(' ')"
        ),
        "inner>#document-fragment>host>BODY>HTML>#document>[object Window] true 0"
    );
    assert_eq!(
        eval(
            &mut b,
            "var through = [];\
             slots[0].addEventListener('tap', function (e) { through.push('slot:' + e.target.id); });\
             root.addEventListener('tap', function (e) { through.push('root:' + e.target.id); });\
             titled.dispatchEvent(new Event('tap', { bubbles: true })); through.join(' ')"
        ),
        "slot:titled root:titled"
    );
    // Focus inside a shadow tree: document.activeElement is the host.
    assert_eq!(
        eval(
            &mut b,
            "root.innerHTML += '<input id=field>'; var field = root.getElementById('field'); field.focus();\
             [document.activeElement === host, root.activeElement === field].join()"
        ),
        "true,true"
    );

    // Custom elements attach their own shadow trees and upgrade inside them.
    eval(
        &mut b,
        "customElements.define('x-badge', class extends HTMLElement {\
           connectedCallback() { this.dataset.up = 'yes'; } });\
         customElements.define('x-card', class extends HTMLElement {\
           constructor() { super(); this.attachShadow({ mode: 'open' }).innerHTML = '<x-badge id=badge></x-badge>'; } });",
    );
    idle(&mut b);
    assert_eq!(
        eval(
            &mut b,
            "var badge = card.shadowRoot.getElementById('badge');\
             [badge.dataset.up, badge.matches(':defined'), card.matches(':defined'),\
              document.createElement('x-unknown').matches(':defined')].join()"
        ),
        "yes,true,true,false"
    );
}

/// React assigns `checked` on every input it creates, text fields included. That must not
/// leak into the field's value (GitHub's "Go to file" box once showed "off").
#[test]
fn checkedness_stays_out_of_text_values_and_html_element_hints_reflect() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv.url("/"));
    let cases = [
        (
            "var t = document.createElement('input'); document.body.appendChild(t); t.checked = false;\
             t.defaultValue = ''; [JSON.stringify(t.value), t.checked].join()",
            "\"\",false",
        ),
        (
            "var c = document.createElement('input'); c.type = 'checkbox'; document.body.appendChild(c);\
             c.checked = true; var on = c.value + c.checked; c.checked = false; [on, c.value, c.checked].join()",
            "ontrue,on,false",
        ),
        (
            "var r1 = document.createElement('input'), r2 = document.createElement('input');\
             [r1, r2].forEach(function (r) { r.type = 'radio'; r.name = 'g'; r.value = 'v'; document.body.appendChild(r); });\
             r1.checked = true; r2.checked = true; [r1.checked, r2.checked, r1.value].join()",
            "false,true,v",
        ),
        (
            "var d = document.createElement('div'); d.innerHTML = '<p spellcheck=false translate=no><span></span></p>';\
             var s = d.querySelector('span'); [s.spellcheck, s.translate, d.spellcheck, d.translate].join()",
            "false,false,true,true",
        ),
        (
            "var e = document.createElement('input'); e.spellcheck = false; e.translate = false;\
             [e.getAttribute('spellcheck'), e.getAttribute('translate')].join()",
            "false,no",
        ),
        (
            "var a = document.createElement('input');\
             ['', 'off', 'on', 'words', 'bogus'].map(function (v) {\
               if (v) a.setAttribute('autocapitalize', v); return a.autocapitalize; }).join()",
            ",none,sentences,words,sentences",
        ),
        (
            "var k = document.createElement('input'); k.setAttribute('enterkeyhint', 'SEND'); var x = k.enterKeyHint;\
             k.enterKeyHint = 'nope'; [x, k.enterKeyHint, k.getAttribute('enterkeyhint')].join()",
            "send,,nope",
        ),
        (
            "var img = document.createElement('img'), link = document.createElement('a'), div = document.createElement('div');\
             link.href = '/'; var before = [img.draggable, link.draggable, div.draggable];\
             div.draggable = true; img.draggable = false; before.concat([div.draggable, img.draggable]).join()",
            "true,true,false,true,false",
        ),
    ];
    for (src, want) in cases {
        assert_eq!(eval(&mut b, src), want, "{src}");
    }
}
