# JavaScript

Phase 1 intentionally shipped **without** JavaScript. Phase 2 embeds a runtime **behind an abstraction** in `axiom-js` so engine code never depends directly on embedder types.

**Planned backend:** [Boa](https://github.com/boa-dev/boa) behind `JsRuntime` / `JsContext` / `JsValue`.

Axiom does **not** ship V8, JavaScriptCore, or SpiderMonkey as the primary JS VM.

## Current tree status

| Item | Phase 2 |
|------|---------|
| `axiom-js` crate | **PLANNED** (stub `JsEngine`, `is_enabled() == false`) |
| `axiom-web` DOM bindings | **PLANNED** |
| Inline `<script>` execution | **PLANNED** |
| `type="module"` | **UNSUPPORTED** |
| WASM | **UNSUPPORTED** |
| Workers | **UNSUPPORTED** |

## Target DOM bindings (Wave A)

| API | Status |
|-----|--------|
| `document.getElementById` | **PLANNED** |
| `document.createElement` | **PLANNED** |
| `element.textContent` | **PLANNED** |
| `addEventListener` | **SUPPORTED** (see [DOM.md](./DOM.md#events)) |
| `appendChild` / `removeChild` | **PLANNED** |
| `setTimeout` / `clearTimeout` | **PLANNED** |
| `console.log` | **PLANNED** |

Demos `demos/javascript.html`, `demos/events.html`, and `demos/browser-test.html` encode the **expected** behavior once the runtime is enabled.

## Promise rejection tracking

Boa's `HostPromiseRejectionTracker` hook feeds HTML's "notify about rejected promises" in `crates/axiom-js/src/platform_prelude.js`. After each microtask checkpoint the prelude fires `unhandledrejection` for rejections that are still unhandled, and `rejectionhandled` when a handler arrives later.

Upstream Boa 0.20.0's `PerformPromiseThen` sets `[[PromiseIsHandled]]` only when the promise is already rejected. A promise that got a handler while pending (through `then`, `catch` or `await`) was therefore reported as unhandled once it rejected. Axiom builds against a patched copy of `boa_engine` 0.20.0 in `third_party/boa_engine` (through `[patch.crates-io]`) that sets the flag on every path; see `third_party/boa_engine/AXIOM_PATCH.md`. Drop the patch once a Boa upgrade includes the fix.

## Garbage collector patch

Upstream `boa_gc` 0.20.0 traced the values of live ephemerons (`WeakMap` entries whose key is still reachable) without marking what it traced. Any cycle behind such a value, which every JS object has through its prototype, was traced forever, and the process aborted with a failed multi-gigabyte allocation. A full collection seldom runs within one realm, so this showed up when navigation built the next realm while the old page still held an object in a `WeakMap`. Axiom builds against a patched `boa_gc` 0.20.0 in `third_party/boa_gc` whose ephemeron phases mark nodes before tracing them; see `third_party/boa_gc/AXIOM_PATCH.md`. The regression tests are in `crates/axiom-js/tests/realms.rs`.

## Known Boa limitation: named functions inside `with`

Boa 0.20 panics ("must be declarative environment") when a named function expression is evaluated inside a `with` statement. Axiom's own code avoids the pattern: event handler attributes compile inside `with` scopes to an anonymous function whose `name` is set afterward. Page scripts that use the pattern still hit the panic.

## Security note

Untrusted script runs with **no real sandbox** in early Phase 2 beyond process isolation — treat as **PLANNED** hardening (see [SECURITY.md](./SECURITY.md)).
