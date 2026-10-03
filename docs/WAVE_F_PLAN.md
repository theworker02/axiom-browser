# Wave F Implementation Plans

This file has two plans. The first is Phase 3 Wave F, "Networking 2.0 — Real Web Navigation, Search Providers & Internet-Ready Browser Pipeline" (2026-09-28). The second, kept for history, is the original Wave F / F.1 transport plan.

# Phase 3 Wave F — Real web navigation and search providers

**Status:** delivered. Stopped before Phase 3 Wave G (browser compatibility) and before any Axiom Search crawler or index work.

The starting point is [WAVE_F_NETWORK_AUDIT.md](WAVE_F_NETWORK_AUDIT.md). It found that the network stack itself was complete; the gaps were in navigation, the omnibox and search. So this wave builds on the existing `NetworkService`, `RequestScheduler`, `CookieService` and `ResourceLoader` and adds no second transport, cookie jar or cache.

| Item | Result | Where / test |
|------|--------|--------------|
| F0 audit | Done | `docs/WAVE_F_NETWORK_AUDIT.md` |
| F1 navigation request/response | Existing `ResourceRequest::document` / `ResourceResponse`; every navigation now has a `NavigationId` and a `NavigationCause`, and stale responses never commit | `networking2::newer_navigation_supersedes_a_slow_one`, `wave_f_navigation::a_newer_background_navigation_supersedes_the_pending_one` |
| F2 transport | Existing reqwest + rustls transport behind `HttpTransport`; TLS verification, SNI, DNS trait, streaming, timeouts, cancellation, gzip/deflate/br were already FUNCTIONAL | `docs/NETWORKING.md` status tables |
| F3 headers | `Accept-Language` added; User-Agent is configurable and stays the honest `Axiom/0.2 (+…)` string (no Chrome impersonation) | `wave_f_navigation::requests_identify_as_axiom_and_send_accept_language` |
| F4 cookies | `CookieService` remains the only jar (reqwest cookie store off) | `axiom-net` `wave_f::cookies_are_stored_and_sent_on_each_redirect_hop`, `networking2::cookies_are_isolated_between_profiles` |
| F5 redirects | Existing redirect engine; the address bar now follows the final URL and history records one visit | `wave_f_navigation::redirects_update_the_address_bar_and_record_one_visit_for_the_final_url` |
| F6 MIME / charset | Existing `Mime` / `TextDecoder`; downloads no longer replace the current document or history | `wave_f_navigation::downloads_leave_the_current_page_and_history_alone` |
| F7 non-blocking navigation | Engine lifecycle events and states; background mode for the desktop | `wave_f_navigation::background_navigation_returns_immediately_and_commits_from_tick`, `navigation_events_report_the_lifecycle_in_order` |
| F8 stop / reload / back / forward | Failed navigations keep history; reload retries the failed URL; link clicks reach the omnibox and history | `wave_f_navigation::failed_navigation_keeps_history_and_reload_retries_the_failed_url`, `link_clicks_are_recorded_and_update_the_address_bar` |
| F9 omnibox classifier | Rewritten: PSL, IPv4/IPv6, ports, IDN, quotes, `?` prefix, unsafe schemes searched | `classifier::tests::*` |
| F10 search providers | `SearchProviderService` (Google, Bing, DuckDuckGo, Axiom Search, custom); the omnibox calls `search_url` | `search::tests::*`, `wave_f_navigation::typed_search_goes_to_the_selected_provider_with_an_encoded_query` |
| F11 per-profile preference | Settings schema v2 `search_provider_id`; private windows inherit without writing back; `axiom://settings/search` | `wave_f_navigation::search_engine_choice_persists_per_profile_and_private_windows_inherit_it` |
| F12 Axiom Search | Honest placeholder page plus the design document; no crawler | `wave_f_navigation::axiom_search_is_an_honest_placeholder`, `docs/AXIOM_SEARCH_ARCHITECTURE.md` |
| F13–F15 resource loading | Existing `ResourceLoader` (CSS, scripts, images in parallel, same-origin checks, mixed-content policy) | `wave_f1`, `wave_g`, `document_loading` tests |
| F16 basic CORS | Simple cross-origin `fetch()` with `Access-Control-Allow-Origin` / `-Credentials` / `-Expose-Headers`; preflight requests fail before sending; navigation never uses CORS | `fetch::tests::cors_check_*`, `wave_g::cross_origin_cors_*` |
| F17 errors | Trusted error pages show a summary and the stable error code only; web content cannot open `axiom://` pages | `wave_f1::navigation_error_shows_trusted_page_without_server_bytes`, `wave_f_navigation::web_content_cannot_open_internal_pages` |
| F18 security indicator | Existing: derived from verified TLS on the committed response only | `networking2::plain_http_pages_are_marked_insecure`, `axiom-net` `h2` certificate tests |
| F19 observability | Existing activity log without cookie or authorization values; navigation log lines strip the query string | `axiom://network`, `axiom_nav` log target |
| F20 cache | Existing RFC 9111 cache; private profiles memory-only | `docs/HTTP_CACHE.md` |
| F21 downloads | Existing `DownloadManager` boundary; a download no longer disturbs the page | see F6 |
| F22 local test servers | `wave_f_navigation.rs` (13 tests) on `axiom-testserver` | `crates/axiom-browser/tests/wave_f_navigation.rs` |
| F23–F24 live smoke / Google milestone | Manual `examples/live_smoke.rs`; not in CI | `docs/WEB_COMPATIBILITY.md` |
| F25 compatibility report | Done | `docs/WEB_COMPATIBILITY.md` |
| F26 limits | Documented in one table | `docs/NETWORKING.md`, "Configurable limits" |
| F27 docs | NETWORKING, SEARCH_PROVIDERS, AXIOM_SEARCH_ARCHITECTURE, WEB_COMPATIBILITY, this plan, PHASE3_PROGRESS, CHANGELOG | — |

