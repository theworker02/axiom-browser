//! Loaded page state: DOM + style + layout + paint + images.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use axiom_compositor::Compositor;
use axiom_document::{BrowsingContextId, DocumentId, ReadyState};
use axiom_dom::{
    DirtyFlags, Document, Namespace, NodeId, NodeKind, ShadowRootInit, ShadowRootMode,
    SlotAssignmentMode,
};
use axiom_events::EventTargetMap;
use axiom_html::{HtmlParser, ParseStep};
use axiom_js::{FrameInfo, JsContext, JsHost, JsRuntime};
use axiom_layout::{RasterImage, Rect};
use axiom_net::{AXIOM_USER_AGENT, DEFAULT_ACCEPT_LANGUAGE};
use axiom_paint::{build_display_list_with_images, rasterize_region, DisplayList, Framebuffer};
use axiom_trace::{TraceKind, TraceTimeline};
use axiom_url::{Origin, Url};

use crate::cookie_jar::CookieJar;
use crate::csp::{element_nonce, DocumentCsp};
use crate::fetch::{
    plan_fetch, resolve_referrer, resolve_url, FetchCommand, FetchPlan, FetchQueue, UploadSlot,
    UPLOAD_HIGH_WATER,
};
use crate::forms::{self, FormState};
use crate::hud::HudStats;
use crate::import_map::ImportMap;
use crate::lifecycle::DocumentReadyState;
use crate::render::{self, RenderState};
use crate::web_storage::{StorageBinder, WebStorageHost};

pub type DecodedImage = RasterImage;

/// A JSON structured-clone payload queued across a browsing-context boundary. Values
/// are serialized before enqueueing so no JS heap object crosses realms.
#[derive(Debug, Clone)]
pub struct FrameMessage {
    pub data_json: String,
    pub origin: String,
}

/// A parent-owned registry of child realm snapshots. Entries carry the child's actual
/// document arena and global browsing/document identities, so a local `NodeId` is
/// always interpreted together with its owning iframe rather than as a parent ID.
/// This is deliberately private to the engine/JS host boundary.
#[derive(Clone)]
pub struct FrameRealm {
    pub browsing_context: BrowsingContextId,
    pub document: DocumentId,
    pub origin: Origin,
    pub opaque_sandbox_origin: bool,
    pub dom: Rc<RefCell<Document>>,
    pub shared: Rc<DocumentShared>,
    pub inbox: Rc<RefCell<Vec<FrameMessage>>>,
}

#[derive(Default)]
pub struct FrameRegistry {
    realms: HashMap<NodeId, FrameRealm>,
}

impl FrameRegistry {
    pub fn insert(&mut self, element: NodeId, realm: FrameRealm) {
        self.realms.insert(element, realm);
    }

    pub fn remove(&mut self, element: NodeId) {
        self.realms.remove(&element);
    }

    pub fn clear(&mut self) {
        self.realms.clear();
    }

    fn get(&self, element: NodeId) -> Option<FrameRealm> {
        self.realms.get(&element).cloned()
    }
}

/// An element script connected to the document whose resource the document loader must
/// start (script-inserted resources go through the same loader as parser-inserted ones).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicInsertion {
    Script(NodeId),
    Stylesheet(NodeId),
    Image(NodeId),
    Style(NodeId),
    /// A nested browsing context. The browsing layer, rather than DOM bindings,
    /// owns its navigation and profile-scoped network state.
    Frame(NodeId),
}

/// State one document shares between the engine and its script host. A new instance is
/// created for every document, so a stale script realm can never reach a newer document.
#[derive(Debug)]
pub struct DocumentShared {
    pub ready_state: Cell<ReadyState>,
    pub insertions: RefCell<Vec<DynamicInsertion>>,
    /// `<script>` elements that already ran or started loading ("already started").
    pub started_scripts: RefCell<HashSet<NodeId>>,
    /// Viewport size in CSS px that media queries evaluate against.
    pub viewport: Cell<(f32, f32)>,
    /// The document's import maps, merged.
    pub import_map: RefCell<ImportMap>,
    /// Form control values, checkedness and selectedness, and the pending submission.
    pub forms: RefCell<FormState>,
    /// Content Security Policy and pending violation reports.
    pub csp: DocumentCsp,
    /// Style and layout inputs and results (shared so script queries can flush them).
    pub render: RefCell<RenderState>,
    /// The viewport's vertical scroll offset, as script sees it.
    pub scroll_y: Cell<f32>,
    /// A scroll offset script asked for; the page applies it before its next frame.
    pub scroll_request: Cell<Option<f32>>,
    /// The scroll offset script was last told about with `scroll` events.
    pub scroll_notified: Cell<f32>,
    /// The layout generation script observers last saw.
    pub observed_layout: Cell<u64>,
    /// Identifies this document in session history entries it created.
    pub document_key: u64,
    /// The document's URL; `pushState` and same-document navigations change it.
    pub url: RefCell<String>,
    /// Session history requests from script, applied by the browsing context.
    pub history_ops: RefCell<Vec<HistoryOp>>,
    /// Session history length and the current entry's script state id, as script sees
    /// them (kept up to date by the browsing context and by `pushState`).
    pub history_length: Cell<usize>,
    pub history_index: Cell<usize>,
    pub history_state: Cell<Option<u64>>,
    /// Response facts behind `document.referrer`, `contentType`, `characterSet`, ….
    pub meta: RefCell<DocumentMeta>,
}

/// What the document's response said about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentMeta {
    /// The navigation request's referrer after its policy (`""` for none).
    pub referrer: String,
    /// MIME type essence (`""` means `text/html`).
    pub content_type: String,
    /// Encoding name as the decoder settled it (`""` means `UTF-8`).
    pub character_set: String,
    /// `Last-Modified` in Unix seconds.
    pub last_modified: Option<i64>,
}

/// A session history or navigation request made by script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryOp {
    /// `pushState` / `replaceState` to `url` (already validated) with script state `state`.
    Push {
        url: String,
        state: u64,
        replace: bool,
    },
    /// `history.go(delta)` / `back()` / `forward()`.
    Traverse(i32),
    /// `location.assign` / `replace` / `href` and friends.
    Navigate {
        url: String,
        replace: bool,
    },
    Reload,
}

static DOCUMENT_KEYS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// HTML "can have its URL rewritten" (`pushState`): same scheme, host and port; outside
/// HTTP(S) only the query and fragment (`file:`) or just the fragment may change.
pub fn can_rewrite_url(document: &str, target: &str) -> bool {
    let (Ok(a), Ok(b)) = (Url::parse(document), Url::parse(target)) else {
        return false;
    };
    if a.scheme != b.scheme || a.host != b.host || a.effective_port() != b.effective_port() {
        return false;
    }
    match a.scheme.as_str() {
        "http" | "https" => true,
        "file" => a.path == b.path,
        _ => a.path == b.path && a.query == b.query,
    }
}

impl DocumentShared {
    pub fn for_document(url: &str) -> Self {
        Self {
            document_key: DOCUMENT_KEYS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            url: RefCell::new(url.to_string()),
            history_ops: RefCell::new(Vec::new()),
            history_length: Cell::new(1),
            history_index: Cell::new(0),
            history_state: Cell::new(None),
            ready_state: Cell::new(ReadyState::Loading),
            insertions: RefCell::new(Vec::new()),
            started_scripts: RefCell::new(HashSet::new()),
            viewport: Cell::new((0.0, 0.0)),
            import_map: RefCell::new(ImportMap::default()),
            forms: RefCell::new(FormState::default()),
            csp: DocumentCsp::new(url),
            render: RefCell::new(RenderState::default()),
            scroll_y: Cell::new(0.0),
            scroll_request: Cell::new(None),
            scroll_notified: Cell::new(0.0),
            observed_layout: Cell::new(0),
            meta: RefCell::new(DocumentMeta::default()),
        }
    }
}

impl Default for DocumentShared {
    fn default() -> Self {
        Self::for_document("about:blank")
    }
}

pub struct Page {
    pub url: String,
    pub title: String,
    pub ready: DocumentReadyState,
    pub document: Rc<RefCell<Document>>,
    pub display_list: DisplayList,
    pub framebuffer: Option<Framebuffer>,
    pub events: EventTargetMap,
    pub compositor: Compositor,
    pub scroll_y: f32,
    pub content_height: f32,
    pub viewport_width: u32,
    pub viewport_height: u32,
    /// The document was served as `application/xhtml+xml` (parsed as HTML).
    pub xhtml: bool,
    /// Parser-inserted stylesheets are still loading: keep the previous frame.
    pub render_blocked: bool,
    pub shared: Rc<DocumentShared>,
    /// Child-realm identities available to this document's script host.
    pub frame_registry: Rc<RefCell<FrameRegistry>>,
    pub js: Option<JsContext>,
    pub hud: HudStats,
    pub trace: Arc<TraceTimeline>,
    pub network_bytes: u64,
    pub network_requests: usize,
    pub cookie_jar: Option<Arc<dyn CookieJar>>,
    pub storage_binder: Option<StorageBinder>,
    /// `fetch()` commands from this document's scripts; a new queue per document.
    pub fetch_queue: FetchQueue,
    /// The `User-Agent` and `Accept-Language` the document's requests carry, as script
    /// sees them through `navigator`.
    pub user_agent: String,
    pub accept_language: String,
}

