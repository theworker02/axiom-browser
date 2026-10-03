# Wave E Implementation Plan — Web Storage

**Status:** PLAN  
**After:** Wave D.1 PSL (complete)  
**Stop:** Wave E boundary — do not start Wave F/G

## Architecture

```text
Profile.store
     │
     ▼
StorageService ──► StorageRepository (localStorage, SQLite / memory)
     │
     └── session map (sessionStorage, never durable)

BrowsingContext / JsHost
     │
     ▼
StorageJar bridge (origin-scoped)
```

## Scope

| API | Persistence |
|-----|-------------|
| `localStorage` | Profile store; private = memory-only |
| `sessionStorage` | Per browsing context, process lifetime only |

## Non-goals

- IndexedDB, Cache API, File System Access
- StorageEvent cross-window fanout (FOUNDATION / optional minimal)
- Wave F networking redesign
