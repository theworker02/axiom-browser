# Phase 3 Progress

Date: 2026-09-29
Stopped at: the end of **Phase 3 Wave I (Content Security Policy)**. (The older "Phase 3 Wave G" section further down is the document loading wave, which came before Wave G browser compatibility.) Axiom Search crawler and index work has not been started. The open security follow-ups are CSP report delivery and error muting for cross-origin scripts.

## Post-Wave I compatibility follow-up: DOM Range, XML and nested-context audit

Date: 2026-10-02. This follow-up continues the WPT-driven compatibility program; it is not a claim of general modern-web completion.

| Piece | Status |
|-------|--------|
| `Range` and `document.createRange()` | FUNCTIONAL for boundary setting/comparison, node/content selection, text extraction, cloning, deletion, insertion, surround, contextual fragments and live boundary updates through tree mutations, character-data changes, `splitText()` and `normalize()` |
| `Selection` / `getSelection()` | PARTIAL — one document selection with range, collapse/select/delete/string APIs; no geometry, editing or form-control text selection |
| WPT regression coverage | FUNCTIONAL — `MutationObserver-childList` Range cases and `Node-normalize-2` now pass; the testharness ratchet was refreshed intentionally |
| Inert `createHTMLDocument`, `createDocument`, `DOMParser` and XML parsing | FUNCTIONAL; confirmed by engine tests. These existed before this follow-up but were incorrectly listed as unsupported in older documentation. |
| Top-level XHTML/XML navigation | FUNCTIONAL for `application/xhtml+xml`, `application/xml`, `text/xml` and `image/svg+xml`: the network document loader retains MIME dispatch, decodes the stream, commits a well-formed XML DOM atomically, and retains a parser-error document for malformed input. XML is intentionally complete-body parsed; incremental XML parsing is deferred. |
| XML CDATA | FUNCTIONAL — `NodeKind::CData`, XML parser preservation, `CDATASection` node type/data and XML serialization are covered by regression tests. |
| Nested browsing contexts / `<iframe>` | PARTIAL — HTTP(S) `src`, `srcdoc` (which takes precedence) and source-less same-origin documents each own a distinct child `BrowsingContext`, DOM/JS realm and resource-loader context. Children are sized from their embedding layout box, tick through the parent-owned event-loop boundary and their framebuffers are composited and clipped into the parent surface. Settled children dispatch `load`; failed/unsupported child navigations dispatch `error`. Same-origin `contentDocument` and `contentWindow.document` expose a deliberately scoped cross-realm proxy (`URL`, `body`/`head`, `getElementById`, `querySelector`, node text/tag name); every lookup carries the embedding iframe identity plus the child-local node ID. Cross-origin and sandboxed opaque-origin access is denied. `contentWindow.postMessage()` performs JSON cloning, target-origin filtering and child-turn event delivery. `sandbox` without `allow-scripts` blocks classic/module execution; without `allow-forms` it blocks form navigation. Full WindowProxy behavior, cross-realm mutation/event identity, popup/top navigation policy, transferable structured cloning and credential partitioning remain deferred. |

New regression coverage lives in `crates/axiom-engine/tests/dom_node_bindings.rs` and `crates/axiom-browser/tests/xml_and_frames.rs`. The next compatibility boundary is completing nested browsing-context semantics and rendering; custom elements and traversal APIs remain after that.

## Completed work

### Phase 3 Wave I: Content Security Policy

Goal: enforce the document's Content Security Policy, the security follow-up left by Wave H. Details: `docs/CSP.md` (directives, matching, behaviour, limitations), `docs/SECURITY.md` (Wave I review).

| Piece | Status |
|-------|--------|
| `axiom-csp` (new crate): policy parsing (headers, `<meta>`, enforce and report-only), source lists, nonces, hashes, `'strict-dynamic'`, `'unsafe-hashes'`, fallback chains, Chrome-worded violation messages; SHA-2 and base64 shared with SRI | FUNCTIONAL |
| Policies from `Content-Security-Policy` / `-Report-Only` headers and `<meta http-equiv>` in `<head>` | FUNCTIONAL |
| Subresources checked before any request (scripts, module imports, stylesheets, `@import`, images, fonts, media, manifests); the preload scanner skips requests a policy would block | FUNCTIONAL |
| Redirect guard: `RedirectCheck` on `NetworkRequest` / `ResourceRequest`, run by the network service on every hop before sending it | FUNCTIONAL |
| Inline scripts, inline modules, import maps, `<style>`, `style` attributes, event handler attributes | FUNCTIONAL |
| `eval()` / `Function()` gated per realm (`EvalGate`), `EvalError` when refused | FUNCTIONAL |
| `fetch()` (`connect-src`), `form-action`, `base-uri`, `upgrade-insecure-requests` | FUNCTIONAL (no upgrade of navigations or redirects) |
| Console reports and trusted `securitypolicyviolation` events; `BrowsingContext::csp_violations()` | FUNCTIONAL |
| Report delivery (`report-uri`, `report-to`), `frame-ancestors`, `sandbox`, Trusted Types | NOT STARTED |

Found and fixed along the way: form submission resolved actions against the first `<base href>` in the DOM on its own. It now shares the loader's frozen base, so a `<base>` refused by `base-uri` cannot redirect forms.

Tests: `axiom-browser/tests/wave_i.rs` (11), `axiom-net/tests/redirect_check.rs` (2), `axiom-csp` unit tests (31), `axiom-engine` `csp::tests` (6). Workspace: 644 tests in 81 suites, all passing; fmt and clippy clean. testharness 4942 / 6589 and reftests 887 / 1034, unchanged. Live smoke (`i1`): all nine targets load with the same stage ratings as `h1` and no CSP violation on any site. MDN's external script count varies between runs (7 in `h1`; 5 and 6 in two Wave I runs, none failed), which is timing on the site, not a policy block.

### Crates changed in Phase 3 Wave I

- `axiom-csp` (new)
- `axiom-net`: `RedirectCheck`, `NetworkRequest::redirect_check`, checked in `check_request_policies`; new `tests/redirect_check.rs`
- `axiom-loader`: `ResourceRequest::redirect_check`
- `axiom-html`: the parser reports `<meta>` elements to the loader
- `axiom-style`: `StyleEngine::set_style_attribute_filter`
- `axiom-js`: `EvalGate` and `JsContext::set_eval_policy`; `__axiom_compileHandler` and `JsHost::csp_allows_handler`; `SecurityPolicyViolationEvent` and `JsContext::dispatch_csp_violation`
- `axiom-engine`: new `csp.rs` (`DocumentCsp`); enforcement in `browsing.rs`, `browsing/document.rs`, `browsing/resources.rs` and `page.rs`; `sri.rs` uses `axiom-csp`'s hashes; `forms.rs` uses the loader's base URL
- `axiom-browser`: new `tests/wave_i.rs`