impl Page {
    pub fn blank(viewport_width: u32, viewport_height: u32, trace: Arc<TraceTimeline>) -> Self {
        Self {
            url: "about:blank".into(),
            title: "New Tab".into(),
            ready: DocumentReadyState::Created,
            document: Rc::new(RefCell::new(Document::new())),
            display_list: DisplayList::default(),
            framebuffer: None,
            events: EventTargetMap::new(),
            compositor: Compositor::new(),
            scroll_y: 0.0,
            content_height: viewport_height as f32,
            viewport_width,
            viewport_height,
            xhtml: false,
            render_blocked: false,
            shared: Rc::new(DocumentShared::default()),
            frame_registry: Rc::new(RefCell::new(FrameRegistry::default())),
            js: None,
            hud: HudStats::default(),
            trace,
            network_bytes: 0,
            network_requests: 0,
            cookie_jar: None,
            storage_binder: None,
            fetch_queue: FetchQueue::default(),
            user_agent: AXIOM_USER_AGENT.into(),
            accept_language: DEFAULT_ACCEPT_LANGUAGE.into(),
        }
    }

    /// Publishes the current viewport to script; `true` when it changed (live media
    /// query lists need re-evaluating).
    pub fn sync_viewport(&mut self) -> bool {
        let size = (self.viewport_width as f32, self.viewport_height as f32);
        self.shared.viewport.replace(size) != size
    }

    /// Load trusted HTML in one piece: inline classic scripts run at their parser position,
    /// then `readystatechange` / `DOMContentLoaded` / `load` fire. No subresource is
    /// loaded (internal pages and test HTML never touch the network).
    pub fn load_html(&mut self, url: &str, html: &str) -> Result<(), String> {
        self.begin_document(url);
        self.set_ready(DocumentReadyState::Parsing);
        let t0 = Instant::now();
        let mut parser = HtmlParser::new();
        parser.feed(html);
        parser.finish();
        loop {
            let step = {
                let _span = self.trace.span(TraceKind::HtmlParse, "html parse");
                parser.step(&mut self.document.borrow_mut())
            };
            if let Some(js) = self.js.as_mut() {
                if let Err(e) = js.upgrade_parsed_custom_elements() {
                    log::warn!("custom element upgrades: {e}");
                }
            }
            match step {
                ParseStep::Script(node) => {
                    self.shared.started_scripts.borrow_mut().insert(node);
                    if let Some(text) = self.inline_script_text(node) {
                        let _ = self.execute_script(Some(node), &text, "inline script");
                    }
                }
                ParseStep::NeedData | ParseStep::Done => break,
            }
        }
        self.update_title();
        self.set_ready(DocumentReadyState::Interactive);
        self.dispatch_document_event("readystatechange", false);
        self.dispatch_document_event("DOMContentLoaded", true);
        self.set_ready(DocumentReadyState::Complete);
        self.dispatch_document_event("readystatechange", false);
        self.dispatch_window_event("load");
        self.hud.js_ms = t0.elapsed().as_secs_f64() * 1000.0;
        Ok(())
    }

    /// Start a new, empty document at `url` with a fresh script realm. Everything that
    /// belonged to the previous document (DOM, styles, images, realm) is dropped.
    pub fn begin_document(&mut self, url: &str) {
        self.frame_registry.borrow_mut().clear();
        self.document = Rc::new(RefCell::new(Document::new()));
        self.url = url.to_string();
        self.title = url.to_string();
        self.xhtml = false;
        self.events = EventTargetMap::new();
        self.render_blocked = false;
        self.shared = Rc::new(DocumentShared::for_document(url));
        self.shared.render.borrow_mut().viewport =
            (self.viewport_width as f32, self.viewport_height as f32);
        self.document.borrow_mut().dirty = DirtyFlags::all();
        self.set_ready(DocumentReadyState::Loading);
        self.bind_js();
    }

    /// Install a complete XML document after the streaming network layer has finished
    /// decoding it. XML parsing is intentionally all-or-nothing: a malformed XML
    /// response must never leave a partially committed document behind.
    pub fn replace_document(&mut self, document: Document, xhtml: bool) {
        self.document = Rc::new(RefCell::new(document));
        self.xhtml = xhtml;
        self.document.borrow_mut().dirty = DirtyFlags::all();
        self.bind_js();
    }

    /// `<title>` text (the URL while there is none).
    pub fn update_title(&mut self) {
        let doc = self.document.borrow();
        self.title = doc
            .head()
            .and_then(|h| doc.find_descendant(h, "title"))
            .map(|id| doc.text_content(id).trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.url.clone());
    }

    /// Engine ready state plus the `document.readyState` script sees.
    pub fn set_ready(&mut self, state: DocumentReadyState) {
        self.ready = state;
        let script_state = match state {
            DocumentReadyState::Interactive => ReadyState::Interactive,
            DocumentReadyState::Complete => ReadyState::Complete,
            _ => ReadyState::Loading,
        };
        self.shared.ready_state.set(script_state);
    }

