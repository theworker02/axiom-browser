# Phase 2 Implementation Plan (from Phase 1 audit)

## Phase 1 inventory (preserve)

| Subsystem | Status | Notes |
|-----------|--------|-------|
| `axiom-url` | Working | http/https parse + join |
| `axiom-net` | Working | sync ureq GET, redirects, simple cache |
| `axiom-html` | Subset | tokenizer/tree builder, not WHATWG-complete |
| `axiom-dom` | Arena DOM | create/append, limited queries |
| `axiom-css` | Subset | type/class/id/descendant, basic decls |
| `axiom-style` | Working | cascade + inheritance for Phase 1 props |
| `axiom-layout` | Block/inline lite | auto margins, approximate text width |
| `axiom-paint` | CPU display list | rects, borders, text via fontdue |
| `axiom-gfx` | Static present | winit + softbuffer, one-shot frame |
| `axiom-js` / `axiom-web` | Stubs | intentionally empty |
| `axiom-engine` | Sync pipeline | navigate/render_html + timings |
| desktop | CLI | headless PPM or static window |

**Rule:** extend these crates; do not replace working parsers/layout/paint with third-party browser engines.

## Phase 2 delivery strategy

Correctness → architecture → observability → performance.

### Wave A — Interactive shell (must ship)
1. BrowsingContext, Document lifecycle, History (back/forward/reload/fragment)
2. Dirty flags + controlled event loop (tasks/microtasks/timers)
3. Mutable DOM APIs + selector engine + mutation → invalidation
4. EventTarget + hit testing + click/keyboard/scroll input
5. Async resource loader (thread + channel) with HTTP cache abstraction
6. Scrolling (overflow + wheel) without full layout each wheel tick when possible
7. `<img>` async decode (PNG/JPEG/GIF/WebP via `image`)
8. Forms: text/password/checkbox/radio/submit + focus/caret
9. `JsRuntime` abstraction + Boa backend + DOM/timers/console
10. Browser chrome: back/forward/reload/URL bar
11. Performance HUD + basic tracing events
12. Demos + docs honesty matrix + expanded tests

### Wave B — Layout/CSS depth (in Phase 2, after Wave A works)
1. Layout tree separate from DOM; box model completeness
2. Flexbox subset with dedicated tests
3. Selector upgrades (:hover/:focus/:nth-child, combinators, attr)
4. Custom properties + calc() scaffolding
5. Compositor layers + GPU path foundations (wgpu when ready; CPU compositor OK first)

### Wave C — Platform readiness (foundations, not completeness)
1. Origin/SecurityOrigin stubs with same-origin checks where cheap
2. Screenshot reference tests + failure artifacts
3. WPT runner scaffolding (subset harness only)
4. Benchmarks JSON
5. Crash resilience on untrusted input paths

## JS runtime choice

Embed **Boa** behind `axiom-js::{JsRuntime, JsContext, JsValue}`.  
Engine code depends only on the abstraction — never on Boa types outside `axiom-js`.

## Async model

Keep winit on the UI thread. Resource loads and image decode run on a worker pool; completion posts `UserEvent`s into the browser loop. No blocking navigate on the UI thread after Wave A.

## Non-goals for Phase 2 declaration

Full WHATWG HTML, complete Flex/Grid, real CSP/CORS, HTTP/2/3, WASM, workers, full WPT pass rates.
