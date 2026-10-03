# Axiom Networking (Wave F / F.1, Phase 3 Wave E "Networking 2.0", Phase 3 Wave G document loading, Phase 3 Wave F real web navigation)

Companion documents: [HTTP_CACHE.md](HTTP_CACHE.md) (cache states, disk layout, eviction), [DOWNLOADS.md](DOWNLOADS.md) (download manager), [SEARCH_PROVIDERS.md](SEARCH_PROVIDERS.md) (omnibox search), [WEB_COMPATIBILITY.md](WEB_COMPATIBILITY.md) (live-site results) and [WAVE_F_NETWORK_AUDIT.md](WAVE_F_NETWORK_AUDIT.md) (the audit this wave started from).

Status markers:

- **FUNCTIONAL**: implemented and covered by a test that checks behaviour from the server side where that is possible.
- **PARTIAL**: works, but has a documented gap.
- **DEFERRED**: not implemented. The model or hook may exist, but nothing uses it.

Unmeasured values are reported as `None` (and as "n/a" in the UI). They are never shown as zero.

## Pipeline

```text
BrowsingContext (one per tab)
      ↓
ResourceLoader          one cancellation context per tab; type, priority, initiator, limits
      ↓
RequestScheduler        one per profile; fixed worker pool, priority + aging, backpressure
      ↓
NetworkService          one per profile; cookies, HttpCache, redirects, DNS, policy, metrics, log
      ↓
ReqwestTransport        async reqwest on a transport-owned tokio runtime; rustls; H1 + H2 (ALPN)
      ↓
ResponseBodyReader      incremental reads, our own gzip/deflate/br decoding, byte counters
```

There are no resource-specific HTTP clients. Documents, stylesheets, scripts and images all go through `ResourceLoader` → `RequestScheduler` → `NetworkService`. The `Browser` owns one `NetworkService`, `HttpCache` and `RequestScheduler` per profile, and every tab attaches to them with `BrowsingContext::set_network`. Re-attaching to the same scheduler (on a tab switch, for example) does nothing, so switching tabs does not cancel in-flight loads.

The CLI `Engine` (`axiom <url>`) builds a standalone `ResourceLoader` with its own service, because it has no profile. It still uses the same pipeline.

## Browser navigation (Phase 3 Wave F)

```text
                   ┌───────────────┐
                   │    Omnibox    │   typed text, suggestion, bookmark, restore
                   └───────┬───────┘
                           │
                 ┌─────────▼──────────┐
                 │  Input Classifier  │   OmniboxInputClassifier (PSL, IP, port, IDNA)
                 └─────┬────────┬─────┘
                       │        │
                      URL     Search
                       │        │
                       │   ┌────▼────────────────────┐
                       │   │  SearchProviderService  │   profile's selected provider
                       │   └────┬────────────────────┘
                       │        │
                       └────┬───┘
                            │
                ┌───────────▼────────────┐
                │ Browser → Tab →        │   navigation id, cause, events,
                │ BrowsingContext        │   history, omnibox, downloads
                └───────────┬────────────┘
                            │
                ┌───────────▼────────────┐
                │ ResourceLoader →       │
                │ RequestScheduler →     │   one per profile
                │ NetworkService         │
                └───┬────────────────┬───┘
                    │                │
          ┌─────────▼──────┐  ┌──────▼──────────┐
          │ CookieService  │  │ HTTP / TLS       │   reqwest + rustls,
          │ (the only jar) │  │ (reqwest)        │   webpki roots, H1/H2
          └────────────────┘  └──────┬──────────┘
                                     │
                         ┌───────────▼────────┐
                         │ Response (typed,   │   redirects, MIME, charset,
                         │ decoded, cached)   │   download detection
                         └───────────┬────────┘
                                     │
                         ┌───────────▼────────┐
                         │ Document loader    │   commit guard, subresources
                         └───────────┬────────┘
                                     │
                      ┌──────────────▼───────────────┐
                      │ HTML / DOM / JS / CSS /      │
                      │ layout / paint               │
                      └──────────────────────────────┘
```

### Entry points

| Entry | Path |
|-------|------|
| Typed text / Enter | `Browser::submit_omnibox` → `Browser::navigate` → classifier → URL or `SearchProviderService::search_url` → `navigate_resolved_with_transition(url, Typed)` |
| Suggestion, bookmark, restore, duplicate | `navigate_resolved_with_transition` with the matching `VisitTransition` |
| Link click | `Browser::handle_content_click` / `click_node` → `BrowsingContext` link default action (`NavigationCause::Link`) → events |
| Back / forward | `Browser::back` / `forward` → `BrowsingContext::back` / `forward` (`NavigationCause::History`) |
| Reload / stop | `Browser::reload_or_stop` → `BrowsingContext::reload` (`CacheMode::NoCache`, retries a failed URL) or `stop_loading` |
| `axiom://` URLs | served by the `Browser` from `InternalPageRegistry` or a routed page (`axiom://network/<id>`, `axiom://settings/search`, `axiom://search`); never fetched |

