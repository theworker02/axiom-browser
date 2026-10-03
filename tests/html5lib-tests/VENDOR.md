# Vendored html5lib-tests (tokenizer)

- Upstream: https://github.com/html5lib/html5lib-tests
- Pinned commit: `224991ec10db04f056a89eed8b0bd8695fd2950e` (2026-06-26)
- License: MIT, see `LICENSE` (upstream file, unmodified)
- Refresh: `tools/compat/vendor.ps1` (never run by CI; suites run offline)

Only `tokenizer/*.test` is vendored. The tree-construction `.dat` files are now
maintained in web-platform-tests and are vendored under
`tests/wpt/html/syntax/parsing/resources/`.
