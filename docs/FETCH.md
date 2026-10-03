# Fetch (Wave G; CORS completed in Wave H)

`fetch()` for page scripts, built on the Wave F resource loading platform. There is no
fetch-specific HTTP client: every request goes through the tab's `ResourceLoader`, the
profile's `RequestScheduler`, `NetworkService`, HTTP cache and `CookieService`.

## Architecture

```text
script: fetch(input, init)
  └─ fetch_prelude.js        WebIDL-style validation, Request/Response/Headers/streams objects
      └─ __axiom_fetchStart  (native, axiom-js)
          └─ DocumentJsHost::fetch_start → plan_fetch  (axiom-engine/src/fetch.rs, the security boundary)
              └─ FetchQueue (per document) ── drained by BrowsingContext on every tick / poll
                  └─ ResourceLoader::start (stream_body) → RequestScheduler → NetworkService
                      ← LoaderEvent::Response → filter_response → JsContext::fetch_response → promise resolves
                      ← LoaderEvent::Chunk    → JsContext::fetch_chunk    → ReadableStream enqueue
                      ← LoaderEvent::Completed/Failed → fetch_complete / fetch_fail
```

- **Request identity** is the loader's `RequestId`. Script only sees an opaque number; the
  URL is never used as a key.
- **Two layers of validation.** The JS prelude rejects malformed input early, with
  spec-shaped errors. Rust re-validates everything (`plan_fetch`), so a script that bypasses
  the prelude gains nothing.
- **Script never touches transport threads.** Commands are queued on the UI thread and
  loader events are delivered on the UI thread, followed by a microtask checkpoint
  (`Context::run_jobs`, then rejection notifications, then `run_jobs` again).
- **Uploads and flow control stay on the UI thread too.** Script writes stream chunks
  through `__axiom_fetchUploadWrite` into an `UploadSender`, and releases response bytes
  through `__axiom_fetchConsumed` into the request's `FlowControl`. Neither exposes a
  transport thread.
- **Streaming.** The promise resolves when headers arrive. The body is delivered chunk by
  chunk as the scheduler reports data; the loader does not assemble streamed bodies.

## Status