### Non-blocking navigation

`BrowsingContext` has two modes, chosen with `set_blocking_navigation`:

- **Blocking** (default; tests, the CLI and headless tools): `navigate`, `reload`, link clicks and back/forward return once the document is interactive (`DOMContentLoaded` fired and no render-blocking stylesheet pending), or after `DOCUMENT_TIMEOUT` (120 s).
- **Background** (the desktop UI, `Browser::set_background_navigation(true)`, set by `axiom_gfx::run_browser`): every navigation call returns immediately with its `NavigationId`. The response commits from `poll_loader` / `tick`, which the window loop calls every frame, so painting, input and other tabs keep running while a page loads.

Either way the context records `NavigationEvent { id, url, cause, kind }` in a bounded queue (256 events; overflow is counted, not blocking), drained by `take_navigation_events`:

| Kind | Meaning |
|------|---------|
| `Started` | a navigation to `url` began (any previous pending one was cancelled first) |
| `Committed { redirected }` | its response became the document; `url` is the final URL after redirects |
| `Interactive` | `DOMContentLoaded` fired |
| `Completed` | `load` fired |
| `Failed { kind }` | the trusted error page is shown; `kind` is the stable `NetworkError::kind_name()` |
| `Cancelled` | superseded by a newer navigation, stopped, or replaced by an internal page |
| `Download` | the response is a download; it went to the download manager and the current document and history were left alone |
| `InternalRequested { initiator }` | the page asked for an `axiom://` URL; the embedder decides |

`navigation_state()` reports `Idle`, `Connecting` (waiting for the response), `Receiving` (body streaming), `Parsing`, `Interactive`, `Completed`, `Failed` or `Cancelled`. A response for a navigation that is no longer pending can never commit (`networking2::newer_navigation_supersedes_a_slow_one`, `wave_f_navigation::a_newer_background_navigation_supersedes_the_pending_one`).

### What the browser does with the events

`Browser::tick` (and every navigation entry point) calls `process_navigation_events`:

- **Omnibox.** On `Committed`, `Failed` and `Cancelled` of the active tab the omnibox shows `Tab::omnibox_url()`: the committed URL (the final one after redirects), the failed URL on an error page, or — while a browser-initiated navigation is pending — its destination. A page-initiated navigation keeps showing the committed URL until it commits, so a page cannot make the address bar claim a destination it has not reached.
- **Visit history.** A visit is recorded on `Interactive`, once, under the final URL, with the transition the navigation started with (`Typed`, `Link`, `Reload`, …). Back/forward is not a new visit. Redirect hops and failed navigations are never recorded.
- **Session history.** A failed navigation adds an entry for the URL that was tried (replaced, not added, for reload and back/forward), so back returns to the page before it, forward retries it and reload retries it (`wave_f_navigation::failed_navigation_keeps_history_and_reload_retries_the_failed_url`). The error page's own `axiom://network-error` URL never enters history.
- **Internal pages.** `InternalRequested` is honoured only when the initiator is itself an `axiom://` page (for example a link on `axiom://settings/search`). Web content cannot open internal pages; the request is logged and dropped (`wave_f_navigation::web_content_cannot_open_internal_pages`).
- **Downloads** are handed to the profile's `DownloadManager` on every tick.

### Request identity (User-Agent and headers)

Axiom sends `User-Agent: Axiom/0.2 (+https://github.com/theworker02/axiom)`. It does not include `Mozilla/5.0`, `AppleWebKit`, `Chrome` or `Safari` tokens. Sites that sniff for them serve Axiom their fallback markup. That is the honest baseline for an independent engine, and it is what the compatibility results in [WEB_COMPATIBILITY.md](WEB_COMPATIBILITY.md) measure. The value is `NetworkServiceConfig::user_agent`, so an embedder can change it deliberately; Axiom never impersonates another browser by default.

Every request also gets `Accept` for its resource type, `Accept-Language` (`NetworkServiceConfig::accept_language`, default `en-US,en;q=0.9`; empty sends none), `Accept-Encoding: gzip, deflate, br` (only codings Axiom decodes itself; `identity` for range requests), `Referer` per referrer policy, and `Cookie` from `CookieService` only (`wave_f_navigation::requests_identify_as_axiom_and_send_accept_language`, `wave_f::referer_follows_policy_and_user_agent_is_set`).

