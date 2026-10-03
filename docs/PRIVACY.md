# Privacy statement

## Product guarantees

Axiom 1.2.8 has **no AI integration**, telemetry service, analytics SDK, advertising SDK,
tracking beacon, account system, or automatic crash-report uploader. It does not send browser
history, bookmarks, open tabs, profile metadata, cookies, Web Storage, downloads, or private-mode
activity to an Axiom-operated service.

Persistent profile data is stored locally under the profile root. Private profiles use in-memory
repositories and are discarded when the private browser instance closes. Diagnostics are local,
trusted browser pages and are not uploaded.

## Network scope

Normal browsing necessarily contacts the destination a user enters. If the user selects Google,
Bing, DuckDuckGo, or another provider, that provider receives the search request it needs to
answer. Those third-party services have their own privacy policies. Axiom does not add a tracking
identifier, telemetry header, or redirector to those requests.

The current application has no automatic update client. Release acquisition is an explicit user
download from GitHub. Until an update system is built and documented, Axiom will not silently check
for or download updates.

## Limits

This statement describes Axiom's own behavior, not arbitrary webpages, extensions (not yet
implemented), operating-system services, network operators, or a user-installed proxy. Web content
can still request its own resources subject to Axiom's implemented cookie, CORS, CSP, and network
policies.