| Item | Status | Evidence |
|------|--------|----------|
| `fetch()` over `ResourceLoader` with `ResourceType::Fetch`, the tab's cancellation context and the profile scheduler | FUNCTIONAL | `wave_g::get_resolves_with_response_metadata_and_json_and_text_bodies` |
| `Headers` (guards, validation, combining, sorted iteration, `getSetCookie`) | FUNCTIONAL | `wave_g::request_headers_and_response_objects_validate_input` |
| `Request` (method normalization, forbidden methods, mode/credentials/cache/redirect enums, body rules, `clone`) | FUNCTIONAL | same |
| `Response` (constructor, `error`, `redirect`, `json`, `clone`) | FUNCTIONAL | same, `wave_g::body_is_single_use_and_clone_tees_it` |
| Body mixin: `text`, `json`, `arrayBuffer`, `bytes`, `body`, `bodyUsed` | FUNCTIONAL | `wave_g::body_is_single_use_and_clone_tees_it` |
| Body mixin: `blob` (type from `Content-Type`), `formData` (`multipart/form-data` and `application/x-www-form-urlencoded`; anything else rejects with `TypeError`) | FUNCTIONAL | `wave_g2::blob_and_form_data_body_readers` |
| `Blob`, `File`, `FormData`, `URLSearchParams` globals | FUNCTIONAL | same, `wave_g2::urlsearchparams_formdata_and_blob_request_bodies`; `new FormData(form)` throws (no form submission model yet) and `endings: 'native'` uses LF |
| Request bodies: string, `ArrayBuffer`, typed arrays | FUNCTIONAL | `wave_g::post_sends_body_and_custom_headers_but_never_forbidden_ones` |
| Request bodies: `Blob` (its type becomes `Content-Type`), `FormData` (multipart with a generated boundary), `URLSearchParams` (`application/x-www-form-urlencoded;charset=UTF-8`) | FUNCTIONAL | `wave_g2::urlsearchparams_formdata_and_blob_request_bodies` (server-side body and header checks) |
| Request bodies: `ReadableStream` with `duplex: 'half'` | FUNCTIONAL | `wave_g2::streaming_request_body_is_sent_as_it_is_produced` (server sees `Transfer-Encoding: chunked` and several chunks), `wave_g2::large_streaming_upload_is_paced_and_arrives_intact` (1 MiB), `wave_g2::an_erroring_upload_stream_fails_the_fetch` |
| Stream bodies: pacing | FUNCTIONAL | script pulls from the stream only while fewer than 256 KiB are queued for the connection; it resumes when the queue drains below 64 KiB |
| Stream bodies: restrictions | FUNCTIONAL | `duplex: 'half'` required; not allowed with `GET`/`HEAD`, `keepalive` or `no-cors`; a 307/308 redirect fails the fetch (the body cannot be replayed) |
| Streaming response bodies (`ReadableStream`, default reader, async iteration, `tee`) | FUNCTIONAL | `wave_g::response_body_streams_chunks_before_the_transfer_completes` (the first chunk reaches script while the server is still sending) |
| Backpressure from script to the network | FUNCTIONAL | `wave_g2::response_backpressure_pauses_the_network_until_script_reads` (the server writes less than half of a 16 MiB body while script does not read); each fetch has a 1 MiB `FlowControl` window, released as script dequeues chunks. A paused fetch keeps its scheduler worker (bounded by the pool size). |
| Cancelling the body stream aborts the transfer | FUNCTIONAL | `wave_g::cancelling_the_body_stream_aborts_the_transfer` (server-observed abort) |
| `AbortController` / `AbortSignal` (`abort`, `timeout`, `any`, `reason`, `throwIfAborted`) | FUNCTIONAL | `wave_g::abort_before_and_during_a_request_cancels_the_network_load`, `wave_g::abort_signal_timeout_rejects_with_timeout_error` |
| Abort → `ResourceLoader::cancel(RequestId)` → network cancellation | FUNCTIONAL | same (the network log shows `Cancelled`) |
| Navigation and tab close cancel in-flight fetches | FUNCTIONAL | `wave_g::navigation_cancels_in_flight_fetches`; tab close drops the tab's loader context (`wave_f1::closing_a_tab_cancels_only_its_requests`) |
| Cache modes → `CacheMode` (`default`, `no-store`, `reload`, `no-cache`, `force-cache`, `only-if-cached`) | FUNCTIONAL | `wave_g::cache_modes_use_the_profile_http_cache`; semantics in `wave_f` |
| Credentials modes → `CredentialsMode`, through the profile `CookieService` | FUNCTIONAL | `wave_g::credentials_modes_use_the_profile_cookie_jar_and_hide_set_cookie`, `wave_g::same_origin_credentials_are_port_aware_for_cross_origin_no_cors` |
| `Set-Cookie` never visible to script | FUNCTIONAL | same |
| Forbidden request headers dropped (`Cookie`, `Host`, `Origin`, `Sec-*`, `Proxy-*`, method overrides for forbidden methods, …) | FUNCTIONAL | `wave_g::post_sends_body_and_custom_headers_but_never_forbidden_ones`, `fetch::tests::forbidden_headers_are_dropped_and_invalid_ones_rejected` |
| `Origin` header on non-GET/HEAD requests (`null` for opaque-origin documents) | FUNCTIONAL | same |
| Redirect modes `follow`, `error`, `manual` (`opaqueredirect`) | FUNCTIONAL | `wave_g::redirect_error_and_manual_modes` |
| `data:` URLs (percent-decoded, base64, MIME parameters kept) | FUNCTIONAL | `wave_g::data_urls_and_unsupported_schemes`, `axiom-loader` unit test |
| Only `http:`, `https:` and `data:` are fetchable | FUNCTIONAL | same, `fetch::tests::only_http_and_data_schemes_are_fetchable` |
| Mixed content (HTTPS page → insecure URL) blocked; loopback treated as trustworthy | FUNCTIONAL | `fetch::tests::mixed_content_is_blocked_but_loopback_is_trustworthy` |
| Network errors surface only as `TypeError("Failed to fetch")`; details go to the log | FUNCTIONAL | `wave_g::http_errors_resolve_but_network_errors_reject_without_details` |
| Promise reactions run as microtasks before timers | FUNCTIONAL | `wave_g::promise_reactions_run_as_microtasks_before_timers` |
| Fetch rows on `axiom://network` (no query strings) | FUNCTIONAL | `wave_g::fetches_appear_on_axiom_network` |
| `priority` (`high` / `low` / `auto`) → scheduler priority | FUNCTIONAL | mapped in `plan_fetch` |
| `referrer: ''` → no referrer; `referrerPolicy` | FUNCTIONAL | mapped onto the existing referrer engine |
| Custom `referrer` URL | FUNCTIONAL | `wave_g2::custom_referrer_urls_apply_only_when_same_origin`, `fetch::tests::referrer_urls_are_used_only_when_same_origin`. A same-origin URL becomes the referrer (still subject to `referrerPolicy`); a cross-origin one falls back to `about:client`. |
| `keepalive` | FUNCTIONAL | `wave_g2::keepalive_fetch_survives_navigation`, `wave_g2::keepalive_fetch_survives_closing_its_tab`, `wave_g2::keepalive_body_quota_is_enforced`. Keepalive requests run on a separate loader context that navigation does not cancel; on tab close the tab manager adopts them until they finish. The request bodies in flight share a 64 KiB quota per document. They keep the 60 s request timeout and have no flow control. |
| `integrity` (SRI: `sha256`, `sha384`, `sha512`) | FUNCTIONAL | `wave_g2::integrity_is_verified_before_the_response_is_revealed`, `sri::tests::*`. The response is held until the whole body has arrived and been hashed; a mismatch rejects with `TypeError` and script never sees the response. Only the strongest algorithm listed is compared. Opaque responses fail. Metadata with no valid token means no check. |
| Unhandled promise rejection reporting | FUNCTIONAL | `wave_g2::unhandled_rejections_fire_events_and_late_handlers_fire_rejectionhandled`. After each microtask checkpoint, `unhandledrejection` (cancelable) fires on `window`; if nothing cancels it, the console logs `Uncaught (in promise) …`. A handler attached later fires `rejectionhandled`. |
| `fetch()` in the headless CLI `Engine` | FUNCTIONAL | `wave_g2::headless_engine_runs_fetch_and_settles_before_the_frame`, `wave_g2::headless_engine_reports_a_settle_timeout`. `Engine::navigate` runs a headless `BrowsingContext` and waits (up to `NavigateOptions::settle_timeout`, 10 s by default) until no fetches, subresources, keepalive requests or timers are pending, then renders. |

