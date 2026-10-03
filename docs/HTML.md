# HTML parsing

HTML is parsed in `axiom-html` (tokenizer + tree builder). Axiom does **not** use a third-party HTML5 parser as the long-term DOM source.

The tree builder (`crates/axiom-html/src/tree_builder.rs`) follows the WHATWG tree-construction algorithm (§13.2.6). It is measured by the html5lib tree-construction suite (1953 / 1959) and the tokenizer by the html5lib tokenizer suite (7017 / 7032). See [COMPATIBILITY.md](COMPATIBILITY.md).

## Coverage matrix

| Area | Status |
|------|--------|
| `<!DOCTYPE>` with public/system ids; quirks / limited-quirks detection (`Document::quirks_mode`) | **SUPPORTED** |
| Elements + attributes | **SUPPORTED** |
| Text nodes, comments | **SUPPORTED** |
| RCDATA / RAWTEXT / script data (`title`, `textarea`, `style`, `script`, …) | **SUPPORTED** |
| Void elements | **SUPPORTED** |
| All insertion modes, optional tags, implied `<html>`/`<head>`/`<body>` | **SUPPORTED** |
| Misnested formatting (active formatting list, Noah's Ark, adoption agency) | **SUPPORTED** |
| `<table>` model with foster parenting, implied `<tbody>`/`<tr>`/`<colgroup>` | **SUPPORTED** (parsing; table layout is separate) |
| `<svg>` / MathML (namespaces, case fixups, integration points, CDATA) | **SUPPORTED** (parsing; not rendered) |
| `<template>` (separate contents fragment, template insertion modes) | **SUPPORTED** (parsing; `template` is `display: none`) |
| `<select>` (current spec: no "in select" modes) | **SUPPORTED** (parsing) |
| Fragment parsing with a context element (`axiom_html::parse_fragment`) | **SUPPORTED** (backs `innerHTML`, `outerHTML`, `insertAdjacentHTML`) |
| Serialization (`axiom_html::serialize_children` / `serialize_outer`, §13.3) | **SUPPORTED** (HTML documents; template contents, raw-text parents, void elements, attribute and text escaping) |
| Scripting flag (`<noscript>` parsing) | **SUPPORTED** (`HtmlParser::with_scripting`; pages always parse with scripting on) |
| Incremental parse (`HtmlParser::feed` / `step`, pause at scripts) | **SUPPORTED** |
| Parse error reporting | **UNSUPPORTED** (recovery is spec-complete; errors are not reported) |
| Processing instructions (`<?target data>`, §13.2.5.72–76; `ProcessingInstruction` DOM node) | **SUPPORTED** (`xml` / `xml-stylesheet` targets become bogus comments, per spec) |
| `<selectedcontent>` (selected option cloned in when popped) | **SUPPORTED** (parser-built trees; later DOM changes do not update it) |
| `document.write` during parsing | **UNSUPPORTED** (ignored) |
| Form owner association | **UNSUPPORTED** |
| `<iframe>` | **UNSUPPORTED** (parsed as an element; no nested browsing context) |
| XHTML / XML documents | **UNSUPPORTED** (parsed as HTML) |

## Integration

```text
  bytes ──► TextDecoder ──► HtmlParser (tokenizer + tree builder) ──► axiom-dom::Document
                                  │
                                  ├── <script> pauses (ParseStep::Script)
                                  └── discovered <link>/<style>/<img>/<base> ──► loader, axiom-css
```

| Feature | Status |
|---------|--------|
| `title` extraction for window chrome | **SUPPORTED** |
| `<meta charset>` influence on decoding | **SUPPORTED** (Wave G `TextDecoder` prescan) |
| `<base href>` for relative URLs | **SUPPORTED** (Wave G) |
| `<link rel=stylesheet>` fetch | **SUPPORTED** (Wave G loader) |

## Forms-related markup

| Element | Status |
|---------|--------|
| `<form>`, `<input>` text/password/checkbox/radio/submit | **PARTIAL** (paint + input Wave A) |
| `<textarea>`, `<select>` | **PLANNED** (parsed; no controls) |
| `<label>` association | **PLANNED** |

Use `demos/forms.html` and `demos/browser-test.html` as manual fixtures.
