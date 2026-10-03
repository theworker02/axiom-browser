# Axiom Search — Architecture (design only)

**Status: design document. Nothing described here is implemented.** Phase 3 Wave F ships only the provider slot (`axiom` in [SEARCH_PROVIDERS.md](SEARCH_PROVIDERS.md)) and an honest placeholder page at `axiom://search?q=…`. No crawler, index or ranking code exists, and none should be written until it is explicitly scheduled.

The goal is a small, independent, privacy-respecting web search engine that can later answer `axiom://search` queries. It reuses Axiom's own URL parsing, HTML tokenizer and text extraction, so the crawler understands pages the way the browser does.

## 1. Components

```text
            seeds, sitemaps, discovered links
                          │
                ┌─────────▼─────────┐
                │  URL frontier     │  per-host queues, politeness, priority
                └─────────┬─────────┘
                          │  (robots.txt allowed, host budget available)
                ┌─────────▼─────────┐
                │  Fetchers         │  axiom-net NetworkService, crawler UA
                └─────────┬─────────┘
                          │  raw response + headers
                ┌─────────▼─────────┐
                │  Document store   │  compressed bodies, content hash, fetch log
                └─────────┬─────────┘
                          │
                ┌─────────▼─────────┐
                │  Processing       │  parse, canonicalize, dedup, language,
                │                   │  extract text/fields/links, spam signals
                └───┬───────────┬───┘
                    │           │ links → frontier
          ┌─────────▼───┐  ┌────▼──────────────┐
          │ Indexer     │  │ Link graph        │  PageRank (offline)
          │ (segments)  │  └────┬──────────────┘
          └─────┬───────┘       │ static scores
                │               │
          ┌─────▼───────────────▼──┐
          │  Query service         │  parse → retrieve → rank → snippets
          └─────────┬──────────────┘
                    │
             axiom://search?q=…
```

Each box is a separate crate or process with a narrow interface. Fetching and indexing scale on their own, and the query service never touches the network.

## 2. Crawling

### 2.1 Crawler identity

- User-Agent: `AxiomBot/<version> (+https://github.com/theworker02/axiom/blob/main/docs/AXIOM_SEARCH_ARCHITECTURE.md)`. It is distinct from the browser's UA, so site owners can target it in `robots.txt`.
- The crawler has no cookies, no credentials and no JavaScript. It reads only `GET` responses.
- It fetches only `http(s)` URLs on standard or declared ports. It never fetches private address ranges (RFC 1918, loopback, link-local), including DNS answers that resolve there. This prevents SSRF against the crawl infrastructure.

### 2.2 URL frontier

- **Two levels**, following the Mercator design:
  - Front queues are ordered by priority: estimated importance, change rate and time since the last fetch.
  - Back queues are one per host (or registrable domain), each with a `next_allowed_at` time.
  - A heap over back queues yields the next host whose politeness delay has expired.
- **Politeness.** At most one connection per host at a time by default. The delay is `max(robots Crawl-delay, k × last response time, 1 s)`. Delays grow on 429/503, honoring `Retry-After`.
- **Budget.** Each host has a daily fetch budget scaled by its static score, so a single huge site cannot starve the rest.
- **Persistence.** Queues live on disk (an append-only log plus periodic snapshots), so a restart resumes where the crawl stopped.
- **Seen set.** A Bloom filter over canonical URL hashes gives the fast "maybe seen" check, backed by an exact on-disk table.

### 2.3 robots.txt

- The crawler fetches `/robots.txt` per origin (scheme + host + port) before any other URL there. It caches the file for 24 h or for its `Cache-Control` lifetime, whichever is shorter.
- It parses per RFC 9309:
  - groups by `User-agent`, with the most specific match winning and `*` as the fallback
  - `Allow`/`Disallow` with the longest match winning and `Allow` winning ties
  - `*` and `$` wildcards
- `4xx` means everything is allowed. `5xx` or a network error means nothing is allowed until a retry succeeds, and after 30 days of errors the last good copy is used.
- `Crawl-delay` is honored (it is not part of the RFC, but it is widely used). `Sitemap:` lines feed §2.4.
- Pages with `<meta name="robots" content="noindex">` or an `X-Robots-Tag: noindex` header are fetched but not indexed. `nofollow` links are not added to the frontier.

### 2.4 Sitemaps

- The crawler discovers sitemaps from `robots.txt` `Sitemap:` lines and from `/sitemap.xml`. It accepts XML sitemaps, sitemap indexes, gzip and plain text lists, with the protocol's limits (50,000 URLs / 50 MB per file).
- `<lastmod>` feeds the recrawl priority. `<priority>` and `<changefreq>` are hints only, capped so that sites cannot inflate themselves.
- Sitemap URLs must be on the sitemap's own host, unless cross-submission is proven through robots.txt.

### 2.5 Fetching

- Fetching reuses `axiom-net`'s transport: TLS verification always on, redirects handled by the crawler so each hop is checked against robots, size limits and decoding.
- Conditional requests use the stored `ETag` / `Last-Modified`. A `304` counts as "unchanged" for freshness.
- Only `text/html`, `application/xhtml+xml` and `text/plain` are processed at first. Everything else is logged and skipped.

