# Reality check — implemented vs scaffolded

This document exists so we do not confuse **crate scaffolding** with **working behavior**.

Last audited against the click→JS→DOM→paint milestone.

## Working end-to-end (exercised by tests)

| Path | Status | Evidence |
|------|--------|----------|
| HTML parse → DOM | **Real** | `axiom-html`, fixture tests |
| CSS parse → cascade → computed style | **Real (subset)** | Phase 1 props + selectors upgrade |
| Layout → display list → CPU raster | **Real (subset)** | `render_fixture`, headless PPM |
| HTTPS navigation | **Real** | `Engine::navigate`, example.com |
| Local file navigation | **Real** | browsing context + demos |
| History back/forward/reload | **Real** | `History` + chrome buttons |
| Hit test | **Real** | layout box walk |
| `querySelector` / `getElementById` | **Real** | selector engine + JS host |
| `element.textContent` get/set from JS | **Real** | mutates DOM + dirty flags |
| `addEventListener("click", fn)` | **Real** | closures stored in JS realm; Rust dispatches on click |
| Click → JS → DOM mutation → paint | **Real** | `tests/click_js_pipeline.rs` |
| Dirty flags / incremental flush | **Real (coarse)** | style/layout/paint/composite bits; not per-subtree yet |
| Scrolling (wheel + offset composite) | **Real** | browsing + compositor scroll |
| `setTimeout(fn, ms)` | **Real** | callback id registry in JS + timer queue |
| Browser chrome | **Real** | back/forward/reload/URL bar |
| Performance HUD numbers | **Real (measured)** | stage timers on flush |

## Partial / limited

| Path | Status | Notes |
|------|--------|-------|
| Event capture phase in JS | Partial | Target + bubble implemented; capture not yet |
| `removeEventListener` | Partial | Implemented in JS store; little test coverage |
| Forms | Partial | Text/password editing + checkbox; no real `<select>` UI |
| Images | Partial | Decode + map to nodes; sizing/object-fit incomplete |
| Async resource loader | Partial | Worker thread + queue works; document nav still mostly sync |
| HTML parser | Partial | Not full WHATWG |
| Flexbox | **Mostly stubbed / absent** | `display:flex` not a real flex algorithm yet |
| Compositor / GPU | Partial | Layer list + CPU scroll crop; not GPU-batched |
| DevTools UI | Scaffold | Docs only |
| WPT harness | Scaffold | Not running WPT |
| Security (origin/CORS/CSP) | Scaffold | Types/docs only |

## Explicitly stubbed or empty

| Item | Notes |
|------|-------|
| Full Chromium-compatible JS DOM | Only the APIs listed above |
| `fetch` / XHR / WebSocket | Not implemented |
| CSS Grid / animations / transforms | Not implemented |
| Accessibility tree | Not implemented |

## The milestone script

```js
const button = document.querySelector("#button");
button.addEventListener("click", () => {
  document.querySelector("#message").textContent = "It works!";
});
```

Demo: `demos/click-works.html`  
Test: `cargo test -p axiom-engine --test click_js_pipeline`

Pipeline on click:

```text
OS click / click_node
  → hit test (or known NodeId)
  → Rust EventTarget dispatch
  → JS __axiom_dispatchJsEvent(path, "click", …)
  → listener runs
  → textContent host → DOM mutation + DirtyFlags
  → update_rendering_if_needed (style/layout/paint as dirty)
  → new framebuffer
```
