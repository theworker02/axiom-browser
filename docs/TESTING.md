# Testing

Axiom tests **owned engine behavior** with Rust tests, fixtures, and (Phase 2) demo pages — not by diffing against Chromium.

## Automated

| Kind | Location | Phase 2 |
|------|----------|---------|
| Workspace unit/integration tests | `cargo test --workspace` | **SUPPORTED** |
| Render fixture (PPM hash/compare) | `crates/axiom-engine/tests/` | **SUPPORTED** |
| HTML/CSS fixtures | `tests/html/`, `tests/css/` | **SUPPORTED** |
| DOM selector tests | `axiom-dom` | **SUPPORTED** |
| Fixture runner CLI | `tools/test-runner` | **SUPPORTED** |
| Conformance suites (html5lib tree + tokenizer, WPT crashtests, reftests, testharness.js) with an expectations ratchet | `tools/compat`, `tests/expectations/` | **SUPPORTED** |

```bash
cargo test --workspace
cargo run -p axiom-test-runner -- # if configured in workspace
cargo run -p axiom-compat -- all  # conformance suites vs. tests/expectations/
```

The conformance corpus is vendored and pinned (`tests/wpt`, `tests/html5lib-tests`) and
runs offline against a loopback server. `cargo test --workspace` includes the ratchet
(`tools/compat/tests/ratchet.rs`), which fails on regressions and on unexpected passes.
Results, limitations and the update workflow are in [COMPATIBILITY.md](COMPATIBILITY.md).

## Manual / interactive

| Asset | Purpose |
|-------|---------|
| `demos/*.html` | Feature-focused pages for Phase 2 |
| `demos/browser-test.html` | Combined smoke test |

Run (interactive shell target):

```bash
cargo run -p axiom-desktop -- demos/browser-test.html
cargo run -p axiom-desktop -- --headless demos/typography.html
```

Paths are relative to the **current working directory** (repo root). HTTPS demos require network; use `--headless` to write `axiom-frame.ppm` without a window.

## Planned (Wave C)

| Item | Status |
|------|--------|
| Screenshot reference tests + failure artifacts | **PARTIAL** (WPT reftests compare frames; failures are not saved as images, but `cargo run -p axiom-compat --example wpt_render -- <path> out.ppm` renders any test) |
| WPT harness scaffolding (subset) | **SUPPORTED** (`tools/compat`, see [COMPATIBILITY.md](COMPATIBILITY.md)) |
| JSON benchmark export | **PLANNED** |
| Fuzzing HTML/CSS parsers | **PLANNED** |

## Status legend (docs)

- **SUPPORTED** — implemented and used in Phase 1/2 path today
- **PARTIAL** — subset, missing integration, or incomplete spec behavior
- **PLANNED** — scheduled in Phase 2 plan
- **UNSUPPORTED** — explicit non-goal for Phase 2

When fixing rendering/layout bugs, add or extend a fixture under `tests/` per [AGENTS.md](../AGENTS.md).