### URL canonicalization

`axiom_url::Url::parse` lowercases hosts, converts internationalized names to punycode with IDNA (`münchen.de` → `xn--mnchen-3ya.de`), validates IPv6 literals and rejects forbidden host code points (`axiom-url` `hosts_are_canonical`). Userinfo is not supported: `'@'` is a forbidden host code point, so `https://bank.example@evil.example/` is rejected instead of being displayed as a misleading address.

### Configurable limits

| Limit | Default | Where |
|-------|---------|-------|
| Connect timeout | 10 s | `NetworkServiceConfig::connect_timeout` |
| Idle read timeout (headers or next chunk) | 300 s | `NetworkServiceConfig::read_timeout` |
| Whole-request timeout | 60 s (none for script `fetch()`, which owns cancellation) | `ResourceRequest::request_timeout_ms` |
| Redirects | 20 | `NetworkServiceConfig::max_redirects` |
| Response header bytes | 256 KiB | `NetworkServiceConfig::max_header_bytes` |
| Body size per type | document 32 MiB, stylesheet 8 MiB, script 16 MiB, image 32 MiB, font 16 MiB, other 64 MiB | `LoaderLimits` |
| Concurrent requests per profile | 12 workers | `SchedulerConfig::max_concurrent` |
| HTTP cache | 64 MiB total, 8 MiB per entry (normal profile: 256 MiB / 16 MiB) | `CacheLimits` |
| Blocking navigation wait | 120 s | `DOCUMENT_TIMEOUT` (blocking mode only) |
| Pending navigation events | 256 | `NavigationEvents` (overflow counted) |

## Crates

| Crate | Role |
|-------|------|
| `axiom-net` | Models, transport, cache, scheduler, service, `test_server` (doc-hidden), `examples/netbench.rs` |
| `axiom-loader` | `ResourceLoader` (browser request semantics above the scheduler) |
| `axiom-document` | Document identity, lifecycle, per-document resource registry, script classification, priorities, preload scanner, content policy and mixed content, diagnostics (no I/O) |
| `axiom-engine` | `BrowsingContext` loads documents and subresources through the loader (`browsing/document.rs`, `browsing/resources.rs`); trusted error page |
| `axiom-browser` | Profile-owned network stack and download manager, `ProfileCookieJar` → `CookieService`, `axiom://network`, `axiom://network/<id>`, `axiom://downloads` |
| `axiom-download` | `DownloadManager`: streaming to partial files, pause/resume with `Range`/`If-Range`, cancel, filename sanitization, private cleanup |

## Status

### Transport and protocols

| Item | Status | Evidence |
|------|--------|----------|
| HTTP/1.1 keep-alive (one TCP connection for 5 sequential requests) | FUNCTIONAL | `wave_f::keep_alive_reuses_one_connection` |
| HTTP/2, negotiated by ALPN; the protocol comes from the response, not from config | FUNCTIONAL | `h2::https_negotiates_http2_via_alpn` (local rustls + hyper server) |
| HTTP/1.1 fallback over TLS | FUNCTIONAL | `h2::http1_only_falls_back_to_http11_over_tls` |
| TLS verification: an untrusted CA, wrong hostname or expired leaf gives a typed `Certificate { kind }` error (`UntrustedIssuer`, `HostnameMismatch`, `Expired`, …) | FUNCTIONAL | `h2::untrusted_certificate_is_rejected`, `h2::hostname_mismatch_is_rejected`, `h2::expired_certificate_is_rejected` |
| Peer certificate metadata on `TlsInfo` (subject, issuer, SANs, serial, validity, SHA-256 fingerprint, validated host name) | FUNCTIONAL | `h2::https_negotiates_http2_via_alpn`. Parsed with `x509-parser` for display only; validation is rustls. |
| HTTP/3 | DEFERRED | `HttpProtocol::Http3` is reserved |
| TLS version and cipher suite on `TlsInfo` | DEFERRED | reqwest does not expose them; reported as `None` and shown as "not exposed by transport" |
| Connections opened | FUNCTIONAL | counted by the `CountConnectionsLayer` connector layer |
| Connections reused / active | DEFERRED | not observable through reqwest; reported as `None` |
| Mid-transfer cancellation (the socket read is interrupted and the server sees the abort) | FUNCTIONAL | `wave_f::cancel_mid_transfer_interrupts_read_and_aborts_stream` |
| Request timeout | FUNCTIONAL | `wave_f::request_timeout_is_enforced` |
| Idle read timeout (`read_timeout`, default 300 s) → `ReadTimeout` | FUNCTIONAL | `wave_e::silent_server_hits_the_read_timeout` |
| Closed port → `ConnectionRefused` | FUNCTIONAL | `wave_e::closed_port_is_connection_refused` |
| Connect timeout → `ConnectTimeout` | PARTIAL | mapped from the transport error; no CI test, because a reliable black-hole address is not available on every machine |
| Response header size limit (`max_header_bytes`, default 256 KiB) → `HeadersTooLarge` | FUNCTIONAL | `wave_e::oversized_response_headers_are_rejected` |
| Every error has a stable `kind_name()` used in logs, the network log and error pages | FUNCTIONAL | asserted in the `wave_e` and `h2` tests |
| Malformed or truncated responses give typed errors, never panics | FUNCTIONAL | `wave_f::malformed_responses_are_errors_not_panics` |
| POST is never retried automatically | FUNCTIONAL | `wave_f::post_is_not_retried_after_connection_failure` |
| Streaming upload | FUNCTIONAL | `RequestBody::Stream` is sent with chunked transfer encoding as it is produced and is never replayed across a 307/308 (`wave_f::streamed_body_is_not_replayed_across_307`). Reader sources read at most 256 KiB ahead of the connection. `StreamingBody::channel()` gives a producer-side `UploadSender` (`write`, `buffered`, `close`, `error`); dropping it without `close` fails the request (`wave_g2::channel_body_is_sent_chunked_as_it_is_written`, `wave_g2::channel_body_error_or_drop_fails_the_request`, `wave_g2::writes_after_the_body_is_gone_are_rejected`) |

