# Axiom patch of boa_engine 0.20.0

This is the published `boa_engine` 0.20.0 crate (license: Unlicense OR MIT, see
`Cargo.toml`), used through `[patch.crates-io]` in the workspace `Cargo.toml`. It is
excluded from the workspace, so `cargo fmt`, `clippy` and `cargo test --workspace` do not
touch it.

## Change

`src/builtins/promise/mod.rs`, `Promise::perform_promise_then`: step 12 of
[PerformPromiseThen](https://tc39.es/ecma262/#sec-performpromisethen), "Set
promise.[[PromiseIsHandled]] to true", ran only when the promise was already rejected.
It now runs on every path. Without it, a promise that got a handler (`then`, `catch`,
`await`, …) while pending reached `HostPromiseRejectionTracker(promise, "reject")` when it
rejected, and the page saw a false `unhandledrejection`.

`Cargo.toml`: `[lints.rust] warnings` is `allow` (priority 100) instead of `warn`.
Cargo caps lints for registry crates but not for path dependencies, and rustc 1.96 warns
about upstream code (unused `sptr::Strict` imports, function-pointer casts) on every
build.

Regression test: `crates/axiom-browser/tests/wave_g2.rs`,
`handlers_attached_while_pending_suppress_unhandled_rejection`.

### Callee names in call errors

Upstream reports every failed call as "not a callable function" / "not a constructor",
which makes page script errors undiagnosable. The bytecompiler (`src/bytecompiler/mod.rs`,
`ByteCompiler::call`, `describe_callee`, `record_call_site`) now records the source text of
each callee (`o.a.b`, `f`, `o[...]`, `g(...).h`, `(intermediate value)`) keyed by the
bytecode offset just past the `Call` / `CallSpread` / `New` / `NewSpread` instruction, in
`CodeBlock::call_sites` (`src/vm/code_block.rs`). The opcodes (`src/vm/opcode/call/mod.rs`
`callable` / `call_site_error`, `src/vm/opcode/new/mod.rs` `constructor`) check
`is_callable` / `is_constructor` up front and throw "`o.a.b` is not a function" /
"`C` is not a constructor", like V8. Calls without a record (optional chains, tagged
templates, `eval`, class field initialisers) keep the upstream message. Boa's own
`src/tests/function.rs` still asserts the old wording; it is not run by the workspace.

Regression test: `crates/axiom-js/tests/engine_diagnostics.rs`.

### V8-style error messages

- Property access on `null` / `undefined` (`src/vm/opcode/get/property.rs`,
  `access_base`) throws "Cannot read properties of undefined (reading 'x')" /
  "Cannot set properties of null (setting 'x')". The bytecompiler also records the base
  expression of `a.b` reads and `a.b(...)` callees in `call_sites`; with the `log` target
  `axiom_js::stack` at debug level, the failing expression is logged
  ("`` `a` is undefined ``"). `Cargo.toml` gains a `log = "0.4"` dependency for this.
- A temporal dead zone read (`src/vm/opcode/locals/mod.rs`) names the binding:
  "Cannot access 'x' before initialization".

### `error.stack`

- `Vm::stack_trace` (`src/vm/mod.rs`) formats up to `STACK_TRACE_LIMIT` (10) frames as
  V8 does: `"\n    at name (url)"`, or `"\n    at url"` for anonymous and top-level
  frames, with `<anonymous>` when the script has no path.
- Error constructors (`src/builtins/error/mod.rs`, `capture_stack_trace`, reached from
  `install_error_cause`) define a non-enumerable `stack` of `Error.prototype.toString()`
  plus the trace. Native errors (`src/error.rs`, `JsNativeError::stack`) capture the trace
  when they are raised (`execute_one` calls `JsError::capture_stack`) and install it in
  `to_opaque`.
- `Error.captureStackTrace(obj)` (a TypeError for non-objects) and
  `Error.stackTraceLimit = 10` exist, as in V8.

Regression tests: `crates/axiom-js/tests/engine_diagnostics.rs`.

## Removing the patch

Delete this directory and the `[patch.crates-io]` entry once Axiom moves to a Boa release
that sets the flag on every path, names callees in call errors and provides `error.stack`,
and keep the regression tests.
