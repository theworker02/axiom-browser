//! Browsing context: navigation, history, document loading, input.

mod document;
mod resources;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use axiom_document::NavigationId;
use axiom_document::{
    AllowAllPolicy, BrowsingContextId, ContentPolicy, DocumentDiagnostics, DocumentEvent,
    DocumentEventKind, DocumentId, DocumentInfo, DocumentKind, DocumentLifecycle,
    ResourceDiagnostics, ResourceRegistry,
};
use axiom_dom::{DirtyFlags, NodeId};
use axiom_events::Event;
use axiom_layout::Rect;
use axiom_loader::{
    CacheMode, DownloadCandidate, HttpMethod, LoaderError, LoaderEvent, NetworkError, RequestBody,
    RequestId, RequestPriority, ResourceLoader, ResourceRequest, ResourceResponse, TlsInfo,
};
use axiom_net::{
    compute_referrer, parse_http_date, CertificateErrorKind, CookieProvider, CookieRequestContext,
    NetworkService, NetworkServiceConfig, ReferrerPolicy, RequestScheduler, SchedulerConfig,
    TextDecoder,
};
use axiom_trace::TraceTimeline;
use axiom_url::{Origin, Url};

use axiom_js::{FetchResponseHead, TimerRequest};

use crate::cookie_jar::CookieJar;
use crate::event_loop::{MicrotaskQueue, TaskQueue, TimerQueue};
use crate::fetch::{
    data_response, document_origin, filter_response, FetchCommand, FetchPlan, FetchPolicy,
};
use crate::forms::{self, FormSubmission};
use crate::hud::{FrameClock, HudStats};
use crate::keepalive::{KeepaliveLoads, KEEPALIVE_QUOTA};
use crate::lifecycle::{History, HistoryEntry};
use crate::navigation::{
    NavigationCause, NavigationEvent, NavigationEventKind, NavigationEvents, NavigationState,
};
use crate::page::{DocumentMeta, FrameMessage, FrameRealm, HistoryOp, Page};
use crate::web_storage::StorageBinder;

use document::{feed_text, DocumentLoad};
pub use document::{RetiredDocument, ScriptError};

/// How long a blocking navigation waits for its document to become interactive.
const DOCUMENT_TIMEOUT: Duration = Duration::from_secs(120);
/// Replaced documents kept for diagnostics.
const MAX_RETIRED_DOCUMENTS: usize = 4;

/// Navigation failure with a typed kind for the trusted error page.
#[derive(Debug)]
struct NavFailure {
    kind: &'static str,
    message: String,
    /// Secret-free sentence for the error page (network failures).
    summary: Option<&'static str>,
    certificate: Option<CertificateErrorKind>,
}

impl From<String> for NavFailure {
    fn from(message: String) -> Self {
        Self {
            kind: "network",
            message,
            summary: None,
            certificate: None,
        }
    }
}

impl From<LoaderError> for NavFailure {
    fn from(e: LoaderError) -> Self {
        let (summary, certificate) = match &e {
            LoaderError::Network(n) => (
                Some(n.summary()),
                match n {
                    NetworkError::Certificate { kind, .. } => Some(*kind),
                    _ => None,
                },
            ),
            _ => (None, None),
        };
        Self {
            kind: e.kind_name(),
            message: e.to_string(),
            summary,
            certificate,
        }
    }
}

struct PendingNavigation {
    id: NavigationId,
    request: RequestId,
    url: String,
    push_history: bool,
    started: Instant,
    cause: NavigationCause,
    /// The request carries a POST form body.
    post: bool,
    /// The document that started the navigation (link, script or form), the source of
    /// the new document's referrer.
    initiator: Option<Url>,
}

/// How far the committed navigation's lifecycle has been reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ReportedStage {
    Committed,
    Interactive,
    /// `Completed` or `Cancelled` was reported; nothing more follows.
    Done,
}

/// Connection security of the committed document, from the network response itself —
/// never inferred from the URL scheme.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SecurityState {
    /// Blank, internal or local document.
    #[default]
    NotNetwork,
    /// Delivered over TLS with a verified certificate chain.
    Secure(Box<TlsInfo>),
    /// Delivered over plain HTTP.
    Insecure,
    /// The last navigation failed certificate validation (an error page is shown).
    CertificateError(CertificateErrorKind),
}

pub struct BrowsingContext {
    pub page: Page,
    pub history: History,
    pub loader: ResourceLoader,
    pub tasks: TaskQueue,
    pub microtasks: MicrotaskQueue,
    pub timers: TimerQueue,
    /// When animation frame callbacks last ran (frames are paced to ~60 Hz).
    last_animation_frame: Option<Instant>,
    pub clock: FrameClock,
    pub show_hud: bool,
    pub loading: bool,
    pub last_error: Option<String>,
    /// Security of the committed document.
    pub security: SecurityState,
    /// A secure document loaded a subresource over plain HTTP.
    pub mixed_content: bool,
    navigation_seq: u64,
    committed_navigation: Option<NavigationId>,
    pending_navigation: Option<PendingNavigation>,
    /// `navigate`, `reload`, back/forward and link clicks wait for the document (tests,
    /// headless rendering) or return at once (the desktop UI thread).
    blocking_navigation: bool,
    nav_events: NavigationEvents,
    committed_cause: NavigationCause,
    reported: Option<(NavigationId, ReportedStage)>,
    /// Failed or cancelled outcome of the latest navigation, until the next one starts.
    terminal_state: Option<NavigationState>,
    /// URL whose navigation failed; the error page shows it and reload retries it.
    failed_url: Option<String>,
    /// Scroll offset to restore when a history navigation becomes interactive.
    pending_scroll: Option<(NavigationId, f32)>,
    /// Navigations that turned out to be downloads, for the browser's download manager.
    download_requests: Vec<DownloadCandidate>,
    viewport_width: u32,
    viewport_height: u32,
    pub chrome_height: u32,
    cookie_jar: Option<Arc<dyn CookieJar>>,
    storage_binder: Option<StorageBinder>,
    /// Loader runs on a profile-owned scheduler (vs. a private standalone one).
    network_shared: bool,
    /// The committed document's loader state (`None` before the first document).
    document: Option<DocumentLoad>,
    /// Which document each subresource request belongs to: events addressed to any other
    /// document are dropped (the generation guard).
    request_owner: HashMap<RequestId, DocumentId>,
    stale_events_dropped: u64,
    retired_documents: VecDeque<RetiredDocument>,
    context_id: BrowsingContextId,
    owner_tab: Option<u64>,
    owner_profile: Option<String>,
    content_policy: Arc<dyn ContentPolicy>,
    pending_fetches: HashMap<RequestId, PendingFetch>,
    /// Keepalive fetches on the current scheduler; they survive navigation.
    keepalive: Option<KeepaliveLoads>,
    /// Keepalive fetches still running on a scheduler this context no longer uses.
    retired_keepalive: Vec<KeepaliveLoads>,
    /// Embedder-controlled script permission (currently used by iframe sandboxing).
    scripts_enabled: bool,
    /// A sandbox without `allow-forms` cannot create a form-navigation request.
    forms_enabled: bool,
    /// A sandbox without `allow-same-origin` receives an opaque origin for frame access
    /// decisions. Network credential partitioning remains owned by the profile service.
    sandboxed_origin: bool,
    /// Nested documents keyed by their embedding `<iframe>` node. Each child owns an
    /// independent Page, DOM, JS realm, document loader and loader context; only the
    /// profile scheduler/service is shared.
    frames: HashMap<NodeId, BrowsingContext>,
    /// Frame elements whose child context completed and already received its trusted
    /// element `load` event. Reset whenever the embedding changes source.
    frame_load_fired: HashSet<NodeId>,
    /// Frame elements whose child navigation failed and already received its element
    /// `error` event. A failed child must never be reported as a successful `load`.
    frame_error_fired: HashSet<NodeId>,
    /// Parent/frame proxies enqueue cloned message payloads here; they are delivered
    /// only during this context's event-loop turn.
    frame_message_inbox: Rc<RefCell<Vec<FrameMessage>>>,
}

struct PendingFetch {
    policy: FetchPolicy,
    /// Body bytes are forwarded to script (false for opaque and null-body responses).
    deliver_body: bool,
    /// Response withheld until the whole body passed the integrity check.
    held: Option<(FetchResponseHead, Vec<u8>)>,
}

impl BrowsingContext {
    pub fn new(viewport_width: u32, viewport_height: u32) -> Self {
        let trace = Arc::new(TraceTimeline::new());
        let loader = ResourceLoader::new(trace.clone());
        Self::build(viewport_width, viewport_height, 48, loader, false)
    }

    /// A context without browser chrome on an existing scheduler (headless rendering).
    pub fn headless(
        viewport_width: u32,
        viewport_height: u32,
        scheduler: Arc<RequestScheduler>,
    ) -> Self {
        let trace = Arc::new(TraceTimeline::new());
        let loader = ResourceLoader::shared(scheduler, trace);
        Self::build(viewport_width, viewport_height, 0, loader, true)
    }

