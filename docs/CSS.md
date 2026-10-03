# CSS

Stylesheets are parsed in `axiom-css`; cascade and computed values in `axiom-style`. User-agent defaults are injected from built-in rules (e.g. `display: none` for `head`, `script`, …).

Axiom does **not** use WebKit/Blink style engines.

## Selectors (parser + cascade)

| Selector | Phase 2 |
|----------|---------|
| Type, universal `*` | **SUPPORTED** |
| `.class`, `#id` | **SUPPORTED** |
| Descendant (whitespace) | **SUPPORTED** |
| Child `>`, adjacent `+`, general `~` | **PARTIAL** (DOM selector engine) |
| Attribute `[name]`, `[name="val"]` | **PARTIAL** |
| `:hover`, `:focus`, `:first-child`, `:nth-child()` | **PARTIAL** (pseudo parsed; matching varies) |
| Specificity + source order | **SUPPORTED** |
| `!important` | **PARTIAL** |

DOM-side `query_selector` reuses the extended selector parser in `axiom-dom::selector`.

## Properties (computed)

| Property | Status |
|----------|--------|
| `display`: none, block, inline, inline-block | **SUPPORTED** |
| `display`: flex, inline-flex, grid, inline-grid | **SUPPORTED** (see `docs/LAYOUT.md`) |
| `width`, `height`, `margin`, `padding` | **PARTIAL** (px, %, auto) |
| `border-width`, `border-color` | **PARTIAL** (uniform edges) |
| `color`, `background-color` | **SUPPORTED** |
| `font-size`, `font-weight`, `line-height`, `text-align` | **SUPPORTED** (subset) |
| `font-family` | **PARTIAL** (fallback to bundled/default) |
| `overflow` / scroll | **PLANNED** |
| `position` / `z-index` | **UNSUPPORTED** |
| `calc()`, custom properties | **PLANNED** (Wave B scaffolding) |

## At-rules

| Rule | Status |
|------|--------|
| `@media` | **UNSUPPORTED** |
| `@keyframes` | **UNSUPPORTED** |
| `@import` | **PLANNED** |

## Author styles

| Source | Status |
|--------|--------|
| `<style>` blocks in document | **SUPPORTED** |
| External `.css` via network | **PLANNED** |
| Inline `style=""` attributes | **PLANNED** |

See `demos/selectors.html`, `demos/box-model.html`, `demos/typography.html`.
