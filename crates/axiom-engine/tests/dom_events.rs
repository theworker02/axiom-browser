//! The events layer as seen from script: dispatch, listener options, event handler
//! attributes, `document.createEvent`, `window.event`, error reporting, and engine clicks.

use axiom_engine::BrowsingContext;

fn page(html: &str) -> BrowsingContext {
    let mut ctx = BrowsingContext::new(400, 300);
    ctx.page.load_html("about:events", html).expect("load html");
    ctx
}

fn eval(ctx: &mut BrowsingContext, src: &str) -> String {
    let js = ctx.page.js.as_mut().expect("page has js");
    js.eval(src)
        .map(|v| v.display)
        .unwrap_or_else(|e| format!("<{e}>"))
}

#[test]
fn dispatch_runs_capture_target_then_bubble() {
    let mut ctx = page("<!doctype html><div id=outer><p id=inner>x</p></div>");
    let got = eval(
        &mut ctx,
        "var log = []; var outer = document.getElementById('outer'); \
         var inner = document.getElementById('inner'); \
         function rec(tag) { return function (e) { log.push(tag + e.eventPhase); }; } \
         window.addEventListener('ping', rec('wc'), true); \
         document.addEventListener('ping', rec('dc'), true); \
         outer.addEventListener('ping', rec('oc'), true); \
         outer.addEventListener('ping', rec('ob')); \
         inner.addEventListener('ping', rec('ib')); \
         inner.addEventListener('ping', rec('ic'), true); \
         document.addEventListener('ping', rec('db')); \
         window.addEventListener('ping', rec('wb')); \
         var r = inner.dispatchEvent(new Event('ping', { bubbles: true })); \
         log.join(' ') + ' ' + r",
    );
    // Capture listeners on the target run before its non-capture ones, whatever the
    // registration order.
    assert_eq!(got, "wc1 dc1 oc1 ic2 ib2 ob3 db3 wb3 true");
    assert_eq!(
        eval(
            &mut ctx,
            "log = []; inner.dispatchEvent(new Event('ping')); log.join(' ')"
        ),
        "wc1 dc1 oc1 ic2 ib2",
        "a non-bubbling event stops after the target"
    );
}

#[test]
fn listener_options_once_passive_and_signal() {
    let mut ctx = page("<!doctype html><div id=d></div>");
    assert_eq!(
        eval(
            &mut ctx,
            "var d = document.getElementById('d'); var n = 0; \
             d.addEventListener('x', function () { n++; }, { once: true }); \
             d.dispatchEvent(new Event('x')); d.dispatchEvent(new Event('x')); n"
        ),
        "1"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "d.addEventListener('y', function (e) { e.preventDefault(); }, { passive: true }); \
             var e = new Event('y', { cancelable: true }); \
             [d.dispatchEvent(e), e.defaultPrevented].join()"
        ),
        "true,false",
        "preventDefault is ignored in a passive listener"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var c = new AbortController(); var m = 0; \
             d.addEventListener('z', function () { m++; }, { signal: c.signal }); \
             d.dispatchEvent(new Event('z')); c.abort(); d.dispatchEvent(new Event('z')); m"
        ),
        "1"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var k = 0; function f() { k++; } \
             d.addEventListener('w', f); d.addEventListener('w', f); \
             d.dispatchEvent(new Event('w')); d.removeEventListener('w', f); \
             d.dispatchEvent(new Event('w')); k"
        ),
        "1",
        "duplicate registrations collapse and removal works"
    );
}

#[test]
fn handle_event_is_looked_up_at_call_time() {
    let mut ctx = page("<!doctype html><div id=d></div>");
    assert_eq!(
        eval(
            &mut ctx,
            "var d = document.getElementById('d'); var o = {}; \
             d.addEventListener('x', o); \
             o.handleEvent = function (e) { window.seen = e.type + ':' + (this === o); }; \
             d.dispatchEvent(new Event('x')); window.seen"
        ),
        "x:true"
    );
}

#[test]
fn handler_attributes_compile_with_element_form_and_document_scopes() {
    let mut ctx = page(
        "<!doctype html><form id=f><input id=i \
         onclick=\"window.r = [id, typeof createElement, this === event.currentTarget].join()\">\
         </form><a id=a onclick=\"return false\">x</a>",
    );
    assert_eq!(
        eval(
            &mut ctx,
            "document.getElementById('i').dispatchEvent(new Event('click')); window.r"
        ),
        "i,function,true"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var i = document.getElementById('i'); [typeof i.onclick, i.onclick.name].join()"
        ),
        "function,onclick"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "document.getElementById('a').dispatchEvent(new Event('click', { cancelable: true }))"
        ),
        "false",
        "returning false cancels"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "i.setAttribute('onclick', 'window.r = 2'); i.click(); window.r"
        ),
        "2",
        "replacing the attribute recompiles"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "i.removeAttribute('onclick'); window.r = 0; i.click(); [i.onclick, window.r].join()"
        ),
        ",0"
    );
}

