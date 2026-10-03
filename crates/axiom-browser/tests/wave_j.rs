//! Wave J: script-visible style and geometry. `getComputedStyle`, CSSOM View
//! (`getBoundingClientRect`, `offset*`, `client*`, `scroll*`), viewport scrolling, the
//! `Image` constructor, `screen` and the full console. Every query flushes style and
//! layout synchronously, so script never observes stale geometry.

use std::time::Duration;

use axiom_browser::Browser;
use axiom_engine::BrowsingContext;
use axiom_net::test_server::{TestResponse, TestServer};
use tempfile::{tempdir, TempDir};

const PAGE: &str = "<!doctype html><html><head><style>\
    body { margin: 0 }\
    #box { position: absolute; left: 10px; top: 20px; width: 100px; height: 50px;\
           padding: 5px; border: 2px solid rgb(1, 2, 3) }\
    #pct { width: 50%; margin: 1px 2px; --brand: red }\
    .hidden { display: none }\
    #tall { height: 3000px }\
    </style></head><body>\
    <div id=box><span id=inner>x</span></div>\
    <div id=pct>p</div>\
    <div class=hidden><div id=ghost></div></div>\
    <div id=tall></div>\
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

fn open(b: &mut Browser, srv: &TestServer) {
    b.navigate_resolved(&srv.url("/"));
    assert!(
        ctx(b).run_until_idle(Duration::from_secs(15)),
        "page never went idle"
    );
}

fn serve() -> TestServer {
    TestServer::spawn(|_| TestResponse::ok(PAGE, "text/html"))
}

#[test]
fn computed_style_reports_resolved_values() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv);
    let cases = [
        ("getComputedStyle(box).width", "100px"),
        ("getComputedStyle(box).position", "absolute"),
        ("getComputedStyle(box).borderTopColor", "rgb(1, 2, 3)"),
        ("getComputedStyle(box).getPropertyValue('padding')", "5px"),
        ("getComputedStyle(pct).width", "400px"),
        ("getComputedStyle(pct).margin", "1px 2px"),
        ("getComputedStyle(pct).getPropertyValue('--brand')", "red"),
        ("getComputedStyle(pct).display", "block"),
        ("getComputedStyle(inner).display", "inline"),
        ("getComputedStyle(inner).width", "auto"),
        ("getComputedStyle(ghost).display", "block"),
        ("getComputedStyle(pct, '::before').display", ""),
        ("getComputedStyle(pct).transform", "none"),
        (
            "getComputedStyle(pct) instanceof CSSStyleDeclaration",
            "true",
        ),
        ("getComputedStyle(pct).length > 50", "true"),
        ("getComputedStyle(pct).cssText", ""),
    ];
    for (src, want) in cases {
        assert_eq!(eval(&mut b, src), want, "{src}");
    }
}

#[test]
fn computed_style_is_live_and_read_only() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv);
    let got = eval(
        &mut b,
        "var cs = getComputedStyle(pct); var before = cs.width;\
         pct.style.width = '200px';\
         var err = ''; try { cs.width = '1px'; } catch (e) { err = e.name; }\
         [before, cs.width, err].join(',')",
    );
    assert_eq!(got, "400px,200px,NoModificationAllowedError");
    assert_eq!(
        eval(
            &mut b,
            "try { getComputedStyle(document); 'no' } catch (e) { e.name }"
        ),
        "TypeError"
    );
}

#[test]
fn element_geometry_matches_layout() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv);
    let cases = [
        (
            "var r = box.getBoundingClientRect(); [r.x, r.y, r.width, r.height, r.right].join()",
            "10,20,114,64,124",
        ),
        (
            "[box.offsetLeft, box.offsetTop, box.offsetWidth, box.offsetHeight].join()",
            "10,20,114,64",
        ),
        ("box.offsetParent === document.body", "true"),
        (
            "[box.clientLeft, box.clientTop, box.clientWidth, box.clientHeight].join()",
            "2,2,110,60",
        ),
        ("inner.offsetParent === box", "true"),
        ("[inner.offsetLeft, inner.offsetTop].join()", "5,5"),
        ("inner.clientWidth", "0"),
        ("inner.getClientRects().length", "1"),
        ("box.getClientRects()[0] instanceof DOMRect", "true"),
        ("ghost.offsetParent === null && ghost.offsetWidth === 0", "true"),
        ("ghost.getBoundingClientRect().width", "0"),
        ("document.documentElement.clientWidth", "800"),
        ("document.documentElement.clientHeight", "600"),
        ("document.documentElement.scrollHeight >= 3000", "true"),
        ("document.scrollingElement === document.documentElement", "true"),
        ("JSON.stringify(new DOMRect(1, 2, 3, 4))", "{\"x\":1,\"y\":2,\"width\":3,\"height\":4,\"top\":2,\"right\":4,\"bottom\":6,\"left\":1}"),
    ];
    for (src, want) in cases {
        assert_eq!(eval(&mut b, src), want, "{src}");
    }
}

