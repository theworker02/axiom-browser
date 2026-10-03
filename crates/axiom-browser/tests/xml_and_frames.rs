//! XML MIME dispatch and nested browsing-context regressions.

use std::time::Duration;

use axiom_browser::Browser;
use axiom_dom::{Namespace, NodeKind};
use axiom_net::test_server::{TestResponse, TestServer};
use tempfile::tempdir;

const IDLE: Duration = Duration::from_secs(10);

fn browser() -> (tempfile::TempDir, Browser) {
    let dir = tempdir().unwrap();
    let browser = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    (dir, browser)
}

#[test]
fn xhtml_mime_uses_the_xml_parser_and_preserves_cdata() {
    let server = TestServer::spawn(|req| {
        match req.path.as_str() {
        "/page.xhtml" => TestResponse::ok(
            "<?xml version='1.0'?><html xmlns='http://www.w3.org/1999/xhtml'><body><Thing><![CDATA[x<y]]></Thing></body></html>",
            "application/xhtml+xml; charset=utf-8",
        ),
        _ => TestResponse::status(404, "not found"),
    }
    });
    let (_dir, mut browser) = browser();
    browser.navigate_resolved(&server.url("/page.xhtml"));
    let context = &mut browser.window.tabs.active_tab_mut().context;
    assert!(context.run_until_idle(IDLE));
    assert!(context.page.xhtml);

    let document = context.page.document.borrow();
    let root = document.document_element().unwrap();
    assert_eq!(document.namespace(root), Some(Namespace::Html));
    let thing = document.find_descendant(root, "Thing").unwrap();
    let cdata = document.get(thing).children[0];
    assert!(matches!(document.get(cdata).kind, NodeKind::CData { ref data } if data == "x<y"));
}

