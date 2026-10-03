# Content Security Policy

Phase 3 Wave I. Axiom enforces the document's Content Security Policy (CSP Level 3) for
subresources, inline script and style, `eval`, `fetch()`, form submissions and `<base>`.
Violations go to the console and fire `securitypolicyviolation` events; nothing is sent to
report endpoints yet.

## Where the pieces live

| Piece | Location |
|-------|----------|
| Parsing, source lists, matching, nonces and hashes, directive fallback, violation messages | `crates/axiom-csp` (no I/O, no engine types) |
| Per-document state: the policy list, the violation queue, per-element result caches | `axiom-engine/src/csp.rs` (`DocumentCsp`, in `DocumentShared`) |
| Policies from headers | `browsing.rs` `commit_network_document` (`Content-Security-Policy`, `Content-Security-Policy-Report-Only`) |
| Policies from `<meta http-equiv>` | `browsing/document.rs` `process_discovered` (only in `<head>`, from the point the parser reaches them) |
| Subresource checks and `upgrade-insecure-requests` | `browsing/resources.rs` `start_resource` |
| Redirect checks | `axiom-net` `NetworkRequest::redirect_check`, run in `check_request_policies` for every redirect hop before it is sent |
| Inline scripts, import maps, inline modules | `browsing/document.rs` `inline_script_allowed`, `page.rs` `collect_insertions` |
| Inline `<style>` and `style` attributes | `browsing/resources.rs` `process_inline_style`, `page.rs` `author_css`, `axiom-style` `StyleEngine::set_style_attribute_filter` |
| Event handler attributes | `axiom-js` `__axiom_compileHandler` → `JsHost::csp_allows_handler` |
| `eval()` and `Function()` | `axiom-js` `EvalGate` in the realm's host data, consulted by Boa's `ensure_can_compile_strings` hook |
| `fetch()` | `page.rs` `fetch_start` |
| `form-action` | `browsing.rs` `start_form_submission` and the navigation request's redirect check |
| `base-uri` | `browsing/document.rs` `process_discovered` |
| Reporting | `Page::report_csp_violations`, called every tick and pump step |

The embedder `ContentPolicy` hook still runs in `request_resource`; it is independent of the
document's CSP.

## Directives

| Directive | Applies to | Status |
|-----------|------------|--------|
| `default-src` | fallback for every fetch directive | SUPPORTED |
| `script-src`, `script-src-elem`, `script-src-attr` | classic and module scripts, imports, import maps, inline scripts, event handlers; `script-src` alone governs `eval` | SUPPORTED |
| `style-src`, `style-src-elem`, `style-src-attr` | stylesheets, `@import`, inline `<style>`, `style` attributes | SUPPORTED |
| `img-src`, `font-src`, `media-src`, `manifest-src` | images, `@font-face`, media, manifests | SUPPORTED |
| `connect-src` | `fetch()` and other script-initiated loads | SUPPORTED |
| `object-src`, `frame-src`, `child-src`, `worker-src` | parsed with their fallback chains | PARSED (Axiom has no plugins, frames or workers) |
| `form-action` | form submission targets and their redirects | SUPPORTED |
| `base-uri` | `<base href>` | SUPPORTED |
| `upgrade-insecure-requests` | subresource and `fetch()` URLs | SUPPORTED (see limitations) |
| `frame-ancestors`, `sandbox`, `report-uri` | ignored in `<meta>`, as the spec requires | header-only; not enforced |
| `report-to`, `report-uri` delivery | — | NOT STARTED |
| `require-trusted-types-for`, `trusted-types` | — | NOT STARTED |

## Source matching

- Keywords: `'self'`, `'none'`, `'unsafe-inline'`, `'unsafe-eval'`, `'unsafe-hashes'`,
  `'strict-dynamic'`, `'report-sample'`; nonces (`'nonce-…'`) and hashes (`'sha256-…'`,
  `'sha384-…'`, `'sha512-…'`, base64 or base64url).
- `*` matches `http`, `https`, `ws`, `wss` and the document's own scheme. Scheme-only and host
  sources allow the secure upgrade (`http:` admits `https:`, `ws:` admits `wss:`).
- `*.example.com` matches subdomains only. A missing port means the scheme's default; `:80`
  also admits `https` on 443. A path ending in `/` is a prefix; paths are ignored after a
  redirect.
- `'self'` matches the document origin and its secure upgrade; documents with an opaque
  origin match nothing with `'self'`.
- A nonce or hash in a list disables `'unsafe-inline'`, as does `'strict-dynamic'` for
  scripts. `'strict-dynamic'` blocks parser-inserted scripts that lack a nonce and allows
  scripts inserted by already trusted script.
- Nonces apply to elements only, and only to nonceable ones (no `<script` or `<style` in an
  attribute name or value). Hashes apply to element text, and to attributes when
  `'unsafe-hashes'` is present.

## Behaviour

- Multiple policies (several headers, comma-separated values, `<meta>`) all apply; a request
  must pass every enforced policy. Report-only policies never block.
- A blocked subresource fails with a `csp:` failure in the resource diagnostics and fires
  `error` like any other failure. The preload scanner skips requests a policy would block;
  the element's own request then records the violation.
- A blocked redirect fails the request at the `network` stage before the target is
  contacted. The report then names only the target's origin.
- `fetch()` rejects with `TypeError("Failed to fetch")`, whether the URL or a redirect was
  refused.
- A blocked `eval()` or `Function()` throws `EvalError`. Event handler attributes are
  compiled only after their own `script-src-attr` check, so the eval gate does not apply to
  them.
- A blocked `<style>` or `style` attribute is ignored. Results are cached per element and
  text, so restyles do not report twice.
- A blocked form submission does nothing (no navigation); a blocked redirect of a form
  navigation fails that navigation.
- A `<base>` refused by `base-uri` is ignored by the loader and by form submission, which
  share the loader's frozen base URL.

## Reporting

Each violation is logged as a `[console]` warning in Chrome's wording (with the
`[Report Only]` prefix and the fallback note). The console message shows the blocked URL
without its query string. The page then gets a trusted `securitypolicyviolation` event
(bubbling, composed) at the element, or at the document when there is no element or it is
disconnected. `sample` is filled only when the policy has `'report-sample'`, truncated to
40 characters. `BrowsingContext::csp_violations()` returns the last 256 reports. The queue
holds at most 1024 pending reports; any beyond that are dropped and counted in the log.
`is_idle` waits until pending reports are delivered.

## Limitations

- Report delivery (`report-uri`, `report-to`) is not implemented.
- `upgrade-insecure-requests` does not upgrade navigations or redirect targets.
- SRI hash sources do not allow external scripts (`integrity` plus a hash source).
- `javascript:` URLs are not supported by Axiom, so they are never checked.
- `<base>` and `<meta>` elements inserted by script are not processed (as before Wave I).
- Module imports use the nonce of the document's first module script that the policy
  allowed, because the module map is shared across scripts.
- `frame-ancestors`, `sandbox` and Trusted Types are not enforced.

## Tests

- `axiom-csp` unit tests (31): parsing, fallbacks, matching, nonces, hashes, messages.
- `axiom-engine` `csp::tests` (6): document state, caching, the queue cap.
- `axiom-net/tests/redirect_check.rs` (2): refused redirect targets are never contacted.
- `axiom-browser/tests/wave_i.rs` (11): end to end with local servers, checked from the
  server side where a request must not happen.