### Content coding and bodies

| Item | Status | Evidence |
|------|--------|----------|
| gzip / deflate / br decoded by Axiom, with separate transferred and decoded byte counts | FUNCTIONAL | `wave_f::content_encodings_decode_with_honest_byte_counts` |
| A corrupt encoded body gives a `Body` error | FUNCTIONAL | `wave_f::corrupt_encoded_body_is_a_body_error` |
| `max_body_bytes` is enforced with and without `Content-Length` | FUNCTIONAL | `wave_f::max_body_bytes_enforced_with_and_without_content_length` |
| The scheduler streams `Data` chunks into a bounded channel (backpressure) | FUNCTIONAL | `wave_f::scheduler_streams_data_events_in_chunks` |
| Per-request flow control: with `NetworkRequest::flow` set, a worker stops reading once the window of delivered-but-unreleased bytes is full, so TCP backpressure reaches the server; cancellation still interrupts a paused worker | FUNCTIONAL | `wave_g2::flow_window_pauses_the_worker_until_the_consumer_releases`, `wave_g2::cancelling_a_paused_request_frees_its_worker` |
| Range / 206 (not cached) | FUNCTIONAL | `wave_f::range_request_returns_206_and_is_not_cached` |
| Incremental delivery to consumers | PARTIAL | the transport and scheduler stream, but `ResourceLoader` assembles a complete body per resource (bounded by `LoaderLimits`) before handing it to the HTML/CSS/JS/image consumers. The HTML parser is not incremental. |

### Cache (RFC 9111 subset)

The `HttpCache` is profile-scoped. The tee that writes into the cache is bounded by `max_entry_bytes` (default 8 MiB; 16 MiB for a normal browser profile). A larger response still streams to the consumer, but is not stored. States, disk layout and eviction are described in [HTTP_CACHE.md](HTTP_CACHE.md).

