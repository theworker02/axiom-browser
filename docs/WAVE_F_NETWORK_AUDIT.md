# Wave F network audit (Networking 2.0 re-audit, 2026-09-28)

This audit was done against the code, not the earlier documents. Where the two disagreed, the code wins and the disagreement is listed at the end.

## Summary

Axiom already had a complete, layered network stack when this re-audit started. It was built in Wave F / F.1, Phase 3 Wave E (cache, typed errors, downloads), Wave G (fetch) and Phase 3 Wave G (document loading). **No second network system was created in this wave.** The gaps were above the network layer:

| Gap found | Where | Fixed in this wave |
|-----------|-------|--------------------|
| Omnibox classification was "has a dot and a letter" | `axiom-browser/src/classifier.rs` | Rewritten with PSL-validated suffixes, IPv4/IPv6, ports, IDN, `?` forced search, quoted phrases |
| One hard-coded search template (DuckDuckGo), no provider ids | `axiom-browser/src/search.rs`, `settings_repo.rs` | `SearchProviderService` with Google, Bing, DuckDuckGo and the Axiom Search placeholder; the choice persists per profile |
| URL hosts not canonicalized (case, IDN) | `axiom-url` | Hosts are lowercased and IDNA-encoded (punycode); hosts containing forbidden characters are rejected |
| The desktop UI thread blocked on every navigation | `Browser::navigate*`, `BrowsingContext::navigate/back/forward/reload` | Background navigation mode: the desktop never waits on the network |
| Link clicks did not update the omnibox or browser history | `BrowsingContext::handle_click` → `navigate` | The engine publishes navigation lifecycle events; the browser consumes them |
| A failed navigation replaced the previous page's history entry | `BrowsingContext::fail_navigation` | A failed navigation adds an entry for the URL that was tried (back still returns to the previous page) |
| No `Accept-Language` header; User-Agent not configurable | `axiom-net/src/service.rs` | Added `accept_language` / `user_agent` to `NetworkServiceConfig` |
| Error pages printed raw library error text | `trusted_network_error_html` | Error pages show the summary and a stable error code only |
| Cross-origin `cors` fetches always failed | `axiom-engine/src/fetch.rs` | Basic CORS for simple requests; requests that need a preflight still fail before sending |

## Libraries

