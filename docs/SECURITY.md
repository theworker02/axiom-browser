# Security

Axiom is an **experimental browser engine**, not a hardened production browser. Phase 2 focuses on correctness and architecture; **security parity with Chrome/Firefox is not a goal** for this milestone.

No Chromium/WebKit/Gecko sandbox or site-isolation model is embedded.

## Origin model

| Feature | Phase 2 |
|---------|---------|
| `SecurityOrigin` / scheme+host+port | **PLANNED** (Wave C stubs) |
| Same-origin checks for cheap paths | **PLANNED** |
| Site isolation / OOPIF | **UNSUPPORTED** |

## Content handling

| Threat surface | Status |
|----------------|--------|
| Malformed HTML/CSS (crash on untrusted input) | **PARTIAL** — resilience work in Wave C |
| JavaScript execution | **PLANNED** — treat all script as trusted in dev |
| `file://` access to arbitrary paths | **PARTIAL** — local demos only; tighten before release |
| Mixed content blocking | **SUPPORTED** for subresources (Phase 3 Wave G): in an HTTPS document, `http:` scripts, stylesheets, fonts and `fetch()` are blocked; images load and mark the page as mixed |
| CSP | **SUPPORTED** (Phase 3 Wave I): header and `<meta>` policies, enforce and report-only; subresources and their redirects, inline script/style/attributes/handlers, `eval`, `fetch()`, `form-action`, `base-uri`, `upgrade-insecure-requests`; console reports and `securitypolicyviolation` events. No report delivery. See [CSP.md](CSP.md) |
| XSS filters | **UNSUPPORTED** (no JS platform yet) |

## Network

| Feature | Status |
|---------|--------|
| HTTPS with rustls | **SUPPORTED** |
| Certificate validation (system roots) | **SUPPORTED** (via TLS stack; never disabled, `accept_invalid_certs` is never used) |
| Typed certificate failures (expired, not yet valid, hostname mismatch, untrusted issuer, revoked, invalid chain) | **SUPPORTED** — shown on a trusted error page and as a certificate-error indicator |
| Security indicator from verified TLS, never from the URL scheme | **SUPPORTED** |
| CORS enforcement | **SUPPORTED** (Phase 3 Wave H: preflight, per-profile preflight cache, per-hop redirect checks with origin tainting, `crossorigin` subresources, module scripts and fonts; see below) |
| Safe redirect limits | **SUPPORTED** (bounded redirects; loops classified; non-http(s) targets refused) |

## Phase 3 Wave E security review (Networking 2.0)

Scope: the network service, HTTP cache (memory and disk), activity stream and logs, `axiom://network` pages, the download manager, navigation supersession and the security indicator. The review was done by reading the code and adding tests for each invariant.

### Findings fixed in this wave

| Finding | Risk | Fix | Evidence |
|---------|------|-----|----------|
| Cached response headers kept `Set-Cookie`, so the new disk cache would have written cookie values into `network-cache/index/*.json`, outside `CookieService` | Cookie secrets persisted in a second store that cookie clearing does not reach | `Set-Cookie`/`Set-Cookie2` are stripped before storing, and a 304 merge cannot add them back | `cache::tests::set_cookie_is_never_stored_in_memory_or_on_disk` |
| A redirect to `file:` came back as a generic `Protocol` error | Misclassified refusal (the target was still refused) | Absolute `Location` values with a non-http(s) scheme are reported as `UnsupportedScheme` | `wave_e::unsupported_schemes_are_rejected_before_and_during_redirects` |
| The request detail page printed header names without escaping | Header names are tokens validated by the HTTP parser, so exploitation is unlikely; fixed as defence in depth | Names and values are both HTML-escaped | code review |

### Invariants checked