#[test]
fn xhtml_document_executes_external_scripts_in_document_order() {
    let server = TestServer::spawn(|req| {
        match req.path.as_str() {
        "/page.xhtml" => TestResponse::ok(
            "<?xml version='1.0'?><html xmlns='http://www.w3.org/1999/xhtml'><body><script src='/first.js'></script><script src='/second.js'></script></body></html>",
            "application/xhtml+xml; charset=utf-8",
        ),
        "/first.js" => TestResponse::ok(
            "window.xmlLog = ['first', document.contentType, document.createCDATASection('x').nodeType];",
            "application/javascript",
        ),
        "/second.js" => TestResponse::ok("window.xmlLog.push('second');", "application/javascript"),
        _ => TestResponse::status(404, "not found"),
    }
    });
    let (_dir, mut browser) = browser();
    browser.navigate_resolved(&server.url("/page.xhtml"));
    let context = &mut browser.window.tabs.active_tab_mut().context;
    assert!(context.run_until_idle(IDLE));

    let log = context
        .page
        .js
        .as_mut()
        .expect("XML document realm")
        .eval("JSON.stringify(window.xmlLog)")
        .expect("XML scripts execute")
        .display;
    assert_eq!(log, r#"["first","application/xhtml+xml",4,"second"]"#);
}

#[test]
fn same_origin_frame_document_factory_uses_the_child_mime_type() {
    let server = TestServer::spawn(|req| match req.path.as_str() {
        "/parent" => TestResponse::ok(
            "<!doctype html><iframe id='frame' src='/child.xhtml'></iframe>",
            "text/html",
        ),
        "/child.xhtml" => TestResponse::ok(
            "<?xml version='1.0'?><html xmlns='http://www.w3.org/1999/xhtml'><body/></html>",
            "application/xhtml+xml",
        ),
        _ => TestResponse::status(404, "not found"),
    });
    let (_dir, mut browser) = browser();
    browser.navigate_resolved(&server.url("/parent"));
    let context = &mut browser.window.tabs.active_tab_mut().context;
    assert!(context.run_until_idle(IDLE));

    let value = context
        .page
        .js
        .as_mut()
        .expect("parent document realm")
        .eval(
            "var d = document.getElementById('frame').contentDocument; \
             var e = d.createElement('Thing'); [d.contentType, e.namespaceURI, e.tagName].join('|')",
        )
        .expect("same-origin frame document factory")
        .display;
    assert_eq!(
        value,
        "application/xhtml+xml|http://www.w3.org/1999/xhtml|THING"
    );
}

#[test]
fn iframe_src_owns_a_distinct_child_browsing_context() {
    let server = TestServer::spawn(|req| match req.path.as_str() {
        "/parent" => TestResponse::ok(
            "<!doctype html><title>parent</title><iframe id='frame' src='/child'></iframe>",
            "text/html",
        ),
        "/child" => TestResponse::ok(
            "<!doctype html><title>child</title><p id='inside'>child document</p>",
            "text/html",
        ),
        _ => TestResponse::status(404, "not found"),
    });
    let (_dir, mut browser) = browser();
    browser.navigate_resolved(&server.url("/parent"));
    let context = &mut browser.window.tabs.active_tab_mut().context;
    assert!(context.run_until_idle(IDLE));
    let frame = context
        .page
        .document
        .borrow()
        .get_element_by_id("frame")
        .unwrap();
    let child = context.frame_context(frame).expect("iframe child context");
    assert_ne!(child.browsing_context_id(), context.browsing_context_id());
    assert_eq!(child.page.title, "child");
    assert!(child
        .page
        .document
        .borrow()
        .get_element_by_id("inside")
        .is_some());
}

#[test]
fn iframe_srcdoc_and_about_blank_are_separate_same_origin_documents() {
    let server = TestServer::spawn(|_| {
        TestResponse::ok(
            "<!doctype html><iframe id='srcdoc' src='/ignored' srcdoc='<p id=inside>inline</p>'></iframe><iframe id='blank'></iframe>",
            "text/html",
        )
    });
    let (_dir, mut browser) = browser();
    browser.navigate_resolved(&server.url("/parent"));
    let context = &mut browser.window.tabs.active_tab_mut().context;
    assert!(context.run_until_idle(IDLE));
    let document = context.page.document.borrow();
    let srcdoc = document.get_element_by_id("srcdoc").unwrap();
    let blank = document.get_element_by_id("blank").unwrap();
    drop(document);
    assert!(context
        .frame_context(srcdoc)
        .unwrap()
        .page
        .document
        .borrow()
        .get_element_by_id("inside")
        .is_some());
    assert_ne!(
        context.frame_context(srcdoc).unwrap().browsing_context_id(),
        context.frame_context(blank).unwrap().browsing_context_id()
    );
}

#[test]
fn removing_an_iframe_destroys_its_child_context() {
    let server = TestServer::spawn(|req| match req.path.as_str() {
        "/parent" => TestResponse::ok(
            "<!doctype html><iframe id='frame' src='/child'></iframe>",
            "text/html",
        ),
        "/child" => TestResponse::ok("<!doctype html><p>child</p>", "text/html"),
        _ => TestResponse::status(404, "not found"),
    });
    let (_dir, mut browser) = browser();
    browser.navigate_resolved(&server.url("/parent"));
    let context = &mut browser.window.tabs.active_tab_mut().context;
    assert!(context.run_until_idle(IDLE));
    let frame = context
        .page
        .document
        .borrow()
        .get_element_by_id("frame")
        .unwrap();
    assert!(context.frame_context(frame).is_some());
    context
        .page
        .js
        .as_mut()
        .unwrap()
        .eval("document.getElementById('frame').remove()")
        .unwrap();
    context.poll_loader();
    assert!(context.frame_context(frame).is_none());
}

#[test]
fn sandbox_without_allow_scripts_blocks_child_classic_scripts() {
    let server = TestServer::spawn(|req| match req.path.as_str() {
        "/parent" => TestResponse::ok(
            "<!doctype html><iframe id='frame' sandbox src='/child'></iframe>",
            "text/html",
        ),
        "/child" => TestResponse::ok(
            "<!doctype html><script>globalThis.childRan = true</script>",
            "text/html",
        ),
        _ => TestResponse::status(404, "not found"),
    });
    let (_dir, mut browser) = browser();
    browser.navigate_resolved(&server.url("/parent"));
    let context = &mut browser.window.tabs.active_tab_mut().context;
    assert!(context.run_until_idle(IDLE));
    let frame = context
        .page
        .document
        .borrow()
        .get_element_by_id("frame")
        .unwrap();
    let child = context.frame_context_mut(frame).unwrap();
    assert_eq!(
        child
            .page
            .js
            .as_mut()
            .unwrap()
            .eval("typeof childRan")
            .unwrap()
            .display,
        "undefined"
    );
}

#[test]
fn iframe_child_framebuffer_is_composited_at_its_layout_box() {
    let server = TestServer::spawn(|req| {
        match req.path.as_str() {
        "/parent" => TestResponse::ok(
            "<!doctype html><style>iframe { width: 80px; height: 60px }</style><iframe id='frame' src='/child'></iframe>",
            "text/html",
        ),
        "/child" => TestResponse::ok(
            "<!doctype html><style>html, body { margin: 0; background: rgb(255, 0, 0) }</style>",
            "text/html",
        ),
        _ => TestResponse::status(404, "not found"),
    }
    });
    let (_dir, mut browser) = browser();
    browser.navigate_resolved(&server.url("/parent"));
    let context = &mut browser.window.tabs.active_tab_mut().context;
    assert!(context.run_until_idle(IDLE));
    let frame = context
        .page
        .document
        .borrow()
        .get_element_by_id("frame")
        .unwrap();
    let rect = context
        .page
        .shared
        .render
        .borrow()
        .layout
        .as_ref()
        .unwrap()
        .node_rect(frame)
        .unwrap();
    let x = rect.x.round() as u32 + 2;
    let y = rect.y.round() as u32 + 2;
    let framebuffer = context.framebuffer().unwrap();
    assert_eq!(
        framebuffer.pixels[(y * framebuffer.width + x) as usize],
        0xff_00_00_ff,
        "the parent framebuffer contains the child document pixels"
    );
}

#[test]
fn iframe_navigation_failure_dispatches_error_not_load() {
    let server = TestServer::spawn(|req| {
        match req.path.as_str() {
        "/parent" => TestResponse::ok(
            "<!doctype html><iframe id='frame' src='file:///axiom-frame-does-not-exist.html'></iframe><script>let f = document.getElementById('frame'); f.onload = () => globalThis.frameLoad = true; f.onerror = () => globalThis.frameError = true;</script>",
            "text/html",
        ),
        _ => TestResponse::status(404, "not found"),
    }
    });
    let (_dir, mut browser) = browser();
    browser.navigate_resolved(&server.url("/parent"));
    let context = &mut browser.window.tabs.active_tab_mut().context;
    assert!(context.run_until_idle(IDLE));
    let js = context.page.js.as_mut().unwrap();
    assert_eq!(
        js.eval("Boolean(globalThis.frameError)").unwrap().display,
        "true"
    );
    assert_eq!(
        js.eval("Boolean(globalThis.frameLoad)").unwrap().display,
        "false"
    );
}

#[test]
fn same_origin_iframe_exposes_scoped_document_and_window_proxies() {
    let server = TestServer::spawn(|req| match req.path.as_str() {
        "/parent" => TestResponse::ok(
            "<!doctype html><iframe id='frame' src='/child'></iframe>",
            "text/html",
        ),
        "/child" => TestResponse::ok(
            "<!doctype html><p id='inside'>child document</p>",
            "text/html",
        ),
        _ => TestResponse::status(404, "not found"),
    });
    let (_dir, mut browser) = browser();
    browser.navigate_resolved(&server.url("/parent"));
    let context = &mut browser.window.tabs.active_tab_mut().context;
    assert!(context.run_until_idle(IDLE));
    let js = context.page.js.as_mut().unwrap();
    assert_eq!(
        js.eval(
            "document.getElementById('frame').contentDocument.getElementById('inside').textContent"
        )
        .unwrap()
        .display,
        "child document"
    );
    assert_eq!(
        js.eval("document.getElementById('frame').contentWindow.document.body.tagName")
            .unwrap()
            .display,
        "BODY"
    );
}

#[test]
fn cross_origin_and_opaque_sandbox_frames_do_not_expose_child_documents() {
    let child = TestServer::spawn(|_| {
        TestResponse::ok(
            "<!doctype html><p id='inside'>private child</p>",
            "text/html",
        )
    });
    let child_url = child.url("/child");
    let parent = TestServer::spawn(move |req| match req.path.as_str() {
        "/cross" => TestResponse::ok(
            format!("<!doctype html><iframe id='frame' src='{child_url}'></iframe>"),
            "text/html",
        ),
        "/sandbox" => TestResponse::ok(
            "<!doctype html><iframe id='frame' sandbox='allow-scripts' src='/child'></iframe>",
            "text/html",
        ),
        "/child" => TestResponse::ok("<!doctype html><p>same-host sandbox child</p>", "text/html"),
        _ => TestResponse::status(404, "not found"),
    });
    let (_dir, mut browser) = browser();
    browser.navigate_resolved(&parent.url("/cross"));
    let context = &mut browser.window.tabs.active_tab_mut().context;
    assert!(context.run_until_idle(IDLE));
    let js = context.page.js.as_mut().unwrap();
    assert_eq!(
        js.eval("document.getElementById('frame').contentDocument === null")
            .unwrap()
            .display,
        "true"
    );
    assert_eq!(
        js.eval("try { document.getElementById('frame').contentWindow.document; 'no' } catch (e) { e.name }")
            .unwrap()
            .display,
        "SecurityError"
    );
    context.navigate_to(&parent.url("/sandbox"));
    assert!(context.run_until_idle(IDLE));
    assert_eq!(
        context
            .page
            .js
            .as_mut()
            .unwrap()
            .eval("document.getElementById('frame').contentDocument === null")
            .unwrap()
            .display,
        "true"
    );
}

#[test]
fn iframe_window_proxy_delivers_json_cloned_messages_on_child_turn() {
    let server = TestServer::spawn(|req| {
        match req.path.as_str() {
        "/parent" => TestResponse::ok(
            "<!doctype html><iframe id='frame' src='/child'></iframe>",
            "text/html",
        ),
        "/child" => TestResponse::ok(
            "<!doctype html><script>window.addEventListener('message', event => { globalThis.received = event.data.answer + ':' + event.origin })</script>",
            "text/html",
        ),
        _ => TestResponse::status(404, "not found"),
    }
    });
    let (_dir, mut browser) = browser();
    browser.navigate_resolved(&server.url("/parent"));
    let context = &mut browser.window.tabs.active_tab_mut().context;
    assert!(context.run_until_idle(IDLE));
    context
        .page
        .js
        .as_mut()
        .unwrap()
        .eval("document.getElementById('frame').contentWindow.postMessage({answer: 42}, '*')")
        .unwrap();
    context.tick();
    let frame = context
        .page
        .document
        .borrow()
        .get_element_by_id("frame")
        .unwrap();
    assert_eq!(
        context
            .frame_context_mut(frame)
            .unwrap()
            .page
            .js
            .as_mut()
            .unwrap()
            .eval("received")
            .unwrap()
            .display,
        format!(
            "42:{}",
            axiom_url::Origin::of_document(&server.url("/parent")).serialize()
        )
    );
}
