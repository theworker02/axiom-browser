# Compatibility

Axiom measures web compatibility with conformance suites that it runs offline, not with a
"percentage of Chrome". Every result is a count with its denominator, reported per file or
directory, and every run is compared with a checked-in expectations file. The ratchet fails
on regressions **and** on unexpected passes, so a result can only change on purpose.

The harness lives in `tools/compat` (`axiom-compat`); the expectations live in
`tests/expectations/`.

## Suites

| Suite | Source (pinned, vendored) | What a pass means |
|-------|---------------------------|-------------------|
| `html5lib-tree` | WPT `html/syntax/parsing/resources/*.dat` | The tree `axiom-html` builds serializes to the expected `#document` dump |
| `html5lib-tokenizer` | html5lib-tests `tokenizer/*.test` | The token stream from `axiom-html`'s tokenizer equals `output` (one test per initial state) |
| `crashtests` | WPT `*/crashtests/` | The page reaches `load` through the browsing pipeline without a panic or hang |
| `reftests` | WPT `css/CSS2/{colors,box-display,margin-padding-clear}`, `css/css-display` | The 800×600 frame matches (or, for `mismatch`, differs from) a reference frame, within `<meta name=fuzzy>` |
| `testharness` | WPT `dom/nodes`, `dom/events`, `dom/collections`, `dom/lists` | File: testharness.js reports harness status OK. Subtest: the subtest reports PASS |

Pins: WPT `f085a1efc1f58fbe263d384b1e335d656fe58e66`, html5lib-tests
`224991ec10db04f056a89eed8b0bd8695fd2950e`. See `tests/wpt/VENDOR.md` and
`tests/html5lib-tests/VENDOR.md`. `tools/compat/vendor.ps1` refreshes the corpus; CI never
runs it and never touches the network.

The engine-driven suites (crashtests, reftests, testharness) load pages from a loopback
server over the vendored tree (`tools/compat/src/server.rs`), through the same
`BrowsingContext` path that browser tabs use. The server substitutes the common `{{host}}`,
`{{ports[...]}}`, `{{location[...]}}` and `{{GET[...]}}` templates. It does not run
wptserve's Python handlers generally; the narrowly required `contenttype_setter.py` behavior is
implemented as a deterministic Rust response. Other handler-backed tests remain recorded as
unsupported until their behavior is deliberately implemented.

## Running

```text
cargo run -p axiom-compat -- all                    # every suite, compared with expectations
cargo run -p axiom-compat -- reftests --filter css/css-display
cargo run -p axiom-compat -- testharness --failures 20 --json out.json
cargo run -p axiom-compat -- all --update           # rewrite expectations from this run
```

`cargo test --workspace` runs the same comparison in `tools/compat/tests/ratchet.rs`, one
test per suite. When a change is intended, run `--update` and review the expectations diff
like code. `--update` refuses to combine with `--filter`.

Each engine-driven test runs on its own worker thread with a budget. The budget is 10 s for
crashtests, 5 s for reftests and 15 s for testharness, plus a hard timeout. Workers run in
parallel (`--jobs`). Results were identical across repeated runs when the baselines were
recorded.

## Current results

Recorded 2026-09-28, after the HTML tree builder rewrite, the processing-instruction and
`<selectedcontent>` work, the DOM core wave, and the `Attr` and events wave:

| Suite | Passed / total |
|-------|----------------|
| html5lib-tree | 1953 / 1959 |
| html5lib-tokenizer | 7017 / 7032 |
| crashtests | 26 / 26 |
| reftests | 693 / 1034 |
| testharness, files (harness OK) | 453 / 585 (ERROR 46, TIMEOUT 86) |
| testharness, subtests | 4375 / 5959 (FAIL 1450, TIMEOUT 85, NOTRUN 49) |

The testharness file count covers `dom/nodes` (with its `moveBefore`,
`insertion-removing-steps` and `Document-contentType` subdirectories), `dom/events` (with
`non-cancelable-when-passive` and `scrolling`), `dom/collections` and `dom/lists`. The
per-group table prints each directory separately.

Reftests per directory:

| Directory | Passed / total |
|-----------|----------------|
| `css/CSS2/box-display` | 46 / 120 |
| `css/CSS2/colors` | 3 / 19 |
| `css/CSS2/margin-padding-clear` | 582 / 682 |
| `css/css-display` | 23 / 80 |
| `css/css-display/run-in` | 39 / 133 |

The checked-in reftest baseline was refreshed on 2026-10-03 after a verified XHTML
entity-resolution repair and the CSS `display: block math` correction. It now records
**898 / 1034** matching reftests. The prior table remains the 2026-09-28 historical
snapshot; rerun `axiom-compat reftests` for the exact current per-directory breakdown.

