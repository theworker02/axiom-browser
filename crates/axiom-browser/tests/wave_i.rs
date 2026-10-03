//! Wave I — Content Security Policy end to end: header and `<meta>` policies, nonces,
//! hashes and `'strict-dynamic'`, inline script/style/attribute/handler checks, eval,
//! `connect-src` for `fetch()`, redirect guards, `form-action`, `base-uri`,
//! `upgrade-insecure-requests`, report-only policies and `securitypolicyviolation`
//! events. Every server is local; a second server on another port is cross-origin.

use std::time::Duration;

use axiom_browser::Browser;
use axiom_csp::hash::HashAlgorithm;
use axiom_csp::Disposition;
use axiom_engine::{BrowsingContext, ResourceDiagnostics};
use axiom_net::test_server::{TestResponse, TestServer};
use tempfile::{tempdir, TempDir};

fn browser(dir: &TempDir) -> Browser {
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    ctx(&mut b).set_chrome_height(0);
    b
}

fn ctx(b: &mut Browser) -> &mut BrowsingContext {
    &mut b.window.tabs.active_tab_mut().context
}

/// A page whose nonced setup script defines `log` and records every
/// `securitypolicyviolation` event as `csp:<effective directive>:<disposition>`.
fn html(head: &str, body: &str) -> String {
    format!(
        "<!doctype html><html><head><title>page</title>\
         <script nonce=n1>var log = [];\n\
         document.addEventListener('securitypolicyviolation', function (e) {{\n\
           log.push('csp:' + e.effectiveDirective + ':' + e.disposition);\n\
         }});</script>{head}</head><body>{body}</body></html>"
    )
}

fn page(csp: &str, head: &str, body: &str) -> TestResponse {
    TestResponse::ok(html(head, body), "text/html")
        .with_header("Cache-Control", "no-store")
        .with_header("Content-Security-Policy", csp)
}

fn js_response(body: &str) -> TestResponse {
    TestResponse::ok(body, "text/javascript").with_header("Cache-Control", "no-store")
}

fn eval(b: &mut Browser, src: &str) -> String {
    let js = ctx(b).page.js.as_mut().expect("page has js");
    js.eval(src)
        .map(|v| v.display)
        .unwrap_or_else(|e| format!("<{e}>"))
}

fn open(b: &mut Browser, url: &str) -> Vec<String> {
    b.navigate_resolved(url);
    settle(b)
}

fn settle(b: &mut Browser) -> Vec<String> {
    assert!(
        ctx(b).run_until_idle(Duration::from_secs(15)),
        "page never went idle"
    );
    serde_json::from_str(&eval(b, "JSON.stringify(log)")).unwrap()
}

fn origin_of(srv: &TestServer) -> String {
    srv.url("").trim_end_matches('/').to_string()
}

fn resource(b: &mut Browser, suffix: &str) -> ResourceDiagnostics {
    ctx(b)
        .resource_diagnostics()
        .into_iter()
        .find(|r| r.url.ends_with(suffix))
        .unwrap_or_else(|| panic!("no resource ending with {suffix}"))
}

fn assert_blocked_by_csp(b: &mut Browser, suffix: &str) {
    let r = resource(b, suffix);
    assert_eq!(r.state, "FAILED", "{suffix}");
    let why = r.failure.unwrap_or_default();
    assert!(why.contains("csp:"), "{suffix}: {why}");
}

fn has(log: &[String], entry: &str) -> bool {
    log.iter().any(|e| e == entry)
}

fn count(log: &[String], entry: &str) -> usize {
    log.iter().filter(|e| *e == entry).count()
}

fn color(b: &mut Browser, selector: &str) -> (u8, u8, u8) {
    let c = ctx(b);
    let node = c
        .page
        .document
        .borrow()
        .query_selector(selector)
        .unwrap_or_else(|| panic!("{selector}"));
    let render = c.page.render();
    let style = render.styles.get(node).expect("styled");
    (style.color.r, style.color.g, style.color.b)
}