    /// Source of a classic inline script element (`None` for external, module, data-block
    /// and empty scripts).
    pub fn inline_script_text(&self, node: NodeId) -> Option<String> {
        let doc = self.document.borrow();
        let has_src = doc.attr(node, "src").is_some_and(|s| !s.trim().is_empty());
        let module = doc
            .attr(node, "type")
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("module"));
        if has_src || module || !is_classic_script_type(doc.attr(node, "type")) {
            return None;
        }
        let mut text = doc.text_content(node);
        if self.xhtml {
            text = unwrap_cdata(&text);
        }
        (!text.trim().is_empty()).then_some(text)
    }

    /// Source of an inline module or import map script element (`None` when empty).
    pub fn inline_module_text(&self, node: NodeId) -> Option<String> {
        let mut text = self.document.borrow().text_content(node);
        if self.xhtml {
            text = unwrap_cdata(&text);
        }
        (!text.trim().is_empty()).then_some(text)
    }

    /// Start a module script job (see [`JsContext::start_module`]).
    pub fn start_module(&mut self, url: &str, source: &str, inline: bool) -> Option<u32> {
        let js = self.js.as_mut()?;
        let _span = self
            .trace
            .span(TraceKind::Javascript, format!("module {url}"));
        let id = js.start_module(url, source, inline);
        flush_console(js);
        Some(id)
    }

    /// Link and evaluate a module job whose graph loaded.
    pub fn run_module(&mut self, id: u32, label: &str) {
        if let Some(js) = self.js.as_mut() {
            let _span = self.trace.span(TraceKind::Javascript, label.to_string());
            js.run_module(id);
            flush_console(js);
        }
    }

    pub fn take_module_fetches(&mut self) -> Vec<String> {
        self.js
            .as_mut()
            .map(JsContext::take_module_fetches)
            .unwrap_or_default()
    }

    pub fn module_fetched(&mut self, url: &str, result: Result<(String, String), String>) {
        if let Some(js) = self.js.as_mut() {
            js.module_fetched(url, result);
            flush_console(js);
        }
    }

    pub fn poll_modules(&mut self) -> Vec<(u32, axiom_js::ModuleOutcome)> {
        let Some(js) = self.js.as_mut() else {
            return Vec::new();
        };
        let out = js.poll_modules();
        flush_console(js);
        out
    }

    pub fn has_module_work(&self) -> bool {
        self.js.as_ref().is_some_and(JsContext::has_module_work)
    }

    /// Report an exception the engine raised for script (a `SyntaxError` or `TypeError`).
    pub fn report_script_error(&mut self, syntax: bool, message: &str) {
        if let Some(js) = self.js.as_mut() {
            js.report_error(syntax, message);
            flush_console(js);
        }
    }

    /// Run a script with `document.currentScript` set to its element.
    pub fn execute_script(
        &mut self,
        node: Option<NodeId>,
        source: &str,
        label: &str,
    ) -> Result<(), String> {
        if let Some(js) = self.js.as_mut() {
            js.set_current_script(node.map(|n| n.0));
        }
        let result = self.run_script_text(source, label);
        if let Some(js) = self.js.as_mut() {
            js.set_current_script(None);
        }
        result
    }

    pub fn run_script_text(&mut self, source: &str, label: &str) -> Result<(), String> {
        let Some(js) = self.js.as_mut() else {
            return Ok(());
        };
        let _span = self.trace.span(TraceKind::Javascript, label.to_string());
        let url = label.contains("://").then_some(label);
        let result = js
            .eval_script(source, url)
            .map(|_| ())
            .map_err(|e| e.to_string());
        if let Err(e) = &result {
            log::warn!("script error ({label}): {e}");
        }
        flush_console(js);
        result
    }

    pub fn dispatch_document_event(&mut self, type_: &str, bubbles: bool) {
        if let Some(js) = self.js.as_mut() {
            if let Err(e) = js.dispatch_document_event(type_, bubbles) {
                log::warn!("{type_} dispatch: {e}");
            }
            flush_console(js);
        }
    }

    pub fn dispatch_window_event(&mut self, type_: &str) {
        if let Some(js) = self.js.as_mut() {
            if let Err(e) = js.dispatch_window_event(type_) {
                log::warn!("{type_} dispatch: {e}");
            }
            flush_console(js);
        }
    }

    pub fn dispatch_element_event(&mut self, node: NodeId, type_: &str) {
        if let Some(js) = self.js.as_mut() {
            if let Err(e) = js.dispatch_element_event(node.0, type_) {
                log::warn!("{type_} dispatch: {e}");
            }
            flush_console(js);
        }
    }

    /// Author CSS in cascade order: inline `<style>` text (after its `@import`s) and
    /// loaded external sheets, except those whose `<link>` is `disabled` and `<style>`s
    /// the Content Security Policy blocks.
    pub fn author_css(&self) -> String {
        render::author_css(&self.document.borrow(), &self.shared)
    }

    /// Style and layout state (styles, layout tree, images, loaded CSS).
    pub fn render(&self) -> std::cell::Ref<'_, RenderState> {
        self.shared.render.borrow()
    }

    pub fn render_mut(&self) -> std::cell::RefMut<'_, RenderState> {
        self.shared.render.borrow_mut()
    }

    pub fn bind_js(&mut self) {
        let runtime = JsRuntime::new();
        let Ok(mut ctx) = runtime.create_context() else {
            self.js = None;
            return;
        };
        let storage = self.storage_binder.as_ref().and_then(|b| b(&self.url));
        self.fetch_queue = FetchQueue::default();
        self.sync_viewport();
        self.shared.url.replace(self.url.clone());
        ctx.set_host(Box::new(DocumentJsHost {
            document: self.document.clone(),
            cookie_jar: self.cookie_jar.clone(),
            storage,
            fetch_queue: self.fetch_queue.clone(),
            shared: Rc::clone(&self.shared),
            frame_registry: Rc::clone(&self.frame_registry),
            inserted_scripts: Vec::new(),
            user_agent: self.user_agent.clone(),
            languages: parse_accept_language(&self.accept_language),
        }));
        let shared = Rc::clone(&self.shared);
        ctx.set_eval_policy(move |source| shared.csp.check_eval(source));
        self.js = Some(ctx);
    }

    /// Report queued CSP violations: a console message each, and a
    /// `securitypolicyviolation` event at the element or document. `true` if any.
    pub fn report_csp_violations(&mut self) -> bool {
        let pending = self.shared.csp.take_pending();
        if pending.is_empty() {
            return false;
        }
        for record in pending {
            let v = &record.violation;
            let message = match Url::parse(&v.blocked_uri) {
                Ok(u) => v
                    .message()
                    .replace(&v.blocked_uri, &format!("{}{}", u.origin(), u.path)),
                Err(_) => v.message(),
            };
            log::warn!("[console] {message}");
            let Some(js) = self.js.as_mut() else {
                continue;
            };
            let init = axiom_js::CspViolationInit {
                document_uri: self.url.clone(),
                blocked_uri: v.blocked_uri.clone(),
                effective_directive: v.effective_directive.clone(),
                original_policy: v.original_policy.clone(),
                source_file: String::new(),
                sample: v.sample.clone(),
                disposition: match v.disposition {
                    axiom_csp::Disposition::Enforce => "enforce".into(),
                    axiom_csp::Disposition::Report => "report".into(),
                },
            };
            if let Err(e) = js.dispatch_csp_violation(record.target.map(|n| n.0), &init) {
                log::warn!("securitypolicyviolation dispatch: {e}");
            }
            flush_console(js);
        }
        true
    }

    pub fn update_rendering_if_needed(&mut self) {
        if self.render_blocked {
            return;
        }
        let dirty = self.document.borrow().dirty;
        if !dirty.any() && self.framebuffer.is_some() {
            return;
        }

        let viewport = (self.viewport_width as f32, self.viewport_height as f32);
        if self.render().viewport != viewport {
            self.render_mut().viewport = viewport;
            self.document.borrow_mut().dirty.style = true;
        }
        let flushed =
            render::flush_style_and_layout(&self.document, &self.shared, Some(&self.trace));
        if flushed.styled {
            self.hud.style_ms = flushed.style_ms;
        }
        if flushed.laid_out {
            self.hud.layout_ms = flushed.layout_ms;
        }

        let dirty = self.document.borrow().dirty;
        if dirty.paint || self.display_list.commands.is_empty() {
            let t0 = Instant::now();
            let _span = self.trace.span(TraceKind::Paint, "paint");
            let shared = Rc::clone(&self.shared);
            if let Some(layout) = &shared.render.borrow().layout {
                self.content_height = layout.content_height.max(self.viewport_height as f32);
                self.hud.layout_nodes = layout.fragment_count();
                let images = &shared.render.borrow().css_images;
                self.display_list = build_display_list_with_images(layout, images);
            }
            let max_scroll = (self.content_height - self.viewport_height as f32).max(0.0);
            self.scroll_y = self.scroll_y.min(max_scroll);
            self.hud.paint_ms = t0.elapsed().as_secs_f64() * 1000.0;
            self.hud.paint_commands = self.display_list.commands.len();
            self.document.borrow_mut().dirty.composite = true;
        }
        if let Some(y) = self.shared.scroll_request.take() {
            let max_scroll = (self.content_height - self.viewport_height as f32).max(0.0);
            self.scroll_y = y.clamp(0.0, max_scroll);
            self.document.borrow_mut().dirty.composite = true;
        }
        self.shared.scroll_y.set(self.scroll_y);

        let dirty = self.document.borrow().dirty;
        if dirty.composite || self.framebuffer.is_none() {
            let t0 = Instant::now();
            let _span = self.trace.span(TraceKind::Composite, "composite+raster");
            self.compositor.clear();
            self.compositor.add_layer(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: self.viewport_width as f32,
                    height: self.content_height,
                },
                self.display_list.clone(),
            );
            self.compositor.set_scroll(1, 0.0, self.scroll_y);
            let fb = rasterize_region(
                &self.display_list,
                0.0,
                self.scroll_y.max(0.0).round(),
                self.viewport_width,
                self.viewport_height,
            );
            self.framebuffer = Some(fb);
            self.hud.composite_ms = t0.elapsed().as_secs_f64() * 1000.0;
            self.hud.gpu_ms = self.hud.composite_ms;
            self.hud.gpu_batches = self.compositor.layer_count();
        }

        self.hud.dom_nodes = self.document.borrow().len();
        self.hud.memory_mb =
            (self.hud.dom_nodes * 256 + self.hud.paint_commands * 64) as f64 / (1024.0 * 1024.0);
        self.hud.network_bytes = self.network_bytes;
        self.hud.network_requests = self.network_requests;
        self.document.borrow_mut().dirty.clear();
    }

    pub fn scroll_by(&mut self, dy: f32) {
        let max_scroll = (self.content_height - self.viewport_height as f32).max(0.0);
        self.set_scroll_y((self.scroll_y + dy).clamp(0.0, max_scroll));
    }

    /// Scroll the viewport to `y` (callers clamp).
    pub fn set_scroll_y(&mut self, y: f32) {
        self.scroll_y = y;
        self.shared.scroll_y.set(y);
        self.shared.scroll_request.set(None);
        self.document.borrow_mut().dirty.composite = true;
    }

    pub fn hit_test(&self, x: f32, y: f32) -> Option<NodeId> {
        self.render()
            .layout
            .as_ref()?
            .hit_test(x, y + self.scroll_y)
    }
}

pub struct DocumentJsHost {
    pub document: Rc<RefCell<Document>>,
    pub frame_registry: Rc<RefCell<FrameRegistry>>,
    pub cookie_jar: Option<Arc<dyn CookieJar>>,
    pub storage: Option<Arc<dyn WebStorageHost>>,
    pub fetch_queue: FetchQueue,
    pub shared: Rc<DocumentShared>,
    /// Inline scripts connected by the last insertion, run by the JS layer right away.
    pub inserted_scripts: Vec<(i32, String)>,
    pub user_agent: String,
    /// Language tags from `Accept-Language`, most preferred first.
    pub languages: Vec<String>,
}

/// Language tags of an `Accept-Language` value by descending quality (`*` and `q=0`
/// entries dropped; ties keep their order).
pub fn parse_accept_language(value: &str) -> Vec<String> {
    let mut tags: Vec<(String, f32)> = value
        .split(',')
        .filter_map(|item| {
            let mut parts = item.split(';');
            let tag = parts.next()?.trim();
            let q = parts
                .filter_map(|p| p.trim().strip_prefix("q="))
                .find_map(|q| q.trim().parse::<f32>().ok())
                .unwrap_or(1.0);
            (!tag.is_empty() && tag != "*" && q > 0.0).then(|| (tag.to_string(), q))
        })
        .collect();
    tags.sort_by(|a, b| b.1.total_cmp(&a.1));
    tags.into_iter().map(|(t, _)| t).collect()
}

impl DocumentJsHost {
    fn node(&self, id: i32) -> Option<NodeId> {
        (id >= 0 && (id as usize) < self.document.borrow().len()).then_some(NodeId(id as usize))
    }

    fn document_url(&self) -> String {
        self.shared.url.borrow().clone()
    }

    fn same_origin_frame(&self, iframe: i32) -> Option<FrameRealm> {
        let iframe = self.node(iframe)?;
        let realm = self.frame_registry.borrow().get(iframe)?;
        let parent = Origin::of_document(&self.document_url());
        (!realm.opaque_sandbox_origin && parent.is_same_origin(&realm.origin)).then_some(realm)
    }

    fn frame_node(realm: &FrameRealm, node: i32) -> Option<NodeId> {
        (node >= 0 && (node as usize) < realm.dom.borrow().len()).then_some(NodeId(node as usize))
    }

    /// Brings style and layout up to date for a script geometry or style query.
    fn flush_for_query(&self) {
        let outcome = render::flush_style_and_layout(&self.document, &self.shared, None);
        if outcome.styled || outcome.laid_out {
            self.shared.render.borrow_mut().forced_flushes += 1;
        }
    }

