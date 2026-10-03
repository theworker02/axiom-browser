# Contributing to Axiom

Thanks for considering a contribution. Axiom is a from-scratch browser engine; correctness, security, and honest capability reporting matter more than feature checklists.

## Before you start

1. Read `AGENTS.md` and `docs/REALITY.md`.
2. Read the active phase docs: `docs/PHASE3_AUDIT.md`, `docs/PHASE3_PROGRESS.md`.
3. Prefer small, tested crates over monoliths.
4. Do **not** embed Chromium/WebKit/Gecko/WebView as the renderer.

## Development loop

```bash
cargo fmt
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

When fixing layout/render bugs, add a fixture under `tests/` when applicable.

## Persistence / profiles

- All durable browser data goes through **profile-owned repositories** (`docs/PERSISTENCE.md`).
- Tests must use temporary profile roots (`tempfile` / `target/…`), never the user’s real profile.
- Private browsing must not leak into persistent history/sessions.

## Pull requests

- Keep PRs focused on one wave/subsystem when possible.
- Include tests that cross subsystem boundaries for claimed features.
- Do not mark stubs as complete in docs.

## Code of conduct

Be respectful. This is a long-horizon systems project; thoughtful review is expected.
