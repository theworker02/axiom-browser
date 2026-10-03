//! Form submission over the network: GET queries and POST bodies in all three encodings,
//! the `Origin` header, implicit submission, the POST resubmission guard on reload and
//! history, and SameSite cookies on cross-site submissions. The server is local; no test
//! depends on an external site.

use std::time::Duration;

use axiom_browser::Browser;
use axiom_engine::BrowsingContext;
use axiom_net::test_server::{TestRequest, TestResponse, TestServer};
use tempfile::tempdir;

fn ctx(b: &mut Browser) -> &mut BrowsingContext {
    &mut b.window.tabs.active_tab_mut().context
}

fn idle(b: &mut Browser) {
    assert!(
        ctx(b).run_until_idle(Duration::from_secs(15)),
        "page never went idle"
    );
}

fn query_param<'a>(path: &'a str, key: &str) -> Option<&'a str> {
    path.split_once('?')?
        .1
        .split('&')
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

/// `/form?method=..&enctype=..&action=..` is a page with one form; `action` may be `here`
/// (`/submit`) or `ip` (`/submit` on 127.0.0.1, a different site than `localhost`).
fn routes(req: &TestRequest) -> TestResponse {
    let path = req.path.split('?').next().unwrap_or("/");
    let html = |body: &str| TestResponse::ok(body.to_string(), "text/html; charset=utf-8");
    match path {
        "/form" => {
            let port = req
                .header("host")
                .and_then(|h| h.rsplit_once(':'))
                .map_or("", |(_, p)| p);
            let method = query_param(&req.path, "method").unwrap_or("get");
            let enctype = query_param(&req.path, "enctype")
                .unwrap_or("application/x-www-form-urlencoded")
                .replace("%2F", "/");
            let action = match query_param(&req.path, "action") {
                Some("ip") => format!("http://127.0.0.1:{port}/submit"),
                Some("redirect") => "/redirect".to_string(),
                _ => "/submit".to_string(),
            };
            html(&format!(
                "<!doctype html><title>form</title>\
                 <form id=f method={method} enctype='{enctype}' action='{action}'>\
                 <input id=q name=q value='a b&amp;c'>\
                 <input type=checkbox name=c checked><input type=checkbox name=off>\
                 <select name=s><option>one<option selected>two</select>\
                 <button id=go name=go value=1>Go</button></form>"
            ))
        }
        "/search" => html(
            "<!doctype html><title>search</title>\
             <form action=/submit><input id=q name=q></form>",
        ),
        "/set-cookies" => html("<!doctype html><title>cookies</title>")
            .with_header("Set-Cookie", "lax=1; Path=/; SameSite=Lax")
            .with_header("Set-Cookie", "strict=1; Path=/; SameSite=Strict"),
        "/redirect" => TestResponse::redirect(303, "/done"),
        _ => html("<!doctype html><title>done</title><p>ok"),
    }
}

fn browser() -> (Browser, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 400, 300).unwrap();
    ctx(&mut b).set_chrome_height(0);
    (b, dir)
}

fn open(b: &mut Browser, url: &str) {
    b.navigate_resolved(url);
    idle(b);
}

fn click(b: &mut Browser, selector: &str) {
    let node = ctx(b)
        .page
        .document
        .borrow()
        .query_selector(selector)
        .unwrap_or_else(|| panic!("{selector}"));
    b.click_node(node);
    idle(b);
}

fn submits(srv: &TestServer) -> Vec<TestRequest> {
    srv.requests()
        .into_iter()
        .filter(|r| r.path.starts_with("/submit"))
        .collect()
}

#[test]
fn get_and_post_submissions_send_the_encoded_entry_list() {
    let srv = TestServer::spawn(routes);
    let (mut b, _dir) = browser();
    let origin = srv.url("");

    open(&mut b, &srv.url("/form"));
    click(&mut b, "#go");
    let get = submits(&srv).pop().expect("GET submission");
    assert_eq!(get.method, "GET");
    assert_eq!(get.path, "/submit?q=a+b%26c&c=on&s=two&go=1");
    assert_eq!(get.header("origin"), None, "GET navigations send no Origin");
    assert_eq!(ctx(&mut b).page.title, "done");
    assert_eq!(
        ctx(&mut b).page.url,
        srv.url("/submit?q=a+b%26c&c=on&s=two&go=1")
    );

    open(&mut b, &srv.url("/form?method=post"));
    click(&mut b, "#go");
    let post = submits(&srv).pop().expect("urlencoded POST");
    assert_eq!(post.method, "POST");
    assert_eq!(post.path, "/submit");
    assert_eq!(
        post.header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(post.body, b"q=a+b%26c&c=on&s=two&go=1");
    assert_eq!(post.header("origin"), Some(origin.as_str()));

    open(&mut b, &srv.url("/form?method=post&enctype=text%2Fplain"));
    click(&mut b, "#go");
    let plain = submits(&srv).pop().expect("text/plain POST");
    assert_eq!(plain.header("content-type"), Some("text/plain"));
    assert_eq!(plain.body, b"q=a b&c\r\nc=on\r\ns=two\r\ngo=1\r\n");

    open(
        &mut b,
        &srv.url("/form?method=post&enctype=multipart%2Fform-data"),
    );
    click(&mut b, "#go");
    let multi = submits(&srv).pop().expect("multipart POST");
    let content_type = multi.header("content-type").unwrap_or_default();
    let boundary = content_type
        .strip_prefix("multipart/form-data; boundary=")
        .expect("multipart content type");
    let body = String::from_utf8(multi.body.clone()).unwrap();
    assert!(
        body.starts_with(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"q\"\r\n\r\na b&c\r\n"
        )),
        "{body}"
    );
    assert!(body.ends_with(&format!("--{boundary}--\r\n")), "{body}");
}

