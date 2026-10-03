# Document loading (Phase 3 Wave G)

How a navigation becomes a document: streaming response → decoder → incremental parser →
DOM, with subresources discovered as the parser goes. Companion documents:
[RESOURCE_LOADING.md](RESOURCE_LOADING.md) (subresources), [SCRIPT_LOADING.md](SCRIPT_LOADING.md)
(script scheduling) and [PAGE_LIFECYCLE.md](PAGE_LIFECYCLE.md) (readyState and events).

Status markers are the ones used in [NETWORKING.md](NETWORKING.md): **FUNCTIONAL** (implemented
and tested), **PARTIAL** (works with a documented gap), **DEFERRED** (not implemented).

## Pipeline

```text
BrowsingContext::start_navigation(url)            NavigationId, pending navigation
  └─ ResourceLoader::start(Document, VeryHigh, stream_body)
       └─ RequestScheduler → NetworkService       (the only HTTP stack; cookies, cache, redirects)
  ← LoaderEvent::Response   → commit: retire the old document, begin_document (DocumentId)
  ← LoaderEvent::Chunk      → TextDecoder (axiom-net) → HtmlParser::feed (axiom-html)
  ← LoaderEvent::Completed  → decoder.finish → parser end of input
pump_document (UI thread, bounded)
  element load/error events → dynamic insertions → ready async scripts
  → parser-blocking script → parse step (discovers <base>/<link>/<img>/<style>/<script>)
  → defer queue → DOMContentLoaded → load
```

Code: `crates/axiom-engine/src/browsing.rs` (navigation, commit, event routing),
`browsing/document.rs` (the `DocumentLoad` driver and pump), `browsing/resources.rs`
(subresources), and the `axiom-document` crate (identity, lifecycle, registry, policy,
diagnostics). The HTML tokenizer and tree builder are Axiom's own (`axiom-html`).

## Identity

Every committed document gets a `DocumentId` (`axiom-document/src/ids.rs`). Its
`DocumentInfo` records the `NavigationId` that created it, the `BrowsingContextId`, the owning
tab and profile, the canonical `Origin` (`axiom-url`), the final URL after redirects, and
the kind (`network`, `file`, `internal`). Tabs set the owner with
`BrowsingContext::set_owner` when they are created.

## Generation guard

A document never receives work addressed to another document:

- Every subresource request is recorded in `request_owner: RequestId → DocumentId`. Loader
  events whose request belongs to any document other than the active one are dropped, counted
  in `stale_events_dropped`, and logged as `StaleEventDropped`.
- Commit calls `retire_document`, which cancels every loader request except the new
  document's own, clears timers, tasks, microtasks and pending `fetch()` state, and moves
  the old `DocumentLoad` into a bounded list of retired snapshots (4). Its pending resources
  become `CANCELED` and it records `DocumentCanceled`.
- Script execution does not hold a borrow of the loader state; after a script returns, the
  driver re-checks that the same `DocumentId` is still active before touching it.
- `check_page_generation` compares the page's `DocumentShared` pointer with the loader's,
  so a page replaced behind the loader's back (for example by `load_local_html`) retires the
  loader document instead of letting it write into the new page.

Evidence: `document_loading::obsolete_document_callbacks_never_touch_the_new_document`
navigates away while the old page's stylesheet, script, image (all delayed 800 ms) and a
300 ms timer are outstanding, waits past all of them, and checks that none of them touched
the new document: no DOM node, global, timer effect, CSS rule or image, and the old
resources are `CANCELED` in the retired snapshot and `Cancelled` in the network log.

## Streaming and decoding

- The document request uses `stream_body`; the body is never assembled before parsing.
  **FUNCTIONAL**: `document_loading::html_streams_into_the_parser_before_the_body_completes`
  (the stylesheet in the first chunk is requested, and `#early` is in the DOM, while the
  server is still sending the body).
