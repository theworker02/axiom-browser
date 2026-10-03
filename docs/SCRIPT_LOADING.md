# Script loading (Phase 3 Wave G)

How `<script>` elements are classified, fetched and scheduled. Scripts run on the UI thread in
the page's `JsContext` (Boa behind `axiom-js`); fetching goes through the resource loader like
every other subresource ([RESOURCE_LOADING.md](RESOURCE_LOADING.md)).

## Classification

`axiom_document::classify_script` decides the kind from `src`, `async`, `defer`, `type` and
whether the parser inserted the element:

| Kind (`script_kind`) | When | Runs |
|------|------|------|
| `inline` | No `src` (`async`/`defer` ignored) | At its parser position, after pending script-blocking stylesheets; synchronously when inserted by script |
| `parser-blocking` | Parser-inserted `src` without `async`/`defer` | Pauses the parser until fetched and executed |
| `async` | Parser-inserted `src` with `async` | As soon as it is available, in completion order |
| `defer` | Parser-inserted `src` with `defer` (and no `async`) | After parsing, in document order, before `DOMContentLoaded` |
| `dynamic` | `src` on a script inserted by script | When available; never blocks the parser |
| `module (unsupported)` | `type="module"` | Never fetched or executed; recorded as `FAILED` |
| `data block` | Any other non-JavaScript `type` | Never executed |

A script element runs at most once ("already started" is tracked per node).

## Scheduling

Scheduling is driven by resource state, not by timers. `pump_document` runs, in order:
element `load`/`error` events, queued insertions, ready async and dynamic scripts, the
pending parser-blocking script, a parse step, the defer queue, `DOMContentLoaded`, `load`.

- **Parser-blocking.** When the parser yields a script, the loader requests it (or adopts a
  preloaded request) and records it as `parser_blocking_script`. The parser does not advance
  until the script is available and no script-blocking stylesheet is pending; meanwhile the
  preload scanner looks ahead. Scripts fetched in parallel still run in document order.
- **Inline.** Runs when the parser reaches it, once no script-blocking stylesheet is pending.
  It sees the DOM parsed so far and `document.readyState == "loading"`.
- **Async.** Queued as soon as its body arrives; runs at the next pump step, even while the
  parser is waiting. Async scripts block `load`, not `DOMContentLoaded`.
- **Defer.** Queued in document order at parse time. After parsing completes (readyState
  `interactive`), each runs once available and once no script-blocking stylesheet is pending.
  `DOMContentLoaded` waits for the whole queue.
- **Dynamic.** A script-inserted `<script src>` is started through the loader (initiator
  `script`, low priority), runs when available, and blocks `load`. A script-inserted inline
  script runs synchronously inside `appendChild`, with `document.currentScript` set.

After an external script runs, its element receives `load` (or `error` if the fetch failed).

Evidence:

- `document_loading::script_order_parser_blocking_inline_and_defer`: a slow parser-blocking
  script still runs first; the inline script sees `#first` but not `#second`; defer scripts
  complete in reverse order (checked from response times) but run in document order after
  `readystatechange: interactive` and before `DOMContentLoaded`; the slow image finishes after
  `DOMContentLoaded` and before `load`.
- `document_loading::async_scripts_run_when_ready_and_do_not_block_dom_content_loaded`.
- `document_loading::dynamic_insertion_goes_through_the_loader`.
- `wave_f1::fixture_subresources_load_concurrently_through_the_loader` (reverse-delayed blocking
  scripts run in order).

## Errors

A script that throws, or fails to compile, is recorded and the page continues:

- `ScriptError { resource, label, message }` in `BrowsingContext::script_errors`;
- the resource becomes `FAILED` with class `script` and stage `execution`;
- `ScriptExecutionFinished` carries the error message;
- the element still receives `load` (the fetch succeeded), as in other browsers.

Fetch failures fail the resource at the `network`, `http-status` or `mime` stage and the
element receives `error`. Evidence:
`document_loading::script_errors_are_recorded_and_do_not_stop_the_page`.

## Content Security Policy

Since Phase 3 Wave I the document's CSP decides whether a script runs
([`CSP.md`](CSP.md)):

- External classic and module scripts, and module imports, are checked against
  `script-src-elem` (falling back to `script-src`, then `default-src`) before they are
  requested. A blocked script fails with a `csp:` reason and fires `error`.
- Inline classic scripts, inline modules and import maps need `'unsafe-inline'`, a matching
  nonce or a matching hash; a blocked one is skipped without events.
- `'strict-dynamic'` trusts scripts inserted by trusted script and blocks parser-inserted
  scripts without a nonce.
- Module imports use the nonce of the document's first module script the policy allowed.
- Event handler attributes are checked against `script-src-attr`; `eval()` and `Function()`
  against `script-src` (`'unsafe-eval'`).

## Reentrancy

- The loader state is not borrowed while a script runs. Script-initiated work (insertions,
  attribute changes, `fetch()`) is queued and processed by the pump after the script returns.
- The only synchronous nested execution is a script-inserted inline script, run by the JS
  layer inside `appendChild` after the DOM host borrow is released.
- After every script, the driver re-checks that the same `DocumentId` is still active before
  recording the outcome, so a script that caused its own document to be replaced cannot write
  into the new one.
- The pump is bounded (100 000 steps per call) so a pathological page cannot hang the UI loop.

## Not supported

- **`document.write` / `document.writeln`**: ignored (no-op). Pages that build their markup
  with `document.write` will be missing that content. This is a deliberate gap: supporting it
  needs parser reentrancy and a script-inserted input stream.
- **`nomodule`** is ignored, so such classic scripts run. (Module scripts and import maps
  are supported since Phase 3 Wave G; see [`WEB_COMPATIBILITY.md`](WEB_COMPATIBILITY.md).)
- **`integrity`** on `<script>` (SRI exists only for `fetch()`), `blocking="render"`,
  `fetchpriority`.

`crossorigin` is supported since Phase 3 Wave H: it switches the script to a `cors`
request, and module scripts always use `cors`. A cross-origin script the server does not
allow fires `error` and never runs (see [`RESOURCE_LOADING.md`](RESOURCE_LOADING.md)).
Error muting is not implemented: an uncaught exception in a classic script's top-level code
is logged but not reported to `window.onerror` at all, and exceptions thrown later from
callbacks (timers, listeners) are reported with their full message whatever origin the
script came from. Browsers report `"Script error."` for cross-origin scripts loaded without
CORS.
- Changing `src` on an already-started script does nothing, as specified; setting `src` on a
  script that was never inserted is picked up only when it is inserted.
