# Event loop

Phase 2 moves `browser/desktop` from a **one-shot render** to a **controlled event loop** on the UI thread (winit). Network and decode work stay on worker threads; completions post messages back.

## Model (target)

```mermaid
sequenceDiagram
  participant W as winit (UI thread)
  participant B as Browser
  participant L as axiom-loader workers
  participant E as axiom-events

  W->>B: WindowEvent (input, resize)
  B->>E: dispatch DOM events
  E->>B: listeners may mutate DOM
  B->>B: run microtasks
  B->>B: run timers (due)
  L-->>B: ResourceResponse (channel)
  B->>B: invalidation + layout/paint if dirty
  B->>W: present frame
```

## Tasks

| Mechanism | Phase 2 |
|-----------|---------|
| UI events (winit) | **PARTIAL** (static window today) |
| DOM event dispatch (capture/bubble) | **PARTIAL** (`axiom-events` crate) |
| `click`, `keydown`, wheel | **PLANNED** (Wave A) |
| Timer queue (`setTimeout`) | **PLANNED** |
| Microtask queue (Promise jobs) | **PLANNED** |
| `requestAnimationFrame` | **PLANNED** |
| Idle callbacks | **UNSUPPORTED** |

## Threading rules

| Rule | Status |
|------|--------|
| No blocking HTTP on UI thread after Wave A | **PLANNED** |
| Loader worker pool | **SUPPORTED** (crate) |
| JS execution on UI thread | **PLANNED** (typical embedder model) |

## Invalidation coupling

DOM mutations set `DirtyFlags`; the browser coalesces **style → layout → paint** work per frame instead of running the full Phase 1 pipeline on every wheel tick when possible (**PARTIAL** / evolving).

See [DOM](./DOM.md), `axiom-events`, and `demos/events.html`.
