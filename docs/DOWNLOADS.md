# Downloads (Phase 3 Wave E foundation)

Crate: `axiom-download`. One `DownloadManager` per browser profile, owned by `Browser` and visible at `axiom://downloads`. There is no download UI beyond that page yet (no shelf, prompts or "open file" actions).

## How a download starts

1. A navigation's response has `Content-Disposition: attachment` (the loader's `ResponseMeta::download`).
2. `BrowsingContext` stops the document request at the response headers. It cancels the request, so the body is never buffered as a page, and records a `DownloadCandidate` (URL, suggested file name, MIME type).
3. `Browser` drains the candidates after every navigation and on every `Browser::tick`, and passes each one to `DownloadManager::start_candidate`. Only `http:` and `https:` URLs are accepted.
4. The manager issues a fresh request of type `ResourceType::Download` (priority low, `CacheMode::NoStore`) on the profile's `RequestScheduler`.

`DownloadManager::start(url, suggested_name)` starts a download directly.

## Pipeline

Downloads are ordinary requests on the profile's scheduler, so they share its cookies (through `CookieService`), TLS verification, redirect rules, connection pool and concurrency limit. There is no separate HTTP client.

- One worker thread per manager waits on commands and on a bounded(64) event channel from the scheduler.
- At most `max_concurrent` downloads (default 3) are active; the rest stay `Queued`.
- The body streams to `<destination>.axiomdownload`. The partial file is renamed to the destination on completion. Nothing is held in memory beyond one chunk.

## States

`Queued → Downloading → Completed`, with `Paused`, `Failed` and `Canceled` as the other outcomes. `DownloadRecord` carries: id, current network request id, URL, final URL, file name, MIME type, expected size (from `Content-Length` when the body is not content-encoded), bytes downloaded, destination, state, start and completion times, a secret-free error, and whether it can be resumed.

## Pause and resume

A download is resumable only if the response has no `Content-Encoding`, has a strong `ETag` or a `Last-Modified`, and has `Accept-Ranges: bytes`. Pausing a non-resumable download is refused.

Resume sends `Range: bytes=<downloaded>-` and `If-Range: <validator>`. A `206` appends to the partial file. A `200` (the resource changed, or ranges were ignored) truncates the partial file and starts again from zero.

## File names

`sanitize_filename(suggested, url_path)`:

- keeps only the last path segment of the suggested name (both `/` and `\` separate), so `../../evil name.bin` becomes `evil name.bin`;
- removes control characters and `< > : " | ? *`;
- trims trailing dots and spaces, rejects names made only of dots, and prefixes a leading dot with `_`;
- prefixes Windows reserved device names (`CON`, `NUL`, `COM1`, …, with or without an extension) with `_`;
- truncates to 200 bytes, keeping the extension;
- falls back to the percent-decoded last URL segment, then to `download`.

`unique_path` then picks `name.ext`, `name (1).ext`, … inside the download directory, skipping names reserved by other active downloads. A sanitized name contains no separator, so the destination is always a direct child of the download directory.

## Directory

1. The `download_directory` browser setting, if set.
2. Otherwise `<profile>/downloads/` for a normal profile.
3. Otherwise (private profile, no setting) `<temp>/axiom-downloads-<profile id>/`.

The directory is created on first use.

## Shutdown and private profiles

`DownloadManager::shutdown` (called from `Browser`'s `Drop`, before the scheduler and network service stop) cancels every running or paused download and deletes its partial file.

- Normal profile: records of finished downloads are kept for the lifetime of the manager. Records are memory-only, so the list is empty after a restart, and unfinished downloads do not resume across restarts.
- Private profile: records are also forgotten. Completed files stay where the user saved them, as in other browsers; unfinished files are removed.

## Tests

- `axiom-download` unit tests: the file name rules (traversal, reserved names, control characters, length, fallback, uniqueness).
- `axiom-download/tests/downloads.rs`: sanitized hostile name, duplicate names, pause/resume with `Range`/`If-Range` checked on the server side, refusing to pause a non-resumable download, cancel removes the partial file, 404 fails, concurrency cap, private and normal shutdown.
- `networking2::navigating_to_an_attachment_hands_off_to_the_download_manager`: end to end through the browser, including the `axiom://downloads` page.
- `networking2::private_window_forgets_downloads_and_deletes_partials`.

## Not implemented

- Download UI (shelf, progress in chrome, open/show-in-folder, prompts for the location).
- Persisting the download list, or resuming across restarts.
- Safe-browsing and dangerous-file checks, and MIME sniffing.
- Downloads started by `<a download>` or by script.
- Per-download bandwidth limits.