## Cross-origin behaviour (CORS, Phase 3 Waves F and H)

The CORS protocol lives in the network service (`axiom-net/src/cors.rs`, driven from
`NetworkService::run`), so `fetch()` and subresources share one implementation, one
preflight cache and one set of tests. `filter_response` repeats the response check on the
UI thread before script sees anything.

| Request | Result |
|---------|--------|
| Same-origin, any mode | Normal `basic` response |
| Cross-origin, `cors` mode (the default), simple request (`GET` / `HEAD` / `POST`, CORS-safelisted headers, no streamed body) | Sent with `Origin`, no preflight. The response reaches script only if it has exactly one `Access-Control-Allow-Origin` that is `*` or the request's origin; with `credentials: 'include'` it must be the exact origin and `Access-Control-Allow-Credentials: true` is required. Script then sees a `cors` response exposing only the safelisted response headers (`Cache-Control`, `Content-Language`, `Content-Length`, `Content-Type`, `Expires`, `Last-Modified`, `Pragma`) plus those listed in `Access-Control-Expose-Headers` (`*` exposes all when credentials are not included). A failed check rejects with `TypeError("Failed to fetch")` and the body is discarded in Rust (`cors::tests::cors_check_matches_origin_and_credentials`, `fetch::tests::cors_check_requires_matching_allow_origin_and_filters_headers`, `wave_g::cross_origin_cors_with_allow_origin_exposes_only_safelisted_and_listed_headers`, `cors::simple_and_same_origin_requests_are_not_preflighted`) |
| Cross-origin, `cors` mode, request that needs a preflight (other methods, non-safelisted headers, streamed body) | An `OPTIONS` preflight is sent first, through the same `NetworkService` (its own row on `axiom://network`, initiator `cors-preflight`). It carries `Origin`, `Access-Control-Request-Method` and `Access-Control-Request-Headers` (lowercase, sorted), never cookies, the request body or the script's headers, and it does not follow redirects. It must return 2xx, pass the CORS check and list the method (unless safelisted) and every non-safelisted header in `Access-Control-Allow-Methods` / `-Headers`; `*` counts only without credentials and never covers `Authorization`. If it fails, the fetch rejects with `TypeError("Failed to fetch")` and **no byte of the actual request is sent** (`cors::a_failing_preflight_sends_no_bytes_of_the_actual_request`, `wave_g::cross_origin_cors_without_allow_origin_rejects_and_no_cors_is_opaque`, `wave_h::fetch_preflights_non_simple_requests_on_the_profile_network_service`) |
| Preflight cache | Per profile, memory only (private profiles included), keyed by origin, URL and credentials mode, one entry per allowed method or header. `Access-Control-Max-Age` defaults to 5 s and is capped at 2 hours; `0` stores nothing. Cleared by `Browser::clear_cache` and when the profile's network service shuts down (`cors::preflight_precedes_a_non_simple_request_and_is_cached_per_profile`, `cors::max_age_zero_is_not_cached`) |
| Cross-origin `cors` request from an opaque-origin document (`file:`, `about:`) | Sent with `Origin: null` and credentials `omit`; the server must allow `null` (`fetch::tests::opaque_origin_documents_send_origin_null_and_no_cookies`) |
| Cross-origin, `same-origin` mode | `TypeError` before sending |
| Cross-origin, `no-cors` mode | Sent with `GET` / `HEAD` / `POST` and CORS-safelisted headers only. Returns an **opaque** response: status 0, no headers, empty body. The body is never forwarded to script. |
| `cors` request redirected | Every cross-origin response, including each redirect, must pass the CORS check before the redirect is followed. After a hop to another origin from a URL that was already cross-origin to the requester, the origin is tainted: later hops send `Origin: null` and the final response must allow `null` (`cors::cross_origin_redirects_are_checked_per_hop_and_taint_the_origin`, `wave_h::fetch_follows_cross_origin_redirects_with_a_tainted_origin`, `fetch::tests::after_a_tainting_redirect_the_cors_check_expects_origin_null`) |
| `same-origin` request redirected cross-origin | The network service refuses to follow the redirect (`NetworkError::Blocked`); script gets `TypeError` (`cors::same_origin_mode_still_refuses_cross_origin_redirects`) |
| `no-cors` request redirected cross-origin | Followed; the response becomes opaque |
| Credentials `same-origin` | Cookies are sent only to hops that are same-origin with the requester; a `cors` request sends none after its first cross-origin hop |

Subresource CORS (the `crossorigin` attribute, module scripts, fonts) is described in
[`RESOURCE_LOADING.md`](RESOURCE_LOADING.md).

The document's Content Security Policy (Phase 3 Wave I) checks `fetch()` against
`connect-src` before anything is sent, applies `upgrade-insecure-requests` to the URL, and
checks every redirect hop in the network service. A refusal rejects with
`TypeError("Failed to fetch")` and fires `securitypolicyviolation`
(`wave_i::connect_src_gates_fetch_including_redirects`; see [`CSP.md`](CSP.md)).

## Security notes

- TLS verification, the cookie service, the cache and the scheduler are the same ones used
  for navigation; fetch cannot opt out of any of them.
- Response headers are filtered in Rust (`filter_response`) before script sees them.
- Opaque responses are filtered in Rust too: their bytes never enter the JS heap.
- Policy rejections at `fetch()` time carry descriptive `TypeError` messages, because script
  already knows the URL it asked for. Network failures do not: server and transport
  details stay in the log.