    fn build(
        viewport_width: u32,
        viewport_height: u32,
        chrome_height: u32,
        loader: ResourceLoader,
        network_shared: bool,
    ) -> Self {
        let trace = Arc::clone(&loader.trace);
        let content_h = viewport_height.saturating_sub(chrome_height);
        let page = Page::blank(viewport_width, content_h, trace);
        Self {
            page,
            history: History::new(),
            loader,
            tasks: TaskQueue::new(),
            microtasks: MicrotaskQueue::new(),
            timers: TimerQueue::new(),
            last_animation_frame: None,
            clock: FrameClock::default(),
            show_hud: true,
            loading: false,
            last_error: None,
            security: SecurityState::NotNetwork,
            mixed_content: false,
            navigation_seq: 0,
            committed_navigation: None,
            pending_navigation: None,
            blocking_navigation: true,
            nav_events: NavigationEvents::default(),
            committed_cause: NavigationCause::Embedder,
            reported: None,
            terminal_state: None,
            failed_url: None,
            pending_scroll: None,
            download_requests: Vec::new(),
            viewport_width,
            viewport_height,
            chrome_height,
            cookie_jar: None,
            storage_binder: None,
            network_shared,
            document: None,
            request_owner: HashMap::new(),
            stale_events_dropped: 0,
            retired_documents: VecDeque::new(),
            context_id: BrowsingContextId::next(),
            owner_tab: None,
            owner_profile: None,
            content_policy: Arc::new(AllowAllPolicy),
            pending_fetches: HashMap::new(),
            keepalive: None,
            retired_keepalive: Vec::new(),
            scripts_enabled: true,
            forms_enabled: true,
            sandboxed_origin: false,
            frames: HashMap::new(),
            frame_load_fired: HashSet::new(),
            frame_error_fired: HashSet::new(),
            frame_message_inbox: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Hand over keepalive fetches that are still in flight (the context is going away);
    /// the caller must keep polling them until they finish.
    pub fn take_keepalive(&mut self) -> Vec<KeepaliveLoads> {
        self.retire_keepalive();
        std::mem::take(&mut self.retired_keepalive)
    }

    /// Keepalive fetches still in flight (including those of previous documents).
    pub fn keepalive_in_flight(&self) -> usize {
        self.keepalive
            .iter()
            .map(KeepaliveLoads::len)
            .sum::<usize>()
            + self
                .retired_keepalive
                .iter()
                .map(KeepaliveLoads::len)
                .sum::<usize>()
    }

    fn keepalive_in_flight_bytes(&self) -> u64 {
        self.keepalive
            .iter()
            .chain(&self.retired_keepalive)
            .map(KeepaliveLoads::in_flight_bytes)
            .sum()
    }

    fn retire_keepalive(&mut self) {
        if let Some(loads) = self.keepalive.take() {
            if !loads.is_empty() {
                self.retired_keepalive.push(loads);
            }
        }
    }

    /// Attach the profile's scheduler (and through it the profile's network service,
    /// cache and cookie authority). Replaces any standalone loader.
    pub fn set_network(&mut self, scheduler: Arc<RequestScheduler>) {
        if self.network_shared && Arc::ptr_eq(self.loader.scheduler(), &scheduler) {
            return;
        }
        let trace = Arc::clone(&self.loader.trace);
        self.cancel_pending_loads();
        self.retire_keepalive();
        self.loader = ResourceLoader::shared(scheduler, trace);
        self.network_shared = true;
    }

    pub fn set_cookie_jar(&mut self, jar: Option<Arc<dyn CookieJar>>) {
        self.cookie_jar = jar.clone();
        self.page.cookie_jar = jar.clone();
        if self.network_shared {
            // The profile network service already talks to the profile cookie authority.
            return;
        }
        let trace = Arc::clone(&self.loader.trace);
        let mut service = NetworkService::new(NetworkServiceConfig::default());
        if let Some(j) = jar {
            service = service.with_cookies(Arc::new(JarCookieProvider(j)));
        }
        let scheduler = Arc::new(RequestScheduler::new(
            Arc::new(service),
            SchedulerConfig::default(),
        ));
        self.cancel_pending_loads();
        self.retire_keepalive();
        self.loader = ResourceLoader::shared(scheduler, trace);
    }

    /// Subresources of the current document that have not reached a terminal state.
    pub fn pending_subresources(&self) -> usize {
        self.document
            .as_ref()
            .map_or(0, |d| d.registry.pending().count())
    }

    /// Nothing left for the event loop to do: no navigation, fetches, subresources,
    /// keepalive requests, timeouts, animation frames or module graphs pending, and the
    /// document finished loading. Intervals repeat forever, so they do not keep it busy.
    pub fn is_idle(&self) -> bool {
        self.pending_navigation.is_none()
            && !self.page.shared.forms.borrow().has_pending_submission()
            && !self.page.shared.csp.has_pending()
            && self.document.as_ref().is_none_or(DocumentLoad::is_settled)
            && self.pending_fetch_count() == 0
            && self.frames.values().all(BrowsingContext::is_idle)
            && self.pending_subresources() == 0
            && self.keepalive_in_flight() == 0
            && !self.timers.has_one_shot()
            && self.page.js.as_ref().is_none_or(|js| {
                !js.has_module_work()
                    && !js.animation_frame_requested()
                    && !js
                        .timer_requests
                        .borrow()
                        .iter()
                        .any(|r| matches!(r, TimerRequest::Set { repeat: false, .. }))
            })
    }

    /// Run the event loop until the document is idle or `timeout` elapsed. Returns whether
    /// it went idle.
    pub fn run_until_idle(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.tick();
            if self.is_idle() {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            // Wakes on the next network event; the short cap keeps timers responsive.
            let events = self
                .loader
                .wait_next((deadline - now).min(Duration::from_millis(2)));
            self.process_events(events);
        }
    }

    /// Wait (event-driven) until every subresource of the document settled or `timeout`
    /// elapsed.
    pub fn wait_for_subresources(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.poll_loader();
            if self.pending_subresources() == 0 {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let events = self
                .loader
                .wait_next((deadline - now).min(Duration::from_millis(50)));
            self.process_events(events);
        }
    }

    /// Stop or network swap: cancel every request of this context. The document stays,
    /// with its pending resources canceled.
    fn cancel_pending_loads(&mut self) {
        if let Some(nav) = self.pending_navigation.take() {
            self.nav_events
                .push(nav.id, &nav.url, nav.cause, NavigationEventKind::Cancelled);
            self.terminal_state = Some(NavigationState::Cancelled);
        }
        self.loader.cancel_all_for_context();
        self.pending_fetches.clear();
        self.page.fetch_queue.borrow_mut().clear();
        self.request_owner.clear();
        self.cancel_document_resources("loads canceled");
        self.frames.clear();
        self.frame_load_fired.clear();
        self.frame_error_fired.clear();
        self.page.frame_registry.borrow_mut().clear();
    }

    /// Replace the active document: cancel its outstanding work (except `keep`), clear its
    /// timers and tasks, and keep a diagnostic snapshot.
    fn retire_document(&mut self, keep: &[RequestId], reason: &str) {
        self.loader.cancel_all_except(keep);
        self.pending_fetches.clear();
        self.page.fetch_queue.borrow_mut().clear();
        self.timers = TimerQueue::new();
        self.last_animation_frame = None;
        self.tasks = TaskQueue::new();
        self.microtasks = MicrotaskQueue::new();
        // Child contexts are owned by the embedding document. Dropping them closes
        // their ResourceLoaders, so obsolete frames cannot commit into a replacement.
        self.frames.clear();
        self.frame_load_fired.clear();
        self.frame_error_fired.clear();
        self.page.frame_registry.borrow_mut().clear();
        if let Some(old) = self.document.take() {
            let retired = old.retire(reason, self.stale_events_dropped);
            self.retired_documents.push_back(retired);
            while self.retired_documents.len() > MAX_RETIRED_DOCUMENTS {
                self.retired_documents.pop_front();
            }
        }
        // Keep ownership of recent documents' requests so their late events are counted
        // as stale instead of vanishing silently.
        let recent: HashSet<DocumentId> = self
            .retired_documents
            .iter()
            .map(|d| d.diagnostics.document)
            .collect();
        self.request_owner
            .retain(|id, owner| keep.contains(id) || recent.contains(owner));
    }

    /// The next document's `navigator` reports what the network service sends.
    fn sync_page_identity(&mut self) {
        let config = self.loader.scheduler().service().config();
        self.page.user_agent = config.user_agent.clone();
        self.page.accept_language = config.accept_language.clone();
    }

    /// Start a new document in this context (the page realm, DOM and timers are fresh).
    fn begin_document(
        &mut self,
        url: &str,
        kind: DocumentKind,
        navigation: Option<NavigationId>,
        started: Instant,
    ) {
        self.page.viewport_height = self.content_height();
        self.page.viewport_width = self.viewport_width;
        self.page.scroll_y = 0.0;
        self.sync_page_identity();
        self.page.begin_document(url);
        let info = DocumentInfo {
            id: DocumentId::next(),
            navigation,
            browsing_context: self.context_id,
            tab: self.owner_tab,
            profile: self.owner_profile.clone(),
            origin: if self.sandboxed_origin {
                Origin::opaque()
            } else {
                Origin::of_document(url)
            },
            url: url.to_string(),
            kind,
        };
        self.document = Some(DocumentLoad::new(
            info,
            started,
            std::rc::Rc::clone(&self.page.shared),
        ));
    }

    /// Record the tab and profile that own this context (document diagnostics).
    pub fn set_owner(&mut self, tab: Option<u64>, profile: Option<String>) {
        self.owner_tab = tab;
        self.owner_profile = profile;
    }

    /// Content policy consulted before every subresource request (embedder rules; the
    /// document's own Content Security Policy is enforced separately).
    pub fn set_content_policy(&mut self, policy: Arc<dyn ContentPolicy>) {
        self.content_policy = policy;
    }

    pub fn set_scripts_enabled(&mut self, enabled: bool) {
        self.scripts_enabled = enabled;
    }

    pub fn set_forms_enabled(&mut self, enabled: bool) {
        self.forms_enabled = enabled;
    }

    /// Mark this nested context as an opaque-origin sandbox for cross-realm access.
    pub fn set_sandboxed_origin(&mut self, sandboxed: bool) {
        self.sandboxed_origin = sandboxed;
    }

    /// The active document's reported Content Security Policy violations, oldest first.
    pub fn csp_violations(&self) -> Vec<axiom_csp::Violation> {
        self.page.shared.csp.violations()
    }

    pub fn browsing_context_id(&self) -> BrowsingContextId {
        self.context_id
    }

    pub fn document_id(&self) -> Option<DocumentId> {
        self.document.as_ref().map(|d| d.info.id)
    }

    pub fn document_info(&self) -> Option<&DocumentInfo> {
        self.document.as_ref().map(|d| &d.info)
    }

    /// The real child browsing context for an embedding element, if it has begun a
    /// supported `src` navigation. This is intentionally an engine diagnostic/API,
    /// not a JS `contentDocument` shortcut across realms.
    pub fn frame_context(&self, element: NodeId) -> Option<&BrowsingContext> {
        self.frames.get(&element)
    }

    pub fn frame_context_mut(&mut self, element: NodeId) -> Option<&mut BrowsingContext> {
        self.frames.get_mut(&element)
    }

    pub fn document_lifecycle(&self) -> Option<&DocumentLifecycle> {
        self.document.as_ref().map(|d| &d.lifecycle)
    }

    pub fn resource_registry(&self) -> Option<&ResourceRegistry> {
        self.document.as_ref().map(|d| &d.registry)
    }

    pub fn document_diagnostics(&self) -> Option<DocumentDiagnostics> {
        self.document
            .as_ref()
            .map(|d| d.diagnostics(self.stale_events_dropped))
    }

    pub fn resource_diagnostics(&self) -> Vec<ResourceDiagnostics> {
        self.document
            .as_ref()
            .map(|d| d.registry.all().iter().map(Into::into).collect())
            .unwrap_or_default()
    }

    /// Structured events of the current document, oldest first.
    pub fn document_events(&self) -> Vec<DocumentEvent> {
        self.document
            .as_ref()
            .map(|d| d.events.events().cloned().collect())
            .unwrap_or_default()
    }

    pub fn script_errors(&self) -> &[ScriptError] {
        self.document
            .as_ref()
            .map_or(&[], |d| d.script_errors.as_slice())
    }

    /// Snapshots of recently replaced documents, oldest first.
    pub fn retired_documents(&self) -> &VecDeque<RetiredDocument> {
        &self.retired_documents
    }

    /// Loader events addressed to replaced documents that were dropped.
    pub fn stale_events_dropped(&self) -> u64 {
        self.stale_events_dropped
    }

    /// `fetch()` calls still in flight for the current document.
    pub fn pending_fetch_count(&self) -> usize {
        self.pending_fetches.len() + self.page.fetch_queue.borrow().commands.len()
    }

    /// Start (or abort) the fetches scripts queued since the last turn of the event loop.
    fn start_queued_fetches(&mut self) {
        let commands: Vec<FetchCommand> = self
            .page
            .fetch_queue
            .borrow_mut()
            .commands
            .drain(..)
            .collect();
        let aborted: HashSet<u64> = commands
            .iter()
            .filter_map(|c| match c {
                FetchCommand::Abort(id) => Some(*id),
                FetchCommand::Start(_) => None,
            })
            .collect();
        for command in commands {
            match command {
                FetchCommand::Start(plan) if aborted.contains(&plan.id()) => {}
                FetchCommand::Start(FetchPlan::Network {
                    request, policy, ..
                }) => {
                    let id = if policy.keepalive {
                        let body_len = request.body.len().unwrap_or(0);
                        if self.keepalive_in_flight_bytes() + body_len > KEEPALIVE_QUOTA {
                            log::warn!(
                                "fetch {}: keepalive quota ({KEEPALIVE_QUOTA} bytes) exceeded",
                                request.url
                            );
                            self.fail_fetch_now(request.id.0);
                            continue;
                        }
                        let scheduler = Arc::clone(self.loader.scheduler());
                        let trace = Arc::clone(&self.loader.trace);
                        self.keepalive
                            .get_or_insert_with(|| KeepaliveLoads::new(scheduler, trace))
                            .start(*request, body_len)
                    } else {
                        self.loader.start(*request)
                    };
                    self.pending_fetches.insert(
                        id,
                        PendingFetch {
                            policy,
                            deliver_body: false,
                            held: None,
                        },
                    );
                }
                FetchCommand::Start(FetchPlan::Data {
                    id,
                    url,
                    mime,
                    body,
                    integrity,
                }) => {
                    if integrity.as_ref().is_some_and(|i| !i.matches(&body)) {
                        log::warn!("fetch({url}): body does not match its integrity metadata");
                        self.fail_fetch_now(id);
                        continue;
                    }
                    let Some(js) = self.page.js.as_mut() else {
                        continue;
                    };
                    let result = js
                        .fetch_response(id, &data_response(&url, &mime))
                        .and_then(|_| js.fetch_chunk(id, body))
                        .and_then(|_| js.fetch_complete(id));
                    if let Err(e) = result {
                        log::warn!("fetch({url}): {e}");
                    }
                }
                FetchCommand::Abort(id) => {
                    let id = RequestId(id);
                    if self.pending_fetches.remove(&id).is_some() {
                        self.cancel_fetch_load(id);
                    }
                }
            }
        }
    }

    fn cancel_fetch_load(&mut self, id: RequestId) {
        match self.keepalive.as_mut().filter(|k| k.contains(id)) {
            Some(loads) => loads.cancel(id),
            None => self.loader.cancel(id),
        }
    }

    /// Reject a fetch that never reached the network.
    fn fail_fetch_now(&mut self, id: u64) {
        self.page.fetch_queue.borrow_mut().finish(id);
        if let Some(js) = self.page.js.as_mut() {
            if let Err(e) = js.fetch_fail(id) {
                log::warn!("fetch callback: {e}");
            }
        }
    }

    /// Deliver a loader event for a pending fetch to script.
    fn deliver_fetch_event(&mut self, ev: LoaderEvent) {
        let id = ev.id();
        let terminal = ev.is_terminal();
        let result = match ev {
            LoaderEvent::Response(resp) => {
                let Some(fetch) = self.pending_fetches.get_mut(&id) else {
                    return;
                };
                let head = match filter_response(&resp, &fetch.policy) {
                    Ok(head) => head,
                    Err(reason) => {
                        log::warn!("fetch {}: {reason}", resp.url.origin());
                        self.pending_fetches.remove(&id);
                        self.cancel_fetch_load(id);
                        self.page.fetch_queue.borrow_mut().finish(id.0);
                        let result = self.with_js(|js| js.fetch_fail(id.0));
                        if let Err(e) = result {
                            log::warn!("fetch callback: {e}");
                        }
                        return;
                    }
                };
                fetch.deliver_body = matches!(head.response_type, "basic" | "cors");
                if fetch.policy.integrity.is_none() {
                    self.with_js(|js| js.fetch_response(id.0, &head))
                } else if fetch.deliver_body {
                    fetch.held = Some((head, Vec::new()));
                    Ok(())
                } else {
                    // An opaque body cannot be checked against integrity metadata.
                    log::warn!(
                        "fetch {}: integrity cannot be verified for an opaque response",
                        resp.url
                    );
                    self.pending_fetches.remove(&id);
                    self.cancel_fetch_load(id);
                    self.page.fetch_queue.borrow_mut().finish(id.0);
                    self.with_js(|js| js.fetch_fail(id.0))
                }
            }
            LoaderEvent::Chunk { bytes, .. } => {
                let Some(fetch) = self.pending_fetches.get_mut(&id) else {
                    return;
                };
                if let Some((_, body)) = &mut fetch.held {
                    body.extend_from_slice(&bytes);
                    Ok(())
                } else if fetch.deliver_body {
                    self.with_js(|js| js.fetch_chunk(id.0, bytes))
                } else {
                    // Never reaches script, so it is consumed on arrival.
                    if let Some(flow) = self.page.fetch_queue.borrow().flows.get(&id.0) {
                        flow.release(bytes.len() as u64);
                    }
                    Ok(())
                }
            }
            LoaderEvent::Completed(resp) => {
                let fetch = self.pending_fetches.remove(&id);
                self.page.network_requests += 1;
                self.page.network_bytes += resp.transferred_bytes;
                match fetch.and_then(|f| f.policy.integrity.zip(f.held)) {
                    Some((integrity, (head, body))) => {
                        if integrity.matches(&body) {
                            self.with_js(|js| {
                                js.fetch_response(id.0, &head)?;
                                if !body.is_empty() {
                                    js.fetch_chunk(id.0, body)?;
                                }
                                js.fetch_complete(id.0)
                            })
                        } else {
                            log::warn!(
                                "fetch {}: body does not match its integrity metadata",
                                resp.url
                            );
                            self.with_js(|js| js.fetch_fail(id.0))
                        }
                    }
                    None => self.with_js(|js| js.fetch_complete(id.0)),
                }
            }
            LoaderEvent::Failed { url, error, .. } => {
                self.pending_fetches.remove(&id);
                if error != NetworkError::Cancelled {
                    log::warn!("fetch {url} failed: {error}");
                }
                self.with_js(|js| js.fetch_fail(id.0))
            }
        };
        if terminal {
            self.page.fetch_queue.borrow_mut().finish(id.0);
        }
        if let Err(e) = result {
            log::warn!("fetch callback: {e}");
        }
    }

    /// Run `f` against the document's script context and flush console output.
    fn with_js(
        &mut self,
        f: impl FnOnce(&mut axiom_js::JsContext) -> Result<(), axiom_js::JsEngineError>,
    ) -> Result<(), axiom_js::JsEngineError> {
        let Some(js) = self.page.js.as_mut() else {
            return Ok(());
        };
        let result = f(js);
        for msg in js.take_console() {
            log::info!("[console] {msg}");
        }
        result
    }

    /// Tell script which streaming uploads have room again.
    fn notify_upload_drains(&mut self) {
        let ids = self.page.fetch_queue.borrow_mut().take_drained_uploads();
        for id in ids {
            if let Err(e) = self.with_js(|js| js.fetch_upload_drained(id)) {
                log::warn!("fetch upload callback: {e}");
            }
        }
    }

    pub fn set_storage_binder(&mut self, binder: Option<StorageBinder>) {
        self.storage_binder = binder.clone();
        self.page.storage_binder = binder;
    }

    pub fn content_height(&self) -> u32 {
        self.viewport_height.saturating_sub(self.chrome_height)
    }

    pub fn navigate(&mut self, url_str: &str) {
        self.navigate_to(url_str);
    }

    /// Navigate to `url_str`, waiting for the document only in blocking mode (see
    /// [`set_blocking_navigation`](Self::set_blocking_navigation)).
    pub fn navigate_to(&mut self, url_str: &str) -> NavigationId {
        self.navigate_with(url_str, true, CacheMode::Default, NavigationCause::Embedder)
    }

    /// Reload revalidates cached responses (Fetch cache mode `no-cache`). After a failed
    /// navigation it retries the URL that failed, not the error page.
    pub fn reload(&mut self) {
        if self.history.current().is_some_and(|e| e.form_post) {
            self.block_form_resubmission(NavigationCause::Reload);
            return;
        }
        let url = self
            .failed_url
            .clone()
            .unwrap_or_else(|| self.page.url.clone());
        self.navigate_with(&url, false, CacheMode::NoCache, NavigationCause::Reload);
    }

    /// Reload / back / forward onto a POST result: show a trusted page instead of
    /// silently sending the form again. The entry stays marked, so this repeats.
    fn block_form_resubmission(&mut self, cause: NavigationCause) {
        let Some(url) = self.history.current().map(|e| e.url.as_str()) else {
            return;
        };
        self.navigation_seq += 1;
        let id = NavigationId(self.navigation_seq);
        self.nav_events
            .push(id, &url, cause, NavigationEventKind::Started);
        let failure = NavFailure {
            kind: "form_resubmission",
            message: format!("not resending form data to {}", redacted_url(&url)),
            summary: Some(
                "This page was the result of a form submission. Axiom does not send form \
                 data again automatically; submit the form again to repeat it.",
            ),
            certificate: None,
        };
        self.fail_navigation(id, failure, &url, false, cause);
        self.history.set_form_post(true);
        self.loading = false;
    }

    /// Blocking (the default): navigation calls return once the document is interactive.
    /// Non-blocking: they return at once and the document commits from
    /// [`poll_loader`](Self::poll_loader) / [`tick`](Self::tick); progress is reported
    /// through [`take_navigation_events`](Self::take_navigation_events). Link clicks,
    /// reload and back/forward follow the same mode.
    pub fn set_blocking_navigation(&mut self, blocking: bool) {
        self.blocking_navigation = blocking;
    }

    pub fn is_blocking_navigation(&self) -> bool {
        self.blocking_navigation
    }

    fn navigate_with(
        &mut self,
        url_str: &str,
        push_history: bool,
        cache_mode: CacheMode,
        cause: NavigationCause,
    ) -> NavigationId {
        let id = self.begin_navigation(url_str, push_history, cache_mode, cause);
        if self.blocking_navigation {
            self.wait_for_document(id, DOCUMENT_TIMEOUT);
        }
        id
    }

    /// Start a navigation without waiting, whatever the blocking mode. A network document
    /// commits from [`poll_loader`](Self::poll_loader) when its response arrives — unless a
    /// newer navigation started in the meantime, which cancels this one. Local and
    /// fragment navigations commit immediately.
    pub fn start_navigation(&mut self, url_str: &str) -> NavigationId {
        self.begin_navigation(url_str, true, CacheMode::Default, NavigationCause::Embedder)
    }

    /// Lifecycle events recorded since the last call, oldest first.
    pub fn take_navigation_events(&mut self) -> Vec<NavigationEvent> {
        self.nav_events.take()
    }

    /// Navigation events discarded because nobody drained them.
    pub fn navigation_events_dropped(&self) -> u64 {
        self.nav_events.dropped()
    }

    /// Where the latest navigation stands.
    pub fn navigation_state(&self) -> NavigationState {
        if self.pending_navigation.is_some() {
            return NavigationState::Connecting;
        }
        if let Some(state) = self.terminal_state {
            return state;
        }
        match &self.document {
            Some(d)
                if d.info.navigation.is_some()
                    && d.info.navigation == self.committed_navigation =>
            {
                if d.lifecycle.canceled {
                    NavigationState::Cancelled
                } else if d.body_request.is_some() {
                    NavigationState::Receiving
                } else if !d.lifecycle.dom_content_loaded_fired {
                    NavigationState::Parsing
                } else if !d.lifecycle.load_fired {
                    NavigationState::Interactive
                } else {
                    NavigationState::Completed
                }
            }
            Some(_) => NavigationState::Completed,
            None => NavigationState::Idle,
        }
    }

    /// URL of the navigation waiting for its response, if any.
    pub fn pending_url(&self) -> Option<&str> {
        self.pending_navigation.as_ref().map(|p| p.url.as_str())
    }

    pub fn pending_navigation_id(&self) -> Option<NavigationId> {
        self.pending_navigation.as_ref().map(|p| p.id)
    }

    /// URL of the navigation whose error page is shown, if the latest one failed.
    pub fn failed_url(&self) -> Option<&str> {
        self.failed_url.as_deref()
    }

    /// The address to show for the committed document: the failed URL on an error page,
    /// otherwise the document URL.
    pub fn display_url(&self) -> String {
        self.failed_url
            .clone()
            .unwrap_or_else(|| self.page.url.clone())
    }

    /// Navigation waiting for its network response, if any.
    pub fn pending_navigation(&self) -> Option<NavigationId> {
        self.pending_navigation.as_ref().map(|p| p.id)
    }

    /// The navigation whose document (or error page) is currently shown.
    pub fn committed_navigation(&self) -> Option<NavigationId> {
        self.committed_navigation
    }

    /// Downloads detected at navigation time since the last call.
    pub fn take_download_requests(&mut self) -> Vec<DownloadCandidate> {
        std::mem::take(&mut self.download_requests)
    }

    fn begin_navigation(
        &mut self,
        url_str: &str,
        push_history: bool,
        cache_mode: CacheMode,
        cause: NavigationCause,
    ) -> NavigationId {
        self.begin_navigation_with(url_str, push_history, cache_mode, cause, None)
    }

    fn begin_navigation_with(
        &mut self,
        url_str: &str,
        push_history: bool,
        cache_mode: CacheMode,
        cause: NavigationCause,
        form: Option<&FormSubmission>,
    ) -> NavigationId {
        self.navigation_seq += 1;
        let id = NavigationId(self.navigation_seq);
        let trimmed = url_str.trim();
        if trimmed.len() >= 6 && trimmed[..6].eq_ignore_ascii_case("axiom:") {
            // Internal pages are trusted browser UI: never read from disk or the network
            // by the page's context. The embedder applies its own policy.
            let initiator = self.page.url.clone();
            self.nav_events.push(
                id,
                trimmed,
                cause,
                NavigationEventKind::InternalRequested { initiator },
            );
            return id;
        }
        if let Some(old) = self.pending_navigation.take() {
            log::debug!("navigation {} superseded by {}", old.id.0, id.0);
            self.loader.cancel(old.request);
            self.nav_events
                .push(old.id, &old.url, old.cause, NavigationEventKind::Cancelled);
        }
        self.loading = true;
        self.last_error = None;
        self.terminal_state = None;
        self.nav_events
            .push(id, trimmed, cause, NavigationEventKind::Started);
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            match Url::parse(trimmed) {
                Ok(url) => {
                    let mut request = ResourceRequest::document(url)
                        .with_cache_mode(cache_mode)
                        .with_priority(RequestPriority::VeryHigh);
                    request.stream_body = true;
                    if form.is_some() {
                        request.redirect_check = self.page.shared.csp.form_action_redirect_check();
                    }
                    if matches!(cause, NavigationCause::Link | NavigationCause::Script) {
                        request.document_url = document_origin(&self.page.url);
                    }
                    let post = form.is_some_and(|f| apply_form_submission(&mut request, f));
                    let initiator = request.document_url.clone();
                    let request = self.loader.start(request);
                    self.pending_navigation = Some(PendingNavigation {
                        id,
                        request,
                        url: trimmed.to_string(),
                        push_history,
                        started: Instant::now(),
                        cause,
                        post,
                        initiator,
                    });
                    return id;
                }
                Err(e) => {
                    let failure = NavFailure {
                        kind: "url",
                        message: e.to_string(),
                        summary: Some(NetworkError::Url(String::new()).summary()),
                        certificate: None,
                    };
                    self.fail_navigation(id, failure, trimmed, push_history, cause);
                }
            }
        } else {
            let result = if url_str.starts_with('#') {
                self.navigate_inner(url_str, push_history)
            } else {
                self.load_file_document(url_str, push_history, id)
            };
            match result {
                Ok(()) => {
                    self.committed_navigation = Some(id);
                    self.committed_cause = cause;
                    self.reported = Some((id, ReportedStage::Committed));
                    let url = self.page.url.clone();
                    self.nav_events.push(
                        id,
                        &url,
                        cause,
                        NavigationEventKind::Committed { redirected: false },
                    );
                    self.report_progress();
                }
                Err(e) => self.fail_navigation(id, e, url_str, push_history, cause),
            }
        }
        self.loading = false;
        id
    }

    /// Report `Interactive` and `Completed` for the committed navigation once its document
    /// reaches them, and keep `loading` true until it is interactive.
    fn report_progress(&mut self) {
        let committed = self.committed_navigation;
        let Some(doc) = self.document.as_ref() else {
            self.loading = self.pending_navigation.is_some();
            return;
        };
        let current = doc.info.navigation.filter(|id| Some(*id) == committed);
        let mut interactive_now = None;
        if let Some(id) = current {
            let (dcl, load, canceled) = (
                doc.lifecycle.dom_content_loaded_fired,
                doc.lifecycle.load_fired,
                doc.lifecycle.canceled,
            );
            let url = doc.info.url.clone();
            let mut stage = match self.reported {
                Some((rid, s)) if rid == id => s,
                _ => ReportedStage::Committed,
            };
            let cause = self.committed_cause;
            if stage < ReportedStage::Interactive && dcl {
                self.nav_events
                    .push(id, &url, cause, NavigationEventKind::Interactive);
                stage = ReportedStage::Interactive;
                interactive_now = Some(id);
            }
            if stage < ReportedStage::Done && load {
                self.nav_events
                    .push(id, &url, cause, NavigationEventKind::Completed);
                stage = ReportedStage::Done;
            } else if stage < ReportedStage::Done && canceled {
                self.nav_events
                    .push(id, &url, cause, NavigationEventKind::Cancelled);
                stage = ReportedStage::Done;
            }
            self.reported = Some((id, stage));
            self.loading = self.pending_navigation.is_some()
                || (stage < ReportedStage::Interactive && !canceled);
        } else {
            self.loading = self.pending_navigation.is_some();
        }
        if let Some(id) = interactive_now {
            if let Some((sid, y)) = self.pending_scroll.take() {
                if sid == id {
                    self.page.set_scroll_y(y);
                }
            }
        }
    }

    /// Whether navigation `id` needs nothing more before a blocking `navigate` returns:
    /// its document is interactive (parsed, `DOMContentLoaded` fired) with no
    /// render-blocking stylesheet pending, or it failed or was superseded.
    fn navigation_settled(&self, id: NavigationId) -> bool {
        if self.pending_navigation.is_some() {
            return self.pending_navigation.as_ref().is_some_and(|p| p.id != id);
        }
        match &self.document {
            Some(d) if d.info.navigation == Some(id) => {
                d.lifecycle.canceled
                    || (d.lifecycle.dom_content_loaded_fired
                        && d.lifecycle.blocking_stylesheets.is_empty())
            }
            _ => true,
        }
    }

    /// Drive the event loop (event-driven, no sleeps) until navigation `id` settled.
    fn wait_for_document(&mut self, id: NavigationId, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            self.poll_loader();
            if self.navigation_settled(id) {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                if let Some(nav) = self.pending_navigation.take() {
                    if nav.id == id {
                        self.loader.cancel(nav.request);
                        self.fail_navigation(
                            id,
                            LoaderError::from(NetworkError::Timeout).into(),
                            &nav.url,
                            nav.push_history,
                            nav.cause,
                        );
                    } else {
                        self.pending_navigation = Some(nav);
                    }
                }
                break;
            }
            let events = self
                .loader
                .wait_next((deadline - now).min(Duration::from_millis(50)));
            self.process_events(events);
        }
        self.loading = self.pending_navigation.is_some();
        self.page.update_rendering_if_needed();
    }

