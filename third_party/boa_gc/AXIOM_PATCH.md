# Axiom patch of boa_gc 0.20.0

This is the published `boa_gc` 0.20.0 crate (license: Unlicense OR MIT, see
`Cargo.toml`), used through `[patch.crates-io]` in the workspace `Cargo.toml`. It is
excluded from the workspace, so `cargo fmt`, `clippy` and `cargo test --workspace` do not
touch it.

## Change

`src/lib.rs`, `Collector::mark_heap`: the three ephemeron phases (live ephemerons, live
weak maps, pending ephemerons) drained the tracer queue by calling each node's
`trace_fn` without checking or setting its mark bit; only the root phase did. Any cycle
reachable from the value of an ephemeron with a live key, which is every JS object held
as a `WeakMap` value (its prototype links back to itself through `constructor`), was
therefore traced forever, and the queue grew until the process aborted with "memory
allocation of … bytes failed". The phases now share `Collector::drain_marking`, which
skips marked nodes and marks the rest before tracing them, as the root phase does.

A full collection runs rarely inside one realm, so the hang showed up when navigation
built a new realm (a large allocation burst) while a page's `WeakMap` held an object
value.

`Cargo.toml`: `[lints.rust] warnings` is `allow` (priority 100) instead of `warn`, as in
the vendored `boa_engine`.

Regression test: `crates/axiom-js/tests/realms.rs`.

## Removing the patch

Delete this directory and its `[patch.crates-io]` entry once Axiom moves to a Boa release
whose ephemeron phases mark what they trace, and keep the regression test.
