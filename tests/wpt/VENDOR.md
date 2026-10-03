# Vendored web-platform-tests subset

- Upstream: https://github.com/web-platform-tests/wpt
- Pinned commit: `f085a1efc1f58fbe263d384b1e335d656fe58e66` (2026-09-27)
- License: BSD 3-Clause, see `LICENSE.md` (upstream file, unmodified)
- Refresh: `tools/compat/vendor.ps1` (never run by CI; suites run offline)

Files keep their upstream paths, so `/resources/testharness.js`, `/common/...` and
`/fonts/Ahem.ttf` resolve against this directory as the server root. Python handlers
(`*.py`) are not vendored: the local server does not run wptserve handlers, and tests
that need them are expected to fail (recorded in `tests/expectations/`).

| Path | Used by |
|------|---------|
| `html/syntax/parsing/resources/*.dat` | html5lib tree-construction runner |
| `html/syntax/parsing/crashtests`, `dom/nodes/crashtests`, `css/css-{backgrounds,color,flexbox}/crashtests` | crashtest runner |
| `dom/nodes`, `dom/events`, `dom/collections`, `dom/lists` | testharness.js runner |
| `css/CSS2/{colors,box-display,margin-padding-clear}`, `css/css-display` | reftest runner |
| `resources/testharness*.js`, `common/`, `fonts/Ahem.ttf` | support files |

Nothing in this directory is edited by hand. Axiom-specific expectations live in
`tests/expectations/`.
