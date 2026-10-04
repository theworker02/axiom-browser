# Changelog

[1.3.2]: https://github.com/theworker02/axiom-browser/releases/tag/v1.3.2
[1.3.1]: https://github.com/theworker02/axiom-browser/releases/tag/v1.3.1

## [1.3.2] — 2026-10-04

### Added

- Focus Space (`axiom://focus`), a profile-local workspace overview with a visible native Orbit
  Rail launcher and `Ctrl+Shift+Space` shortcut. It presents open/loading tabs and real local
  bookmark, history and active-download counts without an account, telemetry or cloud sync.
- Trusted Settings quick controls for performance HUD, session restoration, clear-history-on-exit
  and clear-cookies-on-exit, plus regression coverage for trusted routing and profile persistence.
- `docs/FOCUS_SPACE.md`, documenting the visual system, privacy boundary and scope.

### Changed

- The desktop host honors the persisted background-navigation preference rather than forcing it
  on after startup.
- Refreshed the WPT testharness expectation baseline following intentional XML/XHTML parser
  improvements; the compatibility record reflects observed test outcomes.

## [1.3.1] — 2026-10-03

### Changed

- Native desktop chrome now uses a reduced-motion-aware, state-driven loading accent.
- Installed Windows builds store the normal profile under `%LOCALAPPDATA%\Axiom\Profile` instead of the application working directory.
- A profile-lock startup failure now opens a native explanatory dialog on Windows instead of silently exiting.
- Release artifacts, NSIS metadata, acquisition guidance and trusted version diagnostics are aligned to 1.3.1.

### Added

- `.agents/skills/axiom-setup`: an opt-in repository setup skill for compatible coding agents, documented in `docs/AGENT_SETUP.md`. It does not add AI, telemetry or tracking to Axiom, and it requires explicit consent for external or destructive setup actions.


All notable changes to Axiom are documented here. Dates use ISO-8601 (UTC-ish project local).