| Invariant | How it holds | Evidence |
|-----------|--------------|----------|
| TLS verification is never disabled; no HTTPS→HTTP fallback | The transport has no insecure option and no retry over another scheme | `h2::*_is_rejected` |
| Security state is not derived from the URL | `SecurityState::Secure` is set only from a response whose `TlsInfo.certificate_verified` is true | `chrome::tests::https_indicator_requires_a_verified_tls_response` |
| A stale navigation cannot commit | Each navigation gets a `NavigationId`; only the pending one may commit, and a newer one cancels the old request | `networking2::newer_navigation_supersedes_a_slow_one` |
| Secrets are not shown in diagnostics | The log and activity stream store redacted headers (`Authorization`, `Proxy-Authorization`, `Cookie`, `Set-Cookie`); the pages also hide query strings and escape all text; bodies are never logged | `wave_e::log_entry_carries_request_detail_without_secrets`, `wave_e::activity_stream_reports_a_redirected_request_in_order`, `networking2::request_detail_page_shows_redirects_and_hides_secrets` |
| Structured log lines carry no user data beyond the host | `OutcomeLog` writes ids, method, host, status, protocol, cache state, sizes, duration and error kind | code review |
| Private state is not persisted | A private service ignores `cache_dir`; its cache is cleared on shutdown; the private download manager forgets records and deletes partial files | `wave_e::private_profile_cache_never_touches_disk_and_is_gone_after_shutdown`, `networking2::private_window_forgets_downloads_and_deletes_partials` |
| Profiles are isolated | Each profile has its own service, cache, scheduler, cookie jar and download manager | `networking2::cookies_are_isolated_between_profiles`, `networking2::http_cache_is_per_profile_and_persists_only_for_normal_profiles`, `wave_e::profiles_do_not_share_disk_caches` |
| Disk cache paths cannot be chosen by a server | Object names are generated; tampered or corrupt index records are dropped on open; bodies are checked against the recorded length | `cache::tests::disk_cache_drops_corrupt_and_orphaned_files` |
| Download names cannot escape the download directory | The name is reduced to a single sanitized component (no separators, no reserved device names) and made unique | `axiom-download` unit tests, `downloads::download_streams_to_disk_with_a_sanitized_name` |
| Cache eviction never touches cookies, history or storage | The cache only removes its own entries and files | code review |
| Concurrency is bounded | All requests, including downloads, go through the profile scheduler's fixed worker pool; downloads are capped at 3 active | `wave_f::scheduler_bounds_concurrency`, `downloads::concurrency_is_capped` |
| Non-idempotent requests are never retried | Unchanged from Wave F | `wave_f::post_is_not_retried_after_connection_failure` |
| Error pages never show server bytes | Generated by Axiom from the typed error; all text is escaped | `wave_f1::navigation_error_shows_trusted_page_without_server_bytes` |

### Accepted limitations

- The disk cache is not encrypted at rest. Its index holds full URLs, including query strings, like browsing history. It lives inside the profile directory.
- A private window's completed downloads remain in the download directory, as in other browsers.
- TLS version and cipher suite are not exposed by the transport and are shown as unavailable rather than guessed.
- The trusted error page shows the full URL the user navigated to, as the omnibox does.
- Revocation (OCSP/CRL) is whatever rustls and the platform verifier provide; there is no separate revocation checking.

## Phase 3 Wave G security review (document loading)

Scope: the document loader, subresource requests, script scheduling, `axiom://document`. Done
by reading the code and adding a test for each invariant (`axiom-browser/tests/document_loading.rs`).

