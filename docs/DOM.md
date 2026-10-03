# DOM

Axiom implements its own DOM in `axiom-dom`: an **arena-backed tree** with stable `NodeId` handles. This is not a binding to a foreign engine’s DOM.
Script sees it through `axiom-js`: natives on the `JsHost` trait (implemented by the page in `axiom-engine`) plus the JavaScript interface layer in `crates/axiom-js/src/dom_prelude.js`.

## Data model

| Feature | Status |
|---------|--------|
| Element, text, comment, processing-instruction, doctype, fragment nodes | **SUPPORTED** |
| Ordered, namespaced attributes (namespace, prefix, local name) | **SUPPORTED** |
| Elements in any namespace, with prefixes | **SUPPORTED** |
| Template contents fragments | **SUPPORTED** |
| Quirks mode (affects id/class matching) | **SUPPORTED** |
| `Attr` nodes / `NamedNodeMap` | **SUPPORTED** (identity-stable `Attr` objects; `attributes`, `getAttributeNode(NS)`, `setAttributeNode(NS)`, `removeAttributeNode`, `createAttribute(NS)`) |
| CDATA sections, XML documents | **PARTIAL** — `axiom-xml` preserves CDATA as `CDATASection` nodes; top-level XML/XHTML MIME documents commit an XML DOM after stream completion. Incremental XML parsing remains deferred. |
| Additional documents (`new Document()`, `createDocument`, `createHTMLDocument`, `DOMParser`) | **SUPPORTED** — inert documents share the DOM arena but have no browsing context, loader, script execution or storage access |
| Shadow DOM, custom elements | **UNSUPPORTED** |
| `Range` / basic `Selection` | **SUPPORTED** — live boundary updates for tree, character-data, split and normalization mutations; no geometry APIs or editing integration |
| `TreeWalker` / `NodeIterator` | **PLANNED** |

## Script interfaces

The prototype chain follows the DOM and HTML specs: `Node` → `Element` → `HTMLElement` → per-tag interfaces (`HTMLDivElement`, `HTMLTemplateElement`, …, with `HTMLUnknownElement` for unknown names), plus `SVGElement`, `MathMLElement`, `CharacterData` → `Text` / `Comment` / `ProcessingInstruction`, `DocumentType`, `DocumentFragment` and `Document` → `HTMLDocument`. A node always gets the same wrapper object.

