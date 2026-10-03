---
name: axiom-setup
description: Inspect, build, test, and prepare a local Axiom desktop-browser installation or development profile after the user explicitly asks for setup. Use for this repository only; do not use for general Rust projects or ordinary web browsing.
---

# Axiom local setup

Use this skill when a user asks an agent to set up, build, verify, or prepare the Axiom browser from this repository. It is a repository-development skill, not an Axiom browser feature: it does not embed AI into Axiom, collect telemetry, or upload private browsing data.

## Safety and consent

1. Read `AGENTS.md`, `README.md`, `docs/PRIVACY.md`, `docs/RELEASES/`, and the current `docs/PHASE3_PROGRESS.md` before changing anything.
2. Inspect `git status --short` and preserve unrelated local work.
3. Explain the proposed setup steps in one compact update.
4. Require the user's explicit approval before any action that downloads dependencies, installs software, copies this skill into an agent-global skill directory, changes a profile, writes outside the repository, publishes, or releases software.
5. Never add AI, telemetry, analytics, tracking pixels, an account requirement, or remote synchronization as part of setup.
6. Never use a real browsing profile for tests. Use an explicitly supplied temporary profile directory.

## Repository audit

Run the smallest relevant checks first:

```powershell
cargo check -p axiom-desktop
cargo test -p axiom-browser settings_repo::tests
cargo test -p axiom-desktop
```

Run broader workspace checks only when needed and report existing failures separately. A failed quality gate must never be presented as passing.

## Local development setup

1. Confirm the user wants a development build or a packaged release.
2. Build the native desktop application:

```powershell
cargo build --release -p axiom-desktop
```

3. Launch it without overriding its profile location:

```powershell
.\target\release\axiom.exe
```

The desktop host chooses a user-scoped profile directory. On Windows this is `%LOCALAPPDATA%\Axiom\Profile`; it is intentionally separate from the repository and from test profiles.

4. For a safe render smoke test, use the headless path and an included demo:

```powershell
.\target\release\axiom.exe --headless demos\click-works.html
```

5. Tell the user where the application and user-owned profile are located. Do not inspect or upload their browsing data.

## Optional skill installation

This file is discoverable from `.agents/skills/axiom-setup/`. If the user explicitly requests installation into an agent's global skill directory, first determine that agent's documented skill location, show the exact source and destination, and request confirmation before copying it. Never claim an agent has uploaded the skill to itself unless the copy actually completed and was verified.

## Release setup

For a public release, follow `docs/RELEASES/` and `tools/release/build-windows.ps1`. Before pushing or publishing, verify the version, changelog, license, README, artifact contents, and absence of secrets. Confirm build/test results truthfully and obtain explicit release authorization if it was not already granted.

## Completion report

Report:

- build and test commands actually run, including failures;
- executable and profile locations;
- whether the setup is development-only or packaged;
- settings/privacy defaults applied;
- any remaining compatibility or platform limitations.
