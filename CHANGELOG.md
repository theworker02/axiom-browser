# Changelog

See the detailed project changelog:

**[`docs/CHANGELOG.md`](docs/CHANGELOG.md)**

## Latest

### Unreleased

### 1.2.8 — 2026-10-03

Compatibility milestone: XML/XHTML documents now create live script realms after atomic parsing,
discover resources through the shared loader, and run classic scripts in document order.
Same-origin frame document proxies expose MIME type, detached `createElement()` factories and
`namespaceURI`. XML XHR document responses use the existing `DOMParser`; non-HTML documents keep
their actual response MIME type. Focused WPT passes: XHTML CDATA 8 / 8; iframe namespace factory
52 / 52; Document.contentType 26 / 30. The remaining `data:` and `javascript:` iframe navigation
cases are not claimed as implemented.

XML/XHTML document-realm integration: `application/xhtml+xml`, XML and SVG documents now finish
their atomic parse into a live document realm, discover resources through the shared loader, and
run classic scripts in document order. The offline WPT CDATA XHTML file now passes all 8 outcomes;
the document no longer incorrectly applies HTML-only CDATA rules. Incremental XML parsing and
parser-paused XML scripts remain future work.

Same-origin frame document proxy: `iframe.contentDocument` now exposes the child document's MIME
type plus a detached `createElement()` factory and returned node `namespaceURI`. The WPT
`Document-createElement-namespace` page now passes all 52 outcomes across HTML, XHTML, XML and
SVG child documents. Cross-realm insertion, mutation and event identity are still intentionally
outside this narrow proxy surface.

Phase 3 Wave I (Content Security Policy): a new `axiom-csp` crate and enforcement of header and `<meta>` policies (enforce and report-only) for subresources and their redirects, inline scripts, styles, `style` attributes and event handlers, `eval`, `fetch()`, form submissions and `<base>`, plus `upgrade-insecure-requests`. Violations are logged to the console and fire `securitypolicyviolation` events; report delivery is not implemented. See `docs/CSP.md` and `docs/SECURITY.md`.

Phase 3 Wave H (CORS): preflight requests through the profile's network service, a per-profile in-memory preflight cache, per-hop CORS checks on cross-origin redirects with origin tainting, and subresource CORS (`crossorigin` on scripts, stylesheets and images; module scripts and fonts always use CORS). Windows binaries now get an 8 MiB main-thread stack after the live smoke run hit a stack overflow on Google results. See `docs/FETCH.md`, `docs/RESOURCE_LOADING.md` and `docs/SECURITY.md`.

Phase 3 Wave G (browser compatibility): the seven live-site blockers from Wave F, fixed in order with a live smoke run after each: glyph placement; web fonts (TTF, OTF, WOFF 1.0) plus float, table and paint-order fixes; `navigator`, timers, `requestAnimationFrame`, `matchMedia` and `element.style`; ES module scripts and import maps; SVG images; form submission with all four encodings and a resubmission guard; and flexbox and CSS grid layout. testharness 4942 / 6589, reftests 887 / 1034. See `docs/LAYOUT.md` and `docs/WEB_COMPATIBILITY.md`.

Phase 3 Wave F (real web navigation and search providers): an omnibox classifier backed by the Public Suffix List with IP, port and IDN handling; a search provider service (Google, Bing, DuckDuckGo, and an Axiom Search placeholder) whose choice persists per profile and carries into private windows; `axiom://settings/search`; non-blocking navigation in the desktop UI, driven by engine lifecycle events; an address bar and history that follow redirects, link clicks and failures correctly; `Accept-Language`; basic CORS for simple `fetch()`; and a manual live-site smoke probe. See `docs/NETWORKING.md`, `docs/SEARCH_PROVIDERS.md`, `docs/AXIOM_SEARCH_ARCHITECTURE.md` and `docs/WEB_COMPATIBILITY.md`.

`Attr` and events wave: `Attr`/`NamedNodeMap`, one `EventTarget` implementation for every target, `Event` subclasses, `document.createEvent`, event handler attributes, and a patched `boa_gc` that no longer hangs on `WeakMap` values; the testharness suite adds `dom/events`, `dom/collections` and `dom/lists` (3408 → 4780 / 6544 passing outcomes). See `docs/DOM.md`.

DOM core wave: a spec-shaped DOM interface hierarchy with checked mutations, live collections, a Selectors Level 4 engine, `innerHTML`/`outerHTML`, `MutationObserver`, and a patched Boa that no longer reports handled promise rejections as unhandled (testharness `dom/nodes` 681 → 3408 / 5342 passing outcomes). See `docs/DOM.md`.

Compatibility harness: offline, pinned html5lib and WPT suites (tree construction, tokenizer, crashtests, reftests, testharness.js `dom/nodes`) with an expectations ratchet in `cargo test`, and a WHATWG tree builder with processing instructions and `<selectedcontent>` (html5lib tree construction 91 → 1953 / 1959). See `docs/COMPATIBILITY.md`.

Phase 3 Wave G (document loading): streaming HTML parse, a per-document resource registry and generation guard, state-driven script scheduling (inline, blocking, async, defer, dynamic), `DOMContentLoaded`/`load` lifecycle, stylesheet `@import` and fonts through the loader, `axiom://document`, a controlled test site and `docbench`.

Phase 3 Wave E (Networking 2.0): disk-backed per-profile HTTP cache, download manager foundation, typed network errors and limits, TLS certificate metadata and a verified-TLS security indicator, superseding navigations, network activity stream and the `axiom://network/<id>` detail page.

### 0.2.0 — 2026-09-26

Phase 3 Waves A–C: multi-tab browser, omnibox chrome, and durable profile persistence (SQLite schema v1, history, bookmarks, settings, sessions, private isolation).
