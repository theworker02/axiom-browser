# Acquiring Axiom

This document explains how to obtain, build, run and verify the Axiom browser/engine. The 1.3.2
release train produces a Windows x86_64 portable ZIP and a per-user NSIS installer in addition to
the source distribution. Until Axiom has an Authenticode certificate, release executables are
unsigned and must be verified against the GitHub release checksum.

## Prerequisites

| Requirement | Notes |
|-------------|-------|
| Rust toolchain | Stable `rustc` **1.80+** recommended (`rustup`) |
| Platform | Windows, macOS, or Linux |
| Network | First build downloads crates.io dependencies |
| Disk | Several hundred MB for `target/` |

Optional:

- `git`
- A C toolchain only if you disable `rusqlite`’s `bundled` feature (default is bundled SQLite — no system SQLite required)

## Clone

```bash
git clone https://github.com/theworker02/axiom.git
cd axiom
```

## Windows release package

Download the `Axiom-Setup-1.3.2.exe` installer (when available) or the portable
`Axiom-1.3.2-windows-x86_64.zip` from the GitHub release. The installer writes only to
`%LOCALAPPDATA%\Programs\Axiom`, creates Start Menu/Desktop shortcuts, and can be removed from
Windows Apps or `Uninstall.exe`. It does not install a service, browser extension, or updater.

Build the reproducible package locally:

```powershell
.\tools\release\build-windows.ps1
# With NSIS installed:
.\tools\release\build-windows.ps1 -Installer
```

If you use SSH:

```bash
git clone git@github.com:theworker02/axiom.git
cd axiom
```

## Build

```bash
# Debug (faster compile)
cargo build -p axiom-desktop

# Release (faster runtime)
cargo build -p axiom-desktop --release
```

Binary location (debug):

- Windows: `target/debug/axiom.exe` (package `axiom-desktop`, binary name `axiom`)
- Unix: `target/debug/axiom`

## Run

```bash
# Trusted new-tab page
cargo run -p axiom-desktop

# Remote HTTPS document
cargo run -p axiom-desktop -- https://example.com

# Local demo
cargo run -p axiom-desktop -- demos/click-works.html

# Private browsing (no durable history)
cargo run -p axiom-desktop -- --private

# Headless raster → axiom-frame.ppm
cargo run -p axiom-desktop -- --headless https://example.com
```

Default persistent profile directory (desktop host):

```text
target/axiom-profile/
```

Override for experiments by pointing code/tests at a temp directory — **never** point automated tests at your real user profile.

## Verify the tree

```bash
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

## What you are acquiring

Axiom is an **independent** browser engine:

- Owns HTML → DOM → CSS → style → layout → paint
- Does **not** embed Chromium, WebKit, Gecko, Electron, CEF, or WebView for page rendering
- May use libraries for TLS, HTTP, fonts, windowing, GPU APIs, and JavaScript (Boa)

See `docs/REALITY.md`, `docs/PROFILES.md`, and `docs/PERSISTENCE.md` for capability honesty.

## Support / funding

If you find Axiom useful:

- GitHub: [@theworker02](https://github.com/theworker02)
- thanks.dev: [u/gh/theworker02](https://thanks.dev/u/gh/theworker02)

## License

MIT — see `LICENSE` (or package metadata) when present.