- Decoding is centralized in `axiom_net::TextDecoder` (BOM, then the transport `charset`,
  then a `<meta charset>` prescan of the first 1024 bytes, then UTF-8). Chunk boundaries
  never change the output. A complete `<meta ...>` declaration ends the prescan early, so a
  small first chunk does not stall the parser (fixed during this wave; unit test
  `encoding::tests::complete_meta_declaration_ends_the_prescan_early`). Without any
  declaration the decoder still waits for 1024 bytes or the end of the body, as the HTML
  prescan requires.
- Non-HTML `text/*` documents are streamed into a `<pre>` wrapper.
- The document body is parsed as it arrives and is not buffered by the loader. Subresource
  bodies that one document holds while they are processed are capped at 64 MiB in total (see
  [RESOURCE_LOADING.md](RESOURCE_LOADING.md#limits)).

## Base URL

The first `<base href>` sets the document base (`axiom_document::resolve_base_href`); later
ones are ignored. Every subresource URL resolves against the base in effect when its element
is parsed. CSS `url()`, `@import` and `@font-face` resolve against the stylesheet's final
URL, never the document. **FUNCTIONAL**:
`document_loading::base_href_resolves_css_script_and_images` (relative, parent-directory,
absolute-path and absolute URLs for a stylesheet, a script and images) and
`document_loading::stylesheets_imports_fonts_and_failures`.

## Document kinds

| Kind | Loads | Evidence |
|------|-------|----------|
| `network` | Subresources through `ResourceLoader` only | every `document_loading` test |
| `file` | Relative subresources are read from disk next to the file | `document_loading::local_file_documents_load_relative_resources_from_disk` |
| `internal` | Nothing: trusted `axiom://` pages, error and download pages start with a completed lifecycle, and script insertions are discarded | `document_loading::internal_pages_never_fetch_web_resources` |

## Stop and navigation away

- A new navigation keeps the current document until the response commits. A superseded
  navigation's request is cancelled and can never commit (Wave E, unchanged).
- `stop_loading` cancels the context's pending requests; the parser finishes with the bytes
  it has, and `DOMContentLoaded` and `load` still fire once nothing is pending.
  **FUNCTIONAL**: `document_loading::stop_cancels_pending_resources_and_finishes_the_document`.
- History: a navigation pushes or replaces exactly one entry at commit; `reload` replaces.
  `document_loading::cache_cold_warm_and_reload` checks that reload adds no entry.

## Diagnostics

`BrowsingContext::document_diagnostics`, `resource_diagnostics`, `document_events`,
`script_errors` and `retired_documents` return plain snapshots; `axiom://document` renders
them for the active tab (current document first, then retired ones) with query strings
removed and no bodies. See [PAGE_LIFECYCLE.md](PAGE_LIFECYCLE.md#events-and-timeline).

## Benchmarks

`cargo run -p axiom-browser --example docbench --release` loads a page with 12 stylesheets,
12 deferred scripts and 12 images (half fresh for an hour, half revalidated by ETag) over
loopback, plus a 2 MiB document. Numbers come from the measured document timeline. A sample
run (Windows, release):

| Scenario | p50 DOMContentLoaded | p50 load | Cache states |
|----------|---------------------|----------|--------------|
| Cold (new profile per sample, 5 samples) | 39.4 ms | 53.5 ms | 36 miss |
| Warm (same profile, 10 samples) | 20.6 ms | 32.2 ms | 18 hit, 18 revalidated |
| Reload (10 samples) | 21.0 ms | 32.5 ms | 18 hit, 18 revalidated |

2 MiB document: parse p50 50.8 ms (about 39 MiB/s including DOM construction). The wall time
around that navigation (886 ms) is dominated by style, layout and paint after `load`, not by
loading.

## Gaps

- `document.write` / `document.open` are not supported (ignored; see
  [SCRIPT_LOADING.md](SCRIPT_LOADING.md)).
- No speculative parsing past a blocking script beyond the conservative preload scanner.
- The desktop browser still waits synchronously for `DOMContentLoaded` (plus render-blocking
  stylesheets) in `navigate`; the engine's `start_navigation` is asynchronous.
- iframes, Service Workers and process isolation are out of scope for this wave.