Per-file tables print on every run; `--json` writes every outcome.

### What changed with the tree builder

| Suite | Before | After |
|-------|--------|-------|
| html5lib-tree | 91 / 1959 | 1860 / 1959 (1953 after the follow-up below) |
| reftests | 751 / 1034 | 693 / 1034 |
| testharness files | 211 / 298 | 222 / 298 |
| testharness subtests | 93 / 2878 | 114 / 3283 |

The reftest drop is honest. Before the rewrite, pages that omit `<html>`/`<body>` (most WPT
tests) rendered blank. Test and reference were both blank, so they "matched". 61 of the 62
reftests that went from PASS to FAIL are such pages; they now render and expose layout
Axiom lacks: `display: contents`, blocks inside inlines, negative auto margins. The 62nd
(`containing-block-029.xht`) has a reference whose table now gets the spec's implied
`<colgroup>` and `<tbody>`, and Axiom's table layout has no column support.

The testharness subtest denominator grew because more files now load testharness.js. For
example, `src=/resources/testharness.js` without quotes used to be mis-tokenized. 20 files
went from harness OK to TIMEOUT. Before, their `document.body` was `null`, so they threw
early. Now the body exists, and they wait for iframe `load` / `contentDocument` behavior not yet exposed to script or,
for one XHTML file, define no tests at all.

### Parser follow-up: processing instructions and `<selectedcontent>`

html5lib-tree went from 1860 to 1953 / 1959:

- The tokenizer implements the processing-instruction states (§13.2.5.72–76), and the DOM
  has a `ProcessingInstruction` node inserted wherever a comment would be. This fixed
  `processing-instructions.dat` and the related cases in `tests1.dat` and
  `html5test-com.dat`.
- `select` is in the default "has an element in scope" list (fixes `webkit02.dat` 49).
- When a selected `<option>` is popped off the stack of open elements, its children are
  cloned into the select's first `<selectedcontent>` (`webkit02.dat` 45–48).

html5lib-tokenizer went from 7028 to 7017 / 7032, and the 11 new failures are expected.
`html5lib-tests` at its current upstream HEAD, which is our pin
(`224991ec10db04f056a89eed8b0bd8695fd2950e`), predates the processing-instruction change
and still expects bogus comments. `test2.test` 32–33 expect `<?namespace>` and `<?foo-->`
to be comments. `test3.test` 1159 and 1181–1189 expect a comment for `<?` and `<?a` at end
of file, which the current spec drops. Upstream moved tree tests for processing
instructions to WPT, and Axiom follows the spec.

### DOM core wave

testharness went from 681 to 3408 passing outcomes (file and subtest outcomes counted
together). The total grew from 3847 to 5342 because files that used to stop early now reach
their later subtests. What changed is described in [DOM.md](./DOM.md). In short:

- A spec-shaped interface hierarchy (`Node`, `Element`, `HTMLElement` and per-tag
  subclasses, `CharacterData`, `Document`, …) with checked tree mutations that throw the
  right `DOMException`.
- Live `HTMLCollection`/`NodeList`, `DOMTokenList`, namespaced attributes.
- A Selectors Level 4 engine for `querySelector*`, `matches` and `closest`.
- `innerHTML`/`outerHTML`/`insertAdjacentHTML`, `template.content`.
- `MutationObserver`.
- A fix for falsely reported unhandled promise rejections (see [JAVASCRIPT.md](./JAVASCRIPT.md)).
  Before the fix, any failing `promise_test` turned its whole file into an ERROR.

36 outcomes that used to pass or not exist now fail or time out. Each one needs a feature
Axiom does not have yet:

| Outcomes | Missing feature |
|----------|-----------------|
| `MutationObserver-childList` Range cases (14) | `Range` |
| `MutationObserver-textContent` CDATA case (1, NOTRUN) | `DOMParser` |
| `Node-cloneNode-svg` (2) | `Attr` / `attributes` |
| `Node-isEqualNode-xhtml` (1, NOTRUN) | XML documents |
| `ParentNode-replaceChildren` Document cases (6) | `createHTMLDocument` |
| `Node-appendChild-script-and-custom-from-fragment` (1) | Custom elements |
| `NodeList-static-length-getter-tampered-{1,2,3}` (3, TIMEOUT) | Speed: about 127 million `length` getter calls, too many for Boa in a debug build |

### `Attr` and events wave