| Concern | Library | Notes |
|---------|---------|-------|
| HTTP/1.1, HTTP/2 | `reqwest` 0.12 (`rustls-tls`, `http2`, `stream`) on a transport-owned tokio runtime | Only `axiom-net` depends on an HTTP client |
| TLS | `rustls` 0.23 with `webpki-roots` (Mozilla CA bundle) | Verification is always on; `accept_invalid_certs` is never called. Extra PEM anchors can be added (tests only). |
| Certificate display | `x509-parser` | Display only; validation stays in rustls |
| Content decoding | `flate2`, `brotli-decompressor` | Axiom decodes itself (reqwest's automatic decompression is off) so byte counts are honest |
| DNS | `std::net::ToSocketAddrs` (system resolver) behind the `DnsResolver` trait | Runs on the runtime's blocking pool; injectable for tests |
| PSL | `psl` (Mozilla PSL) | Cookies (Wave D.1) and, since this wave, omnibox suffix validation |
| IDNA | `idna` (already in the dependency graph through reqwest's `url`) | New direct dependency of `axiom-url` |
| `ureq` | Listed in the workspace `Cargo.toml`, **used by no crate** | AGENTS.md still names it; the real transport is reqwest |

## Architecture as found

```text
Browser (one per profile)
  ├─ NetworkService   cookies (CookieProvider → ProfileCookieJar → CookieService), HttpCache
  │                   (disk for normal profiles, memory for private), redirects, DNS, policy,
  │                   metrics, activity stream, log
  ├─ RequestScheduler fixed worker pool (12), priority + aging, bounded channels, flow control
  ├─ DownloadManager  on the same scheduler
  └─ Tab → BrowsingContext → ResourceLoader (one cancellation context per tab)
             ├─ navigation: begin_navigation → ResourceRequest::document (streamed)
             ├─ document: DocumentLoad (streaming parser, registry, generation guard)
             ├─ subresources: request_resource (ContentPolicy, mixed content) → loader
             └─ fetch(): plan_fetch → FetchQueue → loader
```

## Entry points

| Entry | Code | Blocking before this wave |
|-------|------|---------------------------|
| Omnibox Enter | `Browser::submit_omnibox` → `Browser::navigate` → classifier → `navigate_resolved_with_transition` → `Tab::navigate` → `BrowsingContext::navigate` | Yes (waited until `DOMContentLoaded`) |
| Link click | `BrowsingContext::handle_click` → `navigate` | Yes, and the browser never learned about it |
| Back / forward | `BrowsingContext::back/forward` → `apply_entry` → wait | Yes |
| Reload | `BrowsingContext::reload` (`CacheMode::NoCache`) | Yes |
| Async form | `BrowsingContext::start_navigation` (Wave E) | No, but only tests used it |
| Headless CLI | `Engine::navigate` (own standalone loader) | Yes, by design |

## Request flow

1. `ResourceRequest` (type, priority, initiator, cache mode, credentials mode, request mode, redirect mode, referrer, limits) → `ResourceLoader::start` → `RequestId`.
2. `RequestScheduler` queues by priority with aging and dispatches to a worker.
3. `NetworkService::fetch`: scheme check (`http`/`https` only), `HostBlocklist` policies, cache lookup (fresh hit, stale → conditional request, or miss), header preparation (User-Agent, Accept per type, Accept-Encoding, Referer by policy, `Cookie` recomputed from the cookie authority — any caller-supplied `Cookie` is dropped).
4. `ReqwestTransport` sends; the injected DNS resolver is used for every connection; a connector layer counts opened connections.
5. Redirects are followed by the service, not by reqwest (`redirect::Policy::none()`): per-hop cookie store, method/body rewriting (301/302 POST→GET, 303 → GET except HEAD, 307/308 preserve), `Authorization` stripped across origins, loop vs. limit classification, `UnsupportedScheme` for non-HTTP(S) targets, policy re-checked per hop.

## Response flow

1. Headers: status, header size limit, `Set-Cookie` → `CookieProvider::store_set_cookies` (PSL, domain, path, Secure, HttpOnly, SameSite on the request side, Expires/Max-Age), TLS facts (`TlsInfo`, certificate metadata), MIME sniff-free `Content-Type` parse (`Mime`), download detection (`Content-Disposition: attachment`, non-displayable types).
2. Body: `ResponseBodyReader` (own gzip/deflate/br decoding, transferred vs. decoded counts, `max_body_bytes`), cache tee (bounded), scheduler `Data` events with flow control.
3. Loader events (`Response`, `Chunk`, `Completed`, `Failed`) → `BrowsingContext::route_event` → pending navigation, script fetch, or the owning document (stale documents' events dropped and counted).
4. Documents: `TextDecoder` (BOM, transport charset, `<meta>` prescan), streaming `HtmlParser`, `text/*` non-HTML wrapped in `<pre>`; anything else is a download, never parsed as HTML.

## Security boundaries

- `CookieService` is the only cookie store. reqwest's cookie store is not enabled.
- Private profiles: memory-only cookies, cache and downloads list; cleared on close.
- The HTTP cache never stores `Set-Cookie`.
- The security indicator comes from a verified TLS response, never the URL scheme.
- Error pages are generated locally from a typed `NetworkError`; server bytes are never shown.
- Internal `axiom://` pages are resolved by the browser and never fetched; web content cannot load them as subresources.
- Script `fetch()` is re-validated in Rust (`plan_fetch`), forbidden headers are dropped, `Set-Cookie` is filtered.

## Protocol support

| Item | Status found |
|------|--------------|
| HTTP/1.1 keep-alive | FUNCTIONAL (server-verified connection count) |
| HTTP/2 via ALPN | FUNCTIONAL (negotiated protocol read from the response) |
| HTTP/3 / QUIC | DEFERRED (`HttpProtocol::Http3` reserved); the `NetworkService` → transport boundary means a new transport does not touch navigation |
| TLS verification, SNI | FUNCTIONAL (rustls; SNI is sent by rustls for DNS names) |
| gzip / deflate / br | FUNCTIONAL |
| Timeouts: connect, request, idle read | FUNCTIONAL (connect timeout not covered by CI) |
| Cancellation mid-transfer | FUNCTIONAL |
| RFC 9111 cache (memory + disk) | FUNCTIONAL |
| Redirect engine | FUNCTIONAL |
| Download boundary | FUNCTIONAL (`DownloadCandidate` → `DownloadManager`) |

## Known limitations (carried, not introduced)

- DNS, connect and TLS phase timings are not observable through reqwest (reported as `None`).
- TLS version and cipher suite are not exposed by reqwest.
- Connection reuse counts are not observable.
- Store-side SameSite context for `Set-Cookie` is PARTIAL.
- CSP has a hook (`ContentPolicy`) but no parser.
- Subresource CORS (`crossorigin` attribute) is not implemented.

## Documentation vs. code disagreements found

| Document said | Code does |
|---------------|-----------|
| AGENTS.md: HTTP/TLS is `ureq` + rustls | reqwest + rustls; `ureq` is an unused workspace entry |
| NETWORKING.md: the smoke test uses the "system trust store" | reqwest's `rustls-tls` feature uses the bundled `webpki-roots` (Mozilla CA list) |
| PHASE3_PROGRESS.md: "Navigation IDs … the desktop browser still waits synchronously" | Accurate at audit time; fixed in this wave |
