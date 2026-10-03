//! Engine error messages name the failing callee, like V8 ("a.b is not a function"), so page
//! script errors point at the missing API (see `third_party/boa_engine/AXIOM_PATCH.md`).

use axiom_js::{JsContext, JsRuntime};

fn message(ctx: &mut JsContext, src: &str) -> String {
    let wrapped = format!("try {{ {src}; 'no error' }} catch (e) {{ e.name + ': ' + e.message }}");
    ctx.eval(&wrapped)
        .map(|v| v.display)
        .unwrap_or_else(|e| format!("<{e}>"))
}

#[test]
fn call_errors_name_the_callee() {
    let mut c = JsRuntime::new().create_context().expect("realm");
    let cases = [
        (
            "var o = {}; o.missing()",
            "TypeError: o.missing is not a function",
        ),
        (
            "var o = { a: { b: 1 } }; o.a.b()",
            "TypeError: o.a.b is not a function",
        ),
        ("var f = 3; f()", "TypeError: f is not a function"),
        (
            "var o = {}; o['x' + 1]()",
            "TypeError: o[...] is not a function",
        ),
        (
            "var g = () => ({}); g().h()",
            "TypeError: g(...).h is not a function",
        ),
        (
            "var o = {}; o.m(...[1])",
            "TypeError: o.m is not a function",
        ),
        (
            "var o = { p: {} }; o.p()",
            "TypeError: o.p is not a function",
        ),
        ("var C = 1; new C()", "TypeError: C is not a constructor"),
        (
            "var o = {}; new o.K(...[])",
            "TypeError: o.K is not a constructor",
        ),
        (
            "var a = () => 1; new a()",
            "TypeError: a is not a constructor",
        ),
        (
            "(1, 2)()",
            "TypeError: (intermediate value) is not a function",
        ),
        ("var o = {}; o?.m()", "TypeError: not a callable function"),
        (
            "(function () { switch (1) { case 0: let q = 1; case 1: return typeof q } })()",
            "ReferenceError: Cannot access 'q' before initialization",
        ),
        (
            "var u; u.prop",
            "TypeError: Cannot read properties of undefined (reading 'prop')",
        ),
        (
            "var n = null; n['k' + 1]",
            "TypeError: Cannot read properties of null (reading 'k1')",
        ),
        (
            "var u; u[{}]",
            "TypeError: Cannot read properties of undefined",
        ),
        (
            "var u; u.a = 1",
            "TypeError: Cannot set properties of undefined (setting 'a')",
        ),
        (
            "var n = null; n[0] = 1",
            "TypeError: Cannot set properties of null (setting '0')",
        ),
        ("var s = 'ab'; s.length", "no error"),
    ];
    let failures: Vec<String> = cases
        .iter()
        .filter_map(|(src, want)| {
            let got = message(&mut c, src);
            (got != *want).then(|| format!("{src}: got {got:?}, want {want:?}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn annex_b_web_legacy_features_are_enabled() {
    let mut c = JsRuntime::new().create_context().expect("realm");
    let cases = [
        ("encodeURIComponent('a b').substr(1, 3)", "%20"),
        ("'abcdef'.substr(-2)", "ef"),
        ("escape('a b') + unescape('%41')", "a%20bA"),
        ("'x'.anchor('n')", r#"<a name="n">x</a>"#),
        ("typeof Date.prototype.getYear", "function"),
        ("var o = {}; o.__defineGetter__('g', () => 7); o.g", "7"),
        ("<!-- html comment\n42", "42"),
        (
            "{ function hoisted() { return 1 } } typeof hoisted",
            "function",
        ),
    ];
    let failures: Vec<String> = cases
        .iter()
        .filter_map(|(src, want)| {
            let got = c
                .eval(src)
                .map(|v| v.display)
                .unwrap_or_else(|e| format!("<{e}>"));
            (got != *want).then(|| format!("{src:?}: got {got:?}, want {want:?}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn nested_functions_keep_their_own_call_sites() {
    let mut c = JsRuntime::new().create_context().expect("realm");
    assert_eq!(
        message(
            &mut c,
            "function outer() { function inner(x) { return x.run() } return inner({}) } outer()"
        ),
        "TypeError: x.run is not a function"
    );
}

#[test]
fn errors_carry_v8_style_stacks() {
    let mut c = JsRuntime::new().create_context().expect("realm");
    let mut eval = |src: &str, url: Option<&str>| {
        c.eval_script(src, url)
            .map(|v| v.display)
            .unwrap_or_else(|e| format!("<{e}>"))
    };
    // Constructed errors: the header, then one line per frame, innermost first.
    assert_eq!(
        eval(
            "function inner() { return new Error('boom').stack } function outer() { return inner() } outer()",
            Some("https://site.example/app.js"),
        ),
        "Error: boom\n    at inner (https://site.example/app.js)\n    at outer (https://site.example/app.js)\n    at https://site.example/app.js"
    );
    // Engine-raised errors capture the frames that were running when they were thrown.
    assert_eq!(
        eval(
            "function f() { null.x } try { f() } catch (e) { e.stack }",
            None
        ),
        "TypeError: Cannot read properties of null (reading 'x')\n    at f (<anonymous>)\n    at <anonymous>"
    );
    assert_eq!(
        eval(
            "var o = {}; Error.captureStackTrace(o);\
             [typeof o.stack, Object.keys(o).length, Error.stackTraceLimit,\
              Object.getOwnPropertyDescriptor(new Error('x'), 'stack').enumerable].join()",
            None
        ),
        "string,0,10,false"
    );
    assert_eq!(
        eval(
            "try { Error.captureStackTrace(1); 'no' } catch (e) { e.name }",
            None
        ),
        "TypeError"
    );
}
