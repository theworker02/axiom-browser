# Axiom Web Storage

**Status:** Wave E — **FUNCTIONAL** for `localStorage` / `sessionStorage`.

## Architecture

```text
Profile.store
     │
     ▼
StorageService ──► StorageRepository (localStorage)
     │
SessionStorageMap (per Tab — never durable)

BrowsingContext ── StorageBinder ──► OriginStorageAccess
                                          │
                                     document JS
                              localStorage / sessionStorage
```

## Semantics

| API | Persistence | Scope |
|-----|-------------|-------|
| `localStorage` | Profile SQLite (schema v3 `web_storage`) | Origin |
| `sessionStorage` | Memory only | Per tab / browsing context |

- Private profiles: localStorage uses in-memory SQLite — destroyed with the profile.
- Session storage never enters durable storage.
- Quota: default 5 MiB UTF-8 bytes per origin (key+value); `QuotaExceededError` on overflow.
- Clear APIs: `clear_local_storage`, `clear_local_storage_for_origin` (alongside cookie clears).

## JS

Prelude exposes `localStorage` / `sessionStorage` with `getItem` / `setItem` / `removeItem` / `clear` / `key` / `length`.

## Tests

`crates/axiom-browser/tests/wave_e.rs`

## Deferred

- `StorageEvent` cross-window fanout
- IndexedDB / Cache API
- Exact UTF-16 quota accounting
