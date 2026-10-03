# Paint & raster

Paint builds a **display list** from the layout tree in `axiom-paint`, then **CPU-rasterizes** backgrounds, borders, and text (via `fontdue`) into an RGBA framebuffer.

GPU presentation may swap in later via `axiom-gfx`; Phase 2 still treats CPU raster as **SUPPORTED**.

## Display list

| Item | Status |
|------|--------|
| Filled rectangles (backgrounds) | **SUPPORTED** |
| Border strokes | **SUPPORTED** |
| Text glyphs | **SUPPORTED** |
| Rounded corners | **PLANNED** |
| Images (`<img>`) | **PLANNED** (decode + blit, Wave A) |
| Opacity / alpha blending | **PARTIAL** |
| `visibility: hidden` | **PLANNED** |

## Text rendering

| Feature | Status |
|---------|--------|
| System/default font metrics | **SUPPORTED** |
| Font weight / size from style | **SUPPORTED** |
| Subpixel AA | **PLANNED** |
| Web fonts (`@font-face`) | **PLANNED** |

## Output

| Feature | Status |
|---------|--------|
| Full-frame raster each navigation | **SUPPORTED** |
| Dirty-region partial raster | **PLANNED** |
| Headless PPM dump (`--headless`) | **SUPPORTED** |

```text
  LayoutTree ──► build_display_list ──► rasterize ──► Framebuffer
```

Reference fixtures: `tests/html/`, `crates/axiom-engine/tests/render_fixture.rs`.