| Item | Status | Evidence |
|------|--------|----------|
| Freshness from `max-age` | FUNCTIONAL | `wave_f::fresh_max_age_is_served_without_network` |
| `Expires` (past vs future) | FUNCTIONAL | `wave_f::expires_past_is_stale_and_future_is_fresh` |
| Heuristic freshness (10% of the `Last-Modified` age) only when `Last-Modified` exists | FUNCTIONAL | `wave_f::heuristic_freshness_requires_last_modified` |
| ETag → `If-None-Match` → 304 freshen | FUNCTIONAL | `wave_f::etag_revalidation_uses_304` |
| `Last-Modified` → `If-Modified-Since` with `no-cache` | FUNCTIONAL | `wave_f::last_modified_revalidation_with_no_cache` |
| `Vary` variants | FUNCTIONAL | `wave_f::vary_selects_the_matching_variant` |
| `no-store` | FUNCTIONAL | `wave_f::no_store_is_never_cached` |
| Unsafe methods invalidate the cached GET | FUNCTIONAL | `wave_f::unsafe_method_invalidates_cached_get` |
| Cache modes `Reload`, `OnlyIfCached`, `ForceCache`, `NoStore`, `NoCache` | FUNCTIONAL | `wave_f::reload_bypasses_fresh_cache`, `wave_f::only_if_cached_never_touches_the_network`, `wave_f::force_cache_serves_stale_entries_without_network`, `wave_f::no_store_mode_neither_reads_nor_writes_the_cache`, `wave_f::no_cache_mode_revalidates_fresh_entries` |
| The cache buffer stays bounded for a 32 MiB streamed response | FUNCTIONAL | `wave_f::large_streamed_response_keeps_cache_buffer_bounded` |
| Tabs in one profile share one cache | FUNCTIONAL | `wave_f1::tabs_share_the_profile_scheduler_and_cache` |
| Private profile cache is separate and cleared on close | FUNCTIONAL | `wave_f1::private_profile_cache_is_separate_and_cleared_on_close` |
| Disk-backed persistent cache for normal profiles (`<profile>/network-cache/objects` + `index`), LRU order kept across restarts | FUNCTIONAL | `wave_e::persistent_profile_cache_survives_restart`, `networking2::http_cache_is_per_profile_and_persists_only_for_normal_profiles`, `cache::tests::disk_cache_*` |
| Profiles never share a cache; clearing one leaves the other intact | FUNCTIONAL | `wave_e::profiles_do_not_share_disk_caches`, `wave_f::separate_services_do_not_share_cache` |
| Private cache never touches disk, even if a directory is configured | FUNCTIONAL | `wave_e::private_profile_cache_never_touches_disk_and_is_gone_after_shutdown` |
| Cache states `Miss`, `Hit`, `Stale`, `Revalidated`, `Bypassed`, `NotCacheable` | FUNCTIONAL | `wave_f::no_store_is_never_cached` (`NotCacheable`), `wave_f::force_cache_serves_stale_entries_without_network` (`Stale`), `wave_e::persistent_profile_cache_survives_restart` (`Revalidated` after restart) |
| `Set-Cookie` is never stored in the cache (memory or disk), and a 304 cannot add it back | FUNCTIONAL | `cache::tests::set_cookie_is_never_stored_in_memory_or_on_disk` |

### Redirects, cookies, referrer

| Item | Status | Evidence |
|------|--------|----------|
| 301/302/303/307/308 method and body rules, checked on the server side | FUNCTIONAL | `wave_f::redirect_method_and_body_matrix_verified_server_side` |
| Redirect limit (20) | FUNCTIONAL | `wave_f::redirect_limit_is_enforced` |
| At the limit, a repeated hop is `RedirectLoop`; distinct hops are `TooManyRedirects { limit }`. Loops are followed up to the limit because cookies set along the way can legitimately end them. | FUNCTIONAL | `wave_e::redirect_loop_is_classified_as_a_loop`, `wave_e::distinct_hops_over_the_limit_are_too_many_redirects` |
| Redirect chain recorded per request (status, from, to, method, cross-origin) | FUNCTIONAL | `wave_e::redirect_chain_is_followed_and_every_hop_is_logged` |
| Non-http(s) targets (`axiom:`, `file:`, `ftp:`) refused with `UnsupportedScheme`, including as a redirect `Location` | FUNCTIONAL | `wave_e::unsupported_schemes_are_rejected_before_and_during_redirects` |
| Cross-origin redirect strips `Authorization` and records the hop | FUNCTIONAL | `wave_f::cross_origin_redirect_strips_authorization_and_records_metadata` |
| Cookies are read and stored on every redirect hop through `CookieProvider`; forged `Cookie` headers are stripped | FUNCTIONAL | `wave_f::cookies_are_stored_and_sent_on_each_redirect_hop` |
| `CredentialsMode::Omit` | FUNCTIONAL | `wave_f::credentials_omit_sends_and_stores_no_cookies` |
| Browser `Set-Cookie` → next request's `Cookie` header (end to end, server-recorded) | FUNCTIONAL | `wave_d::e2e_httponly_set_cookie_then_cookie_header`, `wave_d::e2e_document_cookie_to_http` |
| SameSite on the request side (`Cookie` header) | FUNCTIONAL | `ProfileCookieJar` builds a `CookieAccessContext` from top-level site, method and navigation type |
| SameSite on the store side (`Set-Cookie` in a cross-site context) | PARTIAL | `process_set_cookies` does not receive the request context yet |
| Referrer policy, fragment stripping, downgrade rule (loopback counts as potentially trustworthy) | FUNCTIONAL | `wave_f::referer_follows_policy_and_user_agent_is_set` |