    fn is_connected(doc: &Document, node: NodeId) -> bool {
        doc.is_connected(node)
    }

    fn id(node: NodeId) -> i32 {
        node.0 as i32
    }

    /// Record the resources of `nodes` (and their subtrees) that are now connected.
    fn connected(&mut self, doc: &Document, nodes: &[NodeId]) {
        for &n in nodes {
            if Self::is_connected(doc, n) {
                collect_insertions(doc, n, &self.shared, &mut self.inserted_scripts);
            }
        }
    }

    /// The nodes an insertion of `node` adds: a fragment's children, or `node`.
    fn inserted_nodes(doc: &Document, node: NodeId) -> Vec<NodeId> {
        match doc.get(node).kind {
            NodeKind::DocumentFragment => doc.get(node).children.clone(),
            _ => vec![node],
        }
    }

    /// `markup` parsed as a fragment in the context of element `context` (a body element
    /// when `None`), imported into `doc` unattached. Parsed scripts are marked started so
    /// they never run.
    fn parse_markup(
        &self,
        doc: &mut Document,
        context: Option<NodeId>,
        markup: &str,
    ) -> Vec<NodeId> {
        let (tag, namespace) = match context {
            Some(c) => (
                doc.tag_name(c).unwrap_or("body").to_string(),
                match doc.namespace(c) {
                    Some(ns @ (Namespace::Html | Namespace::Svg | Namespace::MathMl)) => ns,
                    _ => Namespace::Null,
                },
            ),
            None => ("body".to_string(), Namespace::Html),
        };
        let (scratch, root) = axiom_html::parse_fragment(&tag, namespace, markup, true);
        let nodes: Vec<NodeId> = scratch
            .get(root)
            .children
            .iter()
            .map(|&c| doc.import_node(&scratch, c))
            .collect();
        self.mark_scripts_started(doc, &nodes);
        nodes
    }

    /// Copies a separately parsed document under a new inert Document node.
    fn adopt_parsed_document(&self, parsed: &Document) -> i32 {
        let Some(parsed_root) = parsed.document_id else {
            return -1;
        };
        let document = Rc::clone(&self.document);
        let mut doc = document.borrow_mut();
        let target = doc.create_document_node();
        let nodes: Vec<NodeId> = parsed
            .get(parsed_root)
            .children
            .iter()
            .map(|&c| doc.import_node(parsed, c))
            .collect();
        for &n in &nodes {
            doc.append_child(target, n);
        }
        self.mark_scripts_started(&doc, &nodes);
        Self::id(target)
    }

    /// Script-parsed markup never runs its scripts, even once inserted into the document.
    fn mark_scripts_started(&self, doc: &Document, roots: &[NodeId]) {
        let mut started = self.shared.started_scripts.borrow_mut();
        let mut stack = roots.to_vec();
        while let Some(id) = stack.pop() {
            if doc.tag_name(id) == Some("script") {
                started.insert(id);
            }
            stack.extend(doc.get(id).children.iter().copied());
        }
    }

    /// The fragment-parsing context for `node`: `None` (body) for non-elements and the
    /// HTML `html` element.
    fn markup_context(doc: &Document, node: NodeId) -> Option<NodeId> {
        let is_html_root =
            doc.tag_name(node) == Some("html") && doc.namespace(node) == Some(Namespace::Html);
        (doc.is_element(node) && !is_html_root).then_some(node)
    }

    /// Queue loads that a changed attribute starts on a connected element.
    fn attribute_changed(&self, doc: &Document, node: NodeId, local_name: &str) {
        if !Self::is_connected(doc, node) {
            return;
        }
        let name = local_name.to_ascii_lowercase();
        let insertion = match (doc.tag_name(node), name.as_str()) {
            (Some("img"), "src") => Some(DynamicInsertion::Image(node)),
            (Some("link"), "href" | "rel") => Some(DynamicInsertion::Stylesheet(node)),
            (Some("iframe"), "src" | "srcdoc") => Some(DynamicInsertion::Frame(node)),
            _ => None,
        };
        if let Some(i) = insertion {
            self.shared.insertions.borrow_mut().push(i);
        }
    }
}

/// Record the resources of a subtree that script just connected to the document.
fn collect_insertions(
    doc: &Document,
    root: NodeId,
    shared: &DocumentShared,
    inline_scripts: &mut Vec<(i32, String)>,
) {
    let mut started = shared.started_scripts.borrow_mut();
    let mut queue = shared.insertions.borrow_mut();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        match doc.tag_name(id) {
            Some("script") if !started.contains(&id) => {
                started.insert(id);
                let has_src = doc.attr(id, "src").is_some_and(|s| !s.trim().is_empty());
                let ty = doc.attr(id, "type");
                let module = ty.is_some_and(|t| {
                    let t = t.trim();
                    t.eq_ignore_ascii_case("module") || t.eq_ignore_ascii_case("importmap")
                });
                if has_src || module {
                    queue.push(DynamicInsertion::Script(id));
                } else if is_classic_script_type(ty) {
                    let text = doc.text_content(id);
                    let nonce = element_nonce(doc, id);
                    if !text.trim().is_empty()
                        && shared.csp.check_inline(
                            axiom_csp::InlineKind::Script,
                            Some(id),
                            &text,
                            nonce.as_deref(),
                        )
                    {
                        inline_scripts.push((id.0 as i32, text));
                    }
                }
            }
            Some("link") => queue.push(DynamicInsertion::Stylesheet(id)),
            Some("img") if doc.attr(id, "src").is_some() => queue.push(DynamicInsertion::Image(id)),
            Some("iframe") => queue.push(DynamicInsertion::Frame(id)),
            Some("style") => queue.push(DynamicInsertion::Style(id)),
            _ => {}
        }
        stack.extend(doc.get(id).children.iter().rev().copied());
        if let Some(shadow) = doc.shadow_root(id) {
            stack.push(shadow.root);
        }
    }
}

impl JsHost for DocumentJsHost {
    fn get_attribute(&mut self, node: i32, name: &str) -> Option<String> {
        let node = self.node(node)?;
        let doc = self.document.borrow();
        doc.attributes(node).get(name).map(str::to_string)
    }

    fn remove_attribute(&mut self, node: i32, name: &str) {
        if let Some(node) = self.node(node) {
            self.document.borrow_mut().remove_attr_exact(node, name);
        }
    }

    fn tag_name(&mut self, node: i32) -> String {
        let Some(node) = self.node(node) else {
            return String::new();
        };
        let doc = self.document.borrow();
        match &doc.get(node).kind {
            NodeKind::Element { tag, .. } => tag.clone(),
            NodeKind::Doctype { name, .. } => name.clone(),
            NodeKind::ProcessingInstruction { target, .. } => target.clone(),
            _ => String::new(),
        }
    }

    fn create_text_node(&mut self, data: &str) -> i32 {
        Self::id(self.document.borrow_mut().create_text(data))
    }

    fn create_cdata_section(&mut self, data: &str) -> i32 {
        Self::id(self.document.borrow_mut().create_cdata(data))
    }

    fn create_comment(&mut self, data: &str) -> i32 {
        Self::id(self.document.borrow_mut().create_comment(data))
    }

    fn create_processing_instruction(&mut self, target: &str, data: &str) -> i32 {
        let id = self
            .document
            .borrow_mut()
            .create_processing_instruction(target, data);
        Self::id(id)
    }

    fn create_document_fragment(&mut self) -> i32 {
        Self::id(self.document.borrow_mut().create_document_fragment())
    }

    fn create_element_ns(
        &mut self,
        namespace: Option<&str>,
        qualified_name: &str,
    ) -> Result<i32, String> {
        let name = axiom_dom::validate_and_extract(
            namespace,
            qualified_name,
            axiom_dom::NameContext::Element,
        )
        .map_err(|e| e.name().to_string())?;
        Ok(Self::id(
            self.document.borrow_mut().create_element_qualified(&name),
        ))
    }

    fn insert_before(&mut self, parent: i32, node: i32, child: i32) -> Result<(), String> {
        let (Some(parent), Some(node)) = (self.node(parent), self.node(node)) else {
            return Err("NotFoundError".into());
        };
        let child = if child < 0 {
            None
        } else {
            Some(self.node(child).ok_or("NotFoundError")?)
        };
        let document = Rc::clone(&self.document);
        let mut doc = document.borrow_mut();
        let added = Self::inserted_nodes(&doc, node);
        doc.pre_insert(node, parent, child)
            .map_err(|e| e.name().to_string())?;
        self.connected(&doc, &added);
        Ok(())
    }

    fn replace_child(&mut self, parent: i32, node: i32, child: i32) -> Result<(), String> {
        let (Some(parent), Some(node), Some(child)) =
            (self.node(parent), self.node(node), self.node(child))
        else {
            return Err("NotFoundError".into());
        };
        let document = Rc::clone(&self.document);
        let mut doc = document.borrow_mut();
        let added = Self::inserted_nodes(&doc, node);
        doc.replace_child_checked(parent, node, child)
            .map_err(|e| e.name().to_string())?;
        self.connected(&doc, &added);
        Ok(())
    }

    fn create_document(&mut self) -> i32 {
        Self::id(self.document.borrow_mut().create_document_node())
    }

    fn parse_html_document(&mut self, markup: &str) -> i32 {
        match axiom_html::parse_html_with_scripting(markup, false) {
            Ok(parsed) => self.adopt_parsed_document(&parsed),
            Err(_) => -1,
        }
    }