    /// Loader event for the pending navigation's document request.
    fn on_navigation_event(&mut self, ev: LoaderEvent) {
        let Some(nav) = self.pending_navigation.take() else {
            return;
        };
        match ev {
            LoaderEvent::Response(resp) => self.commit_network_document(nav, *resp),
            LoaderEvent::Completed(resp) => {
                // A non-streamed response (should not happen): commit and parse it whole.
                let body = resp.body.clone();
                let request = nav.request;
                self.commit_network_document(nav, (*resp).clone());
                self.on_body_event(LoaderEvent::Chunk {
                    id: request,
                    bytes: body,
                });
                self.on_body_event(LoaderEvent::Completed(resp));
            }
            LoaderEvent::Failed { error, .. } => {
                self.fail_navigation(
                    nav.id,
                    LoaderError::from(error).into(),
                    &nav.url,
                    nav.push_history,
                    nav.cause,
                );
                self.loading = false;
            }
            LoaderEvent::Chunk { .. } => self.pending_navigation = Some(nav),
        }
    }

    /// Response headers of a navigation arrived: hand downloads off, otherwise commit a
    /// new document and stream its body into the parser.
    fn commit_network_document(&mut self, nav: PendingNavigation, resp: ResourceResponse) {
        if let Some(download) = &resp.download {
            // The current document and history stay as they are.
            self.loader.cancel(nav.request);
            self.count_response(&resp);
            self.download_requests.push(download.clone());
            self.nav_events.push(
                nav.id,
                &resp.url.as_str(),
                nav.cause,
                NavigationEventKind::Download,
            );
            self.loading = false;
            return;
        }
        let security = match &resp.tls {
            Some(tls) if tls.certificate_verified => SecurityState::Secure(Box::new(tls.clone())),
            _ => SecurityState::Insecure,
        };
        let final_url = resp.url.as_str();
        let xml_document = resp.mime.as_ref().is_some_and(|m| {
            matches!(
                m.essence().as_str(),
                "application/xhtml+xml" | "application/xml" | "text/xml" | "image/svg+xml"
            )
        });
        let text_wrapper = matches!(
            &resp.mime,
            Some(m) if !m.is_html() && !xml_document && m.type_ == "text"
        );
        let xhtml = resp
            .mime
            .as_ref()
            .is_some_and(|m| m.essence() == "application/xhtml+xml");
        let charset = resp.mime.as_ref().and_then(|m| m.charset.clone());
        self.retire_document(&[nav.request], "navigation committed");
        self.security = security;
        self.mixed_content = false;
        self.begin_document(&final_url, DocumentKind::Network, Some(nav.id), nav.started);
        let csp = &self.page.shared.csp;
        for value in resp.headers.get_all("content-security-policy") {
            csp.add_header(value, axiom_csp::Disposition::Enforce);
        }
        for value in resp.headers.get_all("content-security-policy-report-only") {
            csp.add_header(value, axiom_csp::Disposition::Report);
        }
        self.page.xhtml = xhtml;
        *self.page.shared.meta.borrow_mut() = DocumentMeta {
            referrer: compute_referrer(
                ReferrerPolicy::default(),
                nav.initiator.as_ref().map(Url::as_str).as_deref(),
                &resp.url,
            )
            .unwrap_or_default(),
            // `Document.contentType` reports the response MIME type even when Axiom
            // does not yet have a dedicated renderer for that document family (for
            // example image documents). The parser choice remains below.
            content_type: resp
                .mime
                .as_ref()
                .map_or_else(String::new, axiom_net::MimeType::essence),
            character_set: String::new(),
            last_modified: resp.headers.get("last-modified").and_then(parse_http_date),
        };
        let doc = self.document.as_mut().expect("document just began");
        doc.body_request = Some(nav.request);
        doc.text_wrapper = text_wrapper;
        doc.xml_document = xml_document;
        doc.xml_source = xml_document.then(String::new);
        doc.decoder = if text_wrapper || xml_document {
            TextDecoder::new(charset.as_deref())
        } else {
            TextDecoder::for_html(charset.as_deref())
        };
        doc.lifecycle.timeline.response_start = Some(Instant::now());
        if text_wrapper {
            doc.parser.feed("<!DOCTYPE html><html><body><pre>");
        }
        self.request_owner.insert(nav.request, doc.info.id);
        self.push_or_replace_history(&final_url, nav.push_history);
        let still_post = resp
            .redirect_chain
            .last()
            .is_none_or(|r| r.method_out == HttpMethod::Post);
        self.history.set_form_post(nav.post && still_post);
        self.committed_navigation = Some(nav.id);
        self.committed_cause = nav.cause;
        self.failed_url = None;
        self.reported = Some((nav.id, ReportedStage::Committed));
        self.nav_events.push(
            nav.id,
            &final_url,
            nav.cause,
            NavigationEventKind::Committed {
                redirected: !resp.redirect_chain.is_empty(),
            },
        );
    }

