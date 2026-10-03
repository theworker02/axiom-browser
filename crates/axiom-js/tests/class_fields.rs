//! Class field initializers run as their own functions, so the outer bindings they read must
//! live in environments, not in the enclosing function's registers
//! (see `third_party/boa_ast/AXIOM_PATCH.md`).

use axiom_js::JsRuntime;

#[test]
fn field_initializers_read_outer_bindings_declared_after_the_class() {
    let cases = [
        (
            "(function(){ class C { x = i }; let i = 2; return new C().x })()",
            "2",
        ),
        (
            "(function(){ class C { #x = i; get() { return this.#x } }; let i = 3; return new C().get() })()",
            "3",
        ),
        (
            "(function(){ let i = 5; class C { static #x = i; static get() { return C.#x } }; return C.get() })()",
            "5",
        ),
        (
            "(function(){ let i = 6; class C { static x = i + 1; y = i * 2 }; return C.x + new C().y })()",
            "19",
        ),
        (
            "(function(){ let k = 'a'; class C { [k] = i }; let i = 9; return new C().a })()",
            "9",
        ),
        (
            "(function(){ let n = 0; class C { x = n++ }; new C(); new C(); return n })()",
            "2",
        ),
    ];
    let mut c = JsRuntime::new().create_context().expect("realm");
    let failures: Vec<String> = cases
        .iter()
        .filter_map(|(src, want)| {
            let got = c
                .eval(src)
                .map(|v| v.display)
                .unwrap_or_else(|e| format!("<{e}>"));
            (got != *want).then(|| format!("{src}: got {got:?}, want {want:?}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