### Phase 3 Wave H: CORS

Goal: finish the CORS protocol left over from Wave F (see "Exact Wave H handoff" below), in one place that `fetch()` and subresources share. Details: `docs/FETCH.md` (cross-origin table), `docs/RESOURCE_LOADING.md` (subresource modes), `docs/SECURITY.md` (Wave H review).

| Piece | Status |
|-------|--------|
| CORS moved into the network service (`axiom-net/src/cors.rs`): safelisted methods and headers, forbidden headers, the response check, origin serialization; `NetworkError::Cors` (kind `cors`) | FUNCTIONAL |
| Preflight (`OPTIONS`) for non-safelisted methods and headers and for streamed bodies, through the same `NetworkService` (own log row, initiator `cors-preflight`); no credentials, no redirects; a failure sends nothing of the actual request | FUNCTIONAL |
| Preflight cache per profile, memory only (private profiles too), keyed by origin, URL and credentials mode; `Access-Control-Max-Age` default 5 s, cap 2 h; cleared by `Browser::clear_cache` and shutdown | FUNCTIONAL |
| `cors` redirects: a CORS check on every cross-origin hop instead of a refusal; origin tainted to `null` after a foreign-to-foreign hop; `same-origin` credentials stop at the first cross-origin hop. `same-origin` mode still refuses cross-origin redirects | FUNCTIONAL |
| `fetch()`: opaque-origin documents may make CORS requests (`Origin: null`, no credentials); `filter_response` repeats the check with the tainted origin | FUNCTIONAL |
| Subresource CORS: `crossorigin` on `script`, `link rel=stylesheet` and `img`; module scripts and their imports; `@font-face` fonts; the preload scanner records the CORS state and the parser only claims a preload made in the same mode | FUNCTIONAL (imports do not inherit `use-credentials`; CSS `@import` stays `no-cors`) |
| Pre-CORS guarantees kept (`wave_g.rs`): no bytes of a failing preflighted request, opaque `no-cors` bodies never reach JS, `Set-Cookie` never exposed | FUNCTIONAL |
| Windows binaries (`axiom`, `axiom-test-runner`, the `axiom-browser` examples) linked with an 8 MiB main-thread stack, like Linux and macOS. The live smoke found Google results overflowing the 1 MiB default: recursion through native callbacks up to the JS recursion limit needs about 1.5 MiB | FUNCTIONAL (`wave_h::runaway_recursion_through_native_callbacks_stops_at_the_limit`) |
| Muting errors from cross-origin `no-cors` scripts (`"Script error."`) | NOT STARTED (see `docs/SCRIPT_LOADING.md`) |

Tests: `axiom-net/tests/cors.rs` (8), `axiom-browser/tests/wave_h.rs` (5), `cors::tests` (7), plus unit tests in `axiom-document` and `axiom-engine`; two `wave_g.rs` tests updated for the new behaviour. Workspace: 595 tests in 77 suites, all passing; fmt and clippy clean. testharness 4942 / 6589 and reftests 887 / 1034, unchanged. Live smoke (`h1`): all nine targets load; the results match Wave G's last run, with no CORS failures on GitHub, MDN, Wikipedia or react.dev.

### Crates changed in Phase 3 Wave H

- `axiom-net`: new `cors.rs` (rules, `PreflightCache`); `NetworkService::run` CORS state, preflight, per-hop checks and tainting; `preflight_cache()`, `clear_cache()`; `NetworkRequest::{unsafe_request, use_cors_preflight}`; `NetworkError::Cors`; new `tests/cors.rs`
- `axiom-loader`: `ResourceRequest::{unsafe_request, use_cors_preflight}`
- `axiom-document`: `CorsSettings` (`crossorigin` states) on resources, preload hints and preload matching
- `axiom-engine`: `fetch.rs` uses the network service's CORS; `browsing/resources.rs` sets the request mode per subresource
- `axiom-browser`: `Browser::clear_cache`; new `tests/wave_h.rs`; `build.rs` (example stack size)
- `browser/desktop`, `tools/test-runner`: `build.rs` (main-thread stack size on Windows)

### Phase 3 Wave G (browser compatibility): the Wave F live-site blockers

Goal: fix, in order, the blockers `docs/WEB_COMPATIBILITY.md` recorded on live sites in Wave F. Each step re-ran the live smoke probe (`examples/live_smoke.rs`). The results and the remaining gaps are in `docs/WEB_COMPATIBILITY.md`.

| Step | Piece | Status |
|------|-------|--------|
| G1 | Glyph placement: bitmaps drawn at `baseline − (height + ymin)` instead of `baseline + ymin`; fixture `tests/rendering/glyph-baseline.html` | FUNCTIONAL |
| G2 | Web fonts (TTF, OTF, WOFF 1.0) used for rendering; `ex`/`ch` units; float, table and paint-order fixes; `<details>`; named window properties; XHTML CDATA scripts; fixture `tests/rendering/floats-tables-block-in-inline.html` | FUNCTIONAL (no WOFF2) |
| G3 | `navigator` (`userAgent`, `language(s)`, `onLine`, `cookieEnabled`, `hardwareConcurrency`), `setInterval`/`clearInterval`, `requestAnimationFrame`, `matchMedia`, `queueMicrotask`, `innerWidth`/`innerHeight`/`devicePixelRatio`, `element.style` (`CSSStyleDeclaration`) | FUNCTIONAL |
| G4 | ES module scripts: module map, static and dynamic `import`, import maps merged per document; a patched `boa_parser` for `let of` (`third_party/boa_parser/AXIOM_PATCH.md`) | FUNCTIONAL |
| G5 | SVG images through resvg, rasterized at intrinsic size, gzip-compressed SVG accepted; fixture `tests/rendering/svg-image.html` | FUNCTIONAL for `<img>`; inline `<svg>` NOT STARTED |
| G6 | Form submission: form control state shared by layout, input and script; GET, `application/x-www-form-urlencoded`, `multipart/form-data` and `text/plain`; implicit submission; the POST resubmission guard with a trusted error page; SameSite initiator for form navigations; Space activates buttons; `new FormData(form)`. testharness +111 | FUNCTIONAL (no file picker, no form named properties) |
| G7 | Flexbox and CSS grid formatting contexts (`docs/LAYOUT.md`); typed grid values in `axiom-style`; fixture `tests/rendering/flex-grid.html`. Reftests +10 | FUNCTIONAL |

testharness: 4942 / 6589. Reftests: 887 / 1034.

### Crates changed in Phase 3 Wave G (browser compatibility)

