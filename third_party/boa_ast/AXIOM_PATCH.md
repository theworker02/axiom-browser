# Axiom patch of boa_ast 0.20.0

This is the published `boa_ast` 0.20.0 crate (license: Unlicense OR MIT, see
`Cargo.toml`), used through `[patch.crates-io]` in the workspace `Cargo.toml`. It is
excluded from the workspace, so `cargo fmt`, `clippy` and `cargo test --workspace` do not
touch it.

## Change

`src/scope_analyzer.rs`, `BindingEscapeAnalyzer::visit_class_element_mut`: the escape
analysis visited class field initializers (`x = i`, `#x = i`, `static x = i`) in the
enclosing scope, although the binding collector gives each one its own function scope and
the bytecompiler runs it as a separate function. An outer `let` / `const` read only by a
field initializer was therefore judged not to escape and kept in a register of the
enclosing frame; the initializer then read the (never initialised) environment slot and
threw "access to uninitialized binding". GitHub's code view hit this. The analyzer now
visits the initializer inside `field.scope`, so reads cross the function boundary and
mark the binding as escaping.

`Cargo.toml`: `[lints.rust] warnings` is `allow` (priority 100) instead of `warn`, as in
the other vendored Boa crates.

Regression test: `crates/axiom-js/tests/class_fields.rs`.

## Removing the patch

Delete this directory and its `[patch.crates-io]` entry once Axiom moves to a Boa release
whose escape analysis visits field initializers in their own scope, and keep the
regression test.
