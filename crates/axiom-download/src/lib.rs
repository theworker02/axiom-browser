//! Download manager.
//!
//! Downloads are ordinary requests on the profile's [`RequestScheduler`] (so they use the
//! profile's cookies, cache policy, TLS verification and connection pool) with
//! [`ResourceType::Download`]. Bodies stream to a `.axiomdownload` partial file next to the
//! destination and are renamed on completion; nothing is buffered in memory.
//!
//! * At most `max_concurrent` downloads run at once; the rest wait in `Queued`.
//! * Pause is offered only when the server supports byte ranges and sent a validator;
//!   resume sends `Range` + `If-Range` and restarts from zero if the server answers `200`.
//! * File names are sanitized (see [`sanitize_filename`]) and made unique inside the
//!   download directory; a name can never escape it.
//! * Download records are memory-only. [`DownloadManager::shutdown`] cancels running
//!   downloads and deletes their partial files; a private manager also forgets its records.

mod filename;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axiom_net::{
    CacheMode, DownloadCandidate, NetworkError, NetworkEvent, NetworkRequest, NetworkRequestId,
    RequestScheduler, ResourceType, ResponseMeta,
};
use axiom_url::Url;
use crossbeam_channel::{bounded, select, unbounded, Receiver, Sender};
use parking_lot::{Condvar, Mutex};

pub use filename::{sanitize_component, sanitize_filename, unique_path};

const PARTIAL_SUFFIX: &str = ".axiomdownload";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DownloadId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    Queued,
    Downloading,
    Paused,
    Completed,
    Failed,
    Canceled,
}

impl DownloadState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Downloading => "downloading",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }

    /// No further progress without user action.
    pub fn is_settled(self) -> bool {
        matches!(
            self,
            Self::Paused | Self::Completed | Self::Failed | Self::Canceled
        )
    }
}

#[derive(Debug, Clone)]
pub struct DownloadRecord {
    pub id: DownloadId,
    /// Current (or last) network request; a resumed download gets a new one.
    pub request_id: Option<NetworkRequestId>,
    pub url: String,
    pub final_url: Option<String>,
    pub filename: Option<String>,
    pub mime: Option<String>,
    /// From `Content-Length` when the body is not content-encoded.
    pub expected_size: Option<u64>,
    pub downloaded: u64,
    pub destination: Option<PathBuf>,
    pub state: DownloadState,
    pub started_at_unix_ms: u64,
    pub completed_at_unix_ms: Option<u64>,
    /// Secret-free failure reason.
    pub error: Option<String>,
    pub resumable: bool,
}

#[derive(Debug, Clone)]
pub struct DownloadConfig {
    pub directory: PathBuf,
    /// Private profile: records are forgotten at shutdown.
    pub private: bool,
    pub max_concurrent: usize,
}

impl DownloadConfig {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            private: false,
            max_concurrent: 3,
        }
    }
}

enum Command {
    Start {
        id: DownloadId,
        url: Url,
        suggested: Option<String>,
    },
    Pause(DownloadId),
    Resume(DownloadId),
    Cancel(DownloadId),
    Shutdown,
}

#[derive(Default)]
struct Records {
    list: Vec<DownloadRecord>,
    next_id: u64,
}

struct Shared {
    records: Mutex<Records>,
    changed: Condvar,
}

impl Shared {
    fn update(&self, id: DownloadId, f: impl FnOnce(&mut DownloadRecord)) {
        let mut g = self.records.lock();
        if let Some(r) = g.list.iter_mut().find(|r| r.id == id) {
            f(r);
        }
        drop(g);
        self.changed.notify_all();
    }

    fn get(&self, id: DownloadId) -> Option<DownloadRecord> {
        self.records
            .lock()
            .list
            .iter()
            .find(|r| r.id == id)
            .cloned()
    }
}

pub struct DownloadManager {
    shared: Arc<Shared>,
    commands: Sender<Command>,
    worker: Option<JoinHandle<()>>,
    config: DownloadConfig,
}

impl DownloadManager {
    pub fn new(scheduler: Arc<RequestScheduler>, config: DownloadConfig) -> Self {
        let shared = Arc::new(Shared {
            records: Mutex::new(Records::default()),
            changed: Condvar::new(),
        });
        let (commands, command_rx) = unbounded();
        let worker = {
            let shared = Arc::clone(&shared);
            let config = config.clone();
            std::thread::Builder::new()
                .name("axiom-downloads".into())
                .spawn(move || Worker::new(scheduler, shared, config).run(command_rx))
                .expect("spawn download worker")
        };
        Self {
            shared,
            commands,
            worker: Some(worker),
            config,
        }
    }