### DNS and policy

| Item | Status | Evidence |
|------|--------|----------|
| Injected `DnsResolver` used by the transport; typed `Dns` error | FUNCTIONAL | `wave_f::injected_resolver_is_used_and_failures_are_typed` |
| `HostBlocklist` blocks the request and redirect targets | FUNCTIONAL | `wave_f::policy_blocks_requests_and_redirect_targets` |
| DNS-over-HTTPS | DEFERRED | |
| Script `fetch()` (Wave G) | FUNCTIONAL | see `docs/FETCH.md` |
| Mixed content for `fetch()` | FUNCTIONAL | `fetch::tests::mixed_content_is_blocked_but_loopback_is_trustworthy` |
| Cross-origin redirects refused for `same-origin` mode requests | FUNCTIONAL | `cors::same_origin_mode_still_refuses_cross_origin_redirects` |
| CORS in the network service (`cors.rs`): `Origin` on cross-origin `cors` requests, the response check (`Access-Control-Allow-Origin` / `-Credentials`) on every cross-origin hop including redirects, origin tainting to `null`, and `NetworkError::Cors` (kind `cors`) on failure. Script sees a `cors` response with safelisted plus `Access-Control-Expose-Headers` headers | FUNCTIONAL (Wave H) | `cors::cross_origin_redirects_are_checked_per_hop_and_taint_the_origin`, `cors::simple_and_same_origin_requests_are_not_preflighted`, `fetch::tests::cors_check_requires_matching_allow_origin_and_filters_headers`, `wave_g::cross_origin_cors_with_allow_origin_exposes_only_safelisted_and_listed_headers` |
| CORS preflight (`OPTIONS`) through the same service, own log row (initiator `cors-preflight`), no credentials, no redirects | FUNCTIONAL (Wave H) | `cors::preflight_precedes_a_non_simple_request_and_is_cached_per_profile`, `cors::a_failing_preflight_sends_no_bytes_of_the_actual_request`, `cors::the_use_cors_preflight_flag_forces_a_preflight_for_a_simple_request`, `wave_h::fetch_preflights_non_simple_requests_on_the_profile_network_service` |
| Preflight cache: per profile, memory only, max-age default 5 s / cap 2 h, 1024 entries; `NetworkService::clear_cache` / `Browser::clear_cache` and shutdown clear it | FUNCTIONAL (Wave H) | `cors::max_age_zero_is_not_cached`, `cors::tests::preflight_cache_matches_origin_url_credentials_and_grants` |
| Subresource CORS (`crossorigin`, module scripts, fonts) | FUNCTIONAL (Wave H) | `wave_h::crossorigin_scripts_and_module_scripts_need_cors_approval`, `wave_h::crossorigin_images_stylesheets_and_fonts`; navigation never goes through CORS |
| Per-request redirect check (`NetworkRequest::redirect_check`): run on every redirect hop before it is sent; a refusal is `NetworkError::Blocked`. The document's CSP uses it for subresources, `fetch()` and form navigations | FUNCTIONAL (Wave I) | `redirect_check::refused_redirect_targets_are_never_contacted`, `wave_i::redirects_of_subresources_are_checked_against_the_policy` |
| Service workers | DEFERRED | — |

### Scheduler

The scheduler runs a fixed pool of `max_concurrent` worker threads (default 12). It never spawns a thread per request.

| Item | Status | Evidence |
|------|--------|----------|
| Bounded concurrency | FUNCTIONAL | `wave_f::scheduler_bounds_concurrency`, `wave_f1::fixture_subresources_load_concurrently_through_the_loader` |
| Priority order plus aging (no starvation) | FUNCTIONAL | `wave_f::scheduler_orders_by_priority_then_ages` |
| `cancel_context` cancels only that tab's work | FUNCTIONAL | `wave_f::cancel_context_only_cancels_that_context`, `wave_f1::closing_a_tab_cancels_only_its_requests` |
| Tab switches keep background loads | FUNCTIONAL | `wave_f1::switching_tabs_keeps_background_loads_alive` |
| Background tabs are drained every tick | FUNCTIONAL | `TabManager::tick_active` polls every tab's loader |
| The desktop UI never blocks on a navigation | FUNCTIONAL | Phase 3 Wave F: the desktop runs in background navigation mode, so every navigation returns at once and commits from the frame loop (`wave_f_navigation::background_navigation_returns_immediately_and_commits_from_tick`). Blocking mode (tests, CLI) is `start_navigation` plus an event-driven wait (`ResourceLoader::wait_next`, no sleeps). |

