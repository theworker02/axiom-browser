# Axiom patch of boa_parser 0.20.0

This is the published `boa_parser` 0.20.0 crate (license: Unlicense OR MIT, see
`Cargo.toml`), used through `[patch.crates-io]` in the workspace `Cargo.toml`. It is
excluded from the workspace, so `cargo fmt`, `clippy` and `cargo test --workspace` do not
touch it.

## Change

`src/parser/statement/declaration/lexical.rs`, `allowed_token_after_let`: `of` was missing
from the tokens that start a lexical binding after `let`. `let of = 1` and
`for (let of of xs)` are valid, but Boa then parsed `let` as an identifier, which strict
code (every module) rejects with "unexpected token 'let', strict reserved word cannot be
an identifier". Minifiers do emit `of` as a variable name.

`Cargo.toml`: `[lints.rust] warnings` is `allow` (priority 100) instead of `warn`, as in
`third_party/boa_engine`.

Regression test: `crates/axiom-js/tests/parser_regressions.rs`.

## Removing the patch

Delete this directory and its `[patch.crates-io]` entry once Axiom moves to a Boa release
that accepts `of` after `let`, and keep the regression test.
