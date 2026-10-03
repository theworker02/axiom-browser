# Compositor

The compositor (`axiom-compositor`) holds **layers** with display lists and **scroll offsets**. Phase 2 starts with a simple model: one primary content layer plus scroll translation — not a full browser compositor tree.

Axiom does **not** reuse Chromium’s cc/layers.

## Layers

| Feature | Phase 2 |
|---------|---------|
| Register layer + bounds + display list | **SUPPORTED** (API) |
| Multiple overlapping layers | **PARTIAL** |
| `transform` / opacity per layer | **PLANNED** |
| Fixed/sticky promotion | **UNSUPPORTED** |

## Scrolling

| Feature | Status |
|---------|--------|
| Per-layer `scroll_x` / `scroll_y` | **SUPPORTED** (API) |
| Wheel events → scroll delta | **PLANNED** (Wave A + events) |
| Compositing scrolled content without full relayout | **PARTIAL** (goal; may relayout early Phase 2) |
| Scrollbars ( painted ) | **PLANNED** |

```text
  Layer 0 (content)  scroll (sx, sy)  ──► blit offset into framebuffer
  Layer 1 (chrome)   scroll (0, 0)    ──► optional UI overlay (PLANNED)
```

## GPU path

| Feature | Status |
|---------|--------|
| CPU composite into softbuffer | **SUPPORTED** |
| `wgpu` swap chain | **PLANNED** (Wave B foundation) |

See `demos/scrolling.html` for overflow and document scroll exercises.