### Page loading

| Item | Status | Evidence |
|------|--------|----------|
| Stylesheets load concurrently and apply in document order (inline `<style>` in source position) | FUNCTIONAL | `wave_f1::fixture_subresources_load_concurrently_through_the_loader` |
| Alternate stylesheets are not requested; wrong-MIME stylesheets are ignored | FUNCTIONAL | same test, plus `wave_f1::stylesheet_with_wrong_mime_is_ignored` |
| Blocking scripts are fetched in parallel and run in document order | FUNCTIONAL | fixture test (reverse-delayed scripts still run in order `0,1,2,3,4`) |
| Images load in the background; first paint does not wait for them | FUNCTIONAL | fixture test (navigation finishes before the 700 ms image delay) |
| Web fonts | PARTIAL | `@font-face` faces whose family is used are fetched through the loader, relative to their stylesheet, and block `load` (`document_loading::stylesheets_imports_fonts_and_failures`); rendering does not use them yet |
| Document loading pipeline: streaming HTML parse, per-document resource registry, generation guard, script scheduling, `@import`, dynamic insertion, lifecycle events | FUNCTIONAL | Phase 3 Wave G; see [DOCUMENT_LOADING.md](DOCUMENT_LOADING.md), [RESOURCE_LOADING.md](RESOURCE_LOADING.md), [SCRIPT_LOADING.md](SCRIPT_LOADING.md), [PAGE_LIFECYCLE.md](PAGE_LIFECYCLE.md) and `axiom-browser/tests/document_loading.rs` |
| Subresource `ContentPolicy` (embedder hook) and mixed-content blocking | FUNCTIONAL | `document_loading::content_policy_blocks_before_any_request`, `axiom-document` `policy::tests` |
| Content Security Policy (headers and `<meta>`, enforce and report-only) | FUNCTIONAL (Wave I; no report delivery) | [CSP.md](CSP.md), `axiom-browser/tests/wave_i.rs` |
| Trusted network error page (server bytes never shown), titled by error kind, with a one-line summary, a "Try again" link for http(s) and the stable error code only (library messages go to the log and `axiom://network`, never the page) | FUNCTIONAL | `wave_f1::navigation_error_shows_trusted_page_without_server_bytes` |
| Failed navigations keep session history intact; the address bar shows the URL that failed; reload retries it | FUNCTIONAL | `wave_f_navigation::failed_navigation_keeps_history_and_reload_retries_the_failed_url` |
| Redirects update the address bar and record one visit (final URL) | FUNCTIONAL | `wave_f_navigation::redirects_update_the_address_bar_and_record_one_visit_for_the_final_url` |
| A download navigation leaves the current document and history untouched | FUNCTIONAL | `wave_f_navigation::downloads_leave_the_current_page_and_history_alone` |
| Security indicator from the committed response's verified TLS, never from the URL scheme; mixed content and certificate errors shown separately | FUNCTIONAL | `chrome::tests::https_indicator_requires_a_verified_tls_response`, `chrome::tests::certificate_errors_and_failures_are_distinct`, `networking2::plain_http_pages_are_marked_insecure` |

### Observability

| Item | Status | Evidence |
|------|--------|----------|
| Timing: TTFB and total measured; DNS, connect, TLS and request-sent are `None` | PARTIAL | `wave_f::timing_reports_only_measured_phases` |
| The network log redacts credentials | FUNCTIONAL | `wave_f::network_log_redacts_credentials` |
| `axiom://network` shows profile data with no secrets (query strings stripped, no cookie values) and "n/a" for unmeasured values | FUNCTIONAL | `wave_f1::network_page_uses_profile_data_and_hides_secrets` |
| Activity stream (`NetworkService::subscribe_activity`): `RequestStarted`, `RequestHeadersReady`, `CacheHit`, `CacheMiss`, `ResponseStarted`, `Redirected`, `ResponseHeadersReady`, `DataReceived`, `RequestCompleted`, `RequestFailed`, `RequestCanceled`. Bounded per subscriber; a slow subscriber loses events (counted) instead of slowing requests; headers are redacted before publishing. | FUNCTIONAL | `wave_e::activity_stream_reports_a_redirected_request_in_order`, `wave_e::activity_stream_reports_cache_hits_and_cancellation`, `wave_e::slow_subscriber_drops_events_instead_of_blocking` |
| Structured log line per request (`target: axiom_net`): request, context and profile ids, method, host, status, protocol, cache state, bytes, duration, error kind. No paths, queries, headers or bodies. | FUNCTIONAL | `OutcomeLog` in `service.rs` |
| `axiom://network/<id>` request detail: general facts, timing ("not measured" where not measured), redirect chain, TLS and certificate, redacted request and response headers, error kind | FUNCTIONAL | `networking2::request_detail_page_shows_redirects_and_hides_secrets` |
| Downloads: an attachment navigation stops at headers and is handed to the profile's `DownloadManager` | FUNCTIONAL | `networking2::navigating_to_an_attachment_hands_off_to_the_download_manager`; see [DOWNLOADS.md](DOWNLOADS.md) |

