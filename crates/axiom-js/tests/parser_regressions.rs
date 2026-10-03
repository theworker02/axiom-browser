//! Syntax the JavaScript parser must accept (see `third_party/boa_parser/AXIOM_PATCH.md`).

use boa_engine::{Context, Module, Source};

fn parses_as_module(src: &str) -> Result<(), String> {
    let mut ctx = Context::default();
    Module::parse(Source::from_bytes(src), None, &mut ctx)
        .map(|_| ())
        .map_err(|e| format!("{src:?}: {e}"))
}

#[test]
fn contextual_keywords_are_let_bindings_in_strict_code() {
    let cases = [
        "{ let of = 1 }",
        "for (let of of []) {}",
        "for (let of = 0; of < 1; of++) {}",
        "{ let async = 1, get = 2, set = 3, from = 4, as = 5, target = 6, meta = 7 }",
        "let\n[a] = [1]",
        "for (let [x, y] of []) {}",
        "switch (1) { case 1: let z = 2; }",
        "class A { static { let t = 1 } let = 1 }",
    ];
    let failures: Vec<String> = cases
        .iter()
        .filter_map(|src| parses_as_module(src).err())
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn let_stays_reserved_as_an_identifier_in_strict_code() {
    assert!(parses_as_module("var let = 1;").is_err());
}