#[test]
fn script_src_admits_nonces_and_listed_sources_only() {
    let other = TestServer::spawn(|req| js_response(&format!("log.push('{}');", req.path)));
    let o = origin_of(&other);
    let csp = format!("script-src 'nonce-n1' {o}/allowed/");
    let head = format!(
        r#"<script nonce=n1>log.push('nonced');</script>
           <script>log.push('bare');</script>
           <script nonce=wrong>log.push('wrong nonce');</script>
           <script src="{o}/allowed/a.js"></script>
           <script src="{o}/denied/b.js"></script>
           <script nonce=n1 src="{o}/denied/c.js"></script>"#
    );
    let srv = TestServer::spawn(move |_| page(&csp, &head, ""));
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    let log = open(&mut b, &srv.url("/"));

    for want in ["nonced", "/allowed/a.js", "/denied/c.js"] {
        assert!(has(&log, want), "missing {want}: {log:?}");
    }
    for never in ["bare", "wrong nonce", "/denied/b.js"] {
        assert!(!has(&log, never), "unexpected {never}: {log:?}");
    }
    assert!(other.requests_for("/denied/b.js").is_empty());
    assert_blocked_by_csp(&mut b, "/denied/b.js");
    assert_eq!(count(&log, "csp:script-src-elem:enforce"), 3, "{log:?}");

    let violations = ctx(&mut b).csp_violations();
    assert_eq!(violations.len(), 3);
    assert!(violations
        .iter()
        .any(|v| v.blocked_uri.ends_with("/denied/b.js")));
    assert!(violations.iter().any(|v| v.blocked_uri == "inline"));
}

#[test]
fn strict_dynamic_trusts_scripts_loaded_by_trusted_scripts() {
    let other = TestServer::spawn(|req| js_response(&format!("log.push('{}');", req.path)));
    let o = origin_of(&other);
    // Host sources are ignored under 'strict-dynamic'.
    let csp = format!("script-src 'nonce-n1' 'strict-dynamic' {o}");
    let head = format!(
        r#"<script nonce=n1>
             var s = document.createElement('script');
             s.src = '{o}/dynamic.js';
             document.head.appendChild(s);
           </script>
           <script src="{o}/parser.js"></script>"#
    );
    let srv = TestServer::spawn(move |_| page(&csp, &head, ""));
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    let log = open(&mut b, &srv.url("/"));

    assert!(has(&log, "/dynamic.js"), "{log:?}");
    assert!(!has(&log, "/parser.js"), "{log:?}");
    assert!(other.requests_for("/parser.js").is_empty());
    assert_blocked_by_csp(&mut b, "/parser.js");
}

#[test]
fn inline_styles_style_attributes_handlers_and_eval_are_gated() {
    let csp = "default-src 'self'; script-src 'nonce-n1'; style-src 'nonce-n1'";
    let head = r#"<style>#b { color: rgb(255, 0, 0); }</style>
           <style nonce=n1>#c { color: rgb(0, 0, 255); }</style>
           <script nonce=n1>
             try { eval('1 + 1'); log.push('eval ran'); } catch (e) { log.push('eval:' + e.name); }
             try { new Function('return 1'); log.push('Function ran'); }
             catch (e) { log.push('Function:' + e.name); }
             window.addEventListener('load', function () { log.push('listener'); });
           </script>"#;
    let body = r#"<p id=a style="color: rgb(255, 0, 0)">a</p><p id=b>b</p><p id=c>c</p>
           <button id=h onclick="log.push('handler')">h</button>"#;
    let srv = TestServer::spawn(move |_| page(csp, head, body));
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/"));
    settle(&mut b);
    let node = ctx(&mut b)
        .page
        .document
        .borrow()
        .query_selector("#h")
        .unwrap();
    b.click_node(node);
    let log = settle(&mut b);

    assert_eq!(color(&mut b, "#a"), (0, 0, 0), "style attribute applied");
    assert_eq!(color(&mut b, "#b"), (0, 0, 0), "un-nonced <style> applied");
    assert_eq!(color(&mut b, "#c"), (0, 0, 255), "nonced <style> ignored");
    for want in ["eval:EvalError", "Function:EvalError", "listener"] {
        assert!(has(&log, want), "missing {want}: {log:?}");
    }
    for never in ["eval ran", "Function ran", "handler"] {
        assert!(!has(&log, never), "unexpected {never}: {log:?}");
    }
    for directive in [
        "style-src-attr",
        "style-src-elem",
        "script-src",
        "script-src-attr",
    ] {
        let entry = format!("csp:{directive}:enforce");
        assert!(has(&log, &entry), "missing {entry}: {log:?}");
    }
    // One report per blocked element, however often styles are recomputed.
    assert_eq!(count(&log, "csp:style-src-attr:enforce"), 1, "{log:?}");
    assert_eq!(count(&log, "csp:style-src-elem:enforce"), 1, "{log:?}");
}