    /// Streamed body of the active document.
    fn on_body_event(&mut self, ev: LoaderEvent) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        match ev {
            LoaderEvent::Chunk { bytes, .. } => {
                if doc.lifecycle.timeline.first_bytes.is_none() {
                    doc.lifecycle.timeline.first_bytes = Some(Instant::now());
                }
                let text = doc.decoder.decode(&bytes);
                note_character_set(&self.page.shared.meta, &doc.decoder);
                if let Some(source) = doc.xml_source.as_mut() {
                    source.push_str(&text);
                } else {
                    feed_text(doc, &text, false);
                }
            }
            LoaderEvent::Completed(resp) => {
                doc.body_request = None;
                let tail = doc.decoder.finish();
                note_character_set(&self.page.shared.meta, &doc.decoder);
                if let Some(source) = doc.xml_source.as_mut() {
                    source.push_str(&tail);
                    let source = std::mem::take(source);
                    let xhtml = self.page.xhtml;
                    let parsed = axiom_xml::parse_for_dom_parser(&source);
                    // XML parsing is atomic, so release the loader borrow before
                    // rebinding the document realm and scheduling its resources.
                    doc.parser.finish();
                    let _ = doc;
                    self.page.replace_document(parsed, xhtml);
                    self.discover_xml_document();
                } else {
                    feed_text(doc, &tail, true);
                }
                self.count_response(&resp);
            }
            LoaderEvent::Failed { error, .. } => {
                doc.body_request = None;
                if error == NetworkError::Cancelled {
                    let tail = doc.decoder.finish();
                    if let Some(source) = doc.xml_source.as_mut() {
                        source.push_str(&tail);
                        doc.parser.finish();
                    } else {
                        feed_text(doc, &tail, true);
                    }
                    return;
                }
                let (nav, url) = (doc.info.navigation, doc.info.url.clone());
                let failure: NavFailure = LoaderError::from(error).into();
                let cause = self.committed_cause;
                match nav {
                    Some(id) => self.fail_navigation(id, failure, &url, false, cause),
                    None => log::warn!("document body failed: {}", failure.message),
                }
                self.loading = false;
            }
            LoaderEvent::Response(_) => {}
        }
    }

    /// Show the trusted error page. Session history gets an entry for the URL that was
    /// tried (pushed for a new navigation, replaced for reload/back/forward), so back
    /// returns to the previous page and reload retries the failed URL.
    fn fail_navigation(
        &mut self,
        id: NavigationId,
        e: NavFailure,
        url: &str,
        push_history: bool,
        cause: NavigationCause,
    ) {
        self.last_error = Some(e.message.clone());
        log::error!("navigation failed: {}", e.message);
        // Trusted, locally generated error surface — never raw network bytes.
        let html = trusted_network_error_html(&e, url);
        let _ = self.load_local_document("axiom://network-error", &html, url, push_history);
        self.security = match e.certificate {
            Some(kind) => SecurityState::CertificateError(kind),
            None => SecurityState::NotNetwork,
        };
        self.committed_navigation = Some(id);
        self.committed_cause = cause;
        self.failed_url = Some(url.to_string());
        self.terminal_state = Some(NavigationState::Failed);
        self.reported = Some((id, ReportedStage::Done));
        self.nav_events
            .push(id, url, cause, NavigationEventKind::Failed { kind: e.kind });
    }

    pub fn back(&mut self) {
        self.traverse_history(-1);
    }

    pub fn forward(&mut self) {
        self.traverse_history(1);
    }

    fn apply_entry(&mut self, entry: &HistoryEntry) {
        let url = entry.url.as_str();
        if url.starts_with("axiom://") {
            // Internal pages are rendered by the browser layer (`Browser::back`), which
            // reads this URL and replaces the document.
            if let Some(nav) = self.pending_navigation.take() {
                self.loader.cancel(nav.request);
                self.nav_events
                    .push(nav.id, &nav.url, nav.cause, NavigationEventKind::Cancelled);
            }
            self.failed_url = None;
            self.terminal_state = None;
            self.page.url = url;
            self.page.set_scroll_y(entry.scroll_y);
            return;
        }
        if entry.form_post {
            self.block_form_resubmission(NavigationCause::History);
            return;
        }
        let id = self.begin_navigation(&url, false, CacheMode::Default, NavigationCause::History);
        self.pending_scroll = Some((id, entry.scroll_y));
        self.report_progress();
        if self.blocking_navigation {
            self.wait_for_document(id, DOCUMENT_TIMEOUT);
        }
    }

    /// Load provided HTML (internal pages / tests) with optional history push.
    pub fn load_local_html(
        &mut self,
        url: &str,
        html: &str,
        push_history: bool,
    ) -> Result<(), String> {
        self.load_local_document(url, html, url, push_history)
    }

    /// A trusted local document at `url` whose session history entry is `history_url`.
    fn load_local_document(
        &mut self,
        url: &str,
        html: &str,
        history_url: &str,
        push_history: bool,
    ) -> Result<(), String> {
        if let Some(nav) = self.pending_navigation.take() {
            self.loader.cancel(nav.request);
            self.nav_events
                .push(nav.id, &nav.url, nav.cause, NavigationEventKind::Cancelled);
        }
        self.failed_url = None;
        self.terminal_state = None;
        self.retire_document(&[], "internal page loaded");
        self.security = SecurityState::NotNetwork;
        self.mixed_content = false;
        self.page.viewport_height = self.content_height();
        self.page.viewport_width = self.viewport_width;
        self.page.scroll_y = 0.0;
        self.sync_page_identity();
        let started = Instant::now();
        self.page.load_html(url, html)?;
        let info = DocumentInfo {
            id: DocumentId::next(),
            navigation: None,
            browsing_context: self.context_id,
            tab: self.owner_tab,
            profile: self.owner_profile.clone(),
            origin: if self.sandboxed_origin {
                Origin::opaque()
            } else {
                Origin::of_document(url)
            },
            url: url.to_string(),
            kind: DocumentKind::Internal,
        };
        let mut doc = DocumentLoad::new(info, started, std::rc::Rc::clone(&self.page.shared));
        let now = Instant::now();
        doc.lifecycle.start_parsing(started);
        doc.lifecycle.finish_parsing(now);
        doc.lifecycle.fire_dom_content_loaded(now);
        doc.lifecycle.begin_load(now);
        doc.lifecycle.finish_load(now);
        doc.events.record(
            DocumentEventKind::LoadFired,
            None,
            None,
            "internal document",
        );
        self.document = Some(doc);
        self.page.document.borrow_mut().dirty = DirtyFlags::all();
        self.page.update_rendering_if_needed();
        self.push_or_replace_history(history_url, push_history);
        Ok(())
    }

    fn push_or_replace_history(&mut self, final_url: &str, push_history: bool) {
        let url = Url::parse(final_url).ok().or_else(|| {
            final_url
                .starts_with("file:")
                .then(|| Url::parse("https://file.local/").ok())
                .flatten()
        });
        if let Some(url) = url {
            let entry = HistoryEntry {
                url,
                title: self.page.title.clone(),
                scroll_y: 0.0,
                form_post: false,
                document_key: self.page.shared.document_key,
                state: None,
            };
            if push_history {
                self.history.push(entry);
            } else {
                self.history.replace(entry);
            }
        }
        self.sync_history_mirror();
    }

    /// Script-visible session history (`history.length`, `history.state`) of the live document.
    fn sync_history_mirror(&self) {
        let shared = &self.page.shared;
        shared.history_length.set(self.history.len().max(1));
        shared.history_index.set(self.history.index());
        let state = self
            .history
            .current()
            .filter(|e| e.document_key == shared.document_key)
            .and_then(|e| e.state);
        shared.history_state.set(state);
    }

    /// Apply the `history` / `location` requests script queued this turn. Requests after a
    /// cross-document navigation belong to a document that is going away.
    fn apply_history_ops(&mut self) {
        let ops: Vec<HistoryOp> = self
            .page
            .shared
            .history_ops
            .borrow_mut()
            .drain(..)
            .collect();
        for op in ops {
            let left_document = match op {
                HistoryOp::Push {
                    url,
                    state,
                    replace,
                } => {
                    self.commit_same_document_entry(&url, Some(state), !replace);
                    false
                }
                HistoryOp::Traverse(delta) => self.traverse_history(delta),
                HistoryOp::Navigate { url, replace } => self.script_navigate(&url, replace),
                HistoryOp::Reload => {
                    self.reload();
                    true
                }
            };
            if left_document {
                break;
            }
        }
    }

    /// A session history entry for the live document at `url` (`pushState`, fragment
    /// navigation); the document itself stays.
    fn commit_same_document_entry(&mut self, url: &str, state: Option<u64>, push: bool) {
        let Ok(parsed) = Url::parse(url) else {
            return;
        };
        self.history.update_scroll(self.page.scroll_y);
        let entry = HistoryEntry {
            url: parsed,
            title: self.page.title.clone(),
            scroll_y: self.page.scroll_y,
            form_post: false,
            document_key: self.page.shared.document_key,
            state,
        };
        if push {
            self.history.push(entry);
        } else {
            self.history.replace(entry);
        }
        self.page.url = url.to_string();
        self.page.shared.url.replace(url.to_string());
        self.sync_history_mirror();
    }

    /// Move `delta` entries through session history. Entries of the live document are
    /// traversed in place (`popstate`, `hashchange`); others load their document. Returns
    /// whether the document is being replaced.
    fn traverse_history(&mut self, delta: i32) -> bool {
        self.history.update_scroll(self.page.scroll_y);
        let Some(entry) = self.history.go(delta).cloned() else {
            return false;
        };
        if entry.document_key != self.page.shared.document_key {
            self.apply_entry(&entry);
            return true;
        }
        let old = std::mem::replace(&mut self.page.url, entry.url.as_str());
        let new = self.page.url.clone();
        self.page.shared.url.replace(new.clone());
        self.sync_history_mirror();
        self.page.set_scroll_y(entry.scroll_y);
        if let Some(js) = self.page.js.as_mut() {
            let hash_change = (fragment(&old) != fragment(&new)).then_some((&*old, &*new));
            if let Err(e) = js.fire_popstate(entry.state, hash_change) {
                log::warn!("popstate: {e}");
            }
        }
        false
    }

    /// `location` navigation from script. Returns whether the document is being replaced.
    fn script_navigate(&mut self, url: &str, replace: bool) -> bool {
        if url
            .trim_start()
            .get(..11)
            .is_some_and(|s| s.eq_ignore_ascii_case("javascript:"))
        {
            return false;
        }
        if is_fragment_navigation(&self.page.url, url) {
            self.navigate_to_fragment(url, !replace);
            return false;
        }
        self.navigate_with(url, !replace, CacheMode::Default, NavigationCause::Script);
        true
    }

    /// Navigate the live document to `url`, which differs from it at most in the fragment:
    /// a new session history entry (unless the URL is unchanged), scroll to the target,
    /// then `hashchange`.
    fn navigate_to_fragment(&mut self, url: &str, push: bool) {
        let old = self.page.url.clone();
        self.commit_same_document_entry(url, None, push && old != url);
        self.scroll_to_fragment(fragment(url).unwrap_or(""));
        if fragment(&old) != fragment(url) {
            if let Some(js) = self.page.js.as_mut() {
                if let Err(e) = js.fire_hashchange(&old, url) {
                    log::warn!("hashchange: {e}");
                }
            }
        }
    }

    /// Scroll to the indicated part of the document: the element with that id (or the
    /// first `<a name>`), or the top for an empty fragment / `top`.
    fn scroll_to_fragment(&mut self, fragment: &str) {
        let decoded = axiom_url::percent_decode(fragment);
        let target = {
            let doc = self.page.document.borrow();
            let anchors: Vec<_> = doc
                .document_element()
                .map(|root| doc.descendants(root))
                .unwrap_or_default()
                .into_iter()
                .filter(|&n| doc.tag_name(n) == Some("a"))
                .collect();
            [fragment, decoded.as_str()]
                .into_iter()
                .filter(|name| !name.is_empty())
                .find_map(|name| {
                    doc.get_element_by_id(name).or_else(|| {
                        anchors
                            .iter()
                            .copied()
                            .find(|&n| doc.attr(n, "name") == Some(name))
                    })
                })
        };
        let y = match target {
            Some(id) => {
                self.page.update_rendering_if_needed();
                let rect = self
                    .page
                    .render()
                    .layout
                    .as_ref()
                    .and_then(|l| l.node_rect(id));
                match rect {
                    Some(rect) => rect.y,
                    None => return,
                }
            }
            None if fragment.is_empty() || decoded.eq_ignore_ascii_case("top") => 0.0,
            None => return,
        };
        let max_scroll = (self.page.content_height - self.page.viewport_height as f32).max(0.0);
        self.page.set_scroll_y(y.clamp(0.0, max_scroll));
    }

    fn navigate_inner(&mut self, url_str: &str, push_history: bool) -> Result<(), NavFailure> {
        if url_str.starts_with('#') {
            let url = resolve_navigation(&self.page.url, url_str);
            self.navigate_to_fragment(&url, push_history);
            return Ok(());
        }
        self.navigate_with(
            url_str,
            push_history,
            CacheMode::Default,
            NavigationCause::Embedder,
        );
        Ok(())
    }

    /// A local file document: read from disk (never through the network), parsed by the
    /// same loader as network documents. Its relative subresources are sibling files.
    fn load_file_document(
        &mut self,
        url_str: &str,
        push_history: bool,
        id: NavigationId,
    ) -> Result<(), NavFailure> {
        let trimmed = url_str.trim();
        let path = file_url_path(trimmed);
        let path = Path::new(path);
        let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let final_url = path
            .canonicalize()
            .map(|p| format!("file:///{}", p.display().to_string().replace('\\', "/")))
            .unwrap_or_else(|_| path.display().to_string());
        self.pending_navigation = None;
        self.retire_document(&[], "navigation committed");
        self.security = SecurityState::NotNetwork;
        self.mixed_content = false;
        // Relative references resolve against the path (the `file:` URL is not parseable).
        let base = path.display().to_string();
        self.begin_document(
            &final_url,
            DocumentKind::LocalFile,
            Some(id),
            Instant::now(),
        );
        let doc = self.document.as_mut().expect("document just began");
        doc.base_url = base;
        let now = Instant::now();
        doc.lifecycle.timeline.response_start = Some(now);
        doc.lifecycle.timeline.first_bytes = Some(now);
        let text = doc.decoder.decode(&bytes);
        let tail = doc.decoder.finish();
        feed_text(doc, &text, false);
        feed_text(doc, &tail, true);
        self.push_or_replace_history(&final_url, push_history);
        self.pump_document();
        Ok(())
    }

    fn count_response(&mut self, resp: &ResourceResponse) {
        self.page.network_requests += 1;
        self.page.network_bytes += resp.transferred_bytes;
        if resp.resource_type != axiom_loader::ResourceType::Document
            && resp.tls.is_none()
            && matches!(self.security, SecurityState::Secure(_))
        {
            self.mixed_content = true;
        }
    }

    /// Apply every loader event that is ready now and advance the document. Never blocks.
    pub fn poll_loader(&mut self) {
        let events = self.loader.poll();
        self.process_events(events);
        // Removing an embedding element tears down its browsing context. This happens
        // before polling children, so a detached frame cannot publish a late result.
        let connected = {
            let dom = self.page.document.borrow();
            self.frames
                .keys()
                .copied()
                .filter(|node| !dom.is_connected(*node))
                .collect::<Vec<_>>()
        };
        for node in connected {
            self.frames.remove(&node);
            self.frame_load_fired.remove(&node);
            self.frame_error_fired.remove(&node);
        }
    }

    /// Advance child event loops, then composite their independent framebuffers into
    /// the embedding document. Child contexts are ticked only through this controlled
    /// parent-owned path: their timers, scripts and network callbacks never execute on
    /// a transport worker and cannot publish into another document generation.
    fn tick_frames_and_composite(&mut self) {
        let placements = self.frame_placements();
        for (node, rect) in &placements {
            let Some(frame) = self.frames.get_mut(node) else {
                continue;
            };
            let width = rect.width.max(1.0).round() as u32;
            let height = rect.height.max(1.0).round() as u32;
            if frame.viewport_width != width || frame.content_height() != height {
                frame.set_viewport(width, height);
            }
            frame.tick();
        }

        self.publish_frame_realms();

        let mut completed = Vec::new();
        for (&node, frame) in &self.frames {
            if !frame.is_idle() {
                continue;
            }
            if frame.last_error.is_some() {
                if !self.frame_error_fired.contains(&node) {
                    completed.push((node, "error"));
                }
            } else if !self.frame_load_fired.contains(&node) {
                completed.push((node, "load"));
            }
        }
        for (node, event) in completed {
            match event {
                "error" => {
                    self.frame_error_fired.insert(node);
                }
                "load" => {
                    self.frame_load_fired.insert(node);
                }
                _ => unreachable!("frame completion events are fixed"),
            }
            self.page.dispatch_element_event(node, event);
        }

        let Some(parent) = self.page.framebuffer.as_mut() else {
            return;
        };
        for (node, rect) in placements {
            let Some(child) = self
                .frames
                .get(&node)
                .and_then(BrowsingContext::framebuffer)
            else {
                continue;
            };
            blit_framebuffer(parent, child, rect, self.page.scroll_y);
        }
    }

    /// Iframe rectangles are resolved from the embedding document's layout tree, never
    /// from attributes. That makes CSS sizing, positioning and parent scrolling govern
    /// the child surface just like every other replaced element.
    fn frame_placements(&self) -> Vec<(NodeId, Rect)> {
        let render = self.page.shared.render.borrow();
        let Some(layout) = render.layout.as_ref() else {
            return Vec::new();
        };
        self.frames
            .keys()
            .filter_map(|node| layout.node_rect(*node).map(|rect| (*node, rect)))
            .collect()
    }

    /// Refresh the parent-visible child realm snapshots after each child turn. A child
    /// navigation replaces its document arena, so retaining a stale `Rc<Document>` here
    /// would be just as unsafe as treating its local node IDs as parent IDs.
    fn publish_frame_realms(&mut self) {
        let mut registry = self.page.frame_registry.borrow_mut();
        registry.clear();
        for (&element, child) in &self.frames {
            let Some(info) = child.document_info() else {
                continue;
            };
            registry.insert(
                element,
                FrameRealm {
                    browsing_context: child.context_id,
                    document: info.id,
                    origin: info.origin.clone(),
                    opaque_sandbox_origin: child.sandboxed_origin,
                    dom: Rc::clone(&child.page.document),
                    shared: Rc::clone(&child.page.shared),
                    inbox: Rc::clone(&child.frame_message_inbox),
                },
            );
        }
    }

    fn deliver_frame_messages(&mut self) {
        let messages = std::mem::take(&mut *self.frame_message_inbox.borrow_mut());
        let Some(js) = self.page.js.as_mut() else {
            return;
        };
        for message in messages {
            if let Err(error) = js.dispatch_frame_message(&message.data_json, &message.origin) {
                log::warn!("frame message dispatch: {error}");
            }
        }
    }

    fn process_events(&mut self, events: Vec<LoaderEvent>) {
        self.start_queued_fetches();
        self.poll_keepalive();
        for ev in events {
            self.route_event(ev);
        }
        self.notify_upload_drains();
        self.pump_document();
        self.report_progress();
    }

    /// Deliver one loader event to its owner: the pending navigation, a script `fetch()`,
    /// or the document that started the request. Events of replaced documents never
    /// reach the active one.
    fn route_event(&mut self, ev: LoaderEvent) {
        let id = ev.id();
        if self
            .pending_navigation
            .as_ref()
            .is_some_and(|p| p.request == id)
        {
            self.on_navigation_event(ev);
            return;
        }
        if self.pending_fetches.contains_key(&id) {
            self.deliver_fetch_event(ev);
            return;
        }
        let Some(owner) = self.request_owner.get(&id).copied() else {
            return;
        };
        if ev.is_terminal() {
            self.request_owner.remove(&id);
        }
        if self.document_id() != Some(owner) {
            self.stale_events_dropped += 1;
            if let Some(doc) = self.document.as_mut() {
                doc.events.record(
                    DocumentEventKind::StaleEventDropped,
                    None,
                    Some(id),
                    format!("event for {owner}"),
                );
            }
            return;
        }
        let Some(doc) = self.document.as_ref() else {
            return;
        };
        if doc.body_request == Some(id) {
            self.on_body_event(ev);
        } else if let Some(rid) = doc.registry.by_request(id) {
            self.on_resource_event(rid, ev);
        }
    }

    /// Keepalive events go to script while their document is alive; afterwards they are
    /// only drained.
    fn poll_keepalive(&mut self) {
        let events = self
            .keepalive
            .as_mut()
            .map(KeepaliveLoads::poll)
            .unwrap_or_default();
        for ev in events {
            if self.pending_fetches.contains_key(&ev.id()) {
                self.deliver_fetch_event(ev);
            } else if let LoaderEvent::Failed { url, error, .. } = ev {
                if error != NetworkError::Cancelled {
                    log::warn!("keepalive fetch {url} failed: {error}");
                }
            }
        }
        for loads in &mut self.retired_keepalive {
            for ev in loads.poll() {
                if let LoaderEvent::Failed { url, error, .. } = ev {
                    log::warn!("keepalive fetch {url} failed: {error}");
                }
            }
        }
        self.retired_keepalive.retain(|l| !l.is_empty());
    }

    pub fn tick(&mut self) -> HudStats {
        let frame_ms = self.clock.tick();
        self.poll_loader();
        self.deliver_frame_messages();
        self.tasks.run_until_empty();

        self.apply_timer_requests();
        for id in self.timers.poll_due() {
            let Some(js) = self.page.js.as_mut() else {
                break;
            };
            match js.fire_timeout(id) {
                Ok(Some(uncaught)) => self.record_callback_error("timer callback", uncaught),
                Ok(None) => {}
                Err(e) => log::warn!("timer {id}: {e}"),
            }
            // Handlers may clear or add timers that are due in this same turn.
            self.apply_timer_requests();
        }

        if self.page.sync_viewport() {
            if let Some(js) = self.page.js.as_mut() {
                if let Err(e) = js.evaluate_media_queries() {
                    log::warn!("media query change: {e}");
                }
            }
        }
        let scroll_y = self.page.shared.scroll_y.get();
        if self.page.shared.scroll_notified.replace(scroll_y) != scroll_y {
            if let Some(js) = self.page.js.as_mut() {
                if let Err(e) = js.notify_scroll() {
                    log::warn!("scroll notification: {e}");
                }
            }
        }
        self.run_animation_frames();

        self.microtasks.checkpoint();
        self.start_queued_fetches();
        // Timers may have inserted scripts, stylesheets or images.
        self.pump_document();
        // Scripts (timers, handlers, parser-inserted) may have submitted a form or used
        // `history` / `location`.
        self.start_form_submission();
        self.apply_history_ops();
        self.report_progress();
        self.page.update_rendering_if_needed();
        self.tick_frames_and_composite();
        self.request_css_images();
        // Observers see this layout in the next animation frame; asking for it now keeps
        // the loop from going idle in between.
        let generation = self.page.shared.render.borrow().layout_generation;
        if self.page.shared.observed_layout.replace(generation) != generation {
            if let Some(js) = self.page.js.as_ref() {
                js.layout_changed();
            }
        }
        // The cascade may have blocked `style` attributes.
        self.page.report_csp_violations();
        self.page.hud.frame_ms = frame_ms;
        self.page.hud.fps = self.clock.fps();
        self.page.hud.cache_hits = self.loader.cache().stats().hits as usize;
        self.page.hud.clone()
    }

    /// Move `setTimeout` / `setInterval` / `clear*` requests from script into the queue.
    fn apply_timer_requests(&mut self) {
        let Some(js) = self.page.js.as_ref() else {
            return;
        };
        let requests: Vec<_> = js.timer_requests.borrow_mut().drain(..).collect();
        for request in requests {
            match request {
                TimerRequest::Set {
                    id,
                    delay_ms,
                    repeat: false,
                } => {
                    self.timers.set_timeout(delay_ms, id);
                }
                TimerRequest::Set {
                    id,
                    delay_ms,
                    repeat: true,
                } => {
                    self.timers.set_interval(delay_ms, id);
                }
                TimerRequest::Clear { id } => self.timers.clear_callback(id),
            }
        }
    }

    /// Run requested animation frame callbacks, at most once per ~16 ms. A render-blocked
    /// document has no rendering opportunities, so its callbacks wait.
    fn run_animation_frames(&mut self) {
        const FRAME: Duration = Duration::from_millis(16);
        if self.page.render_blocked {
            return;
        }
        let Some(js) = self.page.js.as_mut() else {
            return;
        };
        if !js.animation_frame_requested() {
            return;
        }
        let now = Instant::now();
        if self
            .last_animation_frame
            .is_some_and(|last| now.duration_since(last) < FRAME)
        {
            return;
        }
        self.last_animation_frame = Some(now);
        let t0 = Instant::now();
        let result = js.run_animation_frames();
        self.page.hud.js_ms += t0.elapsed().as_secs_f64() * 1000.0;
        match result {
            Ok(errors) => {
                for e in errors {
                    self.record_callback_error("animation frame callback", e);
                }
            }
            Err(e) => log::warn!("animation frame callbacks: {e}"),
        }
    }

    /// An exception escaped a script callback (already reported to the window).
    fn record_callback_error(&mut self, label: &str, message: String) {
        log::warn!("script error ({label}): {message}");
        if let Some(doc) = self.document.as_mut() {
            doc.script_errors.push(ScriptError {
                resource: None,
                label: label.to_string(),
                message,
            });
        }
    }

    pub fn handle_click(&mut self, x: f32, y: f32) {
        let content_y = y - self.chrome_height as f32;
        if content_y < 0.0 {
            return;
        }
        let Some(node) = self.page.hit_test(x, content_y) else {
            return;
        };
        self.dispatch_click_on_node(node, x, content_y);
    }

    /// Programmatic click (tests / same path as input).
    pub fn click_node(&mut self, node: axiom_dom::NodeId) {
        self.dispatch_click_on_node(node, 0.0, 0.0);
    }

    fn dispatch_click_on_node(&mut self, node: axiom_dom::NodeId, x: f32, y: f32) {
        self.page.document.borrow_mut().active = Some(node);

        // 1) Rust EventTarget listeners
        let mut event = Event::new("click");
        event.client_x = x;
        event.client_y = y;
        let path = self.page.document.borrow().parent_chain(node);
        self.page.events.dispatch(&path, &mut event);

        // 2) JavaScript listeners (real closures stored in the JS realm), wrapped in the
        //    form controls' activation behavior (checkbox / radio / label / submit / reset)
        let path_ids: Vec<usize> = path.iter().map(|n| n.0).collect();
        let mut canceled = false;
        let scripted = self.page.js.is_some();
        if let Some(js) = self.page.js.as_mut() {
            match js.dispatch_click(&path_ids, x, y) {
                Ok(not_canceled) => canceled = !not_canceled,
                Err(e) => log::warn!("JS click dispatch: {e}"),
            }
            for msg in js.take_console() {
                log::info!("[console] {msg}");
            }
        }
        if canceled {
            self.run_form_submissions();
            self.apply_history_ops();
            self.page.update_rendering_if_needed();
            return;
        }

        // 3) Default actions (navigation / focus; form activation without a script realm)
        let tag = self
            .page
            .document
            .borrow()
            .tag_name(node)
            .map(|s| s.to_string());
        // Activation behavior belongs to the nearest link ancestor of the target.
        let href = {
            let doc = self.page.document.borrow();
            path.iter()
                .rev()
                .find(|&&id| doc.tag_name(id) == Some("a") && doc.attr(id, "href").is_some())
                .and_then(|&id| doc.attr(id, "href"))
                .map(str::to_string)
        };
        if let Some(href) = href {
            let next = resolve_navigation(&self.page.url, &href);
            if is_fragment_navigation(&self.page.url, &next) {
                self.navigate_to_fragment(&next, true);
            } else {
                self.navigate_with(&next, true, CacheMode::Default, NavigationCause::Link);
            }
        }
        if matches!(
            tag.as_deref(),
            Some("input") | Some("textarea") | Some("button") | Some("select")
        ) {
            self.page.document.borrow_mut().focus = Some(node);
        }
        if !scripted {
            let mut doc = self.page.document.borrow_mut();
            let changed = self.page.shared.forms.borrow_mut().activate_without_script(
                &mut doc,
                &path,
                &self.page.url,
            );
            if changed {
                doc.mark_dirty(DirtyFlags::all());
            }
        }
        self.run_form_submissions();

        // 4) Invalidation → style/layout/paint as needed → new frame
        self.page.update_rendering_if_needed();
    }

    /// Navigate the form submission the page queued, if any, waiting for the document in
    /// blocking mode (like link activation).
    fn run_form_submissions(&mut self) {
        if let Some(id) = self.start_form_submission() {
            if self.blocking_navigation {
                self.wait_for_document(id, DOCUMENT_TIMEOUT);
            }
        }
    }

    fn start_form_submission(&mut self) -> Option<NavigationId> {
        let submission = self.page.shared.forms.borrow_mut().take_submission()?;
        if !self.forms_enabled {
            log::info!("form submission blocked by iframe sandbox");
            return None;
        }
        let target = axiom_csp::CspUrl::parse(&submission.url);
        if target.is_some_and(|t| !self.page.shared.csp.check_form_action(&t)) {
            log::info!(
                "form submission to {} blocked by the Content Security Policy",
                redacted_url(&submission.url)
            );
            return None;
        }
        log::info!(
            "form submission: {} {}",
            match submission.method {
                forms::FormMethod::Get => "GET",
                forms::FormMethod::Post => "POST",
            },
            redacted_url(&submission.url)
        );
        Some(self.begin_navigation_with(
            &submission.url,
            true,
            CacheMode::Default,
            NavigationCause::FormSubmission,
            Some(&submission),
        ))
    }

    pub fn handle_scroll(&mut self, delta_y: f32) {
        self.page.scroll_by(delta_y);
        self.history.update_scroll(self.page.scroll_y);
        let mut event = Event::new("scroll");
        event.scroll_delta_y = delta_y;
        if let Some(body) = self.page.document.borrow().body() {
            let path = self.page.document.borrow().parent_chain(body);
            self.page.events.dispatch(&path, &mut event);
        }
    }

    pub fn handle_key(&mut self, key: &str) {
        let focus = match self.page.document.borrow().focus {
            Some(f) => f,
            None => return,
        };
        let (is_input, is_textarea) = {
            let doc = self.page.document.borrow();
            (
                doc.tag_name(focus) == Some("input"),
                doc.tag_name(focus) == Some("textarea"),
            )
        };
        if key == "Enter" && is_input {
            self.implicit_submission(focus);
            return;
        }
        let key = if key == "Enter" && is_textarea {
            "\n"
        } else {
            key
        };
        let changed = {
            let doc = self.page.document.borrow();
            forms::is_text_control(&doc, focus)
                && self
                    .page
                    .shared
                    .forms
                    .borrow_mut()
                    .edit_text(&doc, focus, key)
        };
        if !changed {
            return;
        }
        self.page
            .document
            .borrow_mut()
            .mark_dirty(DirtyFlags::all());
        let mut event = Event::new("input");
        let path = self.page.document.borrow().parent_chain(focus);
        self.page.events.dispatch(&path, &mut event);
        if let Some(js) = self.page.js.as_mut() {
            if let Err(e) = js.fire_form_event(focus.0, "input") {
                log::warn!("JS input event: {e}");
            }
        }
        self.run_form_submissions();
        self.page.update_rendering_if_needed();
    }

    /// Enter in a form field: the form's default button is activated (which scripts can
    /// cancel), or a form without one is submitted.
    fn implicit_submission(&mut self, node: axiom_dom::NodeId) {
        match self.page.js.as_mut() {
            Some(js) => {
                if let Err(e) = js.implicit_submit(node.0) {
                    log::warn!("JS implicit submission: {e}");
                }
                for msg in js.take_console() {
                    log::info!("[console] {msg}");
                }
            }
            None => {
                let mut doc = self.page.document.borrow_mut();
                if self
                    .page
                    .shared
                    .forms
                    .borrow_mut()
                    .implicit_submit_without_script(&mut doc, node, &self.page.url)
                {
                    doc.mark_dirty(DirtyFlags::all());
                }
            }
        }
        self.run_form_submissions();
        self.page.update_rendering_if_needed();
    }

    pub fn can_go_back(&self) -> bool {
        self.history.can_go_back()
    }

    pub fn can_go_forward(&self) -> bool {
        self.history.can_go_forward()
    }

    /// Resolved href under content coordinates (y already relative to content top).
    pub fn link_href_at(&self, x: f32, content_y: f32) -> Option<String> {
        let node = self.page.hit_test(x, content_y)?;
        let doc = self.page.document.borrow();
        for id in doc.parent_chain(node).into_iter().rev() {
            if doc.tag_name(id) == Some("a") {
                if let Some(href) = doc.attr(id, "href") {
                    return Some(resolve_navigation(&self.page.url, href));
                }
            }
        }
        None
    }

    /// Stop: cancel every queued and in-flight request of this context.
    pub fn stop_loading(&mut self) {
        if let Some(nav) = &self.pending_navigation {
            self.loader.cancel(nav.request);
        }
        self.cancel_pending_loads();
        self.pump_document();
        self.report_progress();
        self.loading = false;
    }

    pub fn framebuffer(&self) -> Option<&axiom_paint::Framebuffer> {
        self.page.framebuffer.as_ref()
    }

    pub fn set_viewport(&mut self, w: u32, h: u32) {
        self.viewport_width = w;
        self.viewport_height = h;
        self.page.viewport_width = w;
        self.page.viewport_height = self.content_height();
        self.page.document.borrow_mut().dirty = DirtyFlags::all();
    }

    pub fn set_chrome_height(&mut self, h: u32) {
        self.chrome_height = h;
        self.page.viewport_height = self.content_height();
        self.page.document.borrow_mut().dirty = DirtyFlags::all();
    }
}

