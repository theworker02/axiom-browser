# Resource loading (Phase 3 Wave G)

How a document's subresources (stylesheets, scripts, images, fonts) are discovered, requested,
tracked and applied. Every network request goes through the tab's `ResourceLoader` →
the profile's `RequestScheduler` → `NetworkService`; there is no resource-specific HTTP client.

```text
parser / preload scanner / script insertion / stylesheet (@import, @font-face)
  └─ start_resource: resolve URL (base or stylesheet URL) → CSP upgrade-insecure-requests
     → ResourceRegistry::add (ResourceId) → document CSP check (fails the resource if blocked)
       ├─ data: URL          → decoded synchronously
       ├─ file document      → read from disk (relative paths of `file` documents only)
       └─ network            → ContentPolicy → mixed-content check → ResourceRequest
                                (with the CSP redirect check) → ResourceLoader::start
                                → RequestScheduler → NetworkService
  ← loader events (routed by RequestId → DocumentId, stale ones dropped)
  → complete_resource: status, MIME, decode → apply to the node → READY / FAILED
```

## Identity and states

A resource is identified by its `ResourceId`, never by its URL; two elements with the same URL
are two resources. The network request is a separate `RequestId`; the registry maps one to the
other.

`ResourceState` (`axiom-document/src/resource.rs`) has eight states: `DISCOVERED`, `QUEUED`,
`FETCHING`, `AVAILABLE`, `PROCESSING`, `READY`, `FAILED` and `CANCELED`. Transitions are
checked (`can_transition_to`); terminal states never change.

## Registry

`ResourceRegistry` is per document (bounded to 2048 resources; further resources are refused
and counted as rejected). Queries: `get`, `by_request`, `pending`, `failed`, `of_kind`,
`stylesheets`, `scripts`, `images`, `fonts`, `all`. Each record holds the URL and final URL,
type, initiator (`navigation`, `parser`, `preload-scanner`, `script`, `stylesheet`), node,
priority, blocking flags (script, render, load), script kind, status, MIME, cache state,
transferred and decoded bytes, timestamps, failure and, for images, intrinsic size.

## Priorities

| Resource | Priority |
|----------|----------|
| Document | very high |
| Parser-blocking script, stylesheet | high |
| Deferred script, image, font | medium |
| Async or script-inserted script, `loading=lazy` image | low |

`axiom_document::priority_for`; the scheduler already orders by priority with aging.
Evidence: `document_loading::images_track_size_failures_and_lazy_priority`.

## Stylesheets

- `<link rel=stylesheet>` (alternate stylesheets are not requested) and `<style>` are handled
  at their parser position. Stylesheets apply in document order.
- `@import` is resolved against the importing sheet's final URL, fetched through the loader
  (initiator `stylesheet`), nested up to 8 levels, and composed in place before the rule text
  of the importing sheet.
- Parser-inserted stylesheets block scripts (both parser-blocking and deferred scripts wait
  for them) and block rendering, capped at 30 s. Script-inserted ones block neither.
- Failure (network, HTTP status, a non-`text/css` MIME type) marks only that sheet `FAILED`;
  the page keeps loading.

Evidence: `document_loading::stylesheets_imports_fonts_and_failures` (server-observed import
and font paths, import order in the composed CSS, the script waits for the sheet, a 404 sheet
fails alone), `wave_f1::stylesheet_with_wrong_mime_is_ignored`.

## Images

`<img src>` is requested when parsed (or when script sets `src` on a connected image). The body
is decoded; intrinsic width and height are stored on the record (`ImageInfo`); the element gets
a `load` or `error` event. Images never block parsing, scripts or `DOMContentLoaded`; they
block `load` unless `loading=lazy`. A lazy image is still requested now (at low priority);
viewport-based deferral is not implemented.

Evidence: `document_loading::images_track_size_failures_and_lazy_priority` (1×1 intrinsic size,
404 and undecodable images fail with the right class and stage, `data:` image, events).

## Fonts

`@font-face` rules are collected from every loaded stylesheet. A face is requested (through the
loader, relative to its stylesheet) only if some rule uses its family, up to 32 per document.
The body must look like a font file (WOFF, WOFF2, TrueType, OpenType, collection). Fonts block
`load`.

**PARTIAL**: fetched fonts are not used for rendering yet; text still uses the built-in font.
Evidence: `document_loading::stylesheets_imports_fonts_and_failures` (the used face is fetched
from the stylesheet-relative path; the unused one is never requested).

## Dynamic insertion

Inserting a `<script src>`, `<link rel=stylesheet>`, `<img src>` or `<style>` into the
document with `appendChild`, setting `src` on a connected `<img>`, or setting `href`/`rel` on a
`<link>` queues an insertion. The pump starts it through the same `start_resource` path, with
initiator `script`. See
[SCRIPT_LOADING.md](SCRIPT_LOADING.md) for script-inserted scripts.
Evidence: `document_loading::dynamic_insertion_goes_through_the_loader`.

## Preload scanner

While the parser waits for a parser-blocking script, `axiom_document::scan_for_preloads` looks
ahead in the unparsed markup for classic external scripts, stylesheets and non-lazy images
(stopping at `<base>`, skipping comments and rawtext). Hits are requested with initiator
`preload-scanner`. When the parser reaches the element it adopts the matching unclaimed
preload (same type and URL) instead of making a second request. Only `network` documents use
the scanner.

