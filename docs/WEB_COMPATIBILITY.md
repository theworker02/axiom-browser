# Web Compatibility — live-site results (Phase 3 Wave G)

Date: 2026-09-29. Build: release. Tool: `cargo run --release -p axiom-browser --example live_smoke -- --png <dir>` with Google as the search provider. Viewport: 1024×640, private window.

**This is a manual measurement, not a CI test.** Third-party sites change without notice, and Google in particular must never be a CI dependency. Re-run the probe and update this file when the relevant engine areas change.

A successful HTTP response does not mean a site renders correctly. Every stage is therefore rated separately. The probe's automatic ratings (node counts, fetch and decode failures, script errors, the share of painted pixels) are heuristics. The layout ratings below come from looking at the saved frames.

## Summary

| Site class (probe) | Network | HTML | CSS | Layout | Images | JavaScript | Events | Forms | Navigation |
|--------------------|---------|------|-----|--------|--------|------------|--------|-------|------------|
| Static HTML (example.com) | PASS | PASS | PASS | PASS | — | PASS | — | — | PASS |
| CSS-heavy (MDN CSS) | PASS | PASS | PARTIAL | PARTIAL | — | PARTIAL | not measured | — | PASS |
| JS-heavy (react.dev) | PASS | PASS | PARTIAL | PARTIAL | PASS | PARTIAL | not measured | not measured | PASS |
| Forms (DuckDuckGo HTML) | PASS | PASS | PARTIAL | PARTIAL | — | PASS (none) | PASS | PASS | PASS |
| Search engine (Google home + query) | PASS | PASS | PARTIAL | PARTIAL | PASS | FAIL | not measured | not measured | PASS |
| Docs site (Rust book) | PASS | PASS | PARTIAL | PARTIAL | — | PARTIAL | not measured | — | PASS |
| Wikipedia-style (Rust article) | PASS | PASS | PARTIAL | PARTIAL | PASS | PASS | not measured | not measured | PASS |
| GitHub-style (rust-lang/rust) | PASS | PASS | PARTIAL | PARTIAL | PASS | PARTIAL | not measured | — | PASS |

What the ratings mean:

- **PASS**: works for what the page needs.
- **PARTIAL**: works but visibly wrong or incomplete.
- **FAIL**: the page's use of the feature does not work.
- **—**: the page does not use the feature.
- **not measured**: needs interactive testing, which the probe does not do.

The ratings describe the category, not the network request alone. For example, CSS is still PARTIAL on most sites even though every stylesheet downloads, because some properties those sheets rely on (`background-image`, `mask-image`, WOFF2 web fonts) are not applied.

### Change since Wave F

Wave F (2026-09-28) listed seven blockers. Wave G addressed all seven, one step each, re-running this probe after every step:

| Step | Blocker | Result on the live sites |
|------|---------|--------------------------|
| G1 | Glyph placement | Descenders and commas sit on the baseline on every page |
| G2 | Fonts and shaping | Web fonts (TTF, OTF, WOFF 1.0) load; floats, tables, `<details>` and paint order fixed |
| G3 | `navigator`, timers | `navigator is not defined` and `setInterval is not defined` are gone from every site |
| G4 | ES module scripts | GitHub's 93 module scripts and MDN's modules are fetched and run |
| G5 | SVG images | 0 `<img>` failures across all probes (Wave F: every SVG failed) |
| G6 | Form submission | The DuckDuckGo HTML page searches from its own box (POST, 43 result links) |
| G7 | Flex and grid layout | GitHub, react.dev, Wikipedia and MDN chrome lay out in rows and columns instead of vertical lists |

Share of the viewport painted (a rough signal, not a score): Wikipedia 3.1% → 24.4%, MDN 21.0% → 23.9%. GitHub fell from 100% to 43.1% and react.dev from 12.4% to 5.7% because overlapping and unhidden content no longer covers the page.

### Wave H re-run (CORS)

Wave H made module scripts, fonts and `crossorigin` subresources use CORS, so the probe was re-run to catch servers that do not send `Access-Control-Allow-Origin`. All nine targets gave the same stage ratings, paint shares and resource counts as the last Wave G run: GitHub's 93 module scripts from `github.githubassets.com` still load, and no resource on any site failed a CORS check. The first re-run did find a crash on the Google results page: a stack overflow on Windows' 1 MiB main-thread stack. Windows binaries are now linked with 8 MiB (`docs/PHASE3_PROGRESS.md`, Wave H).

### Wave I re-run (Content Security Policy)

Wave I started enforcing each page's Content Security Policy, so the probe was re-run to catch policies Axiom enforces more strictly than other browsers. All nine targets gave the same stage ratings and paint shares as the Wave H run, and no site logged a CSP violation or had a resource blocked by its policy. The only count that changed is MDN's external scripts (7 before; 5 and 6 in two Wave I runs, none failed). A CSP block would show as a failed resource, so this is timing on the site.

## Network (all sites)

Every probe committed over HTTPS with a verified certificate, negotiated HTTP/2, and followed redirects. Every stylesheet requested was fetched (0 CSS failures across 9 sites and 62 external sheets), and every `<img>` decoded (73 requested, 0 failed). Documents took 342 ms–1.9 s; Google's results page took 2.3 s, mostly server time. The network stage does not limit compatibility.

## Per site

### Static HTML — `https://example.com/`

- Network 200, 350 ms. 1607 nodes, title "Example Domain", 1 external script, 0 script errors.
- Render: correct. The centered text and link match the page's intent.

### Search engine — Google (`https://www.google.com/`, then the query "rust programming language")