#[test]
fn body_window_reflecting_handlers_forward_to_window() {
    let mut ctx = page("<!doctype html><body onresize=\"window.resized = true\">");
    assert_eq!(
        eval(
            &mut ctx,
            "[typeof window.onresize, window.onresize === document.body.onresize].join()"
        ),
        "function,true"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "window.dispatchEvent(new Event('resize')); window.resized"
        ),
        "true"
    );
    assert_eq!(
        eval(&mut ctx, "document.body.onresize = null; window.onresize"),
        "null"
    );
}

#[test]
fn create_event_accepts_legacy_aliases_only() {
    let mut ctx = page("<!doctype html><div id=d></div>");
    assert_eq!(
        eval(
            &mut ctx,
            "[document.createEvent('MouseEvents') instanceof MouseEvent, \
              document.createEvent('HTMLEvents').constructor === Event, \
              document.createEvent('customevent') instanceof CustomEvent, \
              document.createEvent('UIEvents') instanceof UIEvent].join()"
        ),
        "true,true,true,true"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "try { document.createEvent('WheelEvent'); 'no throw' } catch (e) { e.name }"
        ),
        "NotSupportedError"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var e = document.createEvent('Event'); var d = document.getElementById('d'); \
             var first; try { d.dispatchEvent(e); first = 'no throw'; } catch (x) { first = x.name; } \
             e.initEvent('go', true, true); var n = 0; d.addEventListener('go', function () { n++; }); \
             [first, d.dispatchEvent(e), n, e.isTrusted].join()"
        ),
        "InvalidStateError,true,1,false"
    );
}

#[test]
fn window_event_is_set_only_during_listener_invocation() {
    let mut ctx = page("<!doctype html><div id=d></div>");
    assert_eq!(
        eval(
            &mut ctx,
            "var d = document.getElementById('d'); var ev = new Event('x'); var during; \
             d.addEventListener('x', function () { during = window.event === ev; }); \
             d.dispatchEvent(ev); [during, window.event].join()"
        ),
        "true,"
    );
}

#[test]
fn listener_exceptions_are_reported_to_window_and_do_not_stop_dispatch() {
    let mut ctx = page("<!doctype html><div id=d></div>");
    assert_eq!(
        eval(
            &mut ctx,
            "var d = document.getElementById('d'); var msgs = []; var after = false; \
             window.addEventListener('error', function (e) { \
               msgs.push(e.constructor.name + ':' + (e.error && e.error.message)); e.preventDefault(); }); \
             d.addEventListener('x', function () { throw new Error('boom'); }); \
             d.addEventListener('x', function () { after = true; }); \
             d.dispatchEvent(new Event('x')); [msgs.join(), after].join('|')"
        ),
        "ErrorEvent:boom|true"
    );
}

#[test]
fn disabled_reflects_and_blocks_click() {
    let mut ctx = page("<!doctype html><button id=b disabled>x</button>");
    assert_eq!(
        eval(
            &mut ctx,
            "var b = document.getElementById('b'); var n = 0; b.onclick = function () { n++; }; \
             b.click(); var before = [b.disabled, n].join(); \
             b.disabled = false; b.click(); [before, b.disabled, b.hasAttribute('disabled'), n].join()"
        ),
        "true,0,false,false,1"
    );
}

#[test]
fn canceled_engine_click_skips_default_action() {
    let mut ctx = page(
        "<!doctype html><input type=checkbox id=keep><input type=checkbox id=flip>\
         <script>document.getElementById('keep').addEventListener('click', \
           function (e) { e.preventDefault(); });</script>",
    );
    let (keep, flip) = {
        let doc = ctx.page.document.borrow();
        (
            doc.query_selector("#keep").expect("#keep"),
            doc.query_selector("#flip").expect("#flip"),
        )
    };
    ctx.click_node(keep);
    ctx.click_node(flip);
    assert_eq!(
        eval(
            &mut ctx,
            "[document.getElementById('keep').checked, document.getElementById('flip').checked].join()"
        ),
        "false,true",
        "a canceled click must not toggle; an uncanceled one does"
    );
    let doc = ctx.page.document.borrow();
    assert!(
        !doc.has_attr(flip, "checked"),
        "checkedness is state; the checked attribute stays the default"
    );
    assert_eq!(doc.focus, Some(flip));
}

#[test]
fn engine_clicks_are_trusted_mouse_events() {
    let mut ctx = page(
        "<!doctype html><p id=p>x</p><script>window.seen = ''; \
         document.getElementById('p').addEventListener('click', function (e) { \
           window.seen = [e.constructor.name, e.isTrusted, e.bubbles, e.target.id].join(); });</script>",
    );
    let p = ctx.page.document.borrow().query_selector("#p").expect("#p");
    ctx.click_node(p);
    assert_eq!(eval(&mut ctx, "window.seen"), "MouseEvent,true,true,p");
}