#[test]
fn geometry_reads_see_script_mutations_synchronously() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv);
    let got = eval(
        &mut b,
        "var a = pct.offsetHeight; pct.style.height = '123px'; [a > 0, pct.offsetHeight].join()",
    );
    assert_eq!(got, "true,123");
}

#[test]
fn script_scrolling_moves_the_viewport() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv);
    assert_eq!(eval(&mut b, "scrollY"), "0");
    assert_eq!(
        eval(
            &mut b,
            "scrollTo(0, 500); [scrollY, pageYOffset, document.documentElement.scrollTop,\
             box.getBoundingClientRect().top].join()"
        ),
        "500,500,500,-480"
    );
    ctx(&mut b).run_until_idle(Duration::from_secs(5));
    ctx(&mut b).page.update_rendering_if_needed();
    assert_eq!(ctx(&mut b).page.scroll_y, 500.0);

    assert_eq!(eval(&mut b, "scrollBy({ top: 100 }); scrollY"), "600");
    let max = eval(&mut b, "scrollTo(0, 1e9); scrollY");
    let max: f64 = max.parse().unwrap();
    assert!(
        max > 2000.0 && max < 3000.0,
        "clamped to the content: {max}"
    );
    assert_eq!(
        eval(&mut b, "document.documentElement.scrollTop = 0; scrollY"),
        "0"
    );
    assert_eq!(
        eval(
            &mut b,
            "var t = tall.getBoundingClientRect().top + scrollY; tall.scrollIntoView();\
             [t > 0, scrollY === t, tall.getBoundingClientRect().top].join()"
        ),
        "true,true,0"
    );
}

#[test]
fn viewport_scrolls_fire_scroll_then_scrollend_once_per_frame() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv);
    eval(
        &mut b,
        "var log = [];\
         document.addEventListener('scroll', function (e) { log.push('doc:' + e.type + ':' + scrollY); });\
         window.addEventListener('scrollend', function (e) { log.push('win:' + e.type); });\
         requestAnimationFrame(function () { log.push('raf'); });\
         scrollTo(0, 100); scrollTo(0, 250);",
    );
    assert!(ctx(&mut b).run_until_idle(Duration::from_secs(5)));
    assert_eq!(
        eval(&mut b, "log.join()"),
        "doc:scroll:250,win:scrollend,raf"
    );

    ctx(&mut b).page.scroll_by(50.0);
    assert!(ctx(&mut b).run_until_idle(Duration::from_secs(5)));
    assert_eq!(
        eval(&mut b, "log.slice(3).join()"),
        "doc:scroll:300,win:scrollend"
    );
}

/// A head script inserts a slow stylesheet, then asks for an animation frame.
fn render_blocking_page(blocking: bool) -> String {
    format!(
        "<!doctype html><head><script>\
         var link = document.createElement('link');\
         link.rel = 'stylesheet'; link.href = '/green.css';\
         {}\
         document.head.append(link);\
         requestAnimationFrame(function () {{\
           window.first = getComputedStyle(document.body).backgroundColor;\
         }});\
         </script></head><body>x</body>",
        if blocking {
            "link.setAttribute('blocking', 'render');"
        } else {
            ""
        }
    )
}

#[test]
fn blocking_render_stylesheets_hold_back_animation_frames() {
    let dir = tempdir().unwrap();
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/green.css" => {
            TestResponse::ok("body { background: rgb(0, 128, 0) }", "text/css").delayed(300)
        }
        "/blocking" => TestResponse::ok(render_blocking_page(true), "text/html"),
        _ => TestResponse::ok(render_blocking_page(false), "text/html"),
    });
    let mut b = browser(&dir);
    b.navigate_resolved(&srv.url("/blocking"));
    assert!(ctx(&mut b).run_until_idle(Duration::from_secs(15)));
    assert_eq!(eval(&mut b, "first"), "rgb(0, 128, 0)");

    // Without `blocking`, a script-inserted sheet does not delay the first frame.
    b.navigate_resolved(&srv.url("/plain"));
    assert!(ctx(&mut b).run_until_idle(Duration::from_secs(15)));
    assert_eq!(eval(&mut b, "first"), "rgba(0, 0, 0, 0)");
}

const CUSTOM_ELEMENTS: &str = "<!doctype html><html><head><script>\
    var log = [];\
    class LateEl extends HTMLElement {\
      connectedCallback() { log.push('late:connected:' + this.getAttribute('n')); }\
    }\
    customElements.define('late-el', LateEl);\
    </script></head><body>\
    <my-el a=1 b=x></my-el>\
    <late-el n=1></late-el><late-el n=2></late-el>\
    <script>\
    class MyEl extends HTMLElement {\
      static get observedAttributes() { return ['a']; }\
      constructor() { super(); this.made = true; log.push('ctor'); }\
      connectedCallback() { log.push('connected'); }\
      disconnectedCallback() { log.push('disconnected'); }\
      attributeChangedCallback(n, o, v) { log.push('attr:' + n + ':' + o + ':' + v); }\
    }\
    </script></body></html>";

