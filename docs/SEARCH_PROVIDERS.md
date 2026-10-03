# Search Providers (Phase 3 Wave F)

The omnibox never builds a search URL itself. Text classified as a search goes to `SearchProviderService::search_url(query)`, which uses the profile's selected provider. There is no hard-coded engine anywhere in the omnibox path (`crates/axiom-browser/src/search.rs`, `classifier.rs`, `browser.rs`).

## Input classification

`OmniboxInputClassifier::classify` (also exported as `OmniboxInput`) returns `Url(canonical)` or `Search(text)`. The rules apply in order:

1. A leading `?` forces a search (`?rust-lang.org`). So does text wrapped in double quotes.
2. `http://`, `https://` and `axiom://` inputs are URLs if they parse. `file:` and `about:` pass through. Any other `scheme:` (`javascript:`, `data:`, `mailto:`, …) is searched for, never navigated to.
3. Whitespace anywhere means a search (`rust programming language`).
4. Otherwise the text before the first `/`, `?` or `#` is split into host and port. Userinfo (`me@example.com`, almost always an e-mail address) means a search. So does a non-numeric or out-of-range port.
5. The host is navigable when it is one of:
   - `localhost` or `*.localhost`
   - an IPv4 or bracketed IPv6 literal
   - a domain whose suffix is on the Public Suffix List (`rust-lang.org`, `bbc.co.uk` and `münchen.de` navigate; `file.txt` and `node.js` search)
   - a reserved local-use name (`.test`, `.local`, `.internal`, `.example`, `.home.arpa`)
   - a single label followed by a port or `/` (`intranet:8080`, `router/`)

Hosts are canonicalized by `axiom_url::Url::parse`: lowercase, IDNA punycode, and forbidden code points rejected. Local names and IP literals default to `http`. Public domains default to `https`.

## Providers

| Id | Name | Result URL template | Notes |
|----|------|---------------------|-------|
| `google` | Google | `https://www.google.com/search?q={searchTerms}` | |
| `bing` | Bing | `https://www.bing.com/search?q={searchTerms}` | |
| `duckduckgo` | DuckDuckGo | `https://duckduckgo.com/?q={searchTerms}` | **default** (`DEFAULT_PROVIDER_ID`) |
| `axiom` | Axiom Search | `axiom://search?q={searchTerms}` | placeholder page; see below |
| `custom` | user-named | any `http(s)` template containing `{searchTerms}` | `SearchProvider::custom` rejects anything else (`javascript:`, `axiom:`, no placeholder) |

A `SearchProvider` has `id`, `name`, `search_url_template`, `suggest_url_template` and `icon_url`. Templates use the OpenSearch `{searchTerms}` placeholder. The query is `application/x-www-form-urlencoded` UTF-8, so spaces become `+`, and `&`, `=`, `+`, `?`, `#`, quotes and non-ASCII characters are percent-encoded. For example, `c++ & rust?` becomes `q=c%2B%2B+%26+rust%3F`.

Suggestion and icon templates are recorded but **never contacted**. Omnibox suggestions still come only from local history and bookmarks, so keystrokes never leave the machine.

## Per-profile preference

- The choice is stored in `BrowserSettings` in the profile database: `search_provider_id` plus the provider's name and template, so a custom provider can be restored. That is settings schema v2. A v1 profile's stored template is mapped back to a built-in id when it matches one, otherwise to `custom` if valid, otherwise to the default. An unknown id falls back to DuckDuckGo with a logged warning.
- `Browser::set_search_provider(id)` validates the id, saves the settings and re-renders internal pages. An unknown id is an `UnknownSearchProvider` error and the choice is unchanged.
- **Private windows** opened from a profile (`Browser::new_private_inheriting`, and `axiom --private`, which reads the profile's settings read-only with `BrowserDataStore::read_settings`) start with the same provider. Their settings live in an in-memory store: a provider chosen in the private window affects only that window and never reaches the profile (`wave_f_navigation::search_engine_choice_persists_per_profile_and_private_windows_inherit_it`).

## Internal pages

- `axiom://settings` shows the current provider and links to the search settings.
- `axiom://settings/search` lists every provider with the current one marked. Each entry is a link to `axiom://settings/search?provider=<id>`. Following one applies the choice and shows `axiom://settings/search` again, replacing the history entry instead of adding one. The page is trusted internal content, so its links may open internal URLs; web pages cannot (see [NETWORKING.md](NETWORKING.md), "What the browser does with the events").
- `axiom://search?q=…` is the Axiom Search result page. It says plainly that Axiom Search has no index yet, shows the escaped query, and links the same query on the other providers. It never pretends to have results (`wave_f_navigation::axiom_search_is_an_honest_placeholder`). The design for the real engine is [AXIOM_SEARCH_ARCHITECTURE.md](AXIOM_SEARCH_ARCHITECTURE.md).

## Privacy

- A search is a normal top-level navigation to the provider. It gets the provider's cookies from `CookieService`, and a private window uses its own memory-only jar.
- Log lines about navigations (`axiom_nav` target) strip the query string, so the search terms are not written to logs.
- Visit history records the result page URL, as any browser does. Private windows record nothing.

## Tests

| Test | Covers |
|------|--------|
| `classifier::tests::*` (axiom-browser) | URL vs search rules, PSL, IPv4/IPv6, ports, IDN, quotes, `?` prefix, unsafe schemes |
| `search::tests::*` | templates, form encoding, selection, custom template validation, query round trip |
| `settings_repo::tests::*` | v1 → v2 migration, unknown ids, private inheritance |
| `wave_f_navigation::typed_search_goes_to_the_selected_provider_with_an_encoded_query` | end to end with a local provider |
| `wave_f_navigation::typed_ip_and_port_navigates_directly_instead_of_searching` | `127.0.0.1:port/path` is a URL |
| `wave_f_navigation::search_engine_choice_persists_per_profile_and_private_windows_inherit_it` | persistence, settings page link, private isolation |
| `wave_f_navigation::axiom_search_is_an_honest_placeholder` | placeholder page |