- `axiom-paint`: glyph bitmap placement, paint order
- `axiom-text`: web font loading and decoding (`fonts.rs`; WOFF 1.0 via `flate2`)
- `axiom-style`: `ex`/`ch`; new `grid.rs` with typed track lists, areas, auto-flow and line placements
- `axiom-layout`: float and table fixes, `<details>`; new `flex.rs` and `grid.rs`; flex/grid items in the box tree (`BoxNode::item`)
- `axiom-document`: CDATA sections in XHTML scripts unwrapped (`script.rs`)
- `axiom-js`: new `timers_prelude.js`, `cssom_prelude.js`, `forms_prelude.js` and `modules.rs` (module loader and map); viewport metrics in `window_prelude.js`; `tests/parser_regressions.rs`
- `axiom-engine`: new `import_map.rs` and `forms.rs` (form state, submission, encodings); module fetching in `browsing/document.rs`; the resubmission guard in `browsing.rs`; named window properties; tests `timers_navigator.rs`, `forms_dom.rs`, `render_fixture.rs` fixtures
- `axiom-loader`: new `svg.rs` (resvg)
- `axiom-browser`: form submissions recorded in history (`tab.rs`); `live_smoke` submits the DuckDuckGo form; tests `modules.rs`, `svg_images.rs`, `form_submission.rs`
- `third_party/boa_parser` (new): patched `boa_parser`, excluded from the workspace

### Phase 3 Wave F: Real web navigation, search providers and the internet-ready pipeline

Goal: make typing a URL or a search into the omnibox behave like a real browser on the real web, on top of the existing network stack. The audit is `docs/WAVE_F_NETWORK_AUDIT.md`, the per-item plan `docs/WAVE_F_PLAN.md`, the details `docs/NETWORKING.md` and `docs/SEARCH_PROVIDERS.md`, and live-site results `docs/WEB_COMPATIBILITY.md`.

| Piece | Status |
|-------|--------|
| Omnibox classifier: PSL-validated suffixes, IPv4/IPv6, ports, IDN, quoted phrases, `?` forced search, unsafe schemes searched | FUNCTIONAL |
| `axiom-url` host canonicalization (lowercase, IDNA punycode, forbidden code points rejected) | FUNCTIONAL |
| `SearchProviderService` (Google, Bing, DuckDuckGo default, Axiom Search placeholder, custom template); the omnibox never hard-codes an engine | FUNCTIONAL |
| Per-profile provider choice (settings schema v2), `axiom://settings/search`; private windows inherit it without writing back | FUNCTIONAL |
| Axiom Search: honest placeholder page and `docs/AXIOM_SEARCH_ARCHITECTURE.md` | FUNCTIONAL (design only; no crawler or index) |
| Engine navigation lifecycle events (`Started` … `Completed`, `Failed`, `Cancelled`, `Download`, `InternalRequested`) and `NavigationState` | FUNCTIONAL |
| Desktop background navigation: the UI thread never waits on the network | FUNCTIONAL |
| Address bar follows redirects and link clicks; history records one visit per navigation under the final URL; failed navigations keep history and reload retries them; downloads leave the page alone | FUNCTIONAL |
| Web content cannot open `axiom://` pages | FUNCTIONAL |
| `Accept-Language`; configurable, honest User-Agent (no Chrome impersonation) | FUNCTIONAL |
| Error pages show the stable error code, never library text | FUNCTIONAL |
| Basic CORS for simple cross-origin `fetch()` (`cors` response type, exposed headers, credentials) | FUNCTIONAL; preflight added in Wave H |
| Manual live smoke probe (`examples/live_smoke.rs`), Google milestone | Done manually; results in `docs/WEB_COMPATIBILITY.md` |

### Crates changed in Phase 3 Wave F

- `axiom-url`: canonical hosts (IDNA via `idna`)
- `axiom-net`: `NetworkServiceConfig::{user_agent, accept_language}`, `DEFAULT_ACCEPT_LANGUAGE`
- `axiom-engine`: new `navigation.rs` (events, states, causes); `BrowsingContext` blocking/background modes, `take_navigation_events`, `display_url`, failed-navigation history, download handling without replacing the document; `fetch.rs` basic CORS
- `axiom-js`: `fetch_prelude.js` exposes bodies of `cors` responses
- `axiom-browser`: rewritten `classifier.rs`, `search.rs` (`SearchProviderService`), new `search_pages.rs`, settings schema v2, `Browser::process_navigation_events`, `set_background_navigation`, `set_search_provider`, `new_private_inheriting`, `BrowserDataStore::read_settings`, `handle_content_click`; new `tests/wave_f_navigation.rs` and `examples/live_smoke.rs`
- `axiom-gfx`: the window loop uses background navigation and `handle_content_click`
- `browser/desktop`: `--private` inherits the profile's settings read-only

### `Attr` and events wave

Goal: finish the attribute node model and give every event target one spec-shaped events layer, measured against WPT `dom/events`, `dom/collections` and `dom/lists` (newly vendored) as well as `dom/nodes`. testharness went from 3408 to 4780 passing outcomes (6544 in total; files 453 / 585, subtests 4327 / 5959). Details are in `docs/DOM.md` (Events section) and `docs/COMPATIBILITY.md`.

| Piece | Status |
|-------|--------|
| `Attr` / `NamedNodeMap`: identity-stable attribute nodes, `attributes`, `get/set/removeAttributeNode(NS)`, `createAttribute(NS)` | FUNCTIONAL |
| One `EventTarget` implementation (`events_prelude.js`) for nodes, `window`, `AbortSignal` and `performance`; capture/target/bubble dispatch; `once`, `passive` (with passive-by-default targets), `signal`; `handleEvent`; `window.event` | FUNCTIONAL (no shadow DOM retargeting) |
| `Event` and 27 subclasses (all but `TextEvent` and `BeforeUnloadEvent` constructible), dictionary conversion and legacy `init*Event`; `document.createEvent` alias table | FUNCTIONAL |
| Event handler IDL and content attributes (lazy compilation with document/form/element scopes, `<body>` forwarding to `window`) | FUNCTIONAL |
| Listener exceptions reported as `ErrorEvent` at `window` | FUNCTIONAL (top-level script errors still go only to the script error log) |
| Engine clicks as trusted `MouseEvent`s; canceling skips the default action; `HTMLElement.click()`; `disabled` reflection | FUNCTIONAL |
| Activation behavior for script-dispatched clicks, form-control state (`checked`, `value`) | NOT STARTED |
| Boa GC hang with object values in live `WeakMap` entries, fixed in a patched `boa_gc` 0.20.0 (`third_party/boa_gc`) | FUNCTIONAL |
| Boa panic on named function expressions inside `with`, avoided in handler compilation | FUNCTIONAL (page scripts using the pattern still panic) |
| Engine tests: `axiom-engine/tests/dom_events.rs` (11), Attr cases in `dom_node_bindings.rs`, `axiom-js/tests/realms.rs` (2) | FUNCTIONAL |

