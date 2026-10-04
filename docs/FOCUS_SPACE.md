# Focus Space

`axiom://focus` is Axiom's profile-local workspace overview. It is designed as an
alternative to burying browser state in an overflow menu: the native Orbit Rail has
a visible launcher, and `Ctrl+Shift+Space` opens it from anywhere in Axiom chrome.

## What it shows

- currently open tabs and the active tab;
- actual in-progress tab loads;
- profile bookmark and history counts;
- active download count;
- direct trusted links to a new tab, settings, network diagnostics and downloads.

The dashboard derives each value at render time from the browser instance and its
profile-owned repositories. It has no account, synchronization, analytics, AI service,
or Axiom-operated network request. Private windows use their memory-only profile just
like every other trusted internal page.

## Visual system

Focus Space is intentionally not a clone of another browser's new-tab page. It keeps
the familiar browser hierarchy—tabs, navigation, an address field and settings—while
using Axiom's dark Orbit Rail, violet/teal workspace surface, concise status cards and
high-contrast focused-tab treatment. Motion remains limited to the existing
reduced-motion-aware loading accent in native chrome.

## Trusted boundary

`axiom://focus` is a trusted internal page. Ordinary web documents cannot navigate to
or mutate Axiom internal URLs. The page displays URLs only as local tab metadata; it
does not expose cookies, headers, download destinations or browsing data to webpages.

## Current scope

**FUNCTIONAL:** local workspace overview, launcher, shortcut, profile isolation,
privacy-preserving counts and links to existing trusted tools.

**PARTIAL:** cards are an overview, not a session editor. Reordering, grouping and
cross-window workspaces need dedicated tab-model work.

**DEFERRED:** cloud workspace sync, collaborative workspaces and any account-backed
feature. Axiom intentionally has no account requirement or telemetry channel.
