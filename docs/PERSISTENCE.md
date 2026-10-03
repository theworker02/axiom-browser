# Axiom Persistence

**Status:** Wave C+D — **FUNCTIONAL** for history, bookmarks, settings, sessions, cookies.

## Abstraction

```text
Browser / Profile
      │
      ▼
BrowserDataStore
      │
      ├── HistoryRepository
      ├── BookmarkRepository
      ├── SettingsRepository
      ├── SessionRepository
      └── CookieRepository
      ├── StorageRepository   ← Wave E (localStorage)
      │
      ▼  (later waves — same store)
      ├── PermissionRepository
      ├── SiteDataRepository
      └── DownloadRepository
```

Callers receive repository results through `Profile.store` / `CookieService`. UI modules must not open SQLite directly. See `docs/COOKIES.md`.

## Schema

- Table `schema_metadata (id=1, version)`
- Current version: **`SCHEMA_VERSION = 3`**
- Opening a **newer** unsupported version returns a controlled error (data preserved, not deleted).
- Migration failure returns an error **without** recreating/wiping the database.
- Corrupt DB files are **quarantined** under `quarantine/`; startup fails closed rather than wiping user data.

### v1 tables

- `history_entries`, `bookmark_folders` / `bookmarks`, `settings`, `sessions`

### v2 tables (Wave D)

- `cookies` — identity `UNIQUE(name, domain, path)`; flags for secure/http_only/host_only/persistent; `expires_at_ms` NULL = session
- Indexes: `domain`, `(domain, path)`, `expires_at_ms`, `last_access_time_ms`

### v3 tables (Wave E)

- `web_storage` — `PRIMARY KEY (origin, key)`; `value`, `updated_at_ms`
- Index: `origin`

## Cookies (Wave D)

- **Persistent profile:** durable rows; **session cookies purged on profile open**
- **Private profile:** in-memory only; never written to disk
- Clear APIs: `clear_cookies`, `clear_cookies_for_site` (coherent with history/bookmark clears)
- Cookie values must never appear in persistence logs

## Transactions & crash safety

- History/bookmark mutations use SQLite transactions.
- Session checkpoints: SQLite upsert **and** atomic temp→rename JSON sidecar.
- `clean_shutdown` marker distinguishes clean exit vs crash for restore UX.

## History rules

**Recorded on successful navigation** via `Browser` (not ad-hoc UI writes).

**Skipped:**

- `axiom://` internal pages
- `about:` pages
- Failed navigations (`last_error` set)

**Transitions:** `typed`, `link`, `reload`, `redirect`, `other`

Timestamps are **UTC epoch milliseconds**. Display formatting happens at UI boundaries only.

## Settings

Typed `BrowserSettings` with validation/defaults. Unknown newer fields are ignored via serde; schema version is clamped with a warning.

## Session restore

On persistent profile open, if `restore_previous_session` and a checkpoint exists, tabs are reconstructed by **normal navigation** (no DOM/JS heap serialization).

Private sessions are **never** restored after the private profile ends.

Debounced checkpoints (~750ms) on tab/nav changes; forced clean checkpoint on `shutdown_clean()`.

## Async note

Persistence is synchronous but isolated behind the store API. Hot paint paths do not write on every mouse move. A background writer pool is the documented migration path (PARTIAL / future).

## Privacy / logging

Structured logs use targets `axiom_persist` / `axiom_cookies` and avoid dumping secrets or cookie values. Prefer counts and event names.

## Classification

| Area | Status |
|------|--------|
| History CRUD + search | FUNCTIONAL |
| Bookmarks CRUD + folders | FUNCTIONAL |
| Settings | FUNCTIONAL |
| Session checkpoint/restore | FUNCTIONAL |
| Omnibox history/bookmark sources | FUNCTIONAL |
| Cookies | FUNCTIONAL (PSL FUNCTIONAL — see COOKIES.md) |
| localStorage / sessionStorage | FUNCTIONAL (see STORAGE.md) |
| IndexedDB | DEFERRED |
| Async disk pool | EXPERIMENTAL / deferred |
| 100k-row load tests | PARTIAL (indexes present; large corpus bench deferred) |