/// Blend a child document surface into its iframe border box. The frame is clipped to
/// both its CSS box and the embedding viewport; child pixels use the paint crate's
/// `0xAABBGGRR` representation. This is deliberately a compositor boundary rather
/// than a DOM shortcut: a child owns its own page/realm and only publishes pixels.
fn blit_framebuffer(
    destination: &mut axiom_paint::Framebuffer,
    source: &axiom_paint::Framebuffer,
    rect: Rect,
    parent_scroll_y: f32,
) {
    let origin_x = rect.x.round() as i32;
    let origin_y = (rect.y - parent_scroll_y).round() as i32;
    let width = (rect.width.max(0.0).round() as u32).min(source.width);
    let height = (rect.height.max(0.0).round() as u32).min(source.height);
    for sy in 0..height {
        let dy = origin_y + sy as i32;
        if dy < 0 || dy >= destination.height as i32 {
            continue;
        }
        for sx in 0..width {
            let dx = origin_x + sx as i32;
            if dx < 0 || dx >= destination.width as i32 {
                continue;
            }
            let source_pixel = source.pixels[(sy * source.width + sx) as usize];
            let destination_index = (dy as u32 * destination.width + dx as u32) as usize;
            destination.pixels[destination_index] =
                alpha_over(destination.pixels[destination_index], source_pixel);
        }
    }
}