#[test]
fn enter_in_a_lone_field_submits_implicitly() {
    let srv = TestServer::spawn(routes);
    let (mut b, _dir) = browser();
    open(&mut b, &srv.url("/search"));
    click(&mut b, "#q");
    for key in ["h", "i", " ", "x"] {
        ctx(&mut b).handle_key(key);
    }
    ctx(&mut b).handle_key("Enter");
    idle(&mut b);
    let get = submits(&srv).pop().expect("implicit submission");
    assert_eq!(get.path, "/submit?q=hi+x");
    assert_eq!(ctx(&mut b).page.title, "done");
}

#[test]
fn post_results_are_never_resent_by_reload_or_history() {
    let srv = TestServer::spawn(routes);
    let (mut b, _dir) = browser();
    let posts = |srv: &TestServer| submits(srv).iter().filter(|r| r.method == "POST").count();

    open(&mut b, &srv.url("/form?method=post"));
    click(&mut b, "#go");
    assert_eq!(posts(&srv), 1);

    ctx(&mut b).reload();
    idle(&mut b);
    assert_eq!(posts(&srv), 1, "reload must not resend the form");
    assert_eq!(ctx(&mut b).page.title, "Form data not resent");
    ctx(&mut b).reload();
    idle(&mut b);
    assert_eq!(
        posts(&srv),
        1,
        "reloading the guard page does not resend either"
    );

    b.back();
    idle(&mut b);
    assert_eq!(ctx(&mut b).page.title, "form");
    b.forward();
    idle(&mut b);
    assert_eq!(
        posts(&srv),
        1,
        "forward onto a POST result must not resend it"
    );
    assert_eq!(ctx(&mut b).page.title, "Form data not resent");

    // Post/redirect/get: the result is a GET document, so reload may fetch it again.
    open(&mut b, &srv.url("/form?method=post&action=redirect"));
    click(&mut b, "#go");
    assert_eq!(srv.requests_for("/redirect").len(), 1);
    assert_eq!(ctx(&mut b).page.url, srv.url("/done"));
    ctx(&mut b).reload();
    idle(&mut b);
    assert_eq!(srv.requests_for("/redirect").len(), 1);
    assert_eq!(srv.requests_for("/done").len(), 2);
    assert_eq!(ctx(&mut b).page.title, "done");
}

#[test]
fn cross_site_submissions_withhold_same_site_cookies() {
    let srv = TestServer::spawn(routes);
    let (mut b, _dir) = browser();
    let cookies = |srv: &TestServer| {
        submits(srv)
            .pop()
            .expect("submission")
            .header("cookie")
            .unwrap_or_default()
            .to_string()
    };
    let other_site = format!("http://localhost:{}", srv.port());

    open(&mut b, &srv.url("/set-cookies"));

    open(&mut b, &srv.url("/form?method=post"));
    click(&mut b, "#go");
    let same_site = cookies(&srv);
    assert!(
        same_site.contains("lax=1") && same_site.contains("strict=1"),
        "same-site POST: {same_site:?}"
    );

    open(&mut b, &format!("{other_site}/form?method=post&action=ip"));
    click(&mut b, "#go");
    let cross_post = cookies(&srv);
    assert!(
        !cross_post.contains("lax=1") && !cross_post.contains("strict=1"),
        "cross-site POST must not carry Lax or Strict cookies: {cross_post:?}"
    );

    open(&mut b, &format!("{other_site}/form?action=ip"));
    click(&mut b, "#go");
    let cross_get = cookies(&srv);
    assert!(
        cross_get.contains("lax=1") && !cross_get.contains("strict=1"),
        "cross-site top-level GET carries Lax but not Strict cookies: {cross_get:?}"
    );
}