## 3. Processing

### 3.1 URL canonicalization

A URL is canonicalized before it enters the frontier or the index:

1. Parse with `axiom_url` rules: lowercase scheme and host, IDNA punycode, default port removed, dot segments resolved.
2. Remove the fragment.
3. Remove known tracking parameters (`utm_*`, `gclid`, `fbclid`, `mc_eid`, …) and sort the remaining query parameters. This is kept per host when a site is known to be order-sensitive.
4. Normalize the percent-encoding case (`%2f` → `%2F`) and decode unreserved characters.
5. After fetching, prefer `<link rel="canonical">` when it points to the same registrable domain and the target is fetchable and not itself redirected. Otherwise keep the fetched URL. A redirect chain maps every hop to the final URL.

### 3.2 Duplicate and near-duplicate detection

- **Exact duplicates.** A 128-bit hash (BLAKE3, truncated) of the normalized visible text. Documents with the same hash form a cluster, and the cluster's best-ranked URL is the one shown.
- **Near duplicates.** 64-bit SimHash over word 3-shingles, weighted by TF-IDF. Documents within Hamming distance ≤ 3 are candidates, found with the permuted-table technique (four tables of 16-bit blocks). MinHash with LSH over shingle sets is the fallback for long documents, to confirm candidates.
- **Hashing elsewhere.** URL hashes are 64-bit (xxh3) for frontier and seen-set lookups, with the full string stored to resolve collisions. Content hashes also detect "unchanged since last fetch" when there is no validator.

### 3.3 Text and field extraction

The HTML goes through Axiom's own tokenizer and tree builder, and text is extracted from the DOM. The following are dropped: `script`, `style`, `template`, `noscript` (kept as a fallback when the body is empty), hidden elements (`hidden` attribute, and `display:none` inline styles as a first approximation), and navigation boilerplate detected by link density per block.

Fields extracted:

| Field | Source |
|-------|--------|
| `title` | `<title>`, falling back to the first `<h1>` |
| `headings` | `h1`–`h3` text |
| `body` | main-content text after boilerplate removal |
| `anchor` | text of links **pointing to** this document, from other hosts (added at index time) |
| `url` | tokenized host and path (`rust-lang.org/learn` → `rust lang org learn`) |
| `description` | `<meta name="description">`, used for snippets only, never for ranking alone |
| `lang` | §3.4 |
| `published` / `modified` | `<time>`, `article:*` meta, JSON-LD `datePublished`, HTTP `Last-Modified` (lowest trust) |

### 3.4 Language detection

- The detector runs on the extracted body text: character n-gram (1–3) naive Bayes in the style of CLD/lingua, trained on public corpora, over about 100 languages first.
- `<html lang>` and `Content-Language` are priors, not truth. The detector wins when its confidence is high and they disagree.
- The language selects the tokenizer and stemmer and is stored per document for query-time filtering and boosting. Very short texts are marked `und`.

### 3.5 Tokenization

- Unicode normalization (NFKC) and case folding.
- Word segmentation by UAX #29. Dictionary-based segmentation is needed for Chinese, Japanese and Thai, or character bigrams as the first step.
- Language-specific stemming (Snowball) is stored as a separate "stem" posting list, so exact matches can outrank stemmed ones.
- Stop words are **kept** in the index, because phrase queries need them. They are down-weighted at query time.

### 3.6 Spam and quality signals

These signals are computed per document and per host and combined into a `quality` prior, not a hard filter (except for clear abuse):

- keyword stuffing: term-frequency outliers, and text repeated in title, headings and body
- hidden text: text styled to be invisible, very small fonts, text matching the background color (approximated from inline styles)
- doorway and scaled content: high near-duplicate rate across a host, template-only pages
- link schemes: link farms (dense reciprocal subgraphs), sudden inbound-link spikes, off-topic anchor text
- cloaking checks: occasional fetches with the browser UA, compared by content hash
- known-bad lists: malware and phishing hosts are excluded outright

## 4. Index

### 4.1 Inverted index

- **Segments** are immutable, as in Lucene or Tantivy:
  - a term dictionary: an FST from term to postings offset
  - postings lists per field: doc ids delta-encoded in blocks of 128 with bit-packing (PForDelta-style), plus term frequencies and positions for phrase queries
  - per-document stored fields: URL, title, language, dates, static score, and a compressed text excerpt for snippets
  - field-length norms
- **Doc ids** are dense per segment. A global table maps them to canonical URL hashes.
- **Sharding** is by document (hash of the canonical URL) across machines. Every query fans out to all shards and merges their top-k results.

### 4.2 Index updates

- New and changed documents are written to a small in-memory segment, flushed every N seconds or M documents. Deletions are tombstones in a per-segment live-docs bitmap.
- Background merging (tiered policy) combines small segments and drops deleted documents.
- Readers see a consistent snapshot: a list of segments plus live-docs versions. Commits are atomic manifest swaps, so a crash never exposes half an update.
- A document that changes is deleted and re-added under the same canonical URL. A document that returns 404/410 twice is removed. `noindex` removes it at once.