The suite now also vendors `dom/events`, `dom/collections` and `dom/lists`. Passing
outcomes went from 3408 to 4087 with `Attr`/`NamedNodeMap` and the wider directory set,
then to 4780 / 6544 with the events layer described in [DOM.md](./DOM.md#events):
one `EventTarget` implementation for every target, the `Event` subclasses,
`document.createEvent`, event handler attributes, `window.event` and listener exceptions
reported as `ErrorEvent`.

The wave also fixed two Boa 0.20 bugs that pages can hit:

- The garbage collector traced cycles behind a live `WeakMap` value forever, so a page
  that kept an object in a `WeakMap` aborted with a failed multi-gigabyte allocation on
  its next full collection (navigation triggers one). Fixed in a patched `boa_gc`
  (`third_party/boa_gc/AXIOM_PATCH.md`).
- Named function expressions inside `with` panic ("must be declarative environment").
  Event handler attributes compile inside `with` scopes, so they now compile to
  anonymous functions with a `name` property, which is what the spec's handler function
  is anyway.

Outcomes recorded as regressions in this wave are files that used to time out before
recording any subtest. They now run far enough to report, and they fail on features Axiom
does not have:

| Outcomes | Missing feature |
|----------|-----------------|
| `non-cancelable-when-passive/*` (20 files ERROR, 24 subtests FAIL) | `requestAnimationFrame` and WebDriver input actions (`test_driver.Actions`) |
| `scrolling/*` (49 subtests FAIL) | Scrolling APIs, `scrollend`, input actions |
| `Event-dispatch-click` (TIMEOUT) | Activation behavior for checkbox, radio and `<a>` on script-dispatched clicks |

`dom/nodes/processing-instruction-attributes` (about 130 failing subtests) tests attributes
on processing instructions, a WICG proposal that Axiom does not implement.

## Known limitations

These shape the numbers above and are not hidden by the expectations:

- **Parse errors** are not compared. The html5lib `#errors` sections are ignored, and the
  tree builder does not report parse errors.
- **`scripted_*.dat`** need script execution during tree construction (6 failures, the
  only remaining tree-construction failures).
- **Stale tokenizer tests**: 11 html5lib tokenizer tests expect pre-processing-instruction
  behavior (see above).
- **Lone surrogates** cannot occur in Rust strings (the 4 `unicodeCharsProblematic`
  tokenizer tests).
- The tokenizer suite tests the tokenizer alone. `xmlViolationTests` are excluded
  because they test XML-infoset coercion, not HTML tokenization.
- **XHTML/XML** network responses use `axiom-xml` and create a live XML document realm after
  their atomic parse. Parser-discovered classic external and inline scripts run through the shared
  resource pipeline in document order; incremental XML parsing and parser-paused XML scripts remain
  deferred.
- **SVG documents** (`.svg` tests) run no script, so they never report.
- `.window.js` and `.any.js` tests run as their generated `.window.html` / `.any.html`
  wrappers; **worker variants** are not run.
- **Reference chains** (references with references of their own) are not followed;
  `reftest-wait` is approximated by running the page until its event loop is idle.
- A **file-level testharness PASS** means harness status OK, not that every subtest passed.
- `<iframe>` owns a real child browsing context for HTTP(S) `src`, `srcdoc` and source-less
  documents. The child is sized from the iframe layout box and composited into the parent frame;
  frame removal cancels it. Settled children dispatch `load`, while failed or unsupported frame
  navigations dispatch `error`. `sandbox` blocks scripts unless it carries `allow-scripts`.
  Same-origin `contentDocument` and `contentWindow.document` use a scoped cross-realm proxy for
  basic document queries, node reads and detached `createElement()` factories; cross-origin and
  sandboxed opaque-origin access is denied. `contentWindow.postMessage()` delivers JSON-cloned payloads through the child event
  loop after target-origin filtering. Sandboxed frames require `allow-scripts` for scripts and
  `allow-forms` for form navigation. Full WindowProxy behavior, cross-realm mutation/event
  identity, transferables, sandbox navigation/popups and remaining sandbox tokens remain
  incomplete. Custom elements and `TreeWalker`/`NodeIterator` remain incomplete. `Range`, basic
  `Selection`, inert additional documents and `DOMParser` are implemented.
- WebDriver input actions are still absent. `requestAnimationFrame` and script-dispatched
  activation behavior for the implemented form controls are available.
- `MutationObserver` sees script-driven mutations only; nodes the parser inserts do not
  produce records.

## Where to go next

The failures are grouped so each front has a measurable target:

1. **HTML/DOM**: complete iframe script/rendering semantics, then custom elements and traversal APIs.
2. **CSS/layout**: `display: contents`, blocks inside inlines, table columns and
   `<tbody>`, `run-in` fallbacks.
3. **Real-site snapshots**: a locally served corpus of saved pages with crash, load and
   visual checks, under the same ratchet.