fn alpha_over(background: u32, foreground: u32) -> u32 {
    let alpha = foreground >> 24;
    if alpha == 0 {
        return background;
    }
    if alpha == 0xff {
        return foreground;
    }
    let inverse = 0xff - alpha;
    let blend = |shift: u32| {
        (((foreground >> shift & 0xff) * alpha + (background >> shift & 0xff) * inverse) / 0xff)
            << shift
    };
    0xff00_0000 | blend(16) | blend(8) | blend(0)
}

/// Standalone-context bridge to an engine [`CookieJar`]. Browser profiles use the
/// profile cookie authority directly (with full SameSite context).
struct JarCookieProvider(Arc<dyn CookieJar>);

impl CookieProvider for JarCookieProvider {
    fn cookie_header(&self, ctx: &CookieRequestContext<'_>) -> Option<String> {
        self.0.cookie_header_for_request(&ctx.url.as_str())
    }
    fn store_set_cookies(&self, ctx: &CookieRequestContext<'_>, set_cookies: &[String]) {
        self.0.store_set_cookies(&ctx.url.as_str(), set_cookies);
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn trusted_network_error_html(e: &NavFailure, url: &str) -> String {
    let title = match e.kind {
        "dns" => "Server not found",
        "tls" => "Secure connection failed",
        "certificate" => "Certificate error",
        "connection" | "connection_refused" => "Connection failed",
        "timeout" | "connect_timeout" | "read_timeout" => "Request timed out",
        "redirect_loop" | "too_many_redirects" => "Too many redirects",
        "cancelled" => "Request cancelled",
        "blocked" => "Request blocked",
        "too_large" | "headers_too_large" => "Response too large",
        "protocol" | "body" => "Invalid response",
        "unsupported_scheme" | "url" => "Cannot open this address",
        "form_resubmission" => "Form data not resent",
        _ => "Network error",
    };
    let retry_label = match e.kind {
        "form_resubmission" => "Load this address without form data",
        _ => "Try again",
    };
    let reason = html_escape(e.summary.unwrap_or("The page could not be loaded."));
    // Only the stable error code: library messages can carry internal detail and are
    // kept for logs (`last_error`, axiom://network).
    let diagnostic = format!("Error code: {}", e.kind);
    let url = html_escape(url);
    let retry = if url.starts_with("http://") || url.starts_with("https://") {
        format!(r#"<p><a class="retry" href="{url}">{retry_label}</a></p>"#)
    } else {
        String::new()
    };
    format!(
        r#"<!DOCTYPE html><html><head><title>{title}</title>
<style>
body{{font-family:system-ui,sans-serif;margin:48px;background:#f6f7f9;color:#1a1a1a}}
h1{{font-weight:600;color:#8b1a1a}} .url{{color:#444;word-break:break-all}}
.meta{{color:#666;font-size:13px}} .diag{{color:#666;font-family:monospace;font-size:12px}}
</style></head><body>
<h1>{title}</h1>
<p class="url">{url}</p>
<p>{reason}</p>
{retry}
<p class="diag">{diagnostic}</p>
<p class="meta">Trusted error page — not fetched from the network.</p>
</body></html>"#
    )
}

/// Turn a document request into a form submission's: the submitting document is the
/// initiator (referrer, `Origin`, SameSite) and a POST carries the encoded entry list.
/// Returns whether the request is a POST.
fn apply_form_submission(request: &mut ResourceRequest, form: &FormSubmission) -> bool {
    let origin = document_origin(&form.document_url);
    request.document_url = origin.clone();
    let Some((body, content_type)) = &form.body else {
        return false;
    };
    request.method = HttpMethod::Post;
    request.body = RequestBody::Bytes(body.clone());
    request.headers.set("Content-Type", content_type.clone());
    request.headers.set(
        "Origin",
        origin.as_ref().map_or("null".to_string(), Url::origin),
    );
    true
}

/// URL for logs: scheme, host and path only — submitted queries can carry user input.
fn redacted_url(url: &str) -> String {
    match Url::parse(url) {
        Ok(u) => format!("{}{}", u.origin(), u.path),
        Err(_) => "<invalid url>".to_string(),
    }
}

/// Record the encoding the decoder settled on for `document.characterSet`.
fn note_character_set(meta: &std::cell::RefCell<DocumentMeta>, decoder: &TextDecoder) {
    if let Some(encoding) = decoder.encoding() {
        let mut meta = meta.borrow_mut();
        if meta.character_set != encoding.name() {
            meta.character_set = encoding.name().to_string();
        }
    }
}

fn fragment(url: &str) -> Option<&str> {
    url.split_once('#').map(|(_, f)| f)
}

/// `target` has a fragment and is otherwise `current`: a same-document navigation.
fn is_fragment_navigation(current: &str, target: &str) -> bool {
    let strip = |u: &str| {
        u.split_once('#')
            .map_or(u.to_string(), |(b, _)| b.to_string())
    };
    fragment(target).is_some() && strip(current) == strip(target)
}

fn resolve_navigation(current: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") || href.starts_with("file:") {
        return href.to_string();
    }
    if let Ok(base) = Url::parse(current) {
        if let Ok(joined) = base.join(href) {
            return joined.as_str();
        }
    }
    if current.starts_with("file:") {
        let path = file_url_path(current);
        let parent = Path::new(path).parent().unwrap_or_else(|| Path::new("."));
        return parent.join(href).display().to_string();
    }
    href.to_string()
}

/// Decode an already-accepted `file:` URL for local filesystem access. Windows file URLs use
/// `file:///C:/...`; the leading slash belongs to URL syntax, not the drive path.
fn file_url_path(input: &str) -> &str {
    let path = input
        .strip_prefix("file:///")
        .or_else(|| input.strip_prefix("file://"))
        .unwrap_or(input);
    #[cfg(windows)]
    {
        if let Some(drive_path) = path.strip_prefix('/') {
            if drive_path.as_bytes().get(1) == Some(&b':') {
                return drive_path;
            }
        }
    }
    path
}