    fn parse_xml_document(&mut self, markup: &str) -> i32 {
        self.adopt_parsed_document(&axiom_xml::parse_for_dom_parser(markup))
    }

    fn clone_node(&mut self, node: i32, deep: bool) -> i32 {
        match self.node(node) {
            Some(n) => Self::id(self.document.borrow_mut().clone_node(n, deep)),
            None => -1,
        }
    }

    fn namespace_uri(&mut self, node: i32) -> Option<String> {
        let node = self.node(node)?;
        let doc = self.document.borrow();
        let namespace = doc.namespace(node)?;
        doc.namespace_uri(namespace).map(str::to_string)
    }

    fn prefix(&mut self, node: i32) -> Option<String> {
        let node = self.node(node)?;
        self.document
            .borrow()
            .element_prefix(node)
            .map(str::to_string)
    }

    fn attributes(&mut self, node: i32) -> Vec<axiom_js::DomAttr> {
        let Some(node) = self.node(node) else {
            return Vec::new();
        };
        let doc = self.document.borrow();
        doc.attributes(node)
            .iter()
            .map(|a| axiom_js::DomAttr {
                namespace: a.namespace.clone(),
                prefix: a.prefix.clone(),
                local_name: a.local_name.clone(),
                value: a.value.clone(),
            })
            .collect()
    }

    fn get_attribute_ns(
        &mut self,
        node: i32,
        namespace: Option<&str>,
        local_name: &str,
    ) -> Option<String> {
        let node = self.node(node)?;
        let namespace = namespace.filter(|n| !n.is_empty());
        let doc = self.document.borrow();
        doc.attr_ns(node, namespace, local_name).map(str::to_string)
    }

    fn set_attribute_ns(
        &mut self,
        node: i32,
        namespace: Option<&str>,
        qualified_name: &str,
        value: &str,
    ) -> Result<(), String> {
        let node = self.node(node).ok_or("NotFoundError")?;
        let name = axiom_dom::validate_and_extract(
            namespace,
            qualified_name,
            axiom_dom::NameContext::Attribute,
        )
        .map_err(|e| e.name().to_string())?;
        let mut doc = self.document.borrow_mut();
        doc.set_attr_ns(
            node,
            name.namespace.as_deref(),
            name.prefix.as_deref(),
            &name.local_name,
            value,
        );
        if name.namespace.is_none() {
            self.attribute_changed(&doc, node, &name.local_name);
        }
        Ok(())
    }

    fn remove_attribute_ns(&mut self, node: i32, namespace: Option<&str>, local_name: &str) {
        if let Some(node) = self.node(node) {
            let namespace = namespace.filter(|n| !n.is_empty());
            self.document
                .borrow_mut()
                .remove_attr_ns(node, namespace, local_name);
        }
    }

    fn replace_attribute(&mut self, node: i32, attr: &axiom_js::DomAttr) {
        let Some(node) = self.node(node) else { return };
        let mut doc = self.document.borrow_mut();
        if !doc.is_element(node) {
            return;
        }
        let namespace = attr.namespace.as_deref().filter(|n| !n.is_empty());
        doc.replace_attr_ns(
            node,
            namespace,
            attr.prefix.as_deref(),
            &attr.local_name,
            &attr.value,
        );
        if namespace.is_none() {
            self.attribute_changed(&doc, node, &attr.local_name);
        }
    }

    fn validate_attribute_name(
        &mut self,
        namespace: Option<&str>,
        qualified_name: &str,
    ) -> Result<(Option<String>, Option<String>, String), String> {
        axiom_dom::validate_and_extract(
            namespace,
            qualified_name,
            axiom_dom::NameContext::Attribute,
        )
        .map(|q| (q.namespace, q.prefix, q.local_name))
        .map_err(|e| e.name().to_string())
    }

    fn matches_selectors(&mut self, node: i32, selectors: &str) -> Option<bool> {
        let node = self.node(node)?;
        let list = axiom_dom::parse_selector_list(selectors)?;
        let doc = self.document.borrow();
        Some(axiom_dom::matches(&doc, node, &list, Some(node)))
    }

    fn inner_html(&mut self, node: i32) -> String {
        match self.node(node) {
            Some(n) => axiom_html::serialize_children(&self.document.borrow(), n, true),
            None => String::new(),
        }
    }

    fn outer_html(&mut self, node: i32) -> String {
        match self.node(node) {
            Some(n) => axiom_html::serialize_outer(&self.document.borrow(), n, true),
            None => String::new(),
        }
    }

    fn set_inner_html(&mut self, node: i32, markup: &str) -> Result<(), String> {
        let node = self.node(node).ok_or("NotFoundError")?;
        let document = Rc::clone(&self.document);
        let mut doc = document.borrow_mut();
        let target = match doc.tag_name(node) {
            Some("template") if doc.namespace(node) == Some(Namespace::Html) => {
                doc.template_contents_or_create(node)
            }
            _ => node,
        };
        let context = doc.shadow_host(node).unwrap_or(node);
        let nodes = self.parse_markup(&mut doc, Some(context), markup);
        for child in doc.get(target).children.clone() {
            doc.remove_child(target, child);
        }
        for &n in &nodes {
            doc.append_child(target, n);
        }
        if target == node {
            self.connected(&doc, &nodes);
        }
        Ok(())
    }

    fn set_outer_html(&mut self, node: i32, markup: &str) -> Result<(), String> {
        let node = self.node(node).ok_or("NotFoundError")?;
        let document = Rc::clone(&self.document);
        let mut doc = document.borrow_mut();
        let Some(parent) = doc.get(node).parent else {
            return Ok(());
        };
        if matches!(doc.get(parent).kind, NodeKind::Document) {
            return Err("NoModificationAllowedError".into());
        }
        let context = Self::markup_context(&doc, parent);
        let nodes = self.parse_markup(&mut doc, context, markup);
        for &n in &nodes {
            doc.insert_before(parent, n, Some(node));
        }
        doc.remove_child(parent, node);
        self.connected(&doc, &nodes);
        Ok(())
    }

    fn insert_adjacent_html(
        &mut self,
        node: i32,
        position: &str,
        markup: &str,
    ) -> Result<(), String> {
        let node = self.node(node).ok_or("NotFoundError")?;
        let document = Rc::clone(&self.document);
        let mut doc = document.borrow_mut();
        let position = position.to_ascii_lowercase();
        let parent = doc.get(node).parent;
        let (context, target, reference) = match position.as_str() {
            "beforebegin" | "afterend" => {
                let parent = parent
                    .filter(|&p| !matches!(doc.get(p).kind, NodeKind::Document))
                    .ok_or("NoModificationAllowedError")?;
                let reference = if position == "beforebegin" {
                    Some(node)
                } else {
                    doc.next_sibling(node)
                };
                (Self::markup_context(&doc, parent), parent, reference)
            }
            "afterbegin" => (
                Self::markup_context(&doc, node),
                node,
                doc.get(node).children.first().copied(),
            ),
            "beforeend" => (Self::markup_context(&doc, node), node, None),
            _ => return Err("SyntaxError".into()),
        };
        let nodes = self.parse_markup(&mut doc, context, markup);
        for &n in &nodes {
            doc.insert_before(target, n, reference);
        }
        self.connected(&doc, &nodes);
        Ok(())
    }

    fn closest(&mut self, node: i32, selectors: &str) -> Option<i32> {
        let node = self.node(node)?;
        let list = axiom_dom::parse_selector_list(selectors)?;
        let doc = self.document.borrow();
        Some(axiom_dom::closest(&doc, node, &list).map_or(-1, |id| id.0 as i32))
    }

    fn template_content(&mut self, node: i32) -> Option<i32> {
        let node = self.node(node)?;
        let mut doc = self.document.borrow_mut();
        if doc.tag_name(node) != Some("template") || doc.namespace(node) != Some(Namespace::Html) {
            return None;
        }
        Some(doc.template_contents_or_create(node).0 as i32)
    }

    fn attach_shadow(&mut self, host: i32, flags: u32) -> i32 {
        use axiom_js::shadow_flags as f;
        let Some(host) = self.node(host) else {
            return -1;
        };
        let mode = if flags & f::OPEN != 0 {
            ShadowRootMode::Open
        } else {
            ShadowRootMode::Closed
        };
        let init = ShadowRootInit {
            delegates_focus: flags & f::DELEGATES_FOCUS != 0,
            slot_assignment: if flags & f::MANUAL_SLOTS != 0 {
                SlotAssignmentMode::Manual
            } else {
                SlotAssignmentMode::Named
            },
            clonable: flags & f::CLONABLE != 0,
            serializable: flags & f::SERIALIZABLE != 0,
            ..ShadowRootInit::new(mode)
        };
        let root = self.document.borrow_mut().attach_shadow(host, init);
        root.map_or(-1, Self::id)
    }

    fn shadow_root_info(&mut self, host: i32) -> Option<(i32, u32)> {
        use axiom_js::shadow_flags as f;
        let host = self.node(host)?;
        let doc = self.document.borrow();
        let shadow = doc.shadow_root(host)?;
        let init = shadow.init;
        let flags = [
            (init.mode == ShadowRootMode::Open, f::OPEN),
            (init.delegates_focus, f::DELEGATES_FOCUS),
            (
                init.slot_assignment == SlotAssignmentMode::Manual,
                f::MANUAL_SLOTS,
            ),
            (init.clonable, f::CLONABLE),
            (init.serializable, f::SERIALIZABLE),
        ]
        .iter()
        .filter(|(set, _)| *set)
        .fold(0, |acc, (_, bit)| acc | bit);
        Some((Self::id(shadow.root), flags))
    }

