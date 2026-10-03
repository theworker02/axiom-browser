//! ES module scripts: static graphs, `import.meta`, ordering with `defer` scripts,
//! dynamic `import()`, script-inserted modules and the error paths.

use std::time::{Duration, Instant};

use axiom_browser::Browser;
use axiom_net::test_server::{TestResponse, TestServer};
use tempfile::{tempdir, TempDir};

fn browser(dir: &TempDir) -> Browser {
    Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap()
}

fn js(body: &str) -> TestResponse {
    TestResponse::ok(body, "text/javascript").with_header("Cache-Control", "no-store")
}

fn html(body: &str) -> TestResponse {
    let page = format!(
        "<!doctype html><html><head><script>var log = []; var done = false;\n\
         function fail(e) {{ log.push('error:' + (e && e.name) + ':' + (e && e.message)); done = true; }}\n\
         </script>{body}</head><body></body></html>"
    );
    TestResponse::ok(page, "text/html").with_header("Cache-Control", "no-store")
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

#[test]
fn module_graphs_run_in_order_with_defer_scripts() {
    let srv = TestServer::spawn(|req| {
        match req.path.as_str() {
        "/main.js" => js("import { a } from './lib/a.js';\n\
             import b from '/b.js';\n\
             log.push('main:' + a + b + ':' + import.meta.url.endsWith('/main.js') + ':' + document.readyState);"),
        "/lib/a.js" => js("import './shared.js';\nexport const a = 'A';"),
        "/lib/shared.js" => js("log.push('shared');"),
        "/b.js" => js("import './lib/shared.js';\nexport default 'B';"),
        "/defer.js" => js("log.push('defer');"),
        _ => html(
            r#"<script type="module" src="/main.js"></script>
               <script type="module">log.push('inline:' + (typeof a));</script>
               <script defer src="/defer.js"></script>
               <script>
                 log.push('classic');
                 document.addEventListener('DOMContentLoaded', function () { log.push('dcl'); });
                 window.addEventListener('load', function () { log.push('load'); done = true; });
               </script>"#,
        ),
    }
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    assert_eq!(
        run(&mut b),
        r#"["classic","shared","main:AB:true:interactive","inline:undefined","defer","dcl","load"]"#
    );
    // One module map entry per URL: the shared dependency is fetched once.
    assert_eq!(srv.requests_for("/lib/shared.js").len(), 1);
}

#[test]
fn dynamic_import_and_script_inserted_modules() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/dyn.js" => js("log.push('dyn-eval');\nexport const value = 42;"),
        "/async.js" => js("log.push('async');"),
        _ => html(
            r#"<script type="module" async src="/async.js"></script>
               <script>
                 import('/dyn.js')
                   .then(function (m) { log.push('dyn:' + m.value); return import('./dyn.js'); })
                   .then(function (m) {
                     log.push('same:' + m.value);
                     var s = document.createElement('script');
                     s.type = 'module';
                     s.textContent = "import { value } from '/dyn.js'; log.push('inserted:' + value); done = true;";
                     document.body.appendChild(s);
                   }, fail);
               </script>"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    let log = run(&mut b);
    assert_eq!(
        eval(
            &mut b,
            "JSON.stringify(log.filter(function (e) { return e !== 'async'; }))"
        ),
        r#"["dyn-eval","dyn:42","same:42","inserted:42"]"#,
        "{log}"
    );
    assert_eq!(eval(&mut b, "log.indexOf('async') >= 0"), "true", "{log}");
    assert_eq!(srv.requests_for("/dyn.js").len(), 1);
}

#[test]
fn import_maps_resolve_bare_specifiers() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/vendor/lib.js" => js("export default 'L';"),
        "/packages/x.js" => js("export const x = 'X';"),
        _ => html(
            r#"<script>window.onerror = function (msg) { log.push('onerror:' + msg); };</script>
               <script type="importmap">{"imports": {"lib": "/vendor/lib.js", "pkg/": "/packages/"}}</script>
               <script type="importmap">{ not json</script>
               <script type="module">
                 import lib from 'lib';
                 import { x } from 'pkg/x.js';
                 log.push(lib + x);
                 import('lib').then(function (m) { log.push('dyn:' + m.default); done = true; }, fail);
               </script>"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    let log = run(&mut b);
    assert!(
        log.starts_with(r#"["onerror:Uncaught SyntaxError: invalid import map JSON"#),
        "{log}"
    );
    assert!(log.ends_with(r#""LX","dyn:L"]"#), "{log}");
}

#[test]
fn module_failures_are_reported_the_right_way() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/uses-missing.js" => js("import './missing.js'; log.push('never:missing');"),
        "/missing.js" => TestResponse::status(404, "not found"),
        "/uses-text.js" => js("import './text.js'; log.push('never:text');"),
        "/text.js" => TestResponse::ok("log.push('never:plain');", "text/plain"),
        "/uses-bad.js" => js("import './bad.js'; log.push('never:bad');"),
        "/bad.js" => js("export const = ;"),
        _ => html(
            r#"<script>
                 window.onerror = function (msg) { log.push('onerror:' + msg); };
               </script>
               <script type="module">import 'lodash'; log.push('never:bare');</script>
               <script type="module" src="/uses-missing.js"
                       onerror="log.push('missing:error')" onload="log.push('missing:load')"></script>
               <script type="module" src="/uses-text.js" onerror="log.push('mime:error')"></script>
               <script type="module" src="/uses-bad.js" onload="log.push('bad:load')"></script>
               <script type="module">throw new RangeError('boom');</script>
               <script>window.addEventListener('load', function () { done = true; });</script>"#,
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    let log = run(&mut b);
    let has = |needle: &str| log.contains(needle);
    assert!(!has("never:"), "{log}");
    assert!(
        has(r#"onerror:Uncaught TypeError: Failed to resolve module specifier \"lodash\""#),
        "{log}"
    );
    assert!(has("missing:error") && !has("missing:load"), "{log}");
    assert!(has("mime:error"), "{log}");
    assert!(
        has("onerror:Uncaught SyntaxError") && has("bad:load"),
        "{log}"
    );
    assert!(has("onerror:Uncaught RangeError: boom"), "{log}");
}