    pub fn config(&self) -> &DownloadConfig {
        &self.config
    }

    /// Queue a download of `url`. `suggested` is an untrusted file name hint.
    pub fn start(&self, url: Url, suggested: Option<String>) -> DownloadId {
        let id = {
            let mut g = self.shared.records.lock();
            g.next_id += 1;
            let id = DownloadId(g.next_id);
            g.list.push(DownloadRecord {
                id,
                request_id: None,
                url: url.as_str(),
                final_url: None,
                filename: None,
                mime: None,
                expected_size: None,
                downloaded: 0,
                destination: None,
                state: DownloadState::Queued,
                started_at_unix_ms: now_ms(),
                completed_at_unix_ms: None,
                error: None,
                resumable: false,
            });
            id
        };
        let _ = self.commands.send(Command::Start { id, url, suggested });
        id
    }

    /// Hand-off from a navigation whose response was a download.
    pub fn start_candidate(&self, candidate: &DownloadCandidate) -> Option<DownloadId> {
        let url = Url::parse(&candidate.url).ok()?;
        if url.scheme != "http" && url.scheme != "https" {
            return None;
        }
        Some(self.start(url, candidate.filename.clone()))
    }

    /// Pause a running download. Only possible for resumable downloads.
    pub fn pause(&self, id: DownloadId) -> bool {
        let ok = self
            .shared
            .get(id)
            .is_some_and(|r| r.state == DownloadState::Downloading && r.resumable);
        if ok {
            let _ = self.commands.send(Command::Pause(id));
        }
        ok
    }

    pub fn resume(&self, id: DownloadId) -> bool {
        let ok = self
            .shared
            .get(id)
            .is_some_and(|r| r.state == DownloadState::Paused);
        if ok {
            let _ = self.commands.send(Command::Resume(id));
        }
        ok
    }

    pub fn cancel(&self, id: DownloadId) {
        let _ = self.commands.send(Command::Cancel(id));
    }

    pub fn get(&self, id: DownloadId) -> Option<DownloadRecord> {
        self.shared.get(id)
    }

    /// All records, oldest first.
    pub fn list(&self) -> Vec<DownloadRecord> {
        self.shared.records.lock().list.clone()
    }

    /// Wait until `id` reaches `state` (or `timeout` passes); returns the latest record.
    pub fn wait_for_state(
        &self,
        id: DownloadId,
        state: DownloadState,
        timeout: Duration,
    ) -> Option<DownloadRecord> {
        self.wait_until(id, timeout, |r| r.state == state)
    }

    /// Wait until `id` is paused, completed, failed or canceled.
    pub fn wait_until_settled(&self, id: DownloadId, timeout: Duration) -> Option<DownloadRecord> {
        self.wait_until(id, timeout, |r| r.state.is_settled())
    }

    fn wait_until(
        &self,
        id: DownloadId,
        timeout: Duration,
        done: impl Fn(&DownloadRecord) -> bool,
    ) -> Option<DownloadRecord> {
        let deadline = Instant::now() + timeout;
        let mut g = self.shared.records.lock();
        loop {
            let rec = g.list.iter().find(|r| r.id == id)?.clone();
            if done(&rec) || Instant::now() >= deadline {
                return Some(rec);
            }
            self.shared.changed.wait_until(&mut g, deadline);
        }
    }

    /// Cancel running downloads and delete their partial files. A private manager also
    /// forgets every record. Idempotent.
    pub fn shutdown(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = self.commands.send(Command::Shutdown);
            let _ = worker.join();
        }
        if self.config.private {
            self.shared.records.lock().list.clear();
        }
    }
}

impl Drop for DownloadManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Active {
    id: DownloadId,
    partial: PathBuf,
    file: Option<File>,
    /// Byte offset requested with `Range` (0 for a fresh download).
    resume_from: u64,
}

struct PausedDownload {
    url: Url,
    partial: PathBuf,
    validator: Option<String>,
    downloaded: u64,
}

struct Worker {
    scheduler: Arc<RequestScheduler>,
    shared: Arc<Shared>,
    config: DownloadConfig,
    events_tx: Sender<NetworkEvent>,
    events_rx: Receiver<NetworkEvent>,
    active: HashMap<NetworkRequestId, Active>,
    queue: VecDeque<(DownloadId, Url, Option<String>)>,
    suggested: HashMap<DownloadId, Option<String>>,
    validators: HashMap<DownloadId, Option<String>>,
    paused: HashMap<DownloadId, PausedDownload>,
    /// Destinations chosen for downloads that have not finished yet.
    reserved: HashSet<PathBuf>,
    shutting_down: bool,
}