#[test]
fn hashes_allow_matching_inline_scripts_and_hashed_attributes() {
    let script = "log.push('hashed');";
    let attr = "color: rgb(0, 0, 255)";
    let sha = |s: &str| HashAlgorithm::Sha256.digest_base64(s.as_bytes());
    let csp = format!(
        "script-src 'nonce-n1' 'sha256-{}'; style-src 'unsafe-hashes' 'sha256-{}'",
        sha(script),
        sha(attr)
    );
    let head = format!("<script>{script}</script><script>log.push('unhashed');</script>");
    let body = format!(r#"<p id=a style="{attr}">a</p><p id=b style="color: red">b</p>"#);
    let srv = TestServer::spawn(move |_| page(&csp, &head, &body));
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    let log = open(&mut b, &srv.url("/"));

    assert!(has(&log, "hashed"), "{log:?}");
    assert!(!has(&log, "unhashed"), "{log:?}");
    assert_eq!(color(&mut b, "#a"), (0, 0, 255));
    assert_eq!(color(&mut b, "#b"), (0, 0, 0));
}

#[test]
fn meta_policies_apply_from_where_they_appear() {
    let head = r#"<script>log.push('before');</script>
           <meta http-equiv="Content-Security-Policy" content="script-src 'nonce-n1'">
           <script>log.push('after');</script>
           <script nonce=n1>log.push('nonced');</script>"#;
    let body = r#"<meta http-equiv="Content-Security-Policy" content="script-src 'none'">
           <script nonce=n1>log.push('body meta ignored');</script>"#;
    let srv = TestServer::spawn(move |_| {
        TestResponse::ok(html(head, body), "text/html").with_header("Cache-Control", "no-store")
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    let log = open(&mut b, &srv.url("/"));

    for want in ["before", "nonced", "body meta ignored"] {
        assert!(has(&log, want), "missing {want}: {log:?}");
    }
    assert!(!has(&log, "after"), "{log:?}");
    assert_eq!(ctx(&mut b).csp_violations().len(), 1);
}

#[test]
fn report_only_policies_report_without_blocking() {
    let srv = TestServer::spawn(|_| {
        TestResponse::ok(html("<script>log.push('ran');</script>", ""), "text/html")
            .with_header("Content-Security-Policy-Report-Only", "script-src 'none'")
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    let log = open(&mut b, &srv.url("/"));

    assert!(has(&log, "ran"), "{log:?}");
    // The setup script is reported too; reports are delivered after it ran.
    assert_eq!(count(&log, "csp:script-src-elem:report"), 2, "{log:?}");
    let violations = ctx(&mut b).csp_violations();
    assert_eq!(violations.len(), 2);
    assert!(violations
        .iter()
        .all(|v| v.disposition == Disposition::Report));
    assert!(violations[0].message().starts_with("[Report Only] "));
}

#[test]
fn connect_src_gates_fetch_including_redirects() {
    let other = TestServer::spawn(|_| {
        TestResponse::ok("secret", "text/plain").with_header("Access-Control-Allow-Origin", "*")
    });
    let o = origin_of(&other);
    let head = format!(
        r#"<script nonce=n1>
             function report(label, p) {{
               return p.then(function (r) {{ log.push(label + ':' + r.status); }},
                             function (e) {{ log.push(label + ':' + e.name); }});
             }}
             report('self', fetch('/data'))
               .then(function () {{ return report('cross', fetch('{o}/x')); }})
               .then(function () {{ return report('bounce', fetch('/bounce')); }})
               .then(function () {{ log.push('done'); }});
           </script>"#
    );
    let target = format!("{o}/y");
    let srv = TestServer::spawn(move |req| match req.path.as_str() {
        "/data" => TestResponse::ok("ok", "text/plain"),
        "/bounce" => TestResponse::redirect(302, &target),
        _ => page("connect-src 'self'; script-src 'nonce-n1'", &head, ""),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    let log = open(&mut b, &srv.url("/"));

    assert!(has(&log, "done"), "{log:?}");
    for want in ["self:200", "cross:TypeError", "bounce:TypeError"] {
        assert!(has(&log, want), "missing {want}: {log:?}");
    }
    assert!(
        other.requests().is_empty(),
        "the other origin was contacted"
    );
    assert_eq!(srv.requests_for("/bounce").len(), 1);
    assert_eq!(count(&log, "csp:connect-src:enforce"), 2, "{log:?}");
}

#[test]
fn redirects_of_subresources_are_checked_against_the_policy() {
    let other = TestServer::spawn(|_| TestResponse::ok("p { color: red; }", "text/css"));
    let target = format!("{}/evil.css", origin_of(&other));
    let srv = TestServer::spawn(move |req| match req.path.as_str() {
        "/bounce.css" => TestResponse::redirect(302, &target),
        "/ok.css" => TestResponse::ok("#ok { color: rgb(0, 0, 255); }", "text/css"),
        _ => page(
            "style-src 'self'; script-src 'nonce-n1'",
            r#"<link rel=stylesheet href="/bounce.css"><link rel=stylesheet href="/ok.css">"#,
            "<p id=ok>ok</p>",
        ),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    let log = open(&mut b, &srv.url("/"));

    assert!(other.requests().is_empty(), "the redirect was followed");
    assert_blocked_by_csp(&mut b, "/bounce.css");
    assert_eq!(resource(&mut b, "/ok.css").state, "READY");
    assert_eq!(color(&mut b, "#ok"), (0, 0, 255));
    assert_eq!(count(&log, "csp:style-src-elem:enforce"), 1, "{log:?}");
    // The report names only the redirect target's origin.
    let v = ctx(&mut b).csp_violations();
    assert_eq!(v[0].blocked_uri, origin_of(&other));
}

#[test]
fn form_action_restricts_submission_targets_and_their_redirects() {
    let other = TestServer::spawn(|_| TestResponse::ok("<title>other</title>", "text/html"));
    let elsewhere = format!("{}/landing", origin_of(&other));
    let srv = TestServer::spawn(move |req| {
        let path = req.path.split('?').next().unwrap_or("/");
        let form = |action: &str| {
            page(
                "form-action 'self'; script-src 'nonce-n1'",
                "",
                &format!("<form action='{action}'><button id=go>Go</button></form>"),
            )
        };
        match path {
            "/cross" => form(&elsewhere),
            "/via-redirect" => form("/redirect"),
            "/same" => form("/submit"),
            "/redirect" => TestResponse::redirect(303, &elsewhere),
            _ => TestResponse::ok("<title>submitted</title>", "text/html"),
        }
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    let click = |b: &mut Browser| {
        let node = ctx(b).page.document.borrow().query_selector("#go").unwrap();
        b.click_node(node);
        assert!(ctx(b).run_until_idle(Duration::from_secs(15)));
    };

    open(&mut b, &srv.url("/cross"));
    click(&mut b);
    assert_eq!(
        ctx(&mut b).page.title,
        "page",
        "blocked submission navigated"
    );

    open(&mut b, &srv.url("/via-redirect"));
    click(&mut b);
    let redirects = srv.requests();
    let redirects = redirects.iter().filter(|r| r.path.starts_with("/redirect"));
    assert_eq!(redirects.count(), 1);
    assert_ne!(ctx(&mut b).page.title, "other");

    assert!(
        other.requests().is_empty(),
        "the other origin was contacted"
    );

    open(&mut b, &srv.url("/same"));
    click(&mut b);
    assert_eq!(ctx(&mut b).page.title, "submitted");
}

#[test]
fn base_uri_rejects_disallowed_base_elements() {
    let other = TestServer::spawn(|_| TestResponse::ok("", "text/css"));
    let o = origin_of(&other);
    let head = format!(r#"<base href="{o}/"><link rel=stylesheet href="rel.css">"#);
    let srv = TestServer::spawn(move |req| match req.path.as_str() {
        "/" => page(
            "base-uri 'self'; script-src 'nonce-n1'",
            &head,
            "<form action=submit><button id=go>Go</button></form>",
        ),
        "/rel.css" => TestResponse::ok("", "text/css"),
        _ => TestResponse::ok("<title>submitted</title>", "text/html"),
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    let log = open(&mut b, &srv.url("/"));

    assert_eq!(srv.requests_for("/rel.css").len(), 1);
    assert!(has(&log, "csp:base-uri:enforce"), "{log:?}");

    // Form actions resolve against the same (unchanged) base.
    let node = ctx(&mut b)
        .page
        .document
        .borrow()
        .query_selector("#go")
        .unwrap();
    b.click_node(node);
    assert!(ctx(&mut b).run_until_idle(Duration::from_secs(15)));
    assert_eq!(ctx(&mut b).page.title, "submitted");
    assert!(other.requests().is_empty(), "the blocked base was used");
}

#[test]
fn upgrade_insecure_requests_rewrites_subresource_urls() {
    let other = TestServer::spawn(|_| TestResponse::ok("", "text/css"));
    let o = origin_of(&other);
    let head = format!(r#"<link rel=stylesheet href="{o}/plain.css">"#);
    let srv = TestServer::spawn(move |_| {
        page(
            "upgrade-insecure-requests; script-src 'nonce-n1'",
            &head,
            "",
        )
    });
    let dir = tempdir().unwrap();
    let mut b = browser(&dir);
    open(&mut b, &srv.url("/"));

    let r = resource(&mut b, "/plain.css");
    assert!(r.url.starts_with("https://"), "{}", r.url);
    assert!(other.requests_for("/plain.css").is_empty());
}