#[test]
fn custom_elements_upgrade_construct_and_react() {
    let dir = tempdir().unwrap();
    let srv = TestServer::spawn(|_| TestResponse::ok(CUSTOM_ELEMENTS, "text/html"));
    let mut b = browser(&dir);
    open(&mut b, &srv);
    assert_eq!(
        eval(&mut b, "log.join()"),
        "late:connected:1,late:connected:2",
        "parser-created elements upgrade after their definition"
    );
    let cases = [
        (
            "log = []; customElements.define('my-el', MyEl); var el = document.querySelector('my-el');\
             [el.made, el instanceof MyEl, log.join()].join('|')",
            "true|true|ctor,attr:a:null:1,connected",
        ),
        (
            "log = []; el.setAttribute('a', '2'); el.setAttribute('b', 'y'); el.removeAttribute('a'); log.join()",
            "attr:a:1:2,attr:a:2:null",
        ),
        ("log = []; el.remove(); log.join()", "disconnected"),
        (
            "log = []; var c = document.createElement('my-el'); var d = c.made;\
             document.body.appendChild(c); [d, log.join()].join('|')",
            "true|ctor,connected",
        ),
        (
            "log = []; var n = new MyEl(); [n.localName, n.made, n.isConnected].join()",
            "my-el,true,false",
        ),
        (
            "log = []; document.body.insertAdjacentHTML('beforeend', '<my-el a=7></my-el>'); log.join()",
            "ctor,attr:a:null:7,connected",
        ),
        (
            "[customElements.get('my-el') === MyEl, customElements.getName(MyEl),\
              customElements.get('nope-el')].join()",
            "true,my-el,",
        ),
        (
            "var errs = [];\
             try { customElements.define('my-el', class extends HTMLElement {}); } catch (e) { errs.push(e.name); }\
             try { customElements.define('nodash', class extends HTMLElement {}); } catch (e) { errs.push(e.name); }\
             try { new HTMLElement(); } catch (e) { errs.push(e.name); }\
             errs.join()",
            "NotSupportedError,SyntaxError,TypeError",
        ),
    ];
    for (src, want) in cases {
        assert_eq!(eval(&mut b, src), want, "{src}");
    }
    eval(
        &mut b,
        "var resolved = ''; customElements.whenDefined('later-el').then(function (c) { resolved = c.name; });\
         customElements.define('later-el', class LaterEl extends HTMLElement {});",
    );
    assert!(ctx(&mut b).run_until_idle(Duration::from_secs(5)));
    assert_eq!(eval(&mut b, "resolved"), "LaterEl");
}

#[test]
fn intersection_and_resize_observers_follow_layout_and_scrolling() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv);
    eval(
        &mut b,
        "var seen = [];\
         var target = document.createElement('div'); target.style.height = '10px';\
         document.body.appendChild(target);\
         new IntersectionObserver(function (entries) {\
           entries.forEach(function (e) { seen.push('io:' + e.isIntersecting + ':' + e.intersectionRatio); });\
         }).observe(target);\
         new ResizeObserver(function (entries) {\
           entries.forEach(function (e) { seen.push('ro:' + e.contentRect.width + 'x' + e.contentRect.height); });\
         }).observe(pct);\
         var idle = false; requestIdleCallback(function (d) { idle = d.timeRemaining() > 0; });",
    );
    assert!(ctx(&mut b).run_until_idle(Duration::from_secs(5)));
    let first = eval(&mut b, "seen.sort().join()");
    assert!(first.contains("io:false:0"), "{first}");
    assert!(first.contains("ro:400x"), "{first}");
    assert_eq!(eval(&mut b, "idle"), "true");

    eval(&mut b, "seen = []; target.scrollIntoView();");
    assert!(ctx(&mut b).run_until_idle(Duration::from_secs(5)));
    assert_eq!(eval(&mut b, "seen.join()"), "io:true:1");

    eval(
        &mut b,
        "seen = []; pct.style.width = '100px'; pct.style.height = '20px';",
    );
    assert!(ctx(&mut b).run_until_idle(Duration::from_secs(5)));
    assert_eq!(eval(&mut b, "seen.join()"), "ro:100x20");
}

#[test]
fn image_constructor_screen_and_console() {
    let dir = tempdir().unwrap();
    let srv = serve();
    let mut b = browser(&dir);
    open(&mut b, &srv);
    let cases = [
        (
            "var i = new Image(10, 20); i instanceof HTMLImageElement",
            "true",
        ),
        (
            "[i.getAttribute('width'), i.width, i.height].join()",
            "10,10,20",
        ),
        ("[i.naturalWidth, i.complete].join()", "0,true"),
        ("i.src = 'x.png'; i.complete", "false"),
        (
            "[screen.width, screen.height, screen.colorDepth].join()",
            "800,600,24",
        ),
        (
            "['debug', 'trace', 'table', 'group', 'groupCollapsed', 'groupEnd', 'time',\
              'timeEnd', 'timeLog', 'count', 'countReset', 'assert', 'dir']\
              .every(function (m) { console[m]('x'); return typeof console[m] === 'function'; })",
            "true",
        ),
    ];
    for (src, want) in cases {
        assert_eq!(eval(&mut b, src), want, "{src}");
    }
}
