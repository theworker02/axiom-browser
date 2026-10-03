# Page lifecycle (Phase 3 Wave G)

`document.readyState`, `readystatechange`, `DOMContentLoaded` and `load`, driven by parser and
resource state in `axiom_document::DocumentLifecycle`. Nothing in the lifecycle is decided by
timers or by "the network went quiet".

## States and events

```text
commit ─ readyState "loading" ─ DocumentCreated, ParsingStarted
  parser runs; parser-blocking and inline scripts execute; async scripts run when ready
end of input ─ ParsingCompleted
  readyState "interactive" → readystatechange (document)
  defer scripts run in order (each after pending script-blocking stylesheets)
DOMContentLoaded (document, bubbles)            ← parsing done, no blocking script, defer queue empty
  load-blocking resources settle (images, fonts, stylesheets, async/dynamic scripts)
  their element load/error events are dispatched first
readyState "complete" → readystatechange (document) → load (window) ← DCL fired, nothing load-blocking pending
```

- `DOMContentLoaded` never waits for images, fonts or async scripts.
- `load` waits for every resource flagged `load_blocking` to reach a terminal state
  (`READY`, `FAILED` or `CANCELED`). Lazy images are not load-blocking. A resource that fails
  unblocks `load` like one that succeeds.
- Each event fires exactly once per document. A document that was replaced (`canceled`)
  fires nothing further.
- Resources discovered after `load` (for example a script-inserted image) do not block
  anything already fired.

Evidence: `document_loading::script_order_parser_blocking_inline_and_defer` (the page's own
listener log is exactly
`readystatechange:interactive, defer-1..3, DOMContentLoaded:interactive, readystatechange:complete, load:complete`),
`document_loading::lifecycle_event_order_is_recorded` (the event log order `DocumentCreated <
ParsingStarted < ParsingCompleted < DOMContentLoadedFired < LoadFired`, each once, and
`readyState == "complete"`), `document_loading::async_scripts_run_when_ready_and_do_not_block_dom_content_loaded`,
`document_loading::images_track_size_failures_and_lazy_priority`.

## `DocumentLifecycle`

| Field | Meaning |
|-------|---------|
| `parsing_state` | `not-started`, `parsing`, `blocked-on-script`, `waiting-for-data`, `complete` |
| `ready_state` | `loading`, `interactive`, `complete` (mirrored into the page's `DocumentShared` for script) |
| `blocking_stylesheets` | Parser-inserted stylesheets and their imports still loading; they block scripts and rendering |
| `parser_blocking_script` | The script the parser is paused at |
| `defer_queue` | Deferred scripts, in document order |
| `load_blocking_resources` | Resources `load` waits for |
| `dom_content_loaded_fired`, `load_fired` | Each set once |
| `canceled` | Replaced or stopped; nothing may advance it |
| `timeline` | Measured milestones (below) |

## Rendering

Rendering is held while parser-inserted stylesheets load (the previous frame stays on screen),
for at most 30 s. The desktop browser's `navigate` returns when `DOMContentLoaded` has fired
and no render-blocking stylesheet is pending, so the first frame has the page's CSS; images
fill in later.

## Stop, navigation away and history

- `stop_loading` cancels pending requests; the parser finishes with the bytes it has; the
  canceled resources settle, so `DOMContentLoaded` and `load` fire (browsers differ here; Axiom
  chooses to complete the lifecycle so pages do not hang in `loading`).
  `document_loading::stop_cancels_pending_resources_and_finishes_the_document`.
- Replacing the document cancels it: no further events, timers or callbacks reach it
  ([DOCUMENT_LOADING.md](DOCUMENT_LOADING.md#generation-guard)).
- History gets exactly one entry per committed navigation; `reload` replaces the entry.
  The entry title is updated when parsing completes.

## Events and timeline

Every document keeps a bounded log (1024) of structured `DocumentEvent`s: `seq`, time,
document, optional resource and request ids, and a short secret-free detail. Kinds:
`DocumentCreated`, `ParsingStarted`, `ResourceDiscovered`, `ResourceQueued`, `ResourceReady`,
`ResourceFailed`, `ResourceCanceled`, `ScriptExecutionStarted`, `ScriptExecutionFinished`,
`ParsingCompleted`, `DOMContentLoadedFired`, `LoadFired`, `DocumentCanceled`,
`StaleEventDropped`.

The timeline records `navigation_start`, `response_start`, `first_bytes`, `parse_start`,
`parse_end`, `dom_content_loaded`, `load` (event dispatched) and `complete` (handlers
finished), each in milliseconds since navigation start. Milestones not reached are `None`
and shown as "n/a"; they are never estimated.

`DocumentDiagnostics` adds URL, origin, kind, encoding (with its source), parsing and ready
state, bytes parsed, blocking stylesheets, the paused script, the defer queue, resource counts
(total, pending, failed, rejected by limits), script errors, and the stale events dropped by
the tab. `axiom://document` renders this for the active tab, plus the retired documents, with
query strings removed.
Evidence: `document_loading::document_diagnostics_page_redacts_queries_and_internal_pages_load_nothing`.

## Not implemented

`pageshow`/`pagehide`, `beforeunload`/`unload`, `visibilitychange`, the back/forward cache,
`document.open`, and `load`/`error` events for `<style>` elements. (`<link rel=stylesheet>`,
`<img>` and external `<script>` elements do receive `load`/`error`.)