    fn shadow_host(&mut self, root: i32) -> i32 {
        self.node(root)
            .and_then(|root| self.document.borrow().shadow_host(root))
            .map_or(-1, Self::id)
    }

    fn has_shadow_trees(&mut self) -> bool {
        self.document.borrow().has_shadow_trees()
    }

    fn connected_shadow_roots(&mut self) -> Vec<i32> {
        let doc = self.document.borrow();
        doc.connected_shadow_roots()
            .into_iter()
            .map(|(_, root)| Self::id(root))
            .collect()
    }

    fn assigned_nodes(&mut self, slot: i32, flatten: bool) -> Vec<i32> {
        let Some(slot) = self.node(slot) else {
            return Vec::new();
        };
        let doc = self.document.borrow();
        let nodes = if flatten {
            doc.flattened_assigned_nodes(slot)
        } else {
            doc.assigned_nodes(slot)
        };
        nodes.into_iter().map(Self::id).collect()
    }

    fn assigned_slot(&mut self, node: i32) -> i32 {
        self.node(node)
            .and_then(|node| self.document.borrow().assigned_slot(node))
            .map_or(-1, Self::id)
    }

    fn assign_slot(&mut self, slot: i32, nodes: Vec<i32>) {
        let Some(slot) = self.node(slot) else {
            return;
        };
        let nodes = nodes.into_iter().filter_map(|n| self.node(n)).collect();
        self.document.borrow_mut().assign_slot(slot, nodes);
    }

    fn set_custom_element_defined(&mut self, node: i32) {
        if let Some(node) = self.node(node) {
            self.document.borrow_mut().set_custom_element_defined(node);
        }
    }

    fn create_doctype(&mut self, name: &str, public_id: &str, system_id: &str) -> i32 {
        let id = self
            .document
            .borrow_mut()
            .create_doctype_with_ids(name, public_id, system_id);
        Self::id(id)
    }

    fn dom_version(&mut self) -> Option<u64> {
        Some(self.document.borrow().version())
    }

    fn doctype_ids(&mut self, node: i32) -> (String, String) {
        let Some(node) = self.node(node) else {
            return Default::default();
        };
        match &self.document.borrow().get(node).kind {
            NodeKind::Doctype {
                public_id,
                system_id,
                ..
            } => (public_id.clone(), system_id.clone()),
            _ => Default::default(),
        }
    }

    fn document_node(&mut self, which: &str) -> i32 {
        let doc = self.document.borrow();
        let node = match which {
            "body" => doc.body(),
            "head" => doc.head(),
            "documentElement" => doc.document_element(),
            _ => None,
        };
        node.map_or(-1, |n| n.0 as i32)
    }

    fn document_root(&mut self) -> i32 {
        self.document
            .borrow()
            .document_id
            .map_or(-1, |n| n.0 as i32)
    }

    fn node_type(&mut self, node: i32) -> u16 {
        let Some(node) = self.node(node) else {
            return 0;
        };
        match self.document.borrow().get(node).kind {
            NodeKind::Element { .. } => 1,
            NodeKind::Text { .. } => 3,
            NodeKind::CData { .. } => 4,
            NodeKind::ProcessingInstruction { .. } => 7,
            NodeKind::Comment { .. } => 8,
            NodeKind::Document => 9,
            NodeKind::Doctype { .. } => 10,
            NodeKind::DocumentFragment => 11,
        }
    }

    fn parent_node(&mut self, node: i32) -> i32 {
        self.node(node)
            .and_then(|n| self.document.borrow().get(n).parent)
            .map_or(-1, |p| p.0 as i32)
    }

    fn child_nodes(&mut self, node: i32) -> Vec<i32> {
        let Some(node) = self.node(node) else {
            return Vec::new();
        };
        let doc = self.document.borrow();
        doc.get(node).children.iter().map(|c| c.0 as i32).collect()
    }

    fn remove_child(&mut self, parent: i32, child: i32) -> bool {
        let (Some(parent), Some(child)) = (self.node(parent), self.node(child)) else {
            return false;
        };
        self.document.borrow_mut().remove_child(parent, child)
    }

    fn elements_by_tag_name(&mut self, root: i32, name: &str) -> Vec<i32> {
        let Some(root) = self.node(root) else {
            return Vec::new();
        };
        let doc = self.document.borrow();
        let lower = name.to_ascii_lowercase();
        doc.descendants(root)
            .into_iter()
            .filter(|&id| {
                let Some(qualified) = doc.qualified_name(id) else {
                    return false;
                };
                let wanted = match doc.namespace(id) {
                    Some(Namespace::Html) => &lower,
                    _ => name,
                };
                name == "*" || qualified == *wanted
            })
            .map(|id| id.0 as i32)
            .collect()
    }

    fn query_selector_all(&mut self, root: i32, selectors: &str) -> Vec<i32> {
        let Some(root) = self.node(root) else {
            return Vec::new();
        };
        let doc = self.document.borrow();
        axiom_dom::query_selector_all(&doc, root, selectors)
            .into_iter()
            .map(|id| id.0 as i32)
            .collect()
    }

    fn ready_state(&mut self) -> String {
        self.shared.ready_state.get().as_str().to_string()
    }

    fn take_inserted_scripts(&mut self) -> Vec<(i32, String)> {
        std::mem::take(&mut self.inserted_scripts)
    }

    fn csp_allows_handler(&mut self, node: i32, source: &str) -> bool {
        let target = self.node(node);
        self.shared
            .csp
            .check_inline(axiom_csp::InlineKind::ScriptAttribute, target, source, None)
    }

    fn resolve_url(&mut self, input: &str) -> Option<String> {
        resolve_url(&self.document_url(), input)
    }

    fn control_value(&mut self, node: i32) -> String {
        let Some(node) = self.node(node) else {
            return String::new();
        };
        self.shared
            .forms
            .borrow()
            .value(&self.document.borrow(), node)
    }

    fn set_control_value(&mut self, node: i32, value: &str) {
        let Some(node) = self.node(node) else {
            return;
        };
        let mut doc = self.document.borrow_mut();
        if self
            .shared
            .forms
            .borrow_mut()
            .set_value(&mut doc, node, value)
        {
            doc.mark_dirty(DirtyFlags::all());
        }
    }

    fn control_checked(&mut self, node: i32) -> bool {
        let Some(node) = self.node(node) else {
            return false;
        };
        let doc = self.document.borrow();
        let forms = self.shared.forms.borrow();
        match doc.tag_name(node) {
            Some("option") => forms.option_selected(&doc, node),
            _ => forms.checked(&doc, node),
        }
    }

    fn set_control_checked(&mut self, node: i32, checked: bool) {
        let Some(node) = self.node(node) else {
            return;
        };
        let mut doc = self.document.borrow_mut();
        let mut forms = self.shared.forms.borrow_mut();
        match doc.tag_name(node) {
            Some("option") => forms.set_option_selected(&doc, node, checked),
            _ => forms.set_checked(&doc, node, checked),
        }
        doc.mark_dirty(DirtyFlags::all());
    }

    fn selected_index(&mut self, select: i32) -> i32 {
        let Some(select) = self.node(select) else {
            return -1;
        };
        self.shared
            .forms
            .borrow()
            .selected_index(&self.document.borrow(), select)
    }

    fn form_owner(&mut self, node: i32) -> i32 {
        self.node(node)
            .and_then(|n| forms::form_owner(&self.document.borrow(), n))
            .map_or(-1, Self::id)
    }

    fn form_controls(&mut self, form: i32) -> Vec<i32> {
        let Some(form) = self.node(form) else {
            return Vec::new();
        };
        forms::form_controls(&self.document.borrow(), form)
            .into_iter()
            .map(Self::id)
            .collect()
    }

    fn control_validity(&mut self, node: i32) -> (bool, String, String) {
        let Some(node) = self.node(node) else {
            return (false, String::new(), String::new());
        };
        let doc = self.document.borrow();
        let forms = self.shared.forms.borrow();
        (
            forms.will_validate(&doc, node),
            forms
                .invalidity(&doc, node)
                .map_or("", |i| i.as_str())
                .to_string(),
            forms.validation_message(&doc, node),
        )
    }

    fn set_custom_validity(&mut self, node: i32, message: &str) {
        if let Some(node) = self.node(node) {
            self.shared
                .forms
                .borrow_mut()
                .set_custom_validity(node, message);
        }
    }

    fn invalid_controls(&mut self, form: i32) -> Vec<i32> {
        let Some(form) = self.node(form) else {
            return Vec::new();
        };
        self.shared
            .forms
            .borrow()
            .invalid_controls(&self.document.borrow(), form)
            .into_iter()
            .map(Self::id)
            .collect()
    }

    fn form_entries(&mut self, form: i32, submitter: i32) -> Vec<(String, String, bool)> {
        let Some(form) = self.node(form) else {
            return Vec::new();
        };
        let submitter = self.node(submitter);
        self.shared
            .forms
            .borrow()
            .entry_list(&self.document.borrow(), form, submitter)
            .into_iter()
            .map(|(name, value)| match value {
                forms::EntryValue::Text(t) => (name, t, false),
                forms::EntryValue::File { filename } => (name, filename, true),
            })
            .collect()
    }