### Deferred from this wave

- CORS preflight (`OPTIONS`), `crossorigin` subresource CORS, CSP parsing.
- Sending keystrokes to provider suggestion endpoints (templates are recorded, never contacted).
- HTTP/3, DNS-over-HTTPS.
- Everything the live results in [WEB_COMPATIBILITY.md](WEB_COMPATIBILITY.md) list as a blocker (text rendering, SVG, ES modules, missing JS globals, complex layout). These are Phase 3 Wave G candidates.

---

# Wave F Implementation Plan — Networking 2.0 (original transport wave)

**Status:** Wave F delivered, then hardened in Wave F.1. Stopped before Wave G/H.
**Stop:** Do not start Wave G (Fetch) or Wave H (CORS) until F.1 is accepted.

Per-item status and the test behind each claim are in `docs/NETWORKING.md`.

## Audit summary (Wave E baseline)

| Path | Finding |
|------|---------|
| `axiom-net::HttpClient` | GET-only ureq that read the full body. Replaced by `NetworkService` over an async reqwest transport. |
| `browsing::load_document_bytes` / `load_bytes_relative` | Called the HTTP client directly. Now go through `ResourceLoader` → `RequestScheduler` → `NetworkService`. |
| Cookies | `CookieService` stays the single authority, reached through `CookieProvider`. |
| TLS | rustls with verification on. |

## Why F.1 was needed

When Wave F was accepted, several claims in its report were stronger than the code supported:

- Each tab had its own network service and cache, not one per profile.
- Subresources loaded one after another and blocked the page.
- Cancellation was only checked around the blocking send.
- HTTP/2 was "supported" because it was configured, not because negotiation was verified.
- Some metrics were reported as measured when they were not.

F.1 fixed these and added tests that check behaviour from the server side.

## F.1 deliverables

| Item | Result |
|------|--------|
| Async reqwest transport on a transport-owned runtime; cancellation via `select!` | Done. A mid-transfer cancel reaches the server as an aborted stream. |
| Own content decoding (gzip/deflate/br) with honest byte counters | Done |
| RFC 9111 cache: freshness, validators, Vary, invalidation, all cache modes, bounded tee | Done (memory only) |
| Scheduler: fixed pool, priority + aging, context cancellation, bounded channels, body limits | Done |
| One network service, cache and scheduler per profile; tabs attach; private profile cleared on close | Done |
| Tab switches no longer cancel loads; background tabs drained each tick | Done (bug found by `wave_f1` tests) |
| Parallel CSS/script/image loading, document-order CSS and script execution, non-blocking images | Done |
| TLS errors classified correctly (the rustls error sits inside the `io::Error` payload) | Done |
| HTTP/2 verified by ALPN against a local TLS server; certificate failures tested | Done (`tests/h2.rs`) |
| Unmeasured timings and pool counters reported as `None` / "n/a" | Done |
| `axiom://network` reads the profile log with no secrets | Done |
| `examples/netbench.rs` (JSON output) | Done |
| Fixture `tests/fixtures/network/subresources.html` | Done |

## Remaining PARTIAL / DEFERRED (carried forward)

- Disk-backed HTTP cache (**DEFERRED**)
- TLS version, and DNS/connect/TLS/request-sent timing phases (**DEFERRED**, not observable through reqwest)
- Connection reuse and active counts (**DEFERRED**)
- ~~Streaming upload: buffered up to 64 MiB (**PARTIAL**)~~ Resolved in Wave G.2: chunked streaming with bounded read-ahead (see `docs/NETWORKING.md`)
- Store-side SameSite context for `Set-Cookie` (**PARTIAL**)
- Loader hands complete bodies to consumers; the HTML parser is not incremental (**PARTIAL**)
- ~~The blocking top-level document fetch stalls draining of other tabs while it runs (**PARTIAL**)~~ Resolved in Phase 3 Wave F: the desktop uses background navigation
- Web fonts: no `@font-face` consumer (**DEFERRED**)
- HTTP/3, DNS-over-HTTPS, CSP (**DEFERRED**); basic CORS for simple requests landed in Phase 3 Wave F, preflight is still deferred

## Non-negotiables (held)

- TLS verification is on; `accept_invalid_certs` is never used.
- `CookieService` is the only cookie authority; forged `Cookie` headers are stripped.
- There are no resource-specific HTTP clients.
- A request's identity is its `NetworkRequestId`, never its URL.
- Bodies stream; only bounded buffers (the cache tee and loader limits) exist.
- Worker threads are internal; consumers see channels and tokens.
- Concurrency is bounded by a fixed worker pool.
- Private network state is memory-only and cleared on close.
- The HTTP/2 claim is based on the negotiated protocol.
- Unmeasured timing phases are never reported as measured.