impl Worker {
    fn new(scheduler: Arc<RequestScheduler>, shared: Arc<Shared>, config: DownloadConfig) -> Self {
        // Bounded: a slow disk applies backpressure to the transfer instead of buffering.
        let (events_tx, events_rx) = bounded(64);
        Self {
            scheduler,
            shared,
            config,
            events_tx,
            events_rx,
            active: HashMap::new(),
            queue: VecDeque::new(),
            suggested: HashMap::new(),
            validators: HashMap::new(),
            paused: HashMap::new(),
            reserved: HashSet::new(),
            shutting_down: false,
        }
    }

    fn run(mut self, commands: Receiver<Command>) {
        loop {
            select! {
                recv(commands) -> cmd => match cmd {
                    Ok(Command::Shutdown) | Err(_) => break,
                    Ok(cmd) => self.command(cmd),
                },
                recv(self.events_rx) -> ev => {
                    if let Ok(ev) = ev {
                        self.event(ev);
                    }
                }
            }
        }
        self.shutdown();
    }

    fn command(&mut self, cmd: Command) {
        match cmd {
            Command::Start { id, url, suggested } => {
                self.queue.push_back((id, url, suggested));
                self.pump();
            }
            Command::Pause(id) => self.pause(id),
            Command::Resume(id) => self.resume(id),
            Command::Cancel(id) => self.cancel(id),
            Command::Shutdown => {}
        }
    }

    fn running(&self) -> usize {
        self.active.len()
    }

    fn pump(&mut self) {
        while !self.shutting_down && self.running() < self.config.max_concurrent.max(1) {
            let Some((id, url, suggested)) = self.queue.pop_front() else {
                return;
            };
            self.suggested.insert(id, suggested);
            self.dispatch(id, url, None);
        }
    }

    /// Send the request for `id`; `resume` carries the partial file to continue.
    fn dispatch(&mut self, id: DownloadId, url: Url, resume: Option<PausedDownload>) {
        let mut req = NetworkRequest::get(url.clone(), ResourceType::Download);
        req.cache_mode = CacheMode::NoStore;
        req.initiator = "download".into();
        req.top_level_url = Some(url);
        // Stalls are caught by the service's read timeout; large files may take long.
        req.request_timeout_ms = None;
        let (partial, resume_from) = match &resume {
            Some(p) => {
                req.headers.set("Range", format!("bytes={}-", p.downloaded));
                if let Some(v) = &p.validator {
                    req.headers.set("If-Range", v.clone());
                }
                (p.partial.clone(), p.downloaded)
            }
            None => (PathBuf::new(), 0),
        };
        let request_id = req.id;
        self.shared.update(id, |r| {
            r.request_id = Some(request_id);
            r.state = DownloadState::Downloading;
            r.error = None;
        });
        self.active.insert(
            request_id,
            Active {
                id,
                partial,
                file: None,
                resume_from,
            },
        );
        if let Err(e) = self.scheduler.enqueue(req, self.events_tx.clone()) {
            self.active.remove(&request_id);
            self.fail(id, None, &e);
        }
    }

    fn event(&mut self, ev: NetworkEvent) {
        match ev {
            NetworkEvent::Response { id, meta } => self.response(id, &meta),
            NetworkEvent::Data { id, bytes } => self.data(id, &bytes),
            NetworkEvent::Complete { id } => self.complete(id),
            NetworkEvent::Failed { id, error } => {
                if let Some(a) = self.active.remove(&id) {
                    self.fail(a.id, Some(&a.partial), &error);
                    self.pump();
                }
            }
        }
    }