## 5. Ranking

### 5.1 Retrieval and text relevance (BM25F)

- Candidate generation is a WAND / block-max WAND traversal over the query terms' postings to find the top ~1,000 by an upper-bounded BM25 score.
- Scoring uses **BM25F**: term frequencies are combined across fields with per-field weights and length normalization before saturation.

  \[
  \tilde{tf}_{t,d} = \sum_{f} w_f \cdot \frac{tf_{t,d,f}}{1 - b_f + b_f \cdot \frac{len_{d,f}}{avglen_f}}, \qquad
  \text{score}(q,d) = \sum_{t \in q} \text{idf}(t) \cdot \frac{\tilde{tf}_{t,d}}{k_1 + \tilde{tf}_{t,d}}
  \]

  The starting values are \(k_1 = 1.2\), \(b_{body} = 0.75\), \(b_{title} = 0.3\), and weights of title 3.0, headings 2.0, anchor 2.5, URL 1.5 and body 1.0. They are tuned later against judged queries.
- Exact (unstemmed) matches get a bonus over stem-only matches. Phrase and proximity matches get a bonus from the minimal window that covers the query terms.

### 5.2 Static scores (PageRank)

- The link graph comes from extracted links: canonical source to canonical target, with nofollow links and links inside the same host down-weighted.
- PageRank is computed offline by power iteration (damping 0.85; dangling mass redistributed uniformly) over the host graph and the page graph. It converges when the L1 change is below 1e-6, or after 50 iterations.
- The static score combines log(PageRank), the quality prior (§3.6) and a URL-depth penalty.

### 5.3 Freshness

- Each document has an estimated change rate, learned from successive fetch hashes (a Poisson estimate). That rate drives recrawl priority.
- The query parser marks queries as time-sensitive when they contain "latest", "news" or a year, or match trending terms in the query log if one ever exists. Only for those queries does a decay boost apply: \(\exp(-\Delta t / \tau)\) with \(\tau\) of a few days, based on the published or modified date.

### 5.4 Final ranking

\[
\text{final}(q,d) = \alpha \cdot \text{BM25F}(q,d) + \beta \cdot \text{static}(d) + \gamma \cdot \text{proximity}(q,d) + \delta \cdot \text{fresh}(q,d) + \epsilon \cdot \text{lang}(q,d)
\]

This starts as a hand-tuned linear model. A learning-to-rank model such as LambdaMART over the same features comes later, only once there is a judged evaluation set. The results are then diversified with at most two results per host on the first page. Near-duplicate clusters collapse to one result.

## 6. Queries

### 6.1 Query parsing

- The query goes through the same normalization and tokenization as documents, in the language detected from the query (falling back to the user's `Accept-Language`).
- Supported syntax:
  - `"exact phrase"`
  - `-exclude`
  - `site:example.org`
  - `lang:de`
  - `OR` between terms
- Everything else is literal, and unbalanced quotes are treated as text.
- Spelling correction ("did you mean") uses a noisy-channel model over index term frequencies, with edit distance ≤ 2 via a SymSpell-style deletion dictionary. It is suggested, never silently applied.

### 6.2 Snippets

- For each result, the snippet generator looks in the stored text excerpt for the window of about 160 characters with the highest density of query-term matches. It prefers sentence boundaries and uses up to two fragments joined by "…".
- If there is no body match, it falls back to the meta description, then to the start of the main content.
- Matched terms are wrapped in `<b>` after HTML escaping. Snippets are rendered only by the trusted `axiom://search` page, never inserted as raw HTML.

### 6.3 Autocomplete

- Suggestions come from a prefix-completion FST over popular normalized queries and titles, weighted by frequency. A top-k search of the FST returns results in microseconds.
- **Privacy.** The browser does not send keystrokes to Axiom Search by default, which matches today's behavior for every provider. If autocomplete is enabled, requests carry no cookies, no identifiers and only the typed prefix, and the server keeps no per-user logs.

## 7. Privacy and operations

- The query service logs no IP addresses and no user identifiers. It keeps only aggregate query counts, with k-anonymity thresholds before a query can influence suggestions or freshness.
- Requests have no cookies. The service sets no tracking parameters in result links. Results link directly to the target URL, not through a redirector.
- Everything is observable in the same style as `axiom-net`: crawl rate per host, robots denials, fetch errors by kind, index size, merge backlog, and query latency percentiles.
- Operator controls:
  - a per-host crawl-rate override
  - a removal list (legal and abuse)
  - a rebuild-from-document-store path, so ranking changes never require recrawling

## 8. Milestones (when explicitly scheduled)

1. Offline indexer over a fixed local corpus (for example a Wikipedia dump or the Rust docs) → a working `axiom://search` against it. No crawler.
2. A polite single-machine crawler with a small seed list, obeying robots.txt and sitemaps.
3. Link graph, PageRank and quality signals, plus an evaluation set of judged queries.
4. Sharding, continuous updates and freshness.

Every step lands with tests against local fixtures. Like the browser's live smoke tests, crawling the real web is never part of CI.
