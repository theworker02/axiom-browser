# Axiom Cookies

**Status:** Wave D + D.1 — **FUNCTIONAL** cookie engine with production Public Suffix List.

## Architecture

```text
Profile (BrowserDataStore)
        │
        ▼
   CookieService  ← Clock, PublicSuffixProvider, CookiePolicy, limits
        │
   CookieRepository  (SQLite / in-memory private)
        │
   ┌────┴────┐
Network     document.cookie
(Cookie /   (JsHost → CookieJar)
 Set-Cookie)
```

One coherent jar per profile. Networking and JavaScript never open SQLite.

| Piece | Location |
|-------|----------|
| Domain model | `cookie_types.rs` |
| Set-Cookie + cookie-date | `cookie_parse.rs` |
| PSL abstraction | `cookie_psl.rs` (`PslPublicSuffixProvider` default) |
| Repository | `cookie_repo.rs` |
| Service / policy / limits | `cookie_service.rs` |
| Engine bridge | `CookieJar` + `ProfileCookieJar` |
| Trusted UI | `axiom://cookies` |

## Persistence

- Schema **v2** adds `cookies` table + indexes (`domain`, `domain+path`, `expires`, `last_access`).
- **Persistent** profiles: durable SQLite via the profile store.
- **Private** profiles: same schema in memory only — destroyed with the profile.
- **Session cookies** (`persistent=0`, no `expires_at_ms`): usable during the browser process; **purged on persistent profile open** (Axiom browser-session semantics). They are not restored after restart.
- Failed migrations never silently recreate the database.

## Network integration

Before GET, `BrowsingContext` asks `CookieJar` for a `Cookie` header.  
After response, each `Set-Cookie` value is passed separately into `CookieService` (not comma-merged as ordinary headers).

## document.cookie

Getter: script-visible cookies for the document URL (no HttpOnly, no metadata).  
Setter: Set-Cookie-like assignment through `CookieService` with `CookieSource::Script`.  
Script cannot set or overwrite HttpOnly cookies.

## Domain / Path / Secure / HttpOnly / SameSite

| Rule | Behavior |
|------|----------|
| Host-only | No `Domain` → host must equal request host |
| Domain | Must domain-match request host; **public/private suffix reject via PSL** |
| Path | RFC-oriented default-path + path-match |
| Secure | Stored only from HTTPS; sent only on HTTPS |
| HttpOnly | HTTP yes; `document.cookie` no |
| SameSite | Strict / Lax / None with explicit `CookieAccessContext` (top-level site, navigation, method) |
| SameSite=None | Requires Secure |

## Prefixes

- `__Secure-`: requires Secure + secure context  
- `__Host-`: requires Secure + host-only + `Path=/`

## Limits

Defaults: name ≤1024, value ≤4096, ≤180 per domain, ≤3000 total.  
Eviction: oldest by creation (per-domain) / least-recently-accessed (global). Deterministic.

## Policy

`CookiePolicy::{AllowAll, BlockAll}`. Third-party blocking / partitioning is **DEFERRED** (needs full site-context product work).  
`CookieService` remains the sole cookie policy authority.

## Site data / DevTools foundation

- `clear_cookies` / `clear_cookies_for_site` on store + Browser  
- `CookieInspectRecord` for trusted UI / future DevTools (values omitted by default)  
- `axiom://cookies` lists metadata only

## Observability

Logs target `axiom_cookies`: accepted / rejected(+reason) / expired / evicted. **Never logs cookie values.**

## Public Suffix List — FUNCTIONAL (Wave D.1)

Default provider: **`PslPublicSuffixProvider`** using the maintained [`psl`](https://crates.io/crates/psl) crate (Mozilla Public Suffix List compiled to native code; ICANN + private sections; crates.io auto-sync).

Trait: `PublicSuffixProvider` — injectable for tests (`HeuristicPublicSuffixProvider` remains available).

Covered behavior (tested):

| Case | Result |
|------|--------|
| `com`, `example.com`, `sub.example.com` | suffix vs eTLD+1 |
| `co.uk`, `example.co.uk`, `sub.example.co.uk` | multi-label ICANN |
| `com.au` / `example.com.au` | multi-label ICANN |
| `github.io` / `pages.github.io` | private-section suffix |
| Punycode / IDN hosts where `psl` accepts UTF-8 | eTLD+1 |
| `localhost`, `*.localhost`, IPv4/IPv6 | rejected as Domain targets; host-only site key |

Setting `Domain=` to a public or private suffix is rejected by `CookieService` (`PublicSuffix`). Host-only cookies (no `Domain`) are unchanged.

## Known deviations / DEFERRED

| Item | Status |
|------|--------|
| Full PSL | **FUNCTIONAL** (Wave D.1) |
| Schemeful-site edge cases | FOUNDATION (Site type + registrable domain) |
| CHIPS / partitioned cookies | DEFERRED |
| Third-party blocking UI | DEFERRED |
| Multiple Set-Cookie via ureq | PARTIAL (best-effort split; prefer discrete headers) |
| Background expiry thread | Not required — lazy + `cleanup_expired` |

## Tests

`cookie_psl` unit tests + `CookieService` PSL rejection tests + `crates/axiom-browser/tests/wave_d.rs`.
