# Axiom Profiles

**Status:** Wave C — **FUNCTIONAL** foundations (persistent + private).

## Ownership model

Every `BrowserWindow` / `Tab` / `BrowsingContext` is owned by a **Profile**.

Subsystems must ask:

> Which profile-owned repository owns this data?

Not:

> Where do I write this file?

| Concept | Role |
|---------|------|
| `ProfileId` | Stable UUID |
| `ProfileKind` | `Persistent` \| `Private` |
| `ProfileMetadata` | Name, id, kind, created_at |
| `ProfilePaths` | Centralized directory layout |
| `ProfileManager` | Open/create named profiles under a user-data root |
| `BrowserDataStore` | SQLite (or in-memory) + repositories |

## Directory layout (persistent)

```text
profile/
├── profile.json          # format marker
├── profile_id.json       # stable id
├── profile_meta.json     # metadata
├── profile.lock          # exclusive process lock
├── browser.sqlite        # structured metadata (WAL)
├── history/              # reserved
├── bookmarks/            # reserved
├── sessions/current.json # atomic JSON sidecar
├── settings/
├── site-data/            # Wave E+
├── cache/                # Wave F+
├── downloads/            # Wave U
├── state/clean_shutdown
└── quarantine/           # corrupt DB copies
```

Tests **must** pass an explicit temp root. They never use the developer’s real home profile.

## Private profiles

| Behavior | Persistent | Private |
|----------|------------|---------|
| SQLite on disk | Yes | No (in-memory only) |
| History after close | Yes | Discarded |
| Session restore | Optional (settings) | Never |
| Omnibox history | From this profile | Session-local only |
| Profile lock file | Yes | N/A |

Private navigation is still recorded in the in-memory DB for the lifetime of the private session so omnibox/search within that session works — it is **never** written to a persistent profile root.

## Locking

`ProfileLock` uses an exclusive `fs2` lock on `profile.lock`.

If another Axiom process holds the profile:

```text
StoreError::Lock(AlreadyLocked)
```

No silent multi-writer corruption.

## Classification

| Component | Status |
|-----------|--------|
| Profile identity / paths | FUNCTIONAL |
| SQLite store + schema v1 | FUNCTIONAL |
| Private isolation | FUNCTIONAL |
| Multi-profile isolation | FUNCTIONAL |
| Profile UI switcher | DEFERRED |
| Site-data / cookies dirs | DEFERRED (layout only) |
