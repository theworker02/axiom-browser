//! Realms are created while the previous document's realm is still alive (navigation
//! builds the new one before dropping the old). Building a realm allocates enough to
//! run a full garbage collection over the old one.

use axiom_js::JsRuntime;

#[test]
fn a_second_realm_can_be_created_while_the_first_is_alive() {
    let runtime = JsRuntime::new();
    let mut first = runtime.create_context().expect("first realm");
    first
        .eval(
            "var t = new EventTarget(); t.addEventListener('x', function () {}); \
             t.dispatchEvent(new Event('x')); \
             addEventListener('load', function () {}); dispatchEvent(new Event('load'));",
        )
        .expect("events in the first realm");
    let mut second = runtime.create_context().expect("second realm");
    assert_eq!(
        first.eval("typeof EventTarget").unwrap().display,
        "function"
    );
    assert_eq!(
        second.eval("typeof EventTarget").unwrap().display,
        "function"
    );
    drop(first);
    assert_eq!(
        second
            .eval("new Event('x', { bubbles: true }).bubbles")
            .unwrap()
            .display,
        "true"
    );
}

/// boa_gc 0.20.0 traced cycles behind a live `WeakMap` value forever
/// (`third_party/boa_gc/AXIOM_PATCH.md`).
#[test]
fn collection_survives_weak_map_values_with_cycles() {
    let runtime = JsRuntime::new();
    let mut first = runtime.create_context().expect("first realm");
    first
        .eval(
            "var w = new WeakMap(); var k = {}; var v = { self: null }; v.self = v; \
             w.set(k, v); var ws = new WeakSet(); ws.add(k);",
        )
        .expect("weak map in the first realm");
    let _second = runtime.create_context().expect("second realm");
    let _third = runtime.create_context().expect("third realm");
    assert_eq!(
        first
            .eval("w.get(k).self === v && ws.has(k)")
            .unwrap()
            .display,
        "true"
    );
}
