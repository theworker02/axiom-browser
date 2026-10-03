# Wave D Implementation Plan — Production Cookie Engine

**Status:** PLAN (pre-coding)  
**Boundary:** Stop after Wave D. Do not start Wave E (Web Storage).

## Pre-wave audit summary

| Area | Finding |
|------|---------|
| Schema | `SCHEMA_VERSION = 1` — no cookies table |
| Store | History / bookmarks / settings / session only |
| Net | `HttpClient::get` — no Cookie / Set-Cookie |
| Engine load | `load_document_bytes` bare GET |
| JS | `document` has no `cookie` getter/setter |
| Origin | Scheme+host+port exists; site concepts needed for SameSite |
| Profiles | Persistent SQLite + private in-memory — extend, don’t fork |

Baseline gates (test / fmt / clippy) were green at Wave C stop.

## Architecture

```text
Profile.store (BrowserDataStore)
        │
        ▼
   CookieService  ←── Clock, PublicSuffixProvider, CookiePolicy
        │
   CookieRepository (SQL or same in-memory conn)
        │
   ┌────┴────┐
Network     document.cookie
(Cookie:)   (JsHost bridge)
(Set-Cookie)
```

One jar per profile. Networking and JS never touch SQLite.

## Crates / files

| Piece | Location |
|-------|----------|
| Domain model, parse, date, PSL stub, service, policy, limits | `axiom-browser` cookie modules |
| Schema v1→v2 cookies table + indexes | `schema.rs` |
| Repository | `cookie_repo.rs` |
| Store APIs + clear | `store.rs` / `browser.rs` |
| HttpClient Cookie + Set-Cookie list | `axiom-net` |
| `CookieJarBridge` + load path | `axiom-engine` |
| `document.cookie` natives | `axiom-js` + `DocumentJsHost` |
| `axiom://cookies` | `internal.rs` + browser refresh |
| Tests | `tests/wave_d.rs` + unit tests in modules |
| Docs | `COOKIES.md`, update `PHASE3_PROGRESS` / `PERSISTENCE` |

## Migration

- Fresh DB → apply v1 then v2 (or combined init to `SCHEMA_VERSION=2`)
- Existing v1 → `migrate_v1_to_v2` adds `cookies` + indexes, bumps version
- Newer unsupported → error, no wipe
- Failed migrate → quarantine / fail closed (existing store behavior)

## Semantics (Wave D scope)

| Feature | Behavior |
|---------|----------|
| Identity | name + domain + path |
| Expires / Max-Age | Max-Age wins; ≤0 deletes |
| Domain / host-only | RFC-oriented; PSL via abstraction (PARTIAL) |
| Path | default-path + prefix match |
| Secure / HttpOnly | Central in CookieService |
| SameSite | Strict / Lax / None + `CookieAccessContext` |
| Prefixes | `__Secure-` / `__Host-` |
| Session vs persistent | Session not restored across browser restart (documented) |
| Private | Memory-only; destroy with profile |
| Limits | Size / per-domain / total with deterministic eviction |
| Policy | `AllowAll` / `BlockAll` (third-party DEFERRED) |

## Integration order

1. Model + parser + date + PSL trait  
2. Schema v2 + CookieRepository  
3. CookieService + policy + clock  
4. Net request/response wiring via bridge  
5. `document.cookie`  
6. Site-data clear + `axiom://cookies`  
7. E2E / matrix tests + docs + progress stop  

## Explicit non-goals (Wave D)

- Full Public Suffix List ship (abstraction + documented PARTIAL)
- Partitioned cookies / CHIPS
- Third-party blocking UI
- Wave E localStorage / IndexedDB
- Wave F/G networking/fetch redesign