    fn response(&mut self, request: NetworkRequestId, meta: &ResponseMeta) {
        let Some(active) = self.active.get(&request) else {
            return;
        };
        let id = active.id;
        let resume_from = active.resume_from;
        let prior_partial = active.partial.clone();
        let resumed = resume_from > 0 && meta.status == 206;
        if resume_from > 0 && meta.status == 206 {
            let start = meta.content_range.map(|r| r.start);
            if start != Some(resume_from) {
                return self.abort(request, "server resumed at an unexpected offset");
            }
        } else if !(200..300).contains(&meta.status) || meta.status == 206 {
            return self.abort_with(request, &NetworkError::HttpStatus(meta.status));
        }

        let encoded = !meta.content_encoding.is_empty();
        let validator = meta
            .headers
            .get("etag")
            .filter(|e| !e.trim_start().starts_with("W/"))
            .or_else(|| meta.headers.get("last-modified"))
            .map(str::to_string);
        let resumable = !encoded
            && validator.is_some()
            && meta
                .headers
                .get("accept-ranges")
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("bytes"));
        let expected = if encoded {
            None
        } else if resumed {
            meta.content_range
                .and_then(|r| r.complete_length)
                .or(meta.content_length.map(|l| l + resume_from))
        } else {
            meta.content_length
        };