| Area | Status |
|------|--------|
| `Text`, `Comment`, `DocumentFragment` constructors | **SUPPORTED** |
| Factories: `createElement(NS)`, `createTextNode`, `createComment`, `createProcessingInstruction`, `createDocumentFragment`, `implementation.createDocumentType`, `importNode`, `adoptNode` | **SUPPORTED** (name validation throws `InvalidCharacterError`) |
| Tree mutation: `appendChild`, `insertBefore`, `replaceChild`, `removeChild` with the spec's pre-insertion validity checks | **SUPPORTED** (`HierarchyRequestError`, `NotFoundError`, …) |
| `ParentNode` / `ChildNode`: `append`, `prepend`, `replaceChildren`, `before`, `after`, `replaceWith`, `remove`, element-child navigation | **SUPPORTED** |
| `textContent`, `nodeValue`, `cloneNode`, `isEqualNode`, `isSameNode`, `contains`, `compareDocumentPosition`, `normalize`, `getRootNode`, namespace lookups | **SUPPORTED** |
| `CharacterData` methods, `splitText`, `wholeText` | **SUPPORTED** |
| Attribute methods, including the `NS` variants, `toggleAttribute`, `getAttributeNames` | **SUPPORTED** |
| `classList` (`DOMTokenList`) | **SUPPORTED** |
| Live `HTMLCollection` / `NodeList` (`children`, `childNodes`, `getElementsBy*`); static `NodeList` from `querySelectorAll` | **SUPPORTED** |
| `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `insertAdjacentElement`, `insertAdjacentText` | **SUPPORTED** |
| `template.content` | **SUPPORTED** |
| `MutationObserver` / `MutationRecord` | **SUPPORTED** for script mutations (see below) |
| `EventTarget`, `Event` and its subclasses, event handlers, `document.createEvent` | **SUPPORTED** (see [Events](#events)) |
| `disabled` on form controls | **SUPPORTED** (reflects the content attribute; `fieldset` ancestors are not consulted) |
| `dataset`, form-control IDL attributes (`checked`, `value`, …) | **SUPPORTED** (form named properties, file picker and text-selection control APIs remain incomplete) |

Markup set through `innerHTML` and friends is parsed with the fragment algorithm in `axiom-html` (the context element decides the insertion mode). Scripts it creates never run. Serialization is `axiom_html::serialize_children` / `serialize_outer` (see [HTML.md](./HTML.md)).

### MutationObserver

Records are queued for child-list, attribute and character-data changes made from script (including `innerHTML`, `outerHTML`, `insertAdjacentHTML` and `replaceChildren`). This covers old values, `attributeFilter`, `subtree`, and transient observers on removed subtrees. Callbacks run in a microtask, in observer creation order. Nodes inserted by the HTML parser during page load do not produce records.

## Events

One JavaScript implementation of DOM §2 serves every target: nodes, `window`, `AbortSignal` and `performance` (`crates/axiom-js/src/events_prelude.js`). The DOM prelude supplies the event path (a node's parent, then the document, then `window` for every event but `load`).

| Feature | Status |
|---------|--------|
| `EventTarget` constructor; `addEventListener` with `capture`, `once`, `passive`, `signal`; `removeEventListener`; `dispatchEvent` | **SUPPORTED** |
| Dispatch: capture, at-target (capture listeners first) and bubble phases; `stopPropagation`, `stopImmediatePropagation`, `cancelBubble`, `returnValue`, `composedPath`; listeners removed or added during dispatch | **SUPPORTED** |
| Passive by default for `touchstart`, `touchmove`, `wheel`, `mousewheel` on `window`, the document, the root element and `<body>` | **SUPPORTED** |
| `handleEvent` objects (looked up at call time) | **SUPPORTED** |
| `window.event` during listener invocation | **SUPPORTED** |
| `Event` subclasses with constructors and dictionary conversion: `UIEvent`, `FocusEvent`, `MouseEvent`, `WheelEvent`, `PointerEvent`, `DragEvent`, `KeyboardEvent`, `CompositionEvent`, `InputEvent`, `CustomEvent`, `ErrorEvent`, `HashChangeEvent`, `PopStateEvent`, `PageTransitionEvent`, `MessageEvent`, `StorageEvent`, `ProgressEvent`, `PromiseRejectionEvent`, `AnimationEvent`, `TransitionEvent`, `SubmitEvent`, `CloseEvent`, `ToggleEvent`, `DeviceMotionEvent`, `DeviceOrientationEvent`; `TextEvent` and `BeforeUnloadEvent` without constructors | **SUPPORTED** (the legacy `init*Event` methods included) |
| `document.createEvent` with the spec's alias table (`HTMLEvents`, `MouseEvents`, `UIEvents`, …); other names throw `NotSupportedError` | **SUPPORTED** |
| Event handler IDL attributes (`onclick`, …) on HTML, SVG and MathML elements, the document and `window`; `<body>`/`<frameset>` forward the window-reflecting ones to `window` | **SUPPORTED** |
| Event handler content attributes, compiled lazily with the document, form-owner and element scopes; `return false` cancels; `onerror` on `window` gets five arguments | **SUPPORTED** |
| Exceptions in listeners are reported as a cancelable `ErrorEvent` at `window`, then logged unless canceled | **SUPPORTED** |
| Legacy `webkitAnimation*` / `webkitTransitionEnd` fallback for trusted events | **SUPPORTED** |
| Engine clicks: a trusted `MouseEvent`; canceling it skips the default action (link navigation, focus, checkbox toggle) | **SUPPORTED** |
| `HTMLElement.click()` (untrusted `PointerEvent`; disabled controls do nothing) | **SUPPORTED** |
| Activation behavior for script-dispatched clicks (checkbox, radio, `<label>`, submit/reset) | **SUPPORTED**; link navigation remains the engine's trusted-click default action |
| Shadow DOM retargeting | **UNSUPPORTED** (no shadow DOM; `composedPath()` is the whole path) |
| Input automation (touch, wheel, keyboard from WebDriver actions), `requestAnimationFrame` | **UNSUPPORTED** |

## Selectors

`axiom-dom::selector` is a Selectors Level 4 engine used by `querySelector`, `querySelectorAll`, `matches` and `closest`:

- Parsing follows CSS Syntax for identifiers, escapes, strings and comments. Invalid selectors throw `SyntaxError`.
- Combinators; type, class, id and attribute selectors (all operators, plus the `i`/`s` flags).
- `*|` and `|` namespace prefixes.
- Structural pseudo-classes: `:nth-*` with `of S`, `:only-*`, `:empty`, `:root` and `:scope`.
- Logical pseudo-classes: forgiving `:is`/`:where`, `:not`, and relative `:has`.
- Form-state pseudo-classes, `:lang`, `:dir`, `:any-link` and `:defined`.
- Known pseudo-elements parse but never match an element.

The style cascade does not use this engine yet; it has its own matcher (see [CSS.md](./CSS.md)).

## Invalidation

| Feature | Status |
|---------|--------|
| `DirtyFlags` (style / layout / paint) | **SUPPORTED** |
| Automatic pipeline re-run on mutation | **PARTIAL** (Wave A browser shell) |

```text
  mutation ──► mark_dirty ──► browser schedules style/layout/paint
```

## Document helpers

| Helper | Status |
|--------|--------|
| `document_element`, `head`, `body` | **SUPPORTED** |
| Collect inline `<style>` text | **SUPPORTED** |
| Collect `<script>` sources | **SUPPORTED** |
| Collect `<img src>` for loader | **SUPPORTED** |

## Conformance

WPT `dom/nodes`, `dom/events`, `dom/collections` and `dom/lists` run in the `testharness` suite; current numbers and the list of failures that need missing features are in [COMPATIBILITY.md](./COMPATIBILITY.md).