| Stage | Home page | Query `rust programming language` |
|-------|-----------|-----------------------------------|
| NETWORK | PASS: 200, TLS verified, HTTP/2, 515 ms | PASS: the omnibox classified the text as a search and navigated to `https://www.google.com/search?q=…` through `SearchProviderService`; 200, TLS verified, HTTP/2, 2.3 s |
| DOCUMENT PARSE | PASS: 113 nodes, title "Google" | PASS: 31 nodes, title "Google Search" |
| RENDER | PARTIAL: the logo, header links, "Sign in" button, search box, both buttons and footer render; the search box sits at the left edge instead of the center | FAIL: Google still serves its script-required interstitial ("If you're having trouble accessing Google Search, please click here…") |
| SCRIPT | PARTIAL: 1 of 2 external scripts throws a `TypeError` | FAIL: a timer callback throws `Image is not defined`, so the interstitial never redirects |
| INTERACTION | not measured | not measured |

Axiom does not impersonate Chrome to get result HTML. The next obstacle is the missing `Image` constructor.

### Forms — `https://html.duckduckgo.com/html/`

- Network 200, 352 ms. 77 nodes, no scripts.
- Render: the text input is centered on the gray page. The logo and the submit button are still not visible, probably because both are drawn with CSS background images.
- Form submission: typing "axiom browser engine" and pressing Enter posts the form (`application/x-www-form-urlencoded`) and commits the results page: 200, 43 links, title "axiom browser engine at DuckDuckGo", 2.9 s. The results page renders with titles, URLs, snippets and bold query terms.

### Docs site — `https://doc.rust-lang.org/book/`

- Network 200, 367 ms. 411 nodes, 12 stylesheets, all loaded.
- Render: the title, paragraphs, links, inline code and the callout box render correctly in a centered column. The sidebar stays hidden because the script that opens it fails.
- Fonts: the site's Open Sans and Source Code Pro are WOFF2, which is unsupported, so system fonts are used.
- Script: 3 of 8 external scripts (`toc`, `searcher`, `book`) throw `cannot convert 'null' or 'undefined' to object`.

### Wikipedia-style — Rust article

- Network 200, 922 ms. 19,235 nodes, 2 external and 17 inline stylesheets.
- Images: 28 requested, 0 failed (SVG wordmark, tagline and icons now decode).
- Script: 4 external and 4 inline scripts run with 0 errors.
- Render: the header, three-column grid (navigation, article, tools) and infobox are in place. Icons drawn with `mask-image` render as solid black squares because masks are not supported.

### GitHub-style — `https://github.com/rust-lang/rust`

- Network 200, 1.9 s. 3,262 nodes, 27 stylesheets, all loaded.
- Script: all 93 external module scripts load; 10 throw `TypeError: not a callable function` (root cause not yet identified).
- Images: the one requested SVG decodes.
- Render: the header navigation, repository tabs, file list and the sidebar grid lay out as on GitHub. Menus and dialogs are hidden as intended. The logo and octicons are missing (inline `<svg>`), and the commit-message cells show their gray loading placeholders because the scripts that fill them fail.

### CSS-heavy — MDN `https://developer.mozilla.org/en-US/docs/Web/CSS`

- Network 200, 342 ms. 8,065 nodes, 17 external and 23 inline stylesheets.
- Script: module scripts run; 2 throw `URL is not defined`.
- Render: the header and the three-column grid (sidebar, content, table of contents) are in place.

### JS-heavy — `https://react.dev/`

- Network 200, 1.2 s. 2,590 nodes (server-rendered), 43 images requested, 0 failed.
- Script: the Next.js runtime throws `right-hand side of 'in' should be an object`, so hydration does not happen and the page stays static.
- Render: the navigation is one row, the hero is centered and the buttons are pills. The React logo is missing (inline `<svg>`).

## Remaining compatibility blockers (in order of impact)

1. **Missing JS platform objects.** `getComputedStyle`, `Image` and `URL` are undefined. These break Google's redirect, MDN's client and several initializers. GitHub's `not a callable function` errors and the Next.js `in` error are probably more missing DOM surface.
2. **Inline `<svg>` in HTML and CSS background images.** Inline SVG lays out as an empty box of the right size; only `<img>` sources (raster or SVG) are drawn. `background-image` is not parsed at all (the `background` shorthand keeps only its color). Logos and icon sets are missing (GitHub, react.dev, and probably the DuckDuckGo logo).
3. **WOFF2 fonts.** 42 font loads in this run failed with "WOFF2 fonts are not supported". Pages fall back to system fonts.
4. **`mask-image`.** Unsupported, so masked icons render as solid squares (Wikipedia).
5. **Form gaps.**
   - No named properties on forms (`form.q`).
   - No file picker for `<input type=file>`.
   - Link navigations do not record their initiator, so a `SameSite=Strict` cookie is still sent when a cross-site link is clicked.
6. **Layout gaps.**
   - The Google home search box is not centered.
   - Grid line names that follow an `auto-fill`/`auto-fit` repeat attach to the line before it.

## Reproducing

```text
cargo run --release -p axiom-browser --example live_smoke
cargo run --release -p axiom-browser --example live_smoke -- --png target/live --provider duckduckgo "rust programming language"
cargo run --release -p axiom-browser --example live_smoke -- search-home search-query
```

Probe names are `static`, `search-home`, `search-query`, `docs`, `wikipedia`, `github`, `css-heavy`, `js-heavy` and `forms`. The `forms` probe also types a query into the page's own search box and submits it. Any other argument is typed into the omnibox as-is, so it may be a URL or a search.