    fn submit_form(&mut self, form: i32, submitter: i32) -> Result<(), String> {
        let form = self.node(form).ok_or("no such form")?;
        let submitter = self.node(submitter);
        let mut doc = self.document.borrow_mut();
        let mut forms = self.shared.forms.borrow_mut();
        let planned = forms.plan_submission(&mut doc, form, submitter, &self.document_url())?;
        match planned {
            Some(submission) => forms.queue_submission(submission),
            None => doc.mark_dirty(DirtyFlags::all()),
        }
        Ok(())
    }

    fn reset_form(&mut self, form: i32) {
        if let Some(form) = self.node(form) {
            let mut doc = self.document.borrow_mut();
            self.shared.forms.borrow_mut().reset(&doc, form);
            doc.mark_dirty(DirtyFlags::all());
        }
    }

    fn resolve_url_against(&mut self, base: &str, input: &str) -> Option<String> {
        resolve_url(base, input)
    }

    fn resolve_module_specifier(
        &mut self,
        base: Option<&str>,
        specifier: &str,
    ) -> Result<String, String> {
        let document_url = self.document_url();
        let base = base.unwrap_or(&document_url);
        self.shared.import_map.borrow().resolve(specifier, base)
    }

    fn fetch_start(&mut self, mut init: axiom_js::FetchInit) -> Result<u64, String> {
        let csp = &self.shared.csp;
        if csp.is_active() {
            if let Some(mut resolved) = resolve_url(&self.document_url(), &init.url) {
                if let Some(upgraded) = Url::parse(&resolved).ok().and_then(|u| csp.upgrade(&u)) {
                    resolved = upgraded.as_str();
                    init.url.clone_from(&resolved);
                }
                let allowed = axiom_csp::CspUrl::parse(&resolved).is_none_or(|target| {
                    csp.check_request(
                        axiom_csp::FetchDirective::ConnectSrc,
                        &target,
                        None,
                        false,
                        None,
                    )
                });
                if !allowed {
                    return Err("Failed to fetch".into());
                }
            }
        }
        let mut plan = plan_fetch(&init, &self.document_url()).inspect_err(|e| {
            log::warn!("fetch({}) rejected: {e}", init.url);
        })?;
        if let FetchPlan::Network { request, .. } = &mut plan {
            request.redirect_check =
                csp.redirect_check(axiom_csp::FetchDirective::ConnectSrc, None, false, None);
        }
        let id = plan.id();
        let mut state = self.fetch_queue.borrow_mut();
        if let FetchPlan::Network {
            request, upload, ..
        } = &mut plan
        {
            if let Some(sender) = upload.take() {
                state.uploads.insert(
                    id,
                    UploadSlot {
                        sender,
                        wants_drain: false,
                    },
                );
            }
            if let Some(flow) = &request.flow {
                state.flows.insert(id, flow.clone());
            }
        }
        state.commands.push_back(FetchCommand::Start(plan));
        Ok(id)
    }

    fn fetch_abort(&mut self, id: u64) {
        let mut state = self.fetch_queue.borrow_mut();
        state.finish(id);
        state.commands.push_back(FetchCommand::Abort(id));
    }

    fn fetch_upload_write(&mut self, id: u64, bytes: Vec<u8>) -> Result<bool, String> {
        let mut state = self.fetch_queue.borrow_mut();
        let slot = state
            .uploads
            .get_mut(&id)
            .ok_or_else(|| "the request is no longer active".to_string())?;
        if let Err(e) = slot.sender.write(bytes) {
            state.uploads.remove(&id);
            return Err(e.to_string());
        }
        let below = slot.sender.buffered() < UPLOAD_HIGH_WATER;
        slot.wants_drain = !below;
        Ok(below)
    }

    fn fetch_upload_close(&mut self, id: u64) {
        if let Some(slot) = self.fetch_queue.borrow_mut().uploads.remove(&id) {
            slot.sender.close();
        }
    }

    fn fetch_upload_error(&mut self, id: u64, message: &str) {
        if let Some(slot) = self.fetch_queue.borrow_mut().uploads.remove(&id) {
            slot.sender.error(message);
        }
    }

    fn fetch_consumed(&mut self, id: u64, bytes: u64) {
        if let Some(flow) = self.fetch_queue.borrow().flows.get(&id) {
            flow.release(bytes);
        }
    }

    fn fetch_referrer(&mut self, url: &str) -> String {
        resolve_referrer(&self.document_url(), url)
    }

    fn get_element_by_id(&mut self, id: &str) -> i32 {
        let doc = self.document.borrow();
        let Some(root) = doc.document_id else {
            return -1;
        };
        doc.descendants(root)
            .into_iter()
            .find(|&n| doc.is_element(n) && doc.attributes(n).get("id") == Some(id))
            .map_or(-1, Self::id)
    }

    fn query_selector(&mut self, sel: &str) -> i32 {
        self.document
            .borrow()
            .query_selector(sel)
            .map(|n| n.0 as i32)
            .unwrap_or(-1)
    }

    fn frame_info(&mut self, iframe: i32) -> Option<FrameInfo> {
        let iframe = self.node(iframe)?;
        let realm = self.frame_registry.borrow().get(iframe)?;
        let parent = Origin::of_document(&self.document_url());
        let same_origin = !realm.opaque_sandbox_origin && parent.is_same_origin(&realm.origin);
        Some(FrameInfo {
            browsing_context_id: realm.browsing_context.0,
            document_id: realm.document.0,
            same_origin,
            url: same_origin.then(|| realm.shared.url.borrow().clone()),
            content_type: same_origin.then(|| {
                let content_type = realm.shared.meta.borrow().content_type.clone();
                if content_type.is_empty() {
                    "text/html".into()
                } else {
                    content_type
                }
            }),
        })
    }

    fn frame_get_element_by_id(&mut self, iframe: i32, id: &str) -> i32 {
        let Some(realm) = self.same_origin_frame(iframe) else {
            return -1;
        };
        let doc = realm.dom.borrow();
        let Some(root) = doc.document_id else {
            return -1;
        };
        doc.descendants(root)
            .into_iter()
            .find(|&node| doc.is_element(node) && doc.attributes(node).get("id") == Some(id))
            .map_or(-1, Self::id)
    }

    fn frame_query_selector(&mut self, iframe: i32, selector: &str) -> i32 {
        self.same_origin_frame(iframe)
            .and_then(|realm| realm.dom.borrow().query_selector(selector))
            .map_or(-1, Self::id)
    }

    fn frame_document_node(&mut self, iframe: i32, which: &str) -> i32 {
        let Some(realm) = self.same_origin_frame(iframe) else {
            return -1;
        };
        let doc = realm.dom.borrow();
        let node = match which {
            "body" => doc.body(),
            "head" => doc.head(),
            "documentElement" => doc.document_element(),
            _ => None,
        };
        node.map_or(-1, Self::id)
    }

    fn frame_node_text(&mut self, iframe: i32, node: i32) -> String {
        let Some(realm) = self.same_origin_frame(iframe) else {
            return String::new();
        };
        let Some(node) = Self::frame_node(&realm, node) else {
            return String::new();
        };
        let text = realm.dom.borrow().text_content(node);
        text
    }

    fn frame_node_tag_name(&mut self, iframe: i32, node: i32) -> String {
        let Some(realm) = self.same_origin_frame(iframe) else {
            return String::new();
        };
        let Some(node) = Self::frame_node(&realm, node) else {
            return String::new();
        };
        let doc = realm.dom.borrow();
        let tag = doc.tag_name(node).unwrap_or_default();
        if doc.namespace(node) == Some(Namespace::Html) {
            tag.to_ascii_uppercase()
        } else {
            tag.to_string()
        }
    }

    fn frame_node_namespace_uri(&mut self, iframe: i32, node: i32) -> Option<String> {
        let realm = self.same_origin_frame(iframe)?;
        let node = Self::frame_node(&realm, node)?;
        let dom = realm.dom.borrow();
        dom.namespace(node)
            .and_then(|namespace| dom.namespace_uri(namespace))
            .map(str::to_string)
    }

    fn frame_create_element(&mut self, iframe: i32, tag: &str) -> i32 {
        let Some(realm) = self.same_origin_frame(iframe) else {
            return -1;
        };
        let content_type = {
            let content_type = realm.shared.meta.borrow().content_type.clone();
            if content_type.is_empty() {
                "text/html".to_string()
            } else {
                content_type
            }
        };
        let namespace = if content_type.eq_ignore_ascii_case("text/html")
            || content_type.eq_ignore_ascii_case("application/xhtml+xml")
        {
            Namespace::Html
        } else {
            Namespace::Null
        };
        let id = realm.dom.borrow_mut().create_element_ns(tag, namespace).0 as i32;
        id
    }

    fn frame_post_message(
        &mut self,
        iframe: i32,
        data_json: &str,
        target_origin: &str,
    ) -> Result<(), String> {
        let iframe = self
            .node(iframe)
            .ok_or_else(|| "the iframe is no longer connected".to_string())?;
        let realm = self
            .frame_registry
            .borrow()
            .get(iframe)
            .ok_or_else(|| "the frame has no active browsing context".to_string())?;
        let target = target_origin.trim();
        let target = if target == "/" {
            Origin::of_document(&self.document_url()).serialize()
        } else {
            target.to_string()
        };
        let expected = realm.origin.serialize();
        if target != "*" && target != expected {
            // HTML postMessage silently drops a message when the target origin no
            // longer matches the target's active document.
            return Ok(());
        }
        realm.inbox.borrow_mut().push(FrameMessage {
            data_json: data_json.to_string(),
            origin: Origin::of_document(&self.document_url()).serialize(),
        });
        Ok(())
    }

