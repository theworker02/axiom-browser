# Layout

Layout lives in `axiom-layout`. It builds a **box tree** from the DOM and computed styles, then formats it into a **fragment tree** with absolute page coordinates, which paint turns into a display list. No other engine's layout code is embedded.

```text
  DOM + ComputedStyle
        │  tree.rs: display, anonymous boxes, flex/grid items, replaced elements
        ▼
     box tree
        │  block.rs ─┬─ inline.rs   (line boxes, bidi, text)
        │            ├─ float.rs    (float placement, clearance)
        │            ├─ table.rs    (table grid, column widths)
        │            ├─ flex.rs     (flexbox)
        │            └─ grid.rs     (CSS grid)
        ▼
   LayoutTree (fragments) ──► axiom-paint
```

`axiom-engine` reports the time spent here as the **Layout** stage.

## Box tree (`tree.rs`)

| Feature | Status |
|---------|--------|
| `display: none` / `contents` | SUPPORTED |
| Anonymous block boxes around inline runs in mixed block containers | SUPPORTED |
| Block-in-inline (the inline box becomes a decoration-less block) | SUPPORTED |
| Anonymous table wrappers (rows, cells) | SUPPORTED |
| Flex and grid items: every in-flow child is an item; each contiguous run of text becomes an anonymous item unless it is only collapsible white space | SUPPORTED |
| Replaced elements (`img`, `svg`, `video`, `canvas`, `iframe`, …) and form controls as atomic boxes | SUPPORTED |
| List markers (inside and outside) and `<details>`/`<summary>` | SUPPORTED |

Blockification of flex and grid items, floats, absolutely positioned boxes and the root happens during style computation (`axiom-style`).

## Block formatting (`block.rs`, `float.rs`)

| Feature | Status |
|---------|--------|
| Widths, auto margins, `box-sizing`, `min-*`/`max-*`, `min-content`/`max-content`/`fit-content` widths | SUPPORTED |
| Margin collapsing between siblings, through parents and through empty boxes | SUPPORTED |
| Floats, clearance, formatting context roots beside floats | SUPPORTED |
| Relative positioning; `sticky` behaves as `relative` | PARTIAL |
| Absolute and fixed positioning, static position, shrink-to-fit | SUPPORTED |
| Shrink-to-fit and intrinsic (min/max-content) sizes for every formatting context | SUPPORTED |

## Inline formatting (`inline.rs`)

| Feature | Status |
|---------|--------|
| White-space processing across element boundaries | SUPPORTED |
| Line breaking at UAX #14 opportunities, shortened lines beside floats | SUPPORTED |
| Bidi reordering (UAX #9); shaping with rustybuzz | SUPPORTED |
| `text-align` including `justify` | SUPPORTED |
| Font-metric line boxes, `vertical-align` for inline boxes and atomics | SUPPORTED |

## Tables (`table.rs`)

Automatic column widths from cell min/max content, `table-layout: fixed`, `colspan`/`rowspan`, `border-spacing` and `border-collapse`, row groups, cell `vertical-align`, and captions (top only; `caption-side: bottom` is UNSUPPORTED).

## Flexbox (`flex.rs`)

The CSS Flexbox 1 §9 algorithm:

| Feature | Status |
|---------|--------|
| `flex-direction` (including `-reverse`), `flex-wrap` (including `wrap-reverse`), `order` | SUPPORTED |
| `flex-basis` (`auto`, `content`, lengths, percentages), `flex-grow`/`flex-shrink` with the min/max freeze loop | SUPPORTED |
| Automatic minimum size (`min-width: auto`): the content size suggestion capped by a definite size; `0` for scroll containers | SUPPORTED |
| `justify-content` (all distribution values), main-axis `auto` margins | SUPPORTED |
| `align-items`/`align-self` (`stretch`, `start`/`end`, `center`, `baseline` in rows), cross-axis `auto` margins | SUPPORTED |
| `align-content` for multi-line containers | SUPPORTED |
| `row-gap`/`column-gap`/`gap` | SUPPORTED |
| Column containers with indefinite height clamped by `min-height`/`max-height` (the sticky-footer pattern) | SUPPORTED |
| Right-to-left rows, first/last baselines of the container, `inline-flex` shrink-to-fit | SUPPORTED |
| Intrinsic sizes of the container | PARTIAL (contributions use width, not `flex-basis`) |
| Static position of absolutely positioned children (not aligned by `justify-content`) | PARTIAL |
| Fragmentation, `visibility: collapse` items | UNSUPPORTED |

## Grid (`grid.rs`, parsing in `axiom-style/src/grid.rs`)

Track lists, areas and line placements are parsed when styles are computed, so lengths in them are already resolved against font and viewport units.

| Feature | Status |
|---------|--------|
| `grid-template-columns`/`-rows`: lengths, percentages, `fr`, `auto`, `min-content`, `max-content`, `minmax()`, `fit-content()`, `repeat(<n>)`, named lines | SUPPORTED |
| `repeat(auto-fill)` / `repeat(auto-fit)` (empty `auto-fit` tracks collapse) | SUPPORTED |
| `grid-template-areas` and `grid-area` names, implicit `<name>-start`/`-end` lines | SUPPORTED |
| Line numbers (including negative), named lines, `span <n>`, `span <name>` | SUPPORTED |
| Implicit tracks from `grid-auto-rows`/`-columns` (before and after the explicit grid) | SUPPORTED |
| Auto-placement, `grid-auto-flow: row | column` and `dense` | SUPPORTED |
| Track sizing: fixed, intrinsic contributions (narrowest spans first), maximize, `fr` resolution, stretching `auto` tracks | SUPPORTED |
| `justify-items`/`-self`, `align-items`/`-self`, `justify-content`/`align-content`, gaps, auto margins | SUPPORTED |
| Right-to-left column order, `inline-grid`, container intrinsic sizes | SUPPORTED |
| Line names on the far side of an `auto-fill`/`auto-fit` repeat (they attach to the line before it) | PARTIAL |
| Baseline self-alignment | PARTIAL (treated as `start`) |
| Subgrid, masonry | UNSUPPORTED |

Percentage-sized replaced elements contribute nothing to min-content sizes (CSS Sizing 3 §5.2.2), so responsive images do not force flex items or grid tracks wider.

## Fixtures and tests

- Unit tests: `crates/axiom-layout/src/tests.rs` (block, inline, tables, flex and grid geometry).
- Pixel fixtures: `tests/rendering/*.html`, checked by `crates/axiom-engine/tests/render_fixture.rs`, including `flex-grid.html`.
- WPT reftests (`css/css-display`, CSS2 box model) through `cargo run -p axiom-compat -- reftests`.