## Benchmarks

`cargo run -p axiom-net --example netbench --release` prints JSON. It measures Axiom's overhead over loopback HTTP/1.1 and a local TLS (HTTP/2) server, not real-world network performance. A sample run on the development machine (Windows, release build, Wave E):

| Metric | Result |
|--------|--------|
| Throughput, 16 MiB identity body | about 960 MiB/s |
| 36 requests × 100 ms server delay, limit 12 | 306 ms wall time (3600 ms serial); server peak concurrency 12 |
| Memory cache hit, 32 KiB body | p50 4.5 µs, p95 6.9 µs; 1 server request for 501 fetches |
| Disk cache hit after restart, 256 KiB body | cache open 4.7 ms; p50 134 µs, p95 198 µs; 1 server request |
| Chunked stream, 20 × 25 ms | first `Data` event at 27.7 ms; complete at 509 ms |
| Cancellation, from `cancel()` to `Cancelled` | p50 14.8 µs; the server saw 10 of 10 streams aborted |
| HTTPS cold (new service: TCP + TLS + ALPN) | p50 0.84 ms, max 1.7 ms (loopback) |
| HTTPS warm (reused HTTP/2 connection) | p50 0.064 ms, p95 0.145 ms |
| 6-hop redirect chain | p50 0.34 ms, p95 0.95 ms |

Document-level cold/warm numbers (navigation → `DOMContentLoaded` → `load`) come from `cargo run -p axiom-browser --example docbench --release`; see [DOCUMENT_LOADING.md](DOCUMENT_LOADING.md#benchmarks).

### Real-world smoke test (manual, not CI)

`cargo run -p axiom-net --example https_smoke -- https://example.com/ https://www.wikipedia.org/` fetches each URL twice through a normal service (the Mozilla CA set from `webpki-roots`, compiled into the binary — not the OS trust store; verification on) and prints status, negotiated protocol, cache state, certificate facts and timing. CI never depends on third-party sites. A Wave E run: both sites negotiated `h2` with verified certificates; example.com's second fetch was a `hit` and wikipedia.org's was `revalidated`.

`cargo run --release -p axiom-browser --example live_smoke -- [--png DIR] [--provider ID] [PROBE|URL|QUERY]...` (Phase 3 Wave F) types each target into a private window's omnibox and reports NETWORK, DOCUMENT, CSS, IMAGES, RENDER, SCRIPT and INTERACT separately; `--png` saves each frame. Results are recorded in [WEB_COMPATIBILITY.md](WEB_COMPATIBILITY.md). It is manual only: Google and every other third-party site stay out of CI.

## Security invariants

- TLS verification is always on; `accept_invalid_certs` is never used.
- There is no automatic HTTPS→HTTP downgrade.
- Non-idempotent requests are never retried automatically.
- `CookieService` is the only cookie authority. The network service strips any caller-supplied `Cookie` header.
- A private profile's cache is memory-only and cleared when the profile closes. Its downloads list is forgotten and unfinished files are deleted.
- The HTTP cache never stores `Set-Cookie`; cache eviction never touches cookies, history or storage.
- The security indicator comes from a verified TLS response, never from the URL scheme.
- `axiom://network` and `axiom://network/<id>` redact `Authorization`, `Proxy-Authorization`, `Cookie` and `Set-Cookie`, and hide query strings. Bodies are never logged.
- Error pages are generated by Axiom from a typed error. Server bytes are never shown as trusted chrome.
- Worker threads are internal; the DOM and JS see only loader events.
- Loader events for a replaced document are dropped (counted as stale); they never reach the active document.
- Direct-HTTP audit (Phase 3 Wave G): only `axiom-net` depends on an HTTP client (`reqwest`; `hyper` is a test-only dev-dependency). The `ureq` entry in the workspace `Cargo.toml` is not used by any crate. `ResourceLoader::fetch_sync` has no callers and goes through the scheduler. The DOM, parser, style and JS crates never make requests; they queue work that the document loader starts through `ResourceLoader`.
