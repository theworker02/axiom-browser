# Axiom Agent Guide

## Mission

Build a real browser engine from scratch. Phase 1 ends when:

```text
axiom https://example.com
```

fetches, parses, styles, lays out, paints, and displays a page in a native window — without embedding another engine.

## Non-negotiables

- Do **not** implement JavaScript before first paint works.
- Do **not** use WebView, Blink, Gecko, Servo, Chromium, or similar as the renderer.
- Own HTML parsing, DOM, CSS cascade, style resolution, layout, and paint increasingly over time.
- Libraries for TLS, HTTP, fonts, windowing, and GPU APIs are allowed.

## Phase order

1. **First paint** — URL → net → HTML → DOM → CSS → style → layout → display list → raster → window
2. **More CSS / layout** — flex, more selectors, images, scrolling
3. **Incremental / parallel** — dependency graph, dirty regions, GPU pipeline
4. **JavaScript** — engine + DOM bindings + events/timers/fetch/storage

## Coding rules

- Prefer small, tested crates over monoliths.
- Keep public APIs boring and explicit.
- Instrument every pipeline stage with timings.
- Add a fixture under `tests/` when fixing a rendering/layout bug.
- Avoid chasing full Chrome compatibility as the only north star; pursue GPU-native + parallel architecture as the differentiator.

## Current stack (Phase 1)

| Concern | Choice |
|--------|--------|
| Language | Rust 2021 workspace |
| HTTP/TLS | `reqwest` + rustls (`webpki-roots`), only inside `axiom-net` |
| Window | `winit` |
| Present | `softbuffer` |
| Fonts | `fontdue` |
| Paint | Our CPU rasterizer in `axiom-paint` |

GPU (`wgpu`) is a later present path; keep `axiom-gfx` abstractions soft enough to swap.