    fn create_element(&mut self, tag: &str) -> i32 {
        let id = self.document.borrow_mut().create_element(tag);
        id.0 as i32
    }

    fn set_text_content(&mut self, node: i32, text: &str) {
        if node >= 0 {
            self.document
                .borrow_mut()
                .set_text_content(NodeId(node as usize), text);
        }
    }

    fn get_text_content(&mut self, node: i32) -> String {
        let Some(node) = self.node(node) else {
            return String::new();
        };
        let doc = self.document.borrow();
        match &doc.get(node).kind {
            NodeKind::CData { data }
            | NodeKind::Comment { data }
            | NodeKind::ProcessingInstruction { data, .. } => data.clone(),
            _ => doc.text_content(node),
        }
    }

    fn set_attribute(&mut self, node: i32, name: &str, value: &str) {
        let Some(node) = self.node(node) else {
            return;
        };
        let mut doc = self.document.borrow_mut();
        doc.set_attr_exact(node, name, value);
        self.attribute_changed(&doc, node, name);
    }

    fn append_child(&mut self, parent: i32, child: i32) {
        let (Some(parent), Some(child)) = (self.node(parent), self.node(child)) else {
            return;
        };
        let document = Rc::clone(&self.document);
        let mut doc = document.borrow_mut();
        if parent == child || doc.contains(child, parent) {
            return; // would create a cycle
        }
        doc.append_child(parent, child);
        if Self::is_connected(&doc, child) {
            collect_insertions(&doc, child, &self.shared, &mut self.inserted_scripts);
        }
    }

    fn log(&mut self, msg: &str) {
        log::info!("[console] {msg}");
    }

    fn add_event_listener(&mut self, node: i32, type_: &str) {
        log::debug!("JS listener registered on node {node} for '{type_}'");
    }

    fn user_agent(&mut self) -> String {
        self.user_agent.clone()
    }

    fn languages(&mut self) -> Vec<String> {
        self.languages.clone()
    }

    fn cookie_enabled(&mut self) -> bool {
        self.cookie_jar.is_some()
    }

    fn match_media(&mut self, query: &str) -> bool {
        let (width, height) = self.shared.viewport.get();
        axiom_style::media_matches(query, &axiom_style::MediaEnvironment { width, height })
    }

    fn viewport_size(&mut self) -> (f32, f32) {
        self.shared.viewport.get()
    }

    fn focused_element(&mut self) -> i32 {
        let doc = self.document.borrow();
        doc.focus
            .filter(|&n| doc.is_connected(n))
            .map_or(-1, |n| n.0 as i32)
    }

    fn set_focus(&mut self, node: i32) {
        let node = if node < 0 { None } else { self.node(node) };
        let mut doc = self.document.borrow_mut();
        if doc.focus != node {
            doc.focus = node;
            doc.dirty.paint = true;
        }
    }

    fn css_supports(&mut self, condition: &str) -> bool {
        axiom_style::supports_matches(condition)
    }

    fn document_info(&mut self, key: &str) -> String {
        let meta = self.shared.meta.borrow();
        let or = |value: &str, default: &str| {
            if value.is_empty() {
                default.to_string()
            } else {
                value.to_string()
            }
        };
        match key {
            "referrer" => meta.referrer.clone(),
            "contentType" => or(&meta.content_type, "text/html"),
            "characterSet" => or(&meta.character_set, "UTF-8"),
            "lastModified" => meta
                .last_modified
                .map_or_else(String::new, |s| s.to_string()),
            "compatMode" => match self.document.borrow().quirks_mode {
                axiom_dom::QuirksMode::Quirks => "BackCompat".to_string(),
                _ => "CSS1Compat".to_string(),
            },
            _ => String::new(),
        }
    }

    fn history_length(&mut self) -> u32 {
        self.shared.history_length.get() as u32
    }

    fn history_state(&mut self) -> Option<u64> {
        self.shared.history_state.get()
    }

    fn history_push(
        &mut self,
        url: Option<&str>,
        state: u64,
        replace: bool,
    ) -> Result<String, &'static str> {
        let current = self.document_url();
        let url = match url {
            None => current.clone(),
            Some(input) => resolve_url(&current, input).ok_or("SyntaxError")?,
        };
        if !can_rewrite_url(&current, &url) {
            return Err("SecurityError");
        }
        let shared = &self.shared;
        shared.url.replace(url.clone());
        shared.history_state.set(Some(state));
        if !replace {
            let index = shared.history_index.get() + 1;
            shared.history_index.set(index);
            shared.history_length.set(index + 1);
        }
        shared.history_ops.borrow_mut().push(HistoryOp::Push {
            url: url.clone(),
            state,
            replace,
        });
        Ok(url)
    }

    fn history_traverse(&mut self, delta: i32) {
        let op = if delta == 0 {
            HistoryOp::Reload
        } else {
            HistoryOp::Traverse(delta)
        };
        self.shared.history_ops.borrow_mut().push(op);
    }

    fn navigate(&mut self, url: &str, replace: bool) -> bool {
        let Some(url) = resolve_url(&self.document_url(), url) else {
            return false;
        };
        self.shared
            .history_ops
            .borrow_mut()
            .push(HistoryOp::Navigate { url, replace });
        true
    }

    fn reload(&mut self) {
        self.shared.history_ops.borrow_mut().push(HistoryOp::Reload);
    }

    fn computed_value(&mut self, node: i32, pseudo: &str, property: &str) -> Option<String> {
        let node = self.node(node)?;
        if !pseudo.is_empty() {
            return None;
        }
        self.flush_for_query();
        let doc = self.document.borrow();
        let render = self.shared.render.borrow();
        render::resolved_value(&doc, &render, node, property)
    }

    fn computed_property_names(&mut self) -> Vec<String> {
        axiom_style::serialize::COMPUTED_PROPERTIES
            .iter()
            .map(|p| p.to_string())
            .collect()
    }

    fn element_geometry(&mut self, node: i32) -> Option<axiom_js::ElementGeometry> {
        let node = self.node(node)?;
        self.flush_for_query();
        let doc = self.document.borrow();
        let render = self.shared.render.borrow();
        render::element_geometry(&doc, &render, node, self.shared.scroll_y.get())
    }

    fn scroll_position(&mut self) -> (f64, f64) {
        (0.0, f64::from(self.shared.scroll_y.get()))
    }

    fn scroll_to(&mut self, _x: f64, y: f64) {
        self.flush_for_query();
        let (_, viewport_height) = self.shared.viewport.get();
        let content_height = self
            .shared
            .render
            .borrow()
            .layout
            .as_ref()
            .map_or(0.0, |l| l.content_height);
        let max_scroll = (content_height - viewport_height).max(0.0);
        let y = (y as f32).clamp(0.0, max_scroll);
        self.shared.scroll_y.set(y);
        self.shared.scroll_request.set(Some(y));
        self.document.borrow_mut().dirty.composite = true;
    }

    fn image_natural_size(&mut self, node: i32) -> Option<(u32, u32)> {
        let node = self.node(node)?;
        let render = self.shared.render.borrow();
        render.images.get(&node).map(|img| (img.width, img.height))
    }

    fn get_document_cookie(&mut self) -> String {
        match &self.cookie_jar {
            Some(j) => j.cookies_for_script(&self.document_url()),
            None => String::new(),
        }
    }

    fn set_document_cookie(&mut self, value: &str) {
        if let Some(j) = &self.cookie_jar {
            j.set_cookie_from_script(&self.document_url(), value);
        }
    }

    fn storage_get(&mut self, session: bool, key: &str) -> Option<String> {
        let Some(s) = &self.storage else {
            return None;
        };
        if session {
            s.session_get(key)
        } else {
            s.local_get(key)
        }
    }

    fn storage_set(&mut self, session: bool, key: &str, value: &str) -> Result<(), String> {
        let Some(s) = &self.storage else {
            return Err("SecurityError".into());
        };
        if session {
            s.session_set(key, value)
        } else {
            s.local_set(key, value)
        }
    }

    fn storage_remove(&mut self, session: bool, key: &str) {
        if let Some(s) = &self.storage {
            if session {
                s.session_remove(key);
            } else {
                s.local_remove(key);
            }
        }
    }

    fn storage_clear(&mut self, session: bool) {
        if let Some(s) = &self.storage {
            if session {
                s.session_clear();
            } else {
                s.local_clear();
            }
        }
    }

    fn storage_length(&mut self, session: bool) -> usize {
        match &self.storage {
            Some(s) if session => s.session_length(),
            Some(s) => s.local_length(),
            None => 0,
        }
    }

    fn storage_key(&mut self, session: bool, index: usize) -> Option<String> {
        match &self.storage {
            Some(s) if session => s.session_key(index),
            Some(s) => s.local_key(index),
            None => None,
        }
    }
}

pub use axiom_document::is_classic_script_type;
use axiom_document::unwrap_cdata;

fn flush_console(js: &JsContext) {
    for msg in js.take_console() {
        log::info!("[console] {msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::parse_accept_language;

    #[test]
    fn accept_language_tags_by_quality() {
        assert_eq!(parse_accept_language("en-US,en;q=0.9"), ["en-US", "en"]);
        assert_eq!(
            parse_accept_language("fr;q=0.5, de, *;q=0.1, x;q=0, nl;q=0.5"),
            ["de", "fr", "nl"]
        );
        assert!(parse_accept_language("").is_empty());
    }
}
