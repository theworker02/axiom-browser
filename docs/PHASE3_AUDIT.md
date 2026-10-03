# Phase 3 Audit — Phase 2 baseline

Date: 2026-09-26  
Command: `cargo test --workspace` → **all suites pass** (0 failures).  
No Chromium / WebKit / Gecko / Electron / WebView rendering path exists.

Classification key:

| Tag | Meaning |
|-----|---------|
| COMPLETE | End-to-end behavior works for the scoped subset; tested or demoed |
| PARTIAL | Real code path exists; incomplete vs modern browsers |
| STUB | Interface/crate present; little or no runtime behavior |
| BROKEN | Intended behavior fails (none currently blocking) |
| MISSING | Not started |

## Engine / platform

| Feature | Status | Notes |
|---------|--------|-------|
| URL parse (http/https) + join | COMPLETE | `axiom-url` |
| HTTP(S) GET + redirects | COMPLETE | `ureq` + rustls |
| In-memory HTTP cache | PARTIAL | Simple TTL; no ETag/conditional yet |
| Async resource loader | PARTIAL | Worker + channel; doc nav still mostly sync |
| HTML parser | PARTIAL | Enough for demos; not WHATWG-complete |
| DOM arena + mutations | PARTIAL | Core tree ops + dirty flags; APIs incomplete |
| Selector engine | PARTIAL | type/class/id/attr/combinators/:first-child etc. |
| CSS parse + cascade | PARTIAL | Subset of properties |
| Style → layout → paint → CPU raster | PARTIAL | Block/inline; text approx; no real Flex/Grid |
| Flexbox | STUB / MISSING | `display:flex` not a real flex algorithm |
| CSS Grid | MISSING | |
| Compositor layers | PARTIAL | Layer list + scroll offset; CPU crop |
| GPU compositor / wgpu | MISSING | |
| Dirty invalidation | PARTIAL | Coarse style/layout/paint/composite flags |
| Document lifecycle states | PARTIAL | Enum present; not fully instrumented |
| History back/forward/reload | COMPLETE | Per browsing context |
| Hit testing | COMPLETE | Layout box walk |
| Events (Rust + JS click) | PARTIAL | Target+bubble; capture incomplete |
| JS runtime (Boa) | PARTIAL | Abstraction + host DOM/timers/console |
| `querySelector` / `textContent` / `addEventListener` | COMPLETE | Tested `click_js_pipeline` |
| `setTimeout` | COMPLETE | Real JS Function registry |
| Forms | PARTIAL | Text/password/checkbox; limited UI |
| Images | PARTIAL | Decode PNG/JPEG/GIF/WebP; sizing incomplete |
| Fetch / XHR / WebSocket | MISSING | |
| Cookies | MISSING | |
| localStorage / sessionStorage | MISSING | |
| Origin / CORS / CSP | STUB | Docs + intentions only |
| Multi-tab | MISSING | Single `BrowsingContext` in desktop |
| Profiles / session restore | MISSING | |
| DevTools UI | STUB | Docs only |
| WPT harness | STUB | Not running |
| Multiprocess | MISSING | |
| Accessibility tree | MISSING | |
| SVG / Canvas / Media | MISSING | |

## Browser shell

| Feature | Status | Notes |
|---------|--------|-------|
| Native window + softbuffer | COMPLETE | |
| Address bar + back/forward/reload | COMPLETE | Primitive chrome |
| Omnibox (search/suggest) | MISSING | |
| Tab strip | MISSING | |
| Bookmarks / downloads UI | MISSING | |
| Private browsing | MISSING | |
| Performance HUD | COMPLETE | Measured stage timings |
| Structured tracing | PARTIAL | `axiom-trace` timeline |

## Critical Phase 2 regressions

**None detected** in the audit test run.

Known non-blocking gaps (not regressions):

- Flexbox still absent (called out in REALITY.md)
- Some JS demos may warn on unsupported APIs
- Loader `etag` field unused (dead_code warning)

## Phase 3 entry decision

Proceed with **Wave A — Real multi-tab browser** first (recommended sequence item 2), preserving Phase 2 browsing contexts as the per-tab unit of work.

Do **not** attempt Grid/GPU/multiprocess in this wave.