### Crates changed in the `Attr` and events wave

- `axiom-js`: new `events_prelude.js`; `platform_prelude.js`, `fetch_prelude.js` (`AbortSignal` extends `EventTarget`), `document_prelude.js` and `window_prelude.js` (`Performance`) use it; `dom_prelude.js` gains `Attr`/`NamedNodeMap`, the event path, handler attributes, `createEvent`, `click()` and `disabled`; `__axiom_now` native; `JsContext::dispatch_event` reports cancellation
- `axiom-engine`: a canceled click skips default actions
- `third_party/boa_gc` (new): `boa_gc` 0.20.0 with the ephemeron marking fix, excluded from the workspace
- `tools/compat`: `dom/events`, `dom/collections`, `dom/lists` in the testharness suite and `vendor.ps1`

### DOM core wave

Goal: raise WPT `dom/nodes` through a spec-shaped DOM surface. testharness went from 681 to 3408 passing outcomes (5342 in total; files 307 / 388, subtests 3101 / 4954). Details are in `docs/DOM.md` and `docs/COMPATIBILITY.md`.

| Piece | Status |
|-------|--------|
| `axiom-dom`: ordered namespaced attributes, arbitrary element namespaces and prefixes, name validation, spec pre-insert/replace checks, `import_node` | FUNCTIONAL |
| JS interface hierarchy (`Node` … per-tag `HTML*Element`, `CharacterData`, `Document`), constructors, factories, identity-stable wrappers | FUNCTIONAL (no `Attr`, no additional documents) |
| Tree mutation and `ParentNode`/`ChildNode` APIs with `DOMException`s; `cloneNode`, `isEqualNode`, `compareDocumentPosition`, `normalize`, `CharacterData` methods | FUNCTIONAL |
| Live `HTMLCollection`/`NodeList`, static `querySelectorAll` lists, `DOMTokenList` | FUNCTIONAL |
| Selectors Level 4 engine (`axiom-dom::selector`) for `querySelector*`, `matches`, `closest` | FUNCTIONAL (not yet used by the style cascade) |
| HTML serializer (`axiom_html::serialize_*`); `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `template.content` | FUNCTIONAL |
| `MutationObserver` | FUNCTIONAL (script mutations only; the parser queues no records) |
| Unhandled-rejection false positives (Boa 0.20 `[[PromiseIsHandled]]` bug) fixed in a patched `boa_engine` 0.20.0 (`third_party/boa_engine`, `[patch.crates-io]`) | FUNCTIONAL |
| Engine tests: `axiom-engine/tests/dom_node_bindings.rs`, `axiom-browser/tests/wave_g2.rs` (rejection tracking) | FUNCTIONAL |
| `Range`, `Attr`/`NamedNodeMap`, `createHTMLDocument`/`DOMParser`, XML documents, custom elements | NOT STARTED (`Attr`/`NamedNodeMap` done in the `Attr` and events wave) |

### Crates changed in the DOM core wave

- `axiom-dom`: rewritten `selector.rs`; attribute model, namespaces, validation, `import_node`
- `axiom-html`: new `serialize.rs`
- `axiom-js`: new `dom_prelude.js`; slimmer `window_prelude.js`; new `JsHost` methods and natives (selectors, markup, `template_content`, namespaced attributes, node creation)
- `third_party/boa_engine` (new): `boa_engine` 0.20.0 with the `PerformPromiseThen` fix, excluded from the workspace
- `axiom-engine`: `DocumentJsHost` implements the new host methods; fragment parsing into the page document

### Compatibility harness wave

Web compatibility is now measured by offline conformance suites with an expectations ratchet, not by a "% of Chrome" figure. Details, per-directory results and limitations are in `docs/COMPATIBILITY.md`.

| Piece | Status |
|-------|--------|
| Vendored, pinned corpus: WPT subset (`tests/wpt`, BSD-3) and html5lib tokenizer tests (`tests/html5lib-tests`, MIT); `tools/compat/vendor.ps1` refresh, never run by CI | FUNCTIONAL |
| Expectations ratchet (`tests/expectations/*.txt`): fails on regressions, unexpected passes, status changes and stale entries; `--update` rewrites; enforced by `cargo test` | FUNCTIONAL |
| Loopback WPT-style server over the vendored tree (common `{{...}}` templates; no Python handlers) | FUNCTIONAL |
| html5lib tree-construction runner (1953 / 1959; the rest need parse-time script) | FUNCTIONAL |
| html5lib tokenizer runner (7017 / 7032; 11 upstream tests predate processing instructions) | FUNCTIONAL |
| WPT crashtest runner (26 / 26) | FUNCTIONAL |
| WPT reftest runner on the headless renderer, `match`/`mismatch`, `<meta name=fuzzy>` (693 / 1034) | FUNCTIONAL (no reference chains) |
| testharness.js runner for `dom/nodes` (files 222 / 298, subtests 114 / 3283 at the end of this wave; see the DOM core wave for current numbers) | FUNCTIONAL (no worker variants; SVG documents run no script) |
| Tree builder: full WHATWG insertion modes, implied `html`/`head`/`body`, adoption agency, foster parenting, templates, SVG/MathML, fragments, quirks mode | FUNCTIONAL (no parse errors, `document.write`, form owner) |
| Processing instructions (tokenizer, tree builder, DOM node) and `<selectedcontent>` cloning | FUNCTIONAL |
| Window / Node surface for testharness.js: `self`/`parent`/`top`, read-only `location`, `Node` constants, tree navigation, `nodeType`/`nodeName`, `getElementsByTagName`, `querySelectorAll`, `removeChild`/`remove`, identity-stable wrappers | FUNCTIONAL (collections were static arrays until the DOM core wave) |
| Fixture: `tests/html/implied-body.html` (content without `html`/`head`/`body` tags renders) | FUNCTIONAL |
| Real-site snapshot corpus | NOT STARTED |

### Crates changed in the compatibility harness wave

- `tools/compat` (new, `axiom-compat`): suites, runners, expectations, reports, loopback server, `examples/wpt_render.rs`, `tests/ratchet.rs`
- `axiom-html`: new `tree_builder.rs`; `parse_fragment`, `parse_html_with_scripting`, `HtmlParser::with_scripting`; `FOREIGN_ATTRIBUTES`
- `axiom-dom`: `Namespace`, `QuirksMode`, doctype public/system ids, element namespaces, template contents, `create_element_ns`, `set_attr_exact`, `descendants`, `ProcessingInstruction`
- `axiom-js`: `window_prelude.js`, identity-stable `ElementRef`, node navigation, collection and removal natives
- `axiom-engine`: `DocumentJsHost` implements the new host methods; `tests/dom_node_bindings.rs`; implied-body render test
- `axiom-css`: the UA sheet hides `template`, `base`, `area`, `datalist`, `noembed`, `noframes`, `param`, `rp`, `basefont`

### Waves A–E and D.1 (preserved)

- Tabs, chrome, profiles, cookies (PSL), web storage (schema v3)
- See `docs/COOKIES.md`, `docs/PROFILES.md`, `docs/PERSISTENCE.md`, `docs/STORAGE.md`

### Wave F + F.1: Networking 2.0 and the resource loading platform

The full per-item status, with the test behind each claim, is in `docs/NETWORKING.md`. Summary:

| Piece | Status |
|-------|--------|
| `NetworkService` boundary, typed requests and responses, stable request IDs | FUNCTIONAL |
| Async reqwest transport (rustls, verification on), mid-transfer cancellation | FUNCTIONAL |
| HTTP/1.1 keep-alive | FUNCTIONAL (server-verified connection count) |
| HTTP/2 | FUNCTIONAL, verified by ALPN negotiation against a local TLS server |
| HTTP/3 | DEFERRED |
| Own gzip/deflate/br decoding with transferred and decoded byte counts | FUNCTIONAL |
| RFC 9111 memory cache (freshness, validators, Vary, invalidation, cache modes, bounded tee) | FUNCTIONAL |
| Disk-backed cache | FUNCTIONAL (Wave E) |
| Redirect engine (method/body matrix, limit, cross-origin header stripping) | FUNCTIONAL |
| Cookies through `CookieProvider` → `CookieService` on every hop | FUNCTIONAL |
| Store-side SameSite context for `Set-Cookie` | PARTIAL |
| Scheduler: fixed pool, priority + aging, context cancellation, backpressure | FUNCTIONAL |
| One network service, cache and scheduler per profile; private profile isolated and cleared | FUNCTIONAL |
| Parallel CSS/script/image loading; CSS and script order preserved; images non-blocking | FUNCTIONAL |
| Web fonts | DEFERRED (request type only) |
| Incremental consumers (streaming HTML parse) | PARTIAL |
| Timing: TTFB and total measured; DNS/connect/TLS phases `None` | PARTIAL |
| Pool counters: opened measured; reused/active `None` | PARTIAL |
| `axiom://network` (profile data, no secrets, "n/a" for unmeasured) | FUNCTIONAL |
| Trusted network error page | FUNCTIONAL |
| Download detection and handoff to the download manager | FUNCTIONAL (Wave E) |
| Streaming upload (chunked, bounded read-ahead; channel-fed bodies for script streams) | FUNCTIONAL |
| Per-request flow control (`FlowControl` window pauses the worker) | FUNCTIONAL |
| `examples/netbench.rs` | FUNCTIONAL |

### Phase 3 Wave E: Networking 2.0 (gap closure)

Built on the existing stack; nothing was rewritten. Details and evidence are in `docs/NETWORKING.md`, `docs/HTTP_CACHE.md`, `docs/DOWNLOADS.md` and the Wave E section of `docs/SECURITY.md`.

| Piece | Status |
|-------|--------|
| Disk-backed per-profile HTTP cache (`<profile>/network-cache/objects` + `index`), atomic writes, corruption recovery, LRU order across restarts; private stays memory-only | FUNCTIONAL |
| Cache states `Miss`, `Hit`, `Stale`, `Revalidated`, `Bypassed`, `NotCacheable` | FUNCTIONAL |
| `Set-Cookie` never stored in the cache (security review finding, fixed) | FUNCTIONAL |
| Typed errors: `ConnectionRefused`, `ConnectTimeout`, `ReadTimeout`, `RedirectLoop` vs `TooManyRedirects`, `UnsupportedScheme`, `HeadersTooLarge`, `Certificate { kind }`, with stable `kind_name()` | FUNCTIONAL (connect timeout has no CI test) |
| Idle read timeout and response header size limit | FUNCTIONAL |
| TLS certificate metadata (subject, issuer, SANs, validity, fingerprint) on `TlsInfo`, kept with cached entries | FUNCTIONAL |
| Security indicator from verified TLS only; mixed content and certificate errors shown separately | FUNCTIONAL |
| Navigation IDs: a newer navigation cancels the pending one; stale responses never commit | FUNCTIONAL (engine); the desktop browser waited synchronously until Phase 3 Wave F, which moved it to background navigation |
| Activity stream (11 event types, bounded subscribers, redacted headers) and structured per-request log lines | FUNCTIONAL |
| `axiom://network/<id>` request detail page; `axiom://network` rows link to it | FUNCTIONAL |
| `axiom-download`: `DownloadManager` on the profile scheduler, streaming to partial files, pause/resume with `Range`/`If-Range`, cancel, sanitized unique names, private cleanup; `axiom://downloads` | FUNCTIONAL (foundation; no download UI) |
| Standard local test server endpoints (`TestServer::spawn_standard`) | FUNCTIONAL |
| netbench: cold/warm HTTPS over local TLS, redirect chain, disk cache after restart | FUNCTIONAL |
| Manual real-world smoke test (`examples/https_smoke.rs`, not run in CI) | FUNCTIONAL |

### Wave G: Fetch

The full per-item status is in `docs/FETCH.md`. Summary:

| Piece | Status |
|-------|--------|
| `fetch()` over the tab's `ResourceLoader` (`ResourceType::Fetch`, profile scheduler, cache, cookies); no new client | FUNCTIONAL |
| `Headers`, `Request`, `Response`, body mixin (`text`/`json`/`arrayBuffer`/`bytes`/`body`) | FUNCTIONAL |
| `blob()` / `formData()`, `Blob`/`File`/`FormData`/`URLSearchParams` globals and request bodies | FUNCTIONAL |
| `ReadableStream` request bodies (`duplex: 'half'`), paced by the upload queue | FUNCTIONAL |
| Streaming response bodies (`ReadableStream`) fed by scheduler data events | FUNCTIONAL |
| Script → network backpressure (1 MiB window per fetch) | FUNCTIONAL |
| `AbortController` / `AbortSignal` (`timeout`, `any`) → `ResourceLoader::cancel(RequestId)` | FUNCTIONAL |
| Cache modes → `CacheMode`, credentials modes → `CredentialsMode` | FUNCTIONAL |
| Redirect modes `follow` / `error` / `manual` | FUNCTIONAL |
| Cross-origin: `cors` fails closed before sending, `no-cors` opaque, cross-origin redirects blocked for `same-origin`/`cors` | FUNCTIONAL (pre-CORS); Phase 3 Wave F replaced the blanket `cors` rejection with basic CORS for simple requests |
| Mixed content, forbidden headers, hidden `Set-Cookie`, `Origin` header | FUNCTIONAL |
| `data:` URLs (fixed: percent-decoding and MIME parameters were missing in `parse_data_url`) | FUNCTIONAL |
| `integrity` (SRI sha256/384/512), `keepalive` (survives navigation and tab close, 64 KiB quota), custom same-origin referrer URL | FUNCTIONAL |
| Microtask checkpoint after every script entry (`Context::run_jobs`) | FUNCTIONAL |
| `unhandledrejection` / `rejectionhandled` events and console reporting | FUNCTIONAL |
| `fetch()` and timers in the headless CLI `Engine` (settles before the frame) | FUNCTIONAL |

### Phase 3 Wave G: Document loading, resource pipeline and page lifecycle

Built on the existing network stack and loader; no new HTTP code. Details and evidence are in
`docs/DOCUMENT_LOADING.md`, `docs/RESOURCE_LOADING.md`, `docs/SCRIPT_LOADING.md`,
`docs/PAGE_LIFECYCLE.md` and the Wave G section of `docs/SECURITY.md`.

| Piece | Status |
|-------|--------|
| `DocumentId` linked to `NavigationId`, `BrowsingContextId`, tab, profile, canonical `Origin`, final URL and kind | FUNCTIONAL |
| Generation guard: events, timers, tasks and script outcomes of a replaced document never reach the active one; stale events counted | FUNCTIONAL (`document_loading::obsolete_document_callbacks_never_touch_the_new_document`) |
| Streaming document commit and incremental HTML parse as chunks arrive | FUNCTIONAL |
| Centralized decoding (`TextDecoder`: BOM, transport charset, `<meta>` prescan with early exit) | FUNCTIONAL |
| `<base href>`; stylesheet-relative `url()`, `@import`, `@font-face` | FUNCTIONAL |
| Per-document `ResourceRegistry`: `ResourceId`, eight checked states, queries, bounded | FUNCTIONAL |
| Priorities per resource role; conservative preload scanner with adoption | FUNCTIONAL |
| Stylesheets through the loader: `@import` (depth 8), script- and render-blocking, isolated failures, MIME check | FUNCTIONAL |
| Script classification and scheduling: inline, parser-blocking, async, defer (inverted-completion test), dynamic; errors recorded; reentrancy-safe | FUNCTIONAL |
| `document.write`, module scripts | DEFERRED (documented; `document.write` is ignored, modules are recorded as unsupported) |
| Images: tracked, intrinsic size, `load`/`error`, lazy = low priority and not load-blocking | FUNCTIONAL (no viewport-based lazy deferral) |
| Web fonts: used faces fetched through the loader and validated | PARTIAL (not used for rendering) |
| readyState, `readystatechange`, `DOMContentLoaded` (after parse + defer, not images), `load` (resource-state driven) | FUNCTIONAL |
| Dynamic insertion (`appendChild`, `setAttribute`) through the loader | FUNCTIONAL |
| Cancellation on replacement and stop, separate from cache eviction | FUNCTIONAL |
| `ContentPolicy` (embedder hook) and mixed-content checks before requests | FUNCTIONAL (mixed content unit-tested only; the CSP itself landed in Wave I) |
| Internal pages never load web resources | FUNCTIONAL |
| Cookies via `CookieService`, private isolation, HTTP cache cold/warm/reload | FUNCTIONAL |
| Diagnostics: resource and document snapshots, bounded structured event log, measured timeline, `axiom://document` | FUNCTIONAL |
| Controlled test site (`tests/fixtures/site`, `TestServer::spawn_site`) and `axiom-browser/tests/document_loading.rs` (19 tests) | FUNCTIONAL |
| `examples/docbench.rs` (cold, warm, reload, 2 MiB parse) | FUNCTIONAL |
| Event-driven waits (`ResourceLoader::wait_next`) instead of sleeps in navigation and idle loops | FUNCTIONAL |
| CORS for subresources | FUNCTIONAL (Phase 3 Wave H) |

## Architecture

```text
Browser (profile) ── NetworkService + HttpCache (disk or memory) + RequestScheduler
                     + ProfileCookieJar + DownloadManager (on the same scheduler)
   └─ Tab → BrowsingContext → ResourceLoader (one cancellation context per tab)
          → RequestScheduler → NetworkService → ReqwestTransport → ResponseBodyReader
              ▲
   page script fetch() → plan_fetch → FetchQueue ─┘  (drained by BrowsingContext each tick)
   parser / DOM insertions → DocumentLoad (per DocumentId) ─┘  (resources registered, policy-checked, then started)
```

### Crates changed in Phase 3 Wave G

- `axiom-document` (new crate): ids, `DocumentInfo`, `DocumentLifecycle` and timeline, `ResourceRegistry`, resource states and errors, script classification, priorities, preload scanner, base URL and subresource resolution, CSS `@import`/`@font-face` extraction, `ContentPolicy` and mixed content, events and diagnostics
- `axiom-engine`: `browsing/document.rs` (the document loader and pump), `browsing/resources.rs` (subresources), streaming commit and generation guard in `browsing.rs`, `DocumentShared` and dynamic insertion collection in `page.rs`; `ParsingStarted` and script `ResourceReady` events
- `axiom-html`: incremental `HtmlParser` (feed, step, pause at scripts, discovered elements)
- `axiom-net`: streaming `TextDecoder` (BOM, charset, `<meta>` prescan; a complete declaration now ends the prescan early), `TestServer::spawn_site` / `site_routes` / `png_1x1`
- `axiom-loader`: `wait_next` (event-driven waits), `is_pending`, `cancel_all_except`, `tests/loader_events.rs`
- `axiom-js`: `document_prelude.js` (`readyState`, `currentScript`, document and element listeners, `load`/`error`, attribute reflection, `document.write` stub), synchronous script-inserted inline scripts
- `axiom-url`: canonical `Origin`; `normalize_path` fix
- `axiom-dom`: deterministic attribute order in `fmt_node`
- `axiom-browser`: tab/profile ownership on each context, `axiom://document`, `tests/document_loading.rs`, `examples/docbench.rs`
- `tests/fixtures/site/`: the controlled test site

### Crates changed in Phase 3 Wave E

- `axiom-net`: `disk_cache.rs` (new), disk-backed `HttpCache` with the new cache states and `Set-Cookie` stripping, `activity.rs` (new) activity stream, typed errors and limits in `service.rs`, `CertificateInfo`/`CertificateErrorKind`, `ResourceType::Download`, standard test server routes, `tests/wave_e.rs`, `tests/support/tls_server.rs` (shared by `h2.rs` and netbench), `examples/https_smoke.rs`
- `axiom-download` (new crate): `DownloadManager`, file name sanitization, `tests/downloads.rs`
- `axiom-loader`: `tls` on `ResourceResponse`; download navigations stop at headers
- `axiom-engine`: `NavigationId` / `start_navigation` supersession, `SecurityState`, mixed-content flag, richer trusted error page, download candidates
- `axiom-browser`: per-profile disk cache directory and `DownloadManager`, `Browser::tick`, `axiom://network/<id>`, `axiom://downloads`, security indicator from `SecurityState`, `tests/networking2.rs`
- `axiom-gfx`: indicator glyphs for mixed content and certificate errors, connection line in the site-info panel, calls `Browser::tick`

### Crates changed in G

- `axiom-js`: `fetch_prelude.js` (Headers, Request, Response, AbortController/AbortSignal, ReadableStream, TextEncoder/TextDecoder, DOMException), fetch natives, `fetch_response`/`fetch_chunk`/`fetch_complete`/`fetch_fail`, microtask checkpoint after `eval`
- `axiom-engine`: `fetch.rs` (validation, security policy, response filtering), `FetchQueue` on the page, fetch routing and cancellation in `BrowsingContext`
- `axiom-loader`: `RequestMode`, `RedirectMode` and `stream_body` on `ResourceRequest`; streaming `LoaderEvent::Response` / `Chunk`; `LoaderEvent::is_terminal`; `parse_data_url` fix
- `axiom-net`: `RequestMode` on `NetworkRequest`, cross-origin redirect block for `same-origin`/`cors`, `is_potentially_trustworthy`
- `axiom-url`: `Url::origin`

### Crates changed in G.2 (closing the Wave G gaps)

- `axiom-js`: `platform_prelude.js` (`Event`, `PromiseRejectionEvent`, window event listeners, rejection tracking, `Blob`, `File`, `URLSearchParams`, `FormData`, multipart encode/parse); `blob()`/`formData()`, stream uploads and chunk release in `fetch_prelude.js`; Boa `HostHooks` for promise rejection tracking; upload, flow-release and referrer natives
- `axiom-engine`: `sri.rs` (integrity parsing and verification over `ring`), `keepalive.rs` (`KeepaliveLoads`, `DetachedKeepalive`), upload slots and flow windows in `FetchState`, held integrity responses, headless `BrowsingContext` with `run_until_idle`, `Engine::navigate` settles fetches and timers, `StageTimings::from_trace`
- `axiom-net`: `StreamingBody::channel` / `UploadSender`, `FlowControl` honoured by scheduler workers, bounded read-ahead for reader uploads
- `axiom-loader`: `referrer`, `flow`, `max_body_bytes` and optional `request_timeout_ms` on `ResourceRequest`, passed through to the network request
- `axiom-browser`: the tab manager adopts keepalive loads from closed tabs
- `axiom-html`: fixed a panic in the raw-text end-tag scan when multi-byte text precedes `</script>` / `</style>` (fixture `tests/html/multibyte-rawtext.html`)

### Crates changed in F.1

- `axiom-net`: async transport, decoding, cache, scheduler, service, test server, `h2` tests, netbench
- `axiom-loader`: asynchronous `ResourceLoader` over the shared scheduler
- `axiom-engine`: parallel subresource loading, document-order CSS and scripts, background images, trusted error page, idempotent `set_network`
- `axiom-browser`: profile-owned network stack, `ProfileCookieJar`, `axiom://network`, background-tab draining

## Test inventory (network-related)

| Suite | Tests |
|-------|-------|
| `axiom-net` unit (including `cors::tests`, 7) | 45 |
| `axiom-net/tests/cors.rs` (Wave H: preflight, preflight cache, credentials, per-hop redirect checks, tainting) | 8 |
| `axiom-net/tests/wave_f.rs` | 40 |
| `axiom-net/tests/wave_e.rs` (redirect classification, typed errors, header limit, activity stream, disk cache, private cleanup) | 15 |
| `axiom-net/tests/wave_g2.rs` (channel uploads, flow control) | 5 |
| `axiom-net/tests/h2.rs` (ALPN, certificate kinds and metadata) | 6 |
| `axiom-download` unit (file names) | 5 |
| `axiom-download/tests/downloads.rs` | 9 |
| `axiom-browser/tests/networking2.rs` (navigation race, download handoff, profile isolation, private cleanup, detail page, indicator) | 7 |
| `axiom-browser` `chrome::tests` (security indicator) | 3 |
| `axiom-browser/tests/wave_f1.rs` | 8 |
| `axiom-browser/tests/wave_g.rs` (end-to-end fetch against local servers, including basic CORS) | 20 |
| `axiom-browser/tests/wave_h.rs` (Wave H: fetch preflight and redirects, `crossorigin` scripts, modules, images, stylesheets, fonts; main-thread stack budget) | 5 |
| `axiom-browser/tests/wave_f_navigation.rs` (Phase 3 Wave F: omnibox search, redirects, failed navigation history, background navigation, link visits, internal-page policy, provider persistence, downloads, lifecycle events, request headers) | 13 |
| `axiom-browser` `classifier::tests` / `search::tests` / `settings_repo::tests` / `search_pages::tests` | 6 / 5 / 3 / 2 |
| `axiom-browser/tests/wave_g2.rs` (body types, stream uploads, backpressure, SRI, rejections, keepalive, referrer, headless engine) | 14 |
| `axiom-engine` `fetch::tests` (policy and CORS unit tests) | 14 |
| `axiom-engine` `sri::tests` | 5 |
| `axiom-loader` unit (`data:` URLs) | 1 |
| `axiom-browser/tests/wave_d.rs` (end-to-end cookies over HTTP) | 18 |
| `axiom-browser/tests/document_loading.rs` (Wave G: script order, streaming, lifecycle order, async, stylesheets/imports/fonts, base URL, images, dynamic insertion, navigation race, stop, multi-tab, cookies, private, cache, script errors, content policy, diagnostics page, internal and local documents) | 19 |
| `axiom-document` unit | 26 |
| `axiom-loader/tests/loader_events.rs` | 2 |

Workspace total at the end of Wave G: 335 tests, all passing. At the end of the compatibility harness wave: 376 tests, all passing (including the five suite ratchets in `tools/compat/tests/ratchet.rs`). At the end of Phase 3 Wave F: 443 tests in 66 suites, all passing, none ignored; `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean. At the end of Phase 3 Wave G (browser compatibility): 572 tests in 75 suites, all passing, none ignored; fmt and clippy clean. At the end of Phase 3 Wave H: 595 tests in 77 suites, all passing, none ignored; fmt and clippy clean.

## Schema

`SCHEMA_VERSION = 3` (unchanged in Waves F and G). Settings JSON: `SETTINGS_SCHEMA_VERSION = 2` since Phase 3 Wave F (`search_provider_id`; v1 profiles migrate on load).

## Quality gates

```
cargo test --workspace
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Known limitations / standards deviations

| Item | Notes |
|------|--------|
| Disk HTTP cache | Not encrypted at rest; no `stale-while-revalidate`; eviction is synchronous on store |
| TLS version, cipher suite, DNS/connect/TLS timings, reuse counts | DEFERRED. reqwest does not expose them; reported as `None` / "not measured". |
| Blocking top-level document fetch | Resolved in Phase 3 Wave F for the desktop (background navigation). Tests, the CLI and headless tools still use blocking mode on purpose. |
| `document.write`, `<script integrity>`, `nomodule` | Not supported (`document.write` is ignored). Module scripts and import maps are supported since Phase 3 Wave G (browser compatibility), `crossorigin` since Phase 3 Wave H. |
| Web fonts | TTF, OTF and WOFF 1.0 are used for rendering; WOFF2 is rejected and the fallback font is used |
| Lazy images | Low priority and not load-blocking, but requested immediately (no viewport check) |
| Mixed content | Engine path unit-tested; no HTTPS end-to-end document fixture |
| Connect timeout | Mapped to `ConnectTimeout`, but not covered by CI (needs a black-hole address) |
| Downloads | No download UI, no persisted list, no resume across restarts, no dangerous-file checks |
| HTML incremental parse | FUNCTIONAL for streaming and script pauses (Wave G); no speculative parsing beyond the preload scanner |
| Store-side SameSite | PARTIAL |
| CORS | Complete for `fetch()` and subresources (Phase 3 Wave H). Module imports do not inherit `use-credentials`; CSS `@import` stays `no-cors`; errors from cross-origin `no-cors` scripts are not muted; the preflight shares its request's scheduler worker instead of being scheduled on its own. |
| Real-web rendering | See `docs/WEB_COMPATIBILITY.md`: missing JS globals (`getComputedStyle`, `Image`, `URL`), inline SVG and CSS background images, WOFF2 and `mask-image` are the main blockers on live sites |
| Forms | No named properties on `HTMLFormElement`, no file picker; link navigations do not record their initiator, so a `SameSite=Strict` cookie is sent on a cross-site link click |
| Flow-controlled fetch paused by script | Keeps its scheduler worker while paused (bounded by the 12-worker pool and the 1 MiB window) |
| `keepalive` fetches | Keep the 60 s request timeout and have no flow control; request bodies in flight share a 64 KiB quota per document |
| Streamed request bodies across 307/308 | The fetch fails; the body is not replayed |
| CHIPS / BlockThirdParty cookies | DEFERRED (prior waves) |

## Security concerns

- TLS verification stays on; there is no silent certificate ignore and no HTTPS→HTTP downgrade.
- The private cache is memory-only and cleared on close, even if a cache directory is configured (tested).
- The HTTP cache never stores `Set-Cookie` (Wave E review finding; tested).
- The security indicator comes only from a verified TLS response (tested).
- Download names cannot escape the download directory (tested); private windows delete unfinished downloads (tested).
- The full Wave E review is in `docs/SECURITY.md`.
- Non-idempotent requests are never retried (tested).
- Error pages never contain server bytes (tested).
- `fetch()` is re-validated in Rust; opaque response bytes never reach the JS heap; `Set-Cookie` is filtered before script sees headers; script cannot set `Cookie`, `Host`, `Origin` or other forbidden headers (all tested).
- Cross-origin `cors` requests that need a preflight send nothing of the actual request unless the preflight allows it; a cross-origin response (and every cross-origin redirect) reaches script or the page only when `Access-Control-Allow-Origin` (and, with credentials, `Access-Control-Allow-Credentials`) allows it, and only safelisted or exposed headers are visible (tested; Wave H review in `docs/SECURITY.md`).
- Web content cannot navigate to `axiom://` pages; a page-initiated navigation never changes the address bar before it commits (tested).
- Navigation log lines strip query strings; search terms are not logged.

## Performance concerns

- Concurrency is capped at 12 per profile (a fixed worker pool shared by all tabs).
- The cache tee holds at most 8 MiB per entry; loader bodies are capped per type (`LoaderLimits`).
- Loopback overhead from `netbench` (release): about 960 MiB/s identity throughput, memory cache hit p50 about 4.5 µs, disk cache hit (256 KiB) p50 about 134 µs, cold local HTTPS p50 0.84 ms vs warm 0.064 ms, cancel→`Cancelled` p50 about 15 µs.
- Normal profiles' disk cache budget is 256 MiB (16 MiB per entry); opening it reads only the index.
- `docbench` (release, loopback, 12 stylesheets + 12 defer scripts + 12 images): cold p50 `DOMContentLoaded` 39 ms / `load` 53 ms; warm 21 ms / 32 ms (18 hits, 18 revalidations). 2 MiB document parse p50 51 ms; style, layout and paint after `load` dominate that navigation's wall time.

## Wave H handoff (done)

**Wave H: CORS** — completed in Phase 3 Wave H (section at the top).

1. ~~Simple requests plus `Access-Control-Allow-Origin` / `-Credentials` checks~~ (Phase 3 Wave F). ~~Preflight (`OPTIONS`) for non-safelisted methods and headers~~ (Wave H). The preflight is a request on the same `NetworkService` (same transport, TLS, log and cancellation), made from the actual request's scheduler worker; there is no separate client.
2. ~~Preflight cache per profile (keyed by origin, URL, credentials mode), cleared with the profile's network state, memory-only~~ (Wave H; every profile's cache is memory-only).
3. ~~Extend `filter_response` with the `cors` response type~~ (Phase 3 Wave F).
4. ~~Per-hop CORS check for `cors` redirects, origin tainted to `null`~~ (Wave H). `same-origin` requests still refuse cross-origin redirects.
5. ~~Keep the pre-CORS guarantees tested in `wave_g.rs`~~ (still tested; two tests updated for preflight and redirect behaviour).
6. ~~Subresource CORS (`crossorigin` on `img`/`script`/`link`)~~ (Wave H, in `request_resource`; a failed check fails the resource at the `network` stage with a `cors:` message rather than a new error class). ~~CSP~~ (Wave I; enforced in `start_resource` and the other points listed in `docs/CSP.md`, separately from the embedder `ContentPolicy` hook).