        let (partial, file) = if resumed {
            let partial = prior_partial;
            match OpenOptions::new().append(true).open(&partial) {
                Ok(f) => (partial, f),
                Err(e) => return self.abort(request, &format!("cannot reopen partial file: {e}")),
            }
        } else {
            // Fresh download, or the server ignored our range: start from zero.
            let destination = match self.shared.get(id).and_then(|r| r.destination) {
                Some(d) => d,
                None => {
                    let suggested = meta
                        .download
                        .as_ref()
                        .and_then(|d| d.filename.clone())
                        .or_else(|| self.suggested.get(&id).cloned().flatten());
                    let name = sanitize_filename(suggested.as_deref(), &meta.final_url.path);
                    let reserved = &self.reserved;
                    let dest = unique_path(&self.config.directory, &name, |p| {
                        p.exists() || reserved.contains(p) || partial_path(p).exists()
                    });
                    self.reserved.insert(dest.clone());
                    dest
                }
            };
            let partial = partial_path(&destination);
            if let Err(e) = fs::create_dir_all(&self.config.directory) {
                return self.abort(request, &format!("cannot create download directory: {e}"));
            }
            let file = match File::create(&partial) {
                Ok(f) => f,
                Err(e) => return self.abort(request, &format!("cannot create file: {e}")),
            };
            let filename = destination
                .file_name()
                .map(|n| n.to_string_lossy().into_owned());
            self.shared.update(id, |r| {
                r.destination = Some(destination.clone());
                r.filename = filename;
                r.downloaded = 0;
            });
            (partial, file)
        };
        self.validators.insert(id, validator);
        if let Some(a) = self.active.get_mut(&request) {
            a.partial = partial;
            a.file = Some(file);
        }
        let final_url = meta.final_url.as_str();
        let mime = meta.mime.as_ref().map(|m| m.essence());
        self.shared.update(id, |r| {
            r.final_url = Some(final_url);
            r.mime = mime;
            r.expected_size = expected;
            r.resumable = resumable;
            r.state = DownloadState::Downloading;
        });
    }

    fn data(&mut self, request: NetworkRequestId, bytes: &[u8]) {
        let Some(active) = self.active.get_mut(&request) else {
            return;
        };
        let id = active.id;
        let result = match active.file.as_mut() {
            Some(f) => f.write_all(bytes),
            None => return,
        };
        match result {
            Ok(()) => {
                let n = bytes.len() as u64;
                self.shared.update(id, |r| r.downloaded += n);
            }
            Err(e) => self.abort(request, &format!("write failed: {e}")),
        }
    }

    fn complete(&mut self, request: NetworkRequestId) {
        let Some(mut active) = self.active.remove(&request) else {
            return;
        };
        let id = active.id;
        let flushed = active.file.take().map(|f| f.sync_all()).transpose();
        let Some(destination) = self.shared.get(id).and_then(|r| r.destination) else {
            return self.fail_message(id, Some(&active.partial), "completed without a response");
        };
        let final_dest = if destination.exists() {
            unique_path(
                &self.config.directory,
                &destination
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                |p| p.exists() || self.reserved.contains(p),
            )
        } else {
            destination.clone()
        };
        let renamed = flushed.and_then(|_| fs::rename(&active.partial, &final_dest));
        self.reserved.remove(&destination);
        match renamed {
            Ok(()) => {
                let filename = final_dest
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
                self.shared.update(id, |r| {
                    r.state = DownloadState::Completed;
                    r.destination = Some(final_dest);
                    r.filename = filename;
                    r.completed_at_unix_ms = Some(now_ms());
                });
                self.forget(id);
            }
            Err(e) => self.fail_message(id, Some(&active.partial), &format!("save failed: {e}")),
        }
        self.pump();
    }

    fn pause(&mut self, id: DownloadId) {
        let Some((&request, _)) = self.active.iter().find(|(_, a)| a.id == id) else {
            return;
        };
        let Some(rec) = self.shared.get(id) else {
            return;
        };
        if !rec.resumable {
            return;
        }
        let Some(mut active) = self.active.remove(&request) else {
            return;
        };
        self.scheduler.cancel(request);
        if let Some(f) = active.file.take() {
            let _ = f.sync_all();
        }
        let Ok(url) = Url::parse(rec.final_url.as_deref().unwrap_or(&rec.url)) else {
            return;
        };
        self.paused.insert(
            id,
            PausedDownload {
                url,
                partial: active.partial,
                validator: self.validators.get(&id).cloned().flatten(),
                downloaded: rec.downloaded,
            },
        );
        self.shared.update(id, |r| r.state = DownloadState::Paused);
        self.pump();
    }

    fn resume(&mut self, id: DownloadId) {
        let Some(p) = self.paused.remove(&id) else {
            return;
        };
        let url = p.url.clone();
        self.shared.update(id, |r| r.state = DownloadState::Queued);
        self.dispatch(id, url, Some(p));
    }

    fn cancel(&mut self, id: DownloadId) {
        if let Some(pos) = self.queue.iter().position(|(q, _, _)| *q == id) {
            self.queue.remove(pos);
        }
        if let Some((&request, _)) = self.active.iter().find(|(_, a)| a.id == id) {
            self.scheduler.cancel(request);
            if let Some(a) = self.active.remove(&request) {
                drop(a.file);
                let _ = fs::remove_file(&a.partial);
            }
        }
        if let Some(p) = self.paused.remove(&id) {
            let _ = fs::remove_file(&p.partial);
        }
        let mut settled = false;
        self.shared.update(id, |r| {
            settled = matches!(r.state, DownloadState::Completed | DownloadState::Failed);
            if !settled {
                r.state = DownloadState::Canceled;
                r.completed_at_unix_ms = Some(now_ms());
            }
        });
        if !settled {
            self.release(id);
        }
        self.forget(id);
        self.pump();
    }

    /// Abort the request with a local (I/O or protocol) reason.
    fn abort(&mut self, request: NetworkRequestId, reason: &str) {
        self.scheduler.cancel(request);
        if let Some(a) = self.active.remove(&request) {
            self.fail_message(a.id, Some(&a.partial), reason);
        }
        self.pump();
    }

    fn abort_with(&mut self, request: NetworkRequestId, error: &NetworkError) {
        self.scheduler.cancel(request);
        if let Some(a) = self.active.remove(&request) {
            self.fail(a.id, Some(&a.partial), error);
        }
        self.pump();
    }

    fn fail(&mut self, id: DownloadId, partial: Option<&Path>, error: &NetworkError) {
        let reason = match error {
            NetworkError::HttpStatus(s) => format!("server returned HTTP {s}"),
            e => e.summary().to_string(),
        };
        self.fail_message(id, partial, &reason);
    }

    fn fail_message(&mut self, id: DownloadId, partial: Option<&Path>, reason: &str) {
        if let Some(p) = partial.filter(|p| !p.as_os_str().is_empty()) {
            let _ = fs::remove_file(p);
        }
        log::warn!(target: "axiom_download", "download {} failed: {reason}", id.0);
        let reason = reason.to_string();
        self.shared.update(id, |r| {
            r.state = DownloadState::Failed;
            r.error = Some(reason);
            r.completed_at_unix_ms = Some(now_ms());
        });
        self.release(id);
        self.forget(id);
    }

    fn release(&mut self, id: DownloadId) {
        if let Some(dest) = self.shared.get(id).and_then(|r| r.destination) {
            self.reserved.remove(&dest);
        }
    }

    fn forget(&mut self, id: DownloadId) {
        self.suggested.remove(&id);
        self.validators.remove(&id);
    }

    fn shutdown(&mut self) {
        self.shutting_down = true;
        let running: Vec<DownloadId> = self.active.values().map(|a| a.id).collect();
        let queued: Vec<DownloadId> = self.queue.iter().map(|(id, _, _)| *id).collect();
        let paused: Vec<DownloadId> = self.paused.keys().copied().collect();
        for id in running.into_iter().chain(queued).chain(paused) {
            self.cancel(id);
        }
    }
}

fn partial_path(destination: &Path) -> PathBuf {
    let mut s = destination.as_os_str().to_owned();
    s.push(PARTIAL_SUFFIX);
    PathBuf::from(s)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