## Deduplication and caching

- There is no in-document memory cache: identical URLs requested by different elements are
  separate resources and separate loader requests. Repeats are served by the profile's HTTP
  cache according to its headers (a fresh response is a `hit`, an ETag response is
  `revalidated`); `no-store` responses are fetched again. This keeps each element's load and
  error semantics separate.
- Each record carries the network cache state (`miss`, `hit`, `stale`, `revalidated`,
  `bypassed`, `not_cacheable`).

Evidence: `document_loading::cache_cold_warm_and_reload` (cold `miss`, warm `hit` with no
server request, ETag `revalidated` with `If-None-Match`, reload revalidates the document).

## Cookies, referrer and origin

Subresource requests carry `from_document` and the top-level URL; cookies come only from the
profile's `CookieService` through the network service. The `Referer` is the document URL.
Evidence: `document_loading::subresource_requests_carry_cookies_from_the_cookie_service`,
`document_loading::private_profile_documents_do_not_share_state`.

## Security checks before a request

1. The document's Content Security Policy (Phase 3 Wave I, [`CSP.md`](CSP.md)): the URL is
   upgraded if a policy has `upgrade-insecure-requests`, then checked against the matching
   fetch directive (`script-src-elem`, `style-src-elem`, `img-src`, `font-src`, `media-src`,
   `manifest-src`, else `connect-src`) with the element's nonce and whether the parser
   inserted it. A block fails the resource (`policy-rejection`, failure text starting
   `csp:`) and no request is made. The request carries a redirect check, so each redirect
   hop is checked in the network service before it is sent.
   `ContentPolicy::check(directive, url, document)`, the embedder hook, runs afterwards; the
   default policy allows everything.
2. Mixed content: in a document delivered over HTTPS, `http:` scripts, stylesheets and fonts are
   blocked (`policy-rejection`); images load and flag the document as mixed.
3. `internal` documents never request anything.
4. Request mode (Phase 3 Wave H), set in `BrowsingContext::request_resource`:

   | Subresource | Mode | Credentials |
   |-------------|------|-------------|
   | `<script>`, `<link rel=stylesheet>`, `<img>` without `crossorigin` | `no-cors` | include |
   | The same with `crossorigin` / `crossorigin=anonymous` (or any other value) | `cors` | same-origin |
   | The same with `crossorigin=use-credentials` | `cors` | include |
   | `<script type=module>` | `cors` | same-origin (include with `crossorigin=use-credentials`) |
   | Static and dynamic module imports | `cors` | same-origin (they do not inherit `use-credentials` from the importing script yet) |
   | `@font-face` fonts | `cors` | same-origin |

   A `cors` subresource from another origin passes the network service's CORS check on
   every hop (see [`FETCH.md`](FETCH.md)); a failure fails the resource at the `network`
   stage with a `cors:` message, so a script fires `error` and does not run, a stylesheet
   does not apply and an image or font is not used. Preload-scanner requests remember their
   CORS state, and the parser only claims a preload made in the same mode. CSS `@import`
   stays `no-cors`.

Evidence: `document_loading::content_policy_blocks_before_any_request` (the server never sees
the blocked script), `axiom-document` `policy::tests` (mixed-content matrix; the engine path
needs an HTTPS fixture and is not covered end to end),
`document_loading::internal_pages_never_fetch_web_resources`,
`wave_h::crossorigin_scripts_and_module_scripts_need_cors_approval`,
`wave_h::crossorigin_images_stylesheets_and_fonts`,
`resource::tests::crossorigin_attribute_states`, `preload::tests::records_the_crossorigin_state`.

## MIME validation

| Type | Rule |
|------|------|
| Stylesheet | A declared MIME type other than `text/css` fails the sheet (`mime`) |
| Script | Image, audio, video and CSV types are refused (`mime`) |
| Image | Must decode (`decode`) |
| Font | Must start with a font signature (`decode`) |

## Cancellation

Replacing or stopping a document cancels its pending loader requests (`CANCELED`, and
`Cancelled` in the network log). Cancellation never evicts or modifies the HTTP cache.
Evidence: `document_loading::obsolete_document_callbacks_never_touch_the_new_document`,
`document_loading::stop_cancels_pending_resources_and_finishes_the_document`.

## Errors

`ResourceError { class, stage, message }`. Classes: `fatal-document` (only this replaces the
page, with the trusted error page), `subresource`, `script`, `stylesheet`, `image`, `font`,
`policy-rejection`. Stages: `policy`, `limit`, `url`, `network`, `http-status`, `mime`,
`decode`, `execution`, `canceled`.

## Limits

| Limit | Value |
|-------|-------|
| Resources per document | 2048 |
| Subresource body bytes held per document while processing | 64 MiB |
| `@import` depth | 8 |
| Font requests per document | 32 |
| Preload hints per scan | 64 |
| Document events kept | 1024 (oldest dropped and counted) |
| Retired document snapshots per tab | 4 |
| Pump steps per call | 100 000 |

Concurrency is the profile scheduler's fixed worker pool; the document adds no threads.
