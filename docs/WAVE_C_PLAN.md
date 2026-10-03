# Wave C Implementation Plan

Date: 2026-09-26  
Baseline: Wave A+B green (`cargo test/fmt/clippy` ok)

## Approach

1. **SQLite** (`rusqlite` bundled) as structured metadata store per profile.
2. **ProfilePaths** centralizes directory layout; tests use temp roots only.
3. **Repositories** behind `BrowserDataStore`: history, bookmarks, settings, sessions.
4. **Private** profiles use in-memory store; never write durable history/sessions.
5. Wire navigation → history; omnibox → History/Bookmark/OpenTab sources; internal pages.
6. Docs: PROFILES.md, PERSISTENCE.md, PHASE3_PROGRESS.md, README/CHANGELOG/acquisition.

## Out of scope (stop after Wave C)

Cookies, web storage, networking Wave F — deferred.