Format inspired by [Keep a Changelog](https://keepachangelog.com/). Axiom versions follow the workspace `Cargo.toml` version.

## [Unreleased]

## [1.3.0] — 2026-10-03

### Added — Browser settings foundation

- Versioned profile settings schema v3 with appearance, startup, privacy, cookie, download,
  language, accessibility, permission, and system/developer preferences.
- A category-based trusted `axiom://settings` dashboard informed by Chrome's current settings
  taxonomy, without copying Chrome UI or claiming unimplemented features.
- Explicit local-first defaults: no AI integration, telemetry, analytics, account requirement, or
  private-data upload path.

## [1.2.8] — 2026-10-03

### Added — Compatibility milestone

- Live XML/XHTML document realms after atomic XML parsing, including document-order classic
  external scripts through the existing profile-owned resource pipeline.
- Same-origin `iframe.contentDocument` proxy support for `contentType`, detached
  `createElement()` factories, and returned-node `namespaceURI`.
- XML-family `XMLHttpRequest.responseXML` and `responseType = "document"`, constructed through
  the existing inert `DOMParser` path.
- Offline WPT server support for the narrow `contenttype_setter.py` behavior required by
  `Document.contentType` conformance tests.
- Public privacy statement: no AI integration, telemetry, analytics, account requirement, or
  private-browsing data upload path.
- Official orbital-A SVG mark, embedded Windows executable icon, native window icon, and branded
  trusted new-tab page.

### Changed — Compatibility milestone

- `Document.contentType` preserves a response's real MIME type for non-HTML document families,
  including image documents.
- Workspace and trusted `axiom://version` surface now report Axiom 1.2.8.

### Compatibility — Compatibility milestone

- `Document-createCDATASection-xhtml`: 8 / 8 focused WPT outcomes pass.
- `Document-createElement-namespace`: 52 / 52 focused WPT outcomes pass.
- `Document-contentType`: 26 / 30 focused outcomes pass; `data:` iframe documents and
  `javascript:` iframe navigation remain explicitly unsupported.

---

### Added — Phase 3 Wave I (Content Security Policy)

- New crate `axiom-csp`: CSP Level 3 policy parsing (headers and `<meta>`, enforce and report-only), source lists with nonces, hashes, `'strict-dynamic'`, `'unsafe-hashes'` and `'report-sample'`, directive fallback chains, request, inline, eval, `form-action` and `base-uri` checks, `upgrade-insecure-requests`, and Chrome-worded violation messages. SHA-2 and base64 helpers moved here from the engine's SRI code.
- Enforcement in the engine (`axiom-engine/src/csp.rs`, `DocumentCsp` per document):
  - Subresources (scripts, module imports, stylesheets, images, fonts, media, manifests) are checked in `start_resource` before any request exists. The preload scanner skips requests a policy would block.
  - Inline scripts, inline modules, import maps, `<style>`, `style` attributes (through the new `StyleEngine::set_style_attribute_filter`) and event handler attributes.
  - `eval()` and `Function()` throw `EvalError` unless `'unsafe-eval'` is allowed (a per-realm `EvalGate` behind Boa's `ensure_can_compile_strings` hook).
  - `fetch()` against `connect-src`; form submissions against `form-action`; `<base href>` against `base-uri`.
- `NetworkRequest::redirect_check` / `ResourceRequest::redirect_check` (`RedirectCheck`): the network service runs it on every redirect hop before sending it. The CSP uses it for subresources, `fetch()` and form navigations.
- Violations: a `[console]` warning (without the query string) and a trusted `securitypolicyviolation` event (`SecurityPolicyViolationEvent`) at the element or document; `BrowsingContext::csp_violations()`.
- Docs: new `docs/CSP.md`; Wave I review in `docs/SECURITY.md`.
- Tests: `axiom-browser/tests/wave_i.rs` (11), `axiom-net/tests/redirect_check.rs` (2), `axiom-csp` unit tests (31), `csp::tests` (6).

### Changed — Phase 3 Wave I (Content Security Policy)

- The parser reports `<meta>` elements to the loader (for `<meta http-equiv=Content-Security-Policy>`), alongside `<link>`, `<img>` and `<base>`.
- Event handler attributes are compiled by a native (`__axiom_compileHandler`) that checks `script-src-attr` first, instead of calling `Function()` from the events prelude.

### Fixed — Phase 3 Wave I (Content Security Policy)

- Form submission resolved its action against the first `<base href>` in the DOM on its own, not against the loader's frozen base URL. It now uses the loader's base, so a `<base>` refused by `base-uri` cannot redirect form actions (`wave_i::base_uri_rejects_disallowed_base_elements`).

### Added — Phase 3 Wave H (CORS)

- CORS lives in the network service (`axiom-net/src/cors.rs`), shared by `fetch()` and subresources:
  - Preflight (`OPTIONS`) for non-safelisted methods and headers and for streamed request bodies. It is an ordinary request on the same `NetworkService` (its own row on `axiom://network`, initiator `cors-preflight`), with no credentials and no redirects. If it fails, nothing of the actual request is sent.
  - A per-profile, memory-only preflight cache keyed by origin, URL and credentials mode (`Access-Control-Max-Age` default 5 s, capped at 2 hours), cleared by the new `Browser::clear_cache` / `NetworkService::clear_cache` and on shutdown.
  - `NetworkError::Cors` (kind `cors`).
- Subresource CORS: `crossorigin` on `<script>`, `<link rel=stylesheet>` and `<img>`; module scripts and their imports and `@font-face` fonts always use `cors`. Preload-scanner requests keep their CORS state (`CorsSettings` in `axiom-document`).
- `fetch()` from opaque-origin documents (`file:`, `about:`) may make CORS requests with `Origin: null`.
- Windows binaries (`axiom`, `axiom-test-runner`, the `axiom-browser` examples) are linked with an 8 MiB main-thread stack, as on Linux and macOS (`build.rs`).
- Tests: `axiom-net/tests/cors.rs`, `axiom-browser/tests/wave_h.rs`, `cors::tests`.

### Changed — Phase 3 Wave H (CORS)

- A cross-origin redirect of a `cors` request is followed after a CORS check on the redirect response, instead of being refused. After a hop from one foreign origin to another the origin is tainted and later hops send `Origin: null`. `same-origin` requests still refuse cross-origin redirects.
- `fetch()` requests that need a preflight are preflighted instead of rejected before sending.

### Fixed — Phase 3 Wave H (CORS)

- The live smoke probe crashed with a stack overflow on Google search results on Windows. Recursion through native callbacks up to the JavaScript recursion limit needs about 1.5 MiB, more than Windows' 1 MiB default main-thread stack (`wave_h::runaway_recursion_through_native_callbacks_stops_at_the_limit`).

### Added — Phase 3 Wave G (browser compatibility)

- Web fonts are used for rendering: TTF, OTF and WOFF 1.0 (`axiom-text/src/fonts.rs`). WOFF2 is rejected and the fallback font is used.
- `ex` and `ch` units; `<details>`/`<summary>`; named properties on `window` for elements with an `id` or `name`.
- JS platform objects (`timers_prelude.js`, `cssom_prelude.js`, `window_prelude.js`):
  - `navigator` with an honest user agent.
  - `setInterval`/`clearInterval`, `requestAnimationFrame`/`cancelAnimationFrame`.
  - `matchMedia`, `queueMicrotask`.
  - `innerWidth`, `innerHeight`, `devicePixelRatio`.
  - `element.style` as a `CSSStyleDeclaration`.
- ES module scripts (`axiom-js/src/modules.rs`): module map, static and dynamic `import`, `<script type="importmap">` merged per document (`axiom-engine/src/import_map.rs`). A patched `boa_parser` (`third_party/boa_parser`) parses `let of` loops.
- SVG images (`axiom-loader/src/svg.rs`, resvg), including gzip-compressed SVG, rasterized at their intrinsic size.
- Form submission (`axiom-engine/src/forms.rs`, `axiom-js/src/forms_prelude.js`):
  - Control state is shared by layout, user input and script.
  - Encodings: GET, `application/x-www-form-urlencoded`, `multipart/form-data`, `text/plain`.
  - Implicit submission, and Space activates buttons.
  - `new FormData(form)`.
  - A reload or history traversal to a POST result shows a trusted "confirm form resubmission" page instead of resending.
  - Form navigations carry their initiator for SameSite cookie decisions.
- Flexbox and CSS grid formatting contexts (`axiom-layout/src/flex.rs`, `grid.rs`), with grid values parsed into typed track lists, areas and line placements (`axiom-style/src/grid.rs`). See `docs/LAYOUT.md`.
- Fixtures: `tests/rendering/glyph-baseline.html`, `floats-tables-block-in-inline.html`, `svg-image.html`, `flex-grid.html`.
- Tests: `axiom-engine/tests/timers_navigator.rs`, `forms_dom.rs`; `axiom-browser/tests/modules.rs`, `svg_images.rs`, `form_submission.rs`; `axiom-js/tests/parser_regressions.rs`; new flex and grid cases in `axiom-layout`.
- The `live_smoke` `forms` probe types a query into the DuckDuckGo HTML page and submits it.

### Fixed — Phase 3 Wave G

- Glyphs were drawn below their correct position: descenders rose above the line and commas looked like apostrophes. Bitmaps are now placed at `baseline − (height + ymin)`.
- Float placement, table layout and paint-order bugs found on live sites.
- CDATA sections in XHTML scripts are unwrapped before running.
- Flex and grid containers no longer lay their children out as blocks, which had turned GitHub, MDN and Wikipedia chrome into long vertical lists.

### Compatibility — Phase 3 Wave G

- testharness 4942 / 6589 passing outcomes (+111 from forms); reftests 887 / 1034 (+10 from flex).
- Live sites: see `docs/WEB_COMPATIBILITY.md`.

### Added — Phase 3 Wave F (real web navigation, search providers)

- Omnibox classifier (`crates/axiom-browser/src/classifier.rs`) with these rules:
  - Public Suffix List validated domains, IPv4 and bracketed IPv6 literals, ports, `localhost` and reserved local names.
  - IDN hosts are converted to punycode.
  - A leading `?` or a quoted phrase forces a search.
  - `javascript:`, `data:` and other non-web schemes are searched for, never navigated to.
- `axiom-url` canonicalizes hosts: lowercase, IDNA punycode, and forbidden host code points rejected.
- `SearchProviderService` offers Google, Bing, DuckDuckGo (the default), Axiom Search and a custom `http(s)` template. The omnibox uses `search_url`; no engine is hard-coded.
- The provider choice persists per profile (settings schema v2, `search_provider_id`, migrated from v1). It is chosen on `axiom://settings/search`. Private windows (`Browser::new_private_inheriting`, `axiom --private`) inherit it without writing back.
- The Axiom Search placeholder page (`axiom://search?q=`) and the design document `docs/AXIOM_SEARCH_ARCHITECTURE.md`. No crawler or index.
- Engine navigation lifecycle events and states (`axiom-engine/src/navigation.rs`): `Started`, `Committed { redirected }`, `Interactive`, `Completed`, `Failed { kind }`, `Cancelled`, `Download`, `InternalRequested`.
- The desktop UI uses background navigation (`Browser::set_background_navigation`). Navigations return immediately and commit from the frame loop.
- `NetworkServiceConfig::user_agent` and `accept_language`. Requests send `Accept-Language: en-US,en;q=0.9`. The User-Agent stays the honest `Axiom/0.2 (+…)` string.
- Basic CORS for simple cross-origin `fetch()`:
  - `Origin` header.
  - `Access-Control-Allow-Origin`, `-Allow-Credentials` and `-Expose-Headers` checks.
  - A `cors` response type with safelisted and exposed headers only.
  - Requests that need a preflight still fail before sending.
- `crates/axiom-browser/tests/wave_f_navigation.rs` (13 end-to-end tests on local servers) and the manual `examples/live_smoke.rs` probe.
- Docs: `NETWORKING.md` (navigation pipeline diagram, events, request identity, limits), `SEARCH_PROVIDERS.md`, `AXIOM_SEARCH_ARCHITECTURE.md`, `WEB_COMPATIBILITY.md`, `WAVE_F_NETWORK_AUDIT.md`, `WAVE_F_PLAN.md`.

### Fixed — Phase 3 Wave F

- Link clicks now update the address bar and are recorded in history as `Link` visits.
- Redirected navigations show the final URL and record one visit, not one per hop.
- A failed navigation no longer replaces the previous page's history entry. Back returns to that page, and reload and forward retry the failed URL.
- A download response no longer replaces the current document or its history entry.
- Page-initiated `axiom://` navigations from web content are refused.
- A page-initiated navigation no longer changes the address bar before it commits.
- Error pages no longer print raw library error text. They show the stable error code.
- The JS `fetch()` prelude exposes the body of `cors` responses.

### Added — `Attr` and events wave

- `Attr` and `NamedNodeMap`: identity-stable attribute nodes, `element.attributes`, `getAttributeNode(NS)`, `setAttributeNode(NS)`, `removeAttributeNode`, `document.createAttribute(NS)`.
- One events layer for every target (`crates/axiom-js/src/events_prelude.js`): `EventTarget` with `capture`/`once`/`passive`/`signal`, passive-by-default touch and wheel listeners, `handleEvent`, capture/target/bubble dispatch, `window.event`, and `Window`/`Performance` interfaces. `AbortSignal` now extends `EventTarget`.
- `Event` and 27 subclasses (`MouseEvent`, `KeyboardEvent`, `CustomEvent`, `ErrorEvent`, `MessageEvent`, …) with dictionary conversion and the legacy `init*Event` methods; `document.createEvent` with the spec's alias table.
- Event handler IDL and content attributes on elements, the document and `window`, compiled lazily with the document, form-owner and element scopes; `<body>` and `<frameset>` forward window handlers.
- Exceptions thrown by listeners are reported as a cancelable `ErrorEvent` at `window`.
- Engine clicks dispatch a trusted `MouseEvent`, and canceling it skips the default action. `HTMLElement.click()` and `disabled` on form controls.
- The testharness suite also runs WPT `dom/events`, `dom/collections` and `dom/lists`; passing outcomes went from 3408 to 4780 out of 6544.
- Engine tests in `axiom-engine/tests/dom_events.rs` and `axiom-js/tests/realms.rs`.

### Fixed — `Attr` and events wave

- A page that kept an object in a `WeakMap` aborted with a failed multi-gigabyte allocation at the next full garbage collection, which navigation triggers. The cause was a Boa 0.20 `boa_gc` bug (ephemeron values traced without marking), now fixed in a patched `boa_gc` 0.20.0 (`third_party/boa_gc`). See `docs/JAVASCRIPT.md`.
- Event handler attributes no longer compile to named function expressions inside `with`, which panics Boa 0.20.

### Known gaps — `Attr` and events wave

- No activation behavior for script-dispatched clicks (checkbox, radio, `<a>`, `<label>`, submit) and no form-control state beyond `disabled`.
- No `requestAnimationFrame` or WebDriver input actions, which most remaining `dom/events` failures need.
- Top-level script errors go to the script error log, not to `window`'s `error` event.

### Added — DOM core wave

- A spec-shaped script interface hierarchy in `crates/axiom-js/src/dom_prelude.js`: `Node`, `Element`, `HTMLElement` with per-tag subclasses, `SVGElement`, `MathMLElement`, `CharacterData`, `Text`, `Comment`, `ProcessingInstruction`, `DocumentType`, `DocumentFragment`, `Document`/`HTMLDocument`, with constructors and factories (`createElementNS`, `createProcessingInstruction`, `importNode`, `adoptNode`, `implementation.createDocumentType`).
- Checked tree mutation (`appendChild`, `insertBefore`, `replaceChild`, `removeChild`, `ParentNode`/`ChildNode` methods) that throws the spec's `DOMException`s; `cloneNode`, `isEqualNode`, `compareDocumentPosition`, `normalize`, `CharacterData` methods, `splitText`.
- Namespaced attributes in `axiom-dom` and the `NS` attribute methods, `toggleAttribute`, `getAttributeNames`, `classList`.
- Live `HTMLCollection`/`NodeList`; `querySelectorAll` returns a static `NodeList`.
- A Selectors Level 4 engine (`axiom-dom::selector`) behind `querySelector*`, `matches` and `closest`: CSS escapes, attribute flags, `:is`/`:where`/`:not`/`:has`, `:nth-*(… of S)`, `:scope`, form-state pseudo-classes, `:lang`, `:dir`.
- An HTML serializer (`axiom_html::serialize_children`, `serialize_outer`); `innerHTML`, `outerHTML`, `insertAdjacentHTML` and `template.content`.
- `MutationObserver` / `MutationRecord` for script-driven mutations.
- Engine tests in `axiom-engine/tests/dom_node_bindings.rs` covering all of the above.
- testharness (`dom/nodes`) went from 681 to 3408 passing outcomes out of 5342.

### Fixed — DOM core wave

- Promises that got a handler while pending were reported as unhandled once they rejected. The cause was a Boa 0.20 `[[PromiseIsHandled]]` bug, now fixed in a patched `boa_engine` 0.20.0 (`third_party/boa_engine`, used through `[patch.crates-io]`). It turned every failing `promise_test` into a file-level harness ERROR. See `docs/JAVASCRIPT.md`.

### Known gaps — DOM core wave

- No `Range`, `Attr`/`NamedNodeMap`, additional documents (`createDocument`/`createHTMLDocument` throw `NotSupportedError`), `DOMParser`, XML documents, custom elements, `createEvent` or `dataset`.
- The HTML parser queues no `MutationObserver` records.
- Three `NodeList-static-length-getter-tampered` tests time out in debug builds.

### Added — Parser follow-up

- Processing instructions: tokenizer states §13.2.5.72–76, `Token::ProcessingInstruction`, an `axiom-dom` `ProcessingInstruction` node (`nodeType` 7), inserted wherever comments are.
- `<selectedcontent>`: a selected `<option>` popped off the stack of open elements has its children cloned into the select's first `<selectedcontent>`.
- html5lib tree construction went from 1860 to 1953 / 1959; only the six `scripted_*` tests remain, and they need script execution during parsing.

### Changed — Parser follow-up

- `select` is in the default "has an element in scope" list, matching the current spec.
- The html5lib tokenizer went from 7028 to 7017 / 7032. The 11 new failures are upstream tests that predate processing instructions and still expect bogus comments; see `docs/COMPATIBILITY.md`.

### Added — Compatibility harness wave

- `tools/compat` (`axiom-compat`): offline conformance suites with per-group `passed / total` reports and no global percentage. The suites are html5lib tree construction, the html5lib tokenizer, WPT crashtests, WPT reftests (`match`/`mismatch`, `<meta name=fuzzy>`) and testharness.js for `dom/nodes`. See `docs/COMPATIBILITY.md`.
- A vendored, pinned corpus (`tests/wpt`, `tests/html5lib-tests`) served by a loopback WPT-style server; CI never touches the network.
- An expectations ratchet (`tests/expectations/`) enforced by `cargo test`. It fails on regressions, unexpected passes, status changes and stale entries; `--update` records a new baseline.
- A WHATWG tree builder in `axiom-html` (`tree_builder.rs`) with all insertion modes, implied `html`/`head`/`body`, the adoption agency, foster parenting, templates, SVG/MathML, fragment parsing (`parse_fragment`) and quirks-mode detection. html5lib tree construction went from 91 / 1959 to 1860 / 1959.
- `axiom-dom`: element namespaces, doctype public/system ids, `QuirksMode`, template contents, `descendants`.
- The window/Node surface testharness.js needs: `self`/`parent`/`top`, read-only `location`, `Node` constants, tree navigation, `nodeType`/`nodeName`, `getElementsByTagName`, `querySelectorAll`, `removeChild`/`remove` and identity-stable element wrappers.
- `tests/html/implied-body.html` fixture and `axiom-engine/tests/dom_node_bindings.rs`.

### Fixed — Compatibility harness wave

- Pages that omit `<html>`/`<head>`/`<body>` tags rendered blank; the old parser dropped their content.
- Unquoted attribute values containing `/` (for example `src=/resources/testharness.js`) were cut short.
- The UA stylesheet now hides `template`, `base`, `area`, `datalist`, `noembed`, `noframes`, `param`, `rp` and `basefont`.

### Known gaps — Compatibility harness wave

- Parse errors are not reported. `document.write` during parsing and form owners are not implemented.
- XHTML is parsed as HTML, SVG documents run no script, worker test variants are not run and reftest reference chains are not followed.
- DOM collections are static arrays. There are no iframes and no `DOMParser`.
- Reftests dropped from 751 to 693 / 1034. The old passes were blank-versus-blank frames; see `docs/COMPATIBILITY.md`.

### Added — Phase 3 Wave G (web resource pipeline, document loading, page lifecycle)

- New crate `axiom-document`: `DocumentId`/`BrowsingContextId`/`ResourceId`, `DocumentInfo`, `DocumentLifecycle` with a measured timeline, a per-document `ResourceRegistry` with eight checked resource states, script classification, per-role priorities, a conservative preload scanner, `ContentPolicy` (CSP hook) and mixed-content rules, structured document events and diagnostics.
- Streaming document loading: the navigation commits on response headers; body chunks go through the central `TextDecoder` into a new incremental `HtmlParser`; subresources are discovered as the parser advances.
- Generation guard: every subresource request is owned by a `DocumentId`; a replaced document's loads are cancelled, its timers and tasks cleared, and late events dropped and counted.
- Script scheduling driven by resource state: inline, parser-blocking, `async`, `defer` (document order, before `DOMContentLoaded`) and script-inserted scripts; errors recorded per resource; `document.readyState`, `document.currentScript`, `readystatechange`, `DOMContentLoaded`, window `load`, element `load`/`error`.
- Stylesheets through the loader with stylesheet-relative `@import` (depth 8) and `url()`, script- and render-blocking; used `@font-face` faces fetched and validated; image intrinsic sizes; `<base href>`; dynamic insertion through the loader.
- `axiom://document` diagnostics page; `examples/docbench.rs` (cold/warm/reload document timings); `TestServer::spawn_site` and the controlled test site in `tests/fixtures/site`; `tests/document_loading.rs` (19 tests).
- `ResourceLoader::wait_next`: navigation and idle waits are event-driven instead of sleeping.
- Docs: `DOCUMENT_LOADING.md`, `RESOURCE_LOADING.md`, `SCRIPT_LOADING.md`, `PAGE_LIFECYCLE.md`.

### Fixed — Wave G

- The HTML `<meta charset>` prescan waited for 1024 bytes even after a complete declaration, stalling the parser on small first chunks.
- `ParsingStarted` was never recorded, and scripts reached `READY` without a `ResourceReady` event.
- `axiom-url` path normalization; deterministic attribute order when serializing DOM nodes.

### Known gaps — Wave G

- `document.write` is ignored; module scripts are not supported; fonts are not used for rendering; lazy images are not deferred by viewport; no CORS for subresources (Wave H); no CSP parser.

### Added — Phase 3 Wave E (Networking 2.0)

- Disk-backed per-profile HTTP cache under `<profile>/network-cache/` (`objects/` bodies, `index/` records), atomic writes, corrupt/orphan cleanup, LRU order persisted across restarts; private profiles stay memory-only. See `docs/HTTP_CACHE.md`.
- Cache states `Stale`, `Bypassed` and `NotCacheable` alongside `Miss`, `Hit` and `Revalidated`.
- New crate `axiom-download`: `DownloadManager` on the profile scheduler with streaming to partial files, pause/resume (`Range` + `If-Range`), cancel, a concurrency cap, sanitized unique file names and private-window cleanup; attachment navigations are handed off to it; `axiom://downloads`. See `docs/DOWNLOADS.md`.
- Typed network errors `ConnectionRefused`, `ConnectTimeout`, `ReadTimeout`, `RedirectLoop`, `TooManyRedirects { limit }`, `UnsupportedScheme`, `HeadersTooLarge`, `Certificate { kind }`, with stable `kind_name()`; idle read timeout and response header size limit.
- TLS certificate metadata on `TlsInfo` (subject, issuer, SANs, validity, SHA-256 fingerprint).
- `SecurityState` on `BrowsingContext`; the chrome indicator shows HTTPS only for a verified TLS response, plus distinct mixed-content and certificate-error states and a connection line in the site-info panel.
- `NavigationId` and `BrowsingContext::start_navigation`: a newer navigation cancels the pending one, and stale responses never commit.
- Network activity stream (`NetworkService::subscribe_activity`) and structured per-request log lines.
- `axiom://network/<id>` request detail page (redacted headers, redirect chain, timing, cache state, TLS, error kind).
- Standard local test server endpoints, `tests/wave_e.rs`, `tests/networking2.rs`, shared local TLS server, netbench HTTPS/redirect/disk-cache measurements, and a manual `https_smoke` example (not run in CI).

### Security / privacy — Wave E

- The HTTP cache never stores `Set-Cookie` (in memory or on disk), and a 304 cannot add it back.
- A redirect to `file:` or another non-http(s) scheme is reported as `UnsupportedScheme`.
- Review findings and invariants are recorded in `docs/SECURITY.md`.

## [0.2.0] — 2026-09-26

### Added — Phase 3 Wave C (profiles & persistence)

- SQLite-backed `BrowserDataStore` with schema version **v1** and migrations scaffolding
- `Profile` / `ProfileId` / `ProfileKind::{Persistent,Private}` / `ProfilePaths` / `ProfileManager`
- Exclusive profile locking (`profile.lock`)
- History repository (visits, search, clear, ranking weights for omnibox)
- Bookmarks + folders; Ctrl+D / star control toggles bookmark
- Versioned `BrowserSettings`
- Session checkpointing + restore; clean vs unclean shutdown markers
- Omnibox integration: history, bookmarks, open-tab switch suggestions (deduped)
- Internal pages: live `axiom://history`, `axiom://bookmarks`, `axiom://version`
- Docs: `docs/PROFILES.md`, `docs/PERSISTENCE.md`, `docs/ACQUISITION.md`, `docs/CONTRIBUTING.md`, `docs/WAVE_C_PLAN.md`
- Tests: `crates/axiom-browser/tests/wave_c.rs`

### Added — Phase 3 Wave B (chrome & omnibox)

- Chrome state model, focus owner, omnibox editing/selection
- URL/search classifier, search provider, suggestion model
- Tab strip polish, security indicator, status bar, `axiom://newtab`

### Added — Phase 3 Wave A (multi-tab)

- `axiom-browser` crate: independent `BrowsingContext` per tab
- Tab manager, session snapshot foundations, origin helpers

### Changed

- Desktop default URL is `axiom://newtab`
- Chrome height includes status bar (`CHROME_H = 98`)
- Profile constructors return `Result` (lock / schema failures are explicit)

### Security / privacy

- Private profiles use in-memory storage only
- Private navigations do not enter durable history or restored sessions
- Corrupt databases are quarantined rather than deleted silently

## [0.1.0] — 2026-09

### Added — Phase 1 & 2 foundations

- URL → net → HTML → DOM → CSS → style → layout → paint → window
- Interactive browsing context, Boa JS bindings, click→JS→DOM→paint pipeline
- HUD timings, demos, reality/capability docs

---

[1.2.8]: https://github.com/theworker02/axiom/releases
[0.2.0]: https://github.com/theworker02/axiom/releases
[0.1.0]: https://github.com/theworker02/axiom/releases