| Invariant | How it holds | Evidence |
|-----------|--------------|----------|
| A replaced document's callbacks cannot mutate the active one | Requests are owned by a `DocumentId`; commit cancels everything but the new document request and clears timers/tasks; events for other documents are dropped and counted; script outcomes re-check the document id | `document_loading::obsolete_document_callbacks_never_touch_the_new_document` |
| No resource bypasses the network service | Every network subresource goes through `ResourceLoader` → scheduler → `NetworkService`; only `axiom-net` depends on an HTTP client (audit in `docs/NETWORKING.md`) | code audit; server-side request checks in every `document_loading` test |
| Cookies only from `CookieService` | Subresource requests carry no cookie header of their own; the network service adds cookies from the profile jar | `document_loading::subresource_requests_carry_cookies_from_the_cookie_service`, `document_loading::private_profile_documents_do_not_share_state` |
| Policy runs before the request exists | `ContentPolicy` and the mixed-content check run in `request_resource` before `ResourceLoader::start` | `document_loading::content_policy_blocks_before_any_request` (the server never sees the request) |
| Internal pages are isolated | `internal` documents never start resources; script insertions are discarded | `document_loading::internal_pages_never_fetch_web_resources` |
| Local files are read only for local documents | Relative `file` paths resolve only when the document itself is a `file` document; network documents cannot reach the disk | code review (`resolve_subresource`) |
| Diagnostics do not leak secrets | `axiom://document` strips query strings, escapes all text, never shows bodies; events carry short secret-free details | `document_loading::document_diagnostics_page_redacts_queries_and_internal_pages_load_nothing` |
| Resource use is bounded | Per-document caps on resources, buffered bytes, imports, fonts, events, retired snapshots and pump steps | [RESOURCE_LOADING.md](RESOURCE_LOADING.md#limits) |

Accepted limitations: mixed-content blocking has unit tests but no HTTPS end-to-end
fixture; `document.write` is ignored. (Subresource CORS and the CSP parser, listed here
originally, landed in Waves H and I.)

## Phase 3 Wave H security review (CORS)

Scope: `axiom-net/src/cors.rs`, the CORS steps in `NetworkService::run`, `plan_fetch` /
`filter_response` and the subresource request mode in `request_resource`. Each invariant is
checked from the server side (what was actually sent) in `axiom-net/tests/cors.rs` and
`axiom-browser/tests/wave_h.rs`.

| Invariant | How it holds | Evidence |
|-----------|--------------|----------|
| One CORS implementation | Preflights, checks and tainting live in the network service; `fetch()` and subresources only choose the mode. The preflight is an ordinary request through the same service (logged, cancellable, same TLS and scheduler), never a separate client | `cors::preflight_precedes_a_non_simple_request_and_is_cached_per_profile` (the preflight is on the network log) |
| A failing preflight sends nothing of the actual request | The preflight runs before the actual request is prepared; any failure (status, missing grant, redirect, network) returns `NetworkError::Cors` | `cors::a_failing_preflight_sends_no_bytes_of_the_actual_request`, `wave_g::cross_origin_cors_without_allow_origin_rejects_and_no_cors_is_opaque` |
| Preflights carry no credentials or script data | Credentials `omit`; only `Accept`, `Origin`, `Access-Control-Request-Method` / `-Headers` are set by Axiom; the body is empty | same, `cors::credentialed_requests_need_allow_credentials_and_an_exact_origin` |
| Credentialed responses need explicit consent | With credentials, `Access-Control-Allow-Origin` must be the exact origin and `Access-Control-Allow-Credentials: true`; wildcards never apply, and `*` never covers `Authorization` | `cors::tests::preflight_wildcards_do_not_apply_with_credentials_or_to_authorization`, `cors::credentialed_requests_need_allow_credentials_and_an_exact_origin` |
| Redirects cannot launder an origin | Each cross-origin hop is checked before it is followed; after a foreign-to-foreign hop the origin is `null`, and `same-origin` credentials stop at the first cross-origin hop | `cors::cross_origin_redirects_are_checked_per_hop_and_taint_the_origin` |
| The preflight cache cannot outlive the profile's network state | Memory only, per profile, capped at 1024 entries and 2 hours; cleared by `Browser::clear_cache` and on shutdown | `cors::preflight_precedes_a_non_simple_request_and_is_cached_per_profile`, `cors::tests::preflight_cache_matches_origin_url_credentials_and_grants` |
| Rejected cross-origin bytes never reach script | A failed check is a network error before any body is delivered; `filter_response` re-checks on the UI thread; opaque bodies stay in Rust and `Set-Cookie` is never exposed | `fetch::tests::cors_check_requires_matching_allow_origin_and_filters_headers`, `wave_g::credentials_modes_use_the_profile_cookie_jar_and_hide_set_cookie` |
| Script and page errors do not reveal why | `fetch()` rejects with `TypeError("Failed to fetch")`; a subresource fires `error`. The reason is only on `axiom://network` and in the resource diagnostics | `wave_g::cross_origin_cors_without_allow_origin_rejects_and_no_cors_is_opaque`, `wave_h::crossorigin_scripts_and_module_scripts_need_cors_approval` |

Accepted limitations: module imports do not inherit `use-credentials` from the importing
script; CSS `@import` is always `no-cors`; the preflight cache is not partitioned by
top-level site (it is already per profile); errors from cross-origin `no-cors` scripts are
not muted to `"Script error."` when they reach `window.onerror` from a callback (see
[`SCRIPT_LOADING.md`](SCRIPT_LOADING.md)).

## Phase 3 Wave I security review (CSP)

Scope: `crates/axiom-csp`, `axiom-engine/src/csp.rs` and every enforcement point listed in
[CSP.md](CSP.md). Each invariant has a test; where a request must not happen, the test
checks the server side.

| Invariant | How it holds | Evidence |
|-----------|--------------|----------|
| A blocked subresource is never requested | The check runs in `start_resource` before `request_resource` builds the request; the preload scanner skips requests a policy would block | `wave_i::script_src_admits_nonces_and_listed_sources_only`, `wave_i::strict_dynamic_trusts_scripts_loaded_by_trusted_scripts` |
| Redirects cannot escape the policy | The requester attaches a `RedirectCheck`; the network service runs it on every hop before sending it, and a refusal is `NetworkError::Blocked` | `redirect_check::refused_redirect_targets_are_never_contacted`, `wave_i::redirects_of_subresources_are_checked_against_the_policy`, `wave_i::connect_src_gates_fetch_including_redirects` |
| Inline code needs explicit permission | Inline scripts, import maps, inline modules, `<style>`, `style` attributes and handler attributes each check `check_inline` with the element's nonce (only if nonceable); `'unsafe-inline'` is void when a nonce, hash or `'strict-dynamic'` is present | `wave_i::inline_styles_style_attributes_handlers_and_eval_are_gated`, `wave_i::hashes_allow_matching_inline_scripts_and_hashed_attributes` |
| String compilation is gated | `eval()` and `Function()` go through Boa's `ensure_can_compile_strings`, which asks the realm's `EvalGate`; only Axiom's own handler compilation (after its `script-src-attr` check) bypasses it | `wave_i::inline_styles_style_attributes_handlers_and_eval_are_gated` |
| Form submissions and `<base>` obey the policy | `form-action` is checked before the navigation starts and on each redirect; a refused `<base>` is ignored by the loader and by form action resolution, which share one frozen base | `wave_i::form_action_restricts_submission_targets_and_their_redirects`, `wave_i::base_uri_rejects_disallowed_base_elements` |
| `<meta>` cannot weaken or widen header policies | Policies only accumulate; `<meta>` is honoured only in `<head>` and ignores `report-uri`, `frame-ancestors` and `sandbox` | `wave_i::meta_policies_apply_from_where_they_appear` |
| Report-only never blocks | Report-only verdicts record violations but are never `is_blocked` | `wave_i::report_only_policies_report_without_blocking` |
| Reports do not leak secrets | The console shows the blocked URL without its query; after a redirect the report names only the target's origin; samples need `'report-sample'` and are cut to 40 characters; queued reports are capped | `wave_i::redirects_of_subresources_are_checked_against_the_policy`, `axiom-csp` and `csp::tests` unit tests |
| Script and page errors do not reveal why | `fetch()` rejects with `TypeError("Failed to fetch")`; subresources fire `error`; the reason is in the resource diagnostics and the console | `wave_i::connect_src_gates_fetch_including_redirects` |

Accepted limitations: no report delivery; `upgrade-insecure-requests` does not upgrade
navigations or redirects; hash sources do not allow external scripts via SRI; module
imports share one nonce per document; `frame-ancestors`, `sandbox` and Trusted Types are
not enforced (details in [CSP.md](CSP.md#limitations)).

## Process model

| Feature | Status |
|---------|--------|
| Single-process engine | **SUPPORTED** (current) |
| GPU process / renderer sandbox | **UNSUPPORTED** |

**Guidance:** Do not use Axiom to browse untrusted web content until JS, navigation, and fuzzing milestones explicitly target security.
