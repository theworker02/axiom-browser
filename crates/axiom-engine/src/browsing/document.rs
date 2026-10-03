//! The document loader: one [`DocumentLoad`] per committed document, driven by
//! [`BrowsingContext::pump_document`]. It streams HTML into the incremental parser, runs
//! scripts at their parser positions, and advances readyState, `DOMContentLoaded` and
//! `load` purely from resource and parser state.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use axiom_document::{
    classify_script, resolve_base_href, DocumentDiagnostics, DocumentEventKind, DocumentEventLog,
    DocumentInfo, DocumentKind, DocumentLifecycle, FontFace, ParsingState, PendingScript,
    ResourceDiagnostics, ResourceId, ResourceRegistry, ScriptElement, ScriptKind,
};
use axiom_dom::NodeId;
use axiom_html::{HtmlParser, ParseStep};
use axiom_loader::RequestId;
use axiom_net::{ResourceType, TextDecoder};

use super::BrowsingContext;
use crate::import_map::{ImportMap, ImportMapError};
use crate::lifecycle::DocumentReadyState;
use crate::page::{DecodedImage, DocumentShared, DynamicInsertion};

/// The policy of a `<meta http-equiv="Content-Security-Policy" content>` that is a child
/// of `<head>` (HTML "Content security policy state").
fn meta_csp(doc: &axiom_dom::Document, node: NodeId) -> Option<String> {
    let equiv = doc.attr(node, "http-equiv")?;
    if !equiv.trim().eq_ignore_ascii_case("content-security-policy") {
        return None;
    }
    let parent = doc.get(node).parent?;
    if doc.tag_name(parent) != Some("head") {
        return None;
    }
    doc.attr(node, "content").map(str::to_string)
}

/// Rendering waits at most this long for parser-inserted stylesheets.
const RENDER_BLOCK_TIMEOUT: Duration = Duration::from_secs(30);
/// Pump iterations per call (bounds the work one event-loop turn can do).
const MAX_PUMP_STEPS: usize = 100_000;

/// A script error, attributed to its resource.
#[derive(Debug, Clone)]
pub struct ScriptError {
    pub resource: Option<ResourceId>,
    /// Script URL, or "inline script".
    pub label: String,
    pub message: String,
}

/// Snapshot of a replaced document (bounded history for diagnostics).
#[derive(Debug, Clone)]
pub struct RetiredDocument {
    pub diagnostics: DocumentDiagnostics,
    pub resources: Vec<ResourceDiagnostics>,
    pub events: Vec<axiom_document::DocumentEvent>,
    pub script_errors: Vec<ScriptError>,
}

/// Processing state of one stylesheet resource (external sheet or `@import`).
#[derive(Debug, Default)]
pub(super) struct SheetState {
    pub parent: Option<ResourceId>,
    pub depth: u8,
    /// `<style>` element whose `@import` this is.
    pub inline_owner: Option<NodeId>,
    pub children: Vec<ResourceId>,
    pub own_text: String,
    /// All `@import`s were started (a synchronous child cannot finish the parent early).
    pub children_started: bool,
    /// Composed text (imports first) once ready.
    pub css: Option<String>,
}

#[derive(Debug)]
pub(super) struct PendingFace {
    pub face: FontFace,
    pub base: String,
    pub sheet: Option<ResourceId>,
    pub requested: bool,
    /// Index into `face.sources` of the source to fetch; failures move to the next one.
    pub source: usize,
}

/// Where a module script element is on its way to running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ModuleState {
    /// Its own source is being fetched.
    Fetching,
    /// Job started; its import graph is loading.
    Loading(u32),
    /// Graph loaded; runs now (async) or at the front of the defer queue.
    Ready(u32),
    /// Ran, or failed.
    Done,
}

#[derive(Debug)]
pub(super) struct ModuleScript {
    pub resource: Option<ResourceId>,
    pub label: String,
    /// Parser-inserted without `async`: runs in order with `defer` scripts.
    pub deferred: bool,
    pub state: ModuleState,
}

pub(super) struct DocumentLoad {
    pub info: DocumentInfo,
    pub lifecycle: DocumentLifecycle,
    pub registry: ResourceRegistry,
    pub events: DocumentEventLog,
    pub parser: HtmlParser,
    pub decoder: TextDecoder,
    /// XML-family documents are decoded while streaming, then parsed atomically at end
    /// of body. Unlike HTML, XML cannot expose a recovery tree from partial bytes.
    pub xml_source: Option<String>,
    pub xml_document: bool,
    /// Non-HTML `text/*` shown as preformatted text.
    pub text_wrapper: bool,
    /// The streaming document body request, until it completes.
    pub body_request: Option<RequestId>,
    pub committed_at: Instant,
    /// Base URL for relative references (`<base href>` or the document URL).
    pub base_url: String,
    pub base_locked: bool,
    /// The page generation this document drives (a page replaced behind the loader's back
    /// retires the document).
    pub shared: Rc<DocumentShared>,
    pub bodies: HashMap<ResourceId, Vec<u8>>,
    pub buffered_bytes: u64,
    pub script_sources: HashMap<ResourceId, String>,
    /// `async` and script-inserted scripts whose source arrived, in arrival order.
    pub async_ready: VecDeque<ResourceId>,
    pub sheets: HashMap<ResourceId, SheetState>,
    pub inline_style_imports: HashMap<NodeId, Vec<ResourceId>>,
    /// The current resource of each element role (older ones never apply).
    pub node_resource: HashMap<(NodeId, ResourceType), ResourceId>,
    pub decoded_images: HashMap<ResourceId, DecodedImage>,
    pub font_faces: Vec<PendingFace>,
    pub font_families: HashSet<String>,
    pub fonts_requested: usize,
    /// Family (as declared), bold, italic and `font_faces` index of each requested
    /// `@font-face` source.
    pub font_loads: HashMap<ResourceId, (String, bool, bool, usize)>,
    /// `url()` images of computed styles already requested, as the style wrote them.
    pub css_images_requested: HashSet<String>,
    /// Requests for those images, with the style's URL text.
    pub css_image_loads: HashMap<ResourceId, String>,
    pub preloaded: HashSet<(ResourceType, String)>,
    pub last_preload_scan: Option<(usize, usize)>,
    /// Element `load`/`error` events, dispatched as separate tasks.
    pub element_events: VecDeque<(NodeId, &'static str)>,
    pub script_errors: Vec<ScriptError>,
    pub module_scripts: HashMap<NodeId, ModuleScript>,
    /// Module jobs whose outcome is still due, by job id.
    pub module_jobs: HashMap<u32, NodeId>,
    /// Import fetches, with the URL the module map asked for.
    pub module_imports: HashMap<ResourceId, String>,
    /// Nonce of the first module script the CSP let through; module imports are fetched
    /// with it (the module map is shared, so imports do not know their root script).
    pub module_nonce: Option<String>,
}

impl DocumentLoad {
    pub fn new(info: DocumentInfo, navigation_start: Instant, shared: Rc<DocumentShared>) -> Self {
        let id = info.id;
        let base_url = info.url.clone();
        let mut events = DocumentEventLog::new(id);
        events.record(
            DocumentEventKind::DocumentCreated,
            None,
            None,
            format!("{} {}", info.kind.as_str(), info.url),
        );
        Self {
            info,
            lifecycle: DocumentLifecycle::new(navigation_start),
            registry: ResourceRegistry::new(id),
            events,
            parser: HtmlParser::new(),
            decoder: TextDecoder::for_html(None),
            xml_source: None,
            xml_document: false,
            text_wrapper: false,
            body_request: None,
            committed_at: Instant::now(),
            base_url,
            base_locked: false,
            shared,
            bodies: HashMap::new(),
            buffered_bytes: 0,
            script_sources: HashMap::new(),
            async_ready: VecDeque::new(),
            sheets: HashMap::new(),
            inline_style_imports: HashMap::new(),
            node_resource: HashMap::new(),
            decoded_images: HashMap::new(),
            font_faces: Vec::new(),
            font_families: HashSet::new(),
            fonts_requested: 0,
            font_loads: HashMap::new(),
            css_images_requested: HashSet::new(),
            css_image_loads: HashMap::new(),
            preloaded: HashSet::new(),
            last_preload_scan: None,
            element_events: VecDeque::new(),
            script_errors: Vec::new(),
            module_scripts: HashMap::new(),
            module_jobs: HashMap::new(),
            module_imports: HashMap::new(),
            module_nonce: None,
        }
    }

    /// Every module script ran or failed.
    pub fn modules_settled(&self) -> bool {
        self.module_scripts
            .values()
            .all(|m| m.state == ModuleState::Done)
    }

    pub fn is_pending(&self, rid: ResourceId) -> bool {
        self.registry.get(rid).is_some_and(|r| r.is_pending())
    }

    /// Nothing more will happen to this document by itself.
    pub fn is_settled(&self) -> bool {
        self.lifecycle.canceled || self.lifecycle.load_fired
    }

    pub fn diagnostics(&self, stale_events_dropped: u64) -> DocumentDiagnostics {
        let lc = &self.lifecycle;
        let encoding = (self.info.kind != DocumentKind::Internal).then(|| {
            match (self.decoder.encoding(), self.decoder.source()) {
                (Some(e), Some(s)) => format!("{} ({})", e.name(), s.as_str()),
                _ => "undetermined".to_string(),
            }
        });
        DocumentDiagnostics {
            document: self.info.id,
            navigation: self.info.navigation,
            url: self.info.url.clone(),
            origin: self.info.origin.serialize(),
            kind: self.info.kind.as_str(),
            encoding,
            ready_state: lc.ready_state.as_str(),
            parsing_state: lc.parsing_state.as_str(),
            bytes_parsed: self
                .xml_source
                .as_ref()
                .map_or_else(|| self.parser.bytes_parsed(), |s| s.len()),
            blocking_stylesheets: lc.blocking_stylesheets.iter().copied().collect(),
            parser_blocking_script: lc.parser_blocking_script.as_ref().map(|p| p.label.clone()),
            defer_queue: lc.defer_queue.iter().map(|p| p.label.clone()).collect(),
            load_blocking: lc.load_blocking_resources.len(),
            total_resources: self.registry.len(),
            pending_resources: self.registry.pending().count(),
            failed_resources: self.registry.failed().count(),
            rejected_resources: self.registry.rejected_count(),
            script_errors: self.script_errors.len(),
            dom_content_loaded_fired: lc.dom_content_loaded_fired,
            load_fired: lc.load_fired,
            canceled: lc.canceled,
            timeline: lc.timeline.milestones(),
            stale_events_dropped,
            events_recorded: self.events.len(),
        }
    }

    pub fn retire(mut self, reason: &str, stale_events_dropped: u64) -> RetiredDocument {
        let canceled = self.registry.cancel_pending(reason).len();
        if !self.lifecycle.load_fired {
            self.events.record(
                DocumentEventKind::DocumentCanceled,
                None,
                None,
                format!("{reason}; {canceled} pending requests canceled"),
            );
        }
        self.lifecycle.cancel();
        RetiredDocument {
            diagnostics: self.diagnostics(stale_events_dropped),
            resources: self.registry.all().iter().map(Into::into).collect(),
            events: self.events.events().cloned().collect(),
            script_errors: self.script_errors,
        }
    }
}

impl BrowsingContext {
    /// Advance the active document as far as its state allows. Never blocks.
    pub(super) fn pump_document(&mut self) {
        self.check_page_generation();
        for _ in 0..MAX_PUMP_STEPS {
            if !self.pump_step() {
                break;
            }
        }
        self.sync_render_blocking();
    }

    /// `Page::load_html` (or another direct page replacement) started a document the
    /// loader does not drive: the loader's document is obsolete.
    fn check_page_generation(&mut self) {
        let replaced = self
            .document
            .as_ref()
            .is_some_and(|d| !Rc::ptr_eq(&d.shared, &self.page.shared));
        if replaced {
            self.retire_document(&[], "page replaced");
        }
    }

    fn pump_step(&mut self) -> bool {
        let Some(doc) = self.document.as_mut() else {
            return false;
        };
        if doc.info.kind == DocumentKind::Internal {
            // Internal pages never load web resources.
            self.page.shared.insertions.borrow_mut().clear();
            return false;
        }
        if doc.lifecycle.canceled {
            return false;
        }
        if let Some((node, type_)) = doc.element_events.pop_front() {
            self.page.dispatch_element_event(node, type_);
            return true;
        }
        if self.page.report_csp_violations() {
            return true;
        }
        if self.drain_insertions() {
            return true;
        }
        if self.service_modules() {
            return true;
        }
        let Some(doc) = self.document.as_mut() else {
            return false;
        };
        if let Some(rid) = doc.async_ready.pop_front() {
            self.run_external_script(rid);
            return true;
        }
        if let Some(pending) = doc.lifecycle.parser_blocking_script.clone() {
            return self.try_run_parser_blocking(pending);
        }
        if !doc.lifecycle.parsing_complete() {
            return self.parse_step();
        }
        if let Some(front) = doc.lifecycle.defer_queue.front().cloned() {
            if doc.lifecycle.scripts_blocked_by_stylesheets() {
                return false;
            }
            if let Some(state) = doc.module_scripts.get(&front.node).map(|m| m.state) {
                let fetch_failed = front.resource.is_some_and(|rid| {
                    !doc.is_pending(rid) && !doc.script_sources.contains_key(&rid)
                });
                match state {
                    ModuleState::Ready(_) => {
                        doc.lifecycle.defer_queue.pop_front();
                        self.run_module_script(front.node);
                    }
                    ModuleState::Done => {
                        doc.lifecycle.defer_queue.pop_front();
                    }
                    ModuleState::Fetching if fetch_failed => {
                        // The error event was queued when the fetch failed.
                        if let Some(m) = doc.module_scripts.get_mut(&front.node) {
                            m.state = ModuleState::Done;
                        }
                        doc.lifecycle.defer_queue.pop_front();
                    }
                    ModuleState::Fetching | ModuleState::Loading(_) => return false,
                }
                return true;
            }
            if front.resource.is_none() {
                doc.lifecycle.defer_queue.pop_front();
                if let Some(text) = self.page.inline_script_text(front.node) {
                    if self.inline_script_allowed(front.node, &text) {
                        self.run_script(Some(front.node), None, &text, &front.label);
                    }
                }
                return true;
            }
            let rid = front.resource.expect("classic defer scripts are external");
            if doc.is_pending(rid) && !doc.script_sources.contains_key(&rid) {
                return false;
            }
            doc.lifecycle.defer_queue.pop_front();
            self.run_external_script(rid);
            return true;
        }
        if doc.lifecycle.ready_for_dom_content_loaded() {
            self.fire_dom_content_loaded();
            return true;
        }
        if doc.lifecycle.ready_for_load() && doc.modules_settled() {
            self.fire_load();
            return true;
        }
        false
    }

    fn try_run_parser_blocking(&mut self, pending: PendingScript) -> bool {
        let Some(doc) = self.document.as_mut() else {
            return false;
        };
        if doc.lifecycle.scripts_blocked_by_stylesheets() {
            return false;
        }
        match pending.resource {
            None => {
                doc.lifecycle.parser_blocking_script = None;
                doc.lifecycle.parsing_state = ParsingState::Parsing;
                if let Some(text) = self.page.inline_script_text(pending.node) {
                    if self.inline_script_allowed(pending.node, &text) {
                        self.run_script(Some(pending.node), None, &text, "inline script");
                    }
                }
                true
            }
            Some(rid) => {
                if doc.is_pending(rid) && !doc.script_sources.contains_key(&rid) {
                    self.scan_preloads();
                    return false;
                }
                doc.lifecycle.parser_blocking_script = None;
                doc.lifecycle.parsing_state = ParsingState::Parsing;
                self.run_external_script(rid);
                true
            }
        }
    }

    fn parse_step(&mut self) -> bool {
        let page_doc = Rc::clone(&self.page.document);
        let (step, discovered) = {
            let Some(doc) = self.document.as_mut() else {
                return false;
            };
            if doc.lifecycle.timeline.parse_start.is_none() {
                doc.lifecycle.start_parsing(Instant::now());
                doc.events
                    .record(DocumentEventKind::ParsingStarted, None, None, "");
            }
            let _span = self
                .page
                .trace
                .span(axiom_trace::TraceKind::HtmlParse, "html parse");
            let step = doc.parser.step(&mut page_doc.borrow_mut());
            (step, doc.parser.take_discovered())
        };
        for node in discovered {
            self.process_discovered(node);
        }
        if let Some(js) = self.page.js.as_mut() {
            if let Err(e) = js.upgrade_parsed_custom_elements() {
                log::warn!("custom element upgrades: {e}");
            }
        }
        match step {
            ParseStep::Script(node) => {
                self.handle_parser_script(node);
                true
            }
            ParseStep::NeedData => {
                if let Some(doc) = self.document.as_mut() {
                    doc.lifecycle.parsing_state = ParsingState::WaitingForData;
                }
                false
            }
            ParseStep::Done => {
                self.finish_parsing();
                true
            }
        }
    }

    /// An element the parser just completed (`<base>`, `<link>`, `<img>`, `<meta>`,
    /// `<style>`).
    fn process_discovered(&mut self, node: NodeId) {
        let (tag, attr) = {
            let d = self.page.document.borrow();
            let tag = d.tag_name(node).unwrap_or("").to_string();
            let attr = match tag.as_str() {
                "base" => d.attr(node, "href").map(str::to_string),
                "img" => d.attr(node, "src").map(str::to_string),
                "meta" => meta_csp(&d, node),
                _ => None,
            };
            (tag, attr)
        };
        match tag.as_str() {
            "base" => {
                let Some(doc) = self.document.as_mut() else {
                    return;
                };
                if doc.base_locked {
                    return;
                }
                if let Some(base) = attr.and_then(|h| resolve_base_href(&doc.info.url, &h)) {
                    if self.page.shared.csp.check_base_uri(&base, node) {
                        self.page
                            .shared
                            .forms
                            .borrow_mut()
                            .set_base_url(base.clone());
                        doc.base_url = base;
                        doc.base_locked = true;
                    }
                }
            }
            "meta" => {
                if let Some(policy) = attr {
                    self.page.shared.csp.add_meta(&policy);
                }
            }
            "link" => self.start_link_stylesheet(node, true),
            "img" if attr.is_some() => self.start_image(node, axiom_document::Initiator::Parser),
            "iframe" => self.start_frame(node),
            "style" => self.process_inline_style(node, true),
            _ => {}
        }
    }

    fn handle_parser_script(&mut self, node: NodeId) {
        self.page.shared.started_scripts.borrow_mut().insert(node);
        let (src, is_async, defer, type_attr) = {
            let d = self.page.document.borrow();
            (
                d.attr(node, "src").map(|s| s.trim().to_string()),
                d.has_attr(node, "async"),
                d.has_attr(node, "defer"),
                d.attr(node, "type").map(str::to_string),
            )
        };
        let kind = classify_script(ScriptElement {
            src: src.as_deref(),
            is_async,
            defer,
            type_attr: type_attr.as_deref(),
            parser_inserted: true,
        });
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        match kind {
            ScriptKind::ClassicInline => {
                if doc.lifecycle.scripts_blocked_by_stylesheets() {
                    doc.lifecycle.parser_blocking_script = Some(PendingScript {
                        node,
                        resource: None,
                        label: "inline script".into(),
                    });
                    doc.lifecycle.parsing_state = ParsingState::BlockedOnScript;
                } else if let Some(text) = self.page.inline_script_text(node) {
                    if self.inline_script_allowed(node, &text) {
                        self.run_script(Some(node), None, &text, "inline script");
                    }
                }
            }
            ScriptKind::ClassicBlocking => {
                let src = src.unwrap_or_default();
                if let Some(rid) = self.start_script(node, &src, kind) {
                    let doc = self.document.as_mut().expect("document");
                    doc.lifecycle.parser_blocking_script = Some(PendingScript {
                        node,
                        resource: Some(rid),
                        label: src,
                    });
                    doc.lifecycle.parsing_state = ParsingState::BlockedOnScript;
                    self.scan_preloads();
                }
            }
            ScriptKind::ClassicDefer => {
                let src = src.unwrap_or_default();
                if let Some(rid) = self.start_script(node, &src, kind) {
                    let doc = self.document.as_mut().expect("document");
                    doc.lifecycle.defer_queue.push_back(PendingScript {
                        node,
                        resource: Some(rid),
                        label: src,
                    });
                }
            }
            ScriptKind::ClassicAsync => {
                self.start_script(node, src.as_deref().unwrap_or_default(), kind);
            }
            ScriptKind::Module | ScriptKind::ModuleAsync => {
                self.start_module_script(node, src, kind)
            }
            ScriptKind::ImportMap => self.register_import_map(node, src.is_some()),
            ScriptKind::ClassicDynamic | ScriptKind::DataBlock => {}
        }
    }

    /// Discover resources after an XML-family document has been parsed atomically.
    ///
    /// XML has no error-recovery tree that can safely be exposed while bytes are still
    /// arriving, but it still owns a normal document realm after its successful parse.
    /// This walk deliberately uses the same resource and script paths as HTML. Classic
    /// XML scripts are put in the document-order defer queue: the complete XML tree is
    /// already available, and preserving script order is more important than pretending
    /// we could have paused an atomic XML parser at each script element.
    pub(super) fn discover_xml_document(&mut self) {
        let elements = {
            let page_doc = self.page.document.borrow();
            let Some(root) = page_doc.document_id else {
                return;
            };
            let mut pending = vec![root];
            let mut elements = Vec::new();
            while let Some(id) = pending.pop() {
                if page_doc.tag_name(id).is_some() {
                    elements.push(id);
                }
                pending.extend(page_doc.get(id).children.iter().rev().copied());
            }
            elements
        };

        if let Some(doc) = self.document.as_mut() {
            doc.lifecycle.start_parsing(Instant::now());
            doc.events.record(
                DocumentEventKind::ParsingStarted,
                None,
                None,
                "XML document",
            );
        }

        // A base URL and CSP metadata influence every later discovery, regardless of
        // where they occur in the XML tree.
        for &node in &elements {
            let tag = self
                .page
                .document
                .borrow()
                .tag_name(node)
                .unwrap_or("")
                .to_string();
            if matches!(tag.as_str(), "base" | "meta") {
                self.process_discovered(node);
            }
        }
        for &node in &elements {
            let tag = self
                .page
                .document
                .borrow()
                .tag_name(node)
                .unwrap_or("")
                .to_string();
            if matches!(tag.as_str(), "link" | "img" | "iframe" | "style") {
                self.process_discovered(node);
            }
        }
        for node in elements {
            if self.page.document.borrow().tag_name(node) == Some("script") {
                self.queue_xml_script(node);
            }
        }
        self.finish_parsing();
    }

    fn queue_xml_script(&mut self, node: NodeId) {
        self.page.shared.started_scripts.borrow_mut().insert(node);
        let (src, is_async, defer, type_attr) = {
            let d = self.page.document.borrow();
            (
                d.attr(node, "src").map(|s| s.trim().to_string()),
                d.has_attr(node, "async"),
                d.has_attr(node, "defer"),
                d.attr(node, "type").map(str::to_string),
            )
        };
        let kind = classify_script(ScriptElement {
            src: src.as_deref(),
            is_async,
            defer,
            type_attr: type_attr.as_deref(),
            parser_inserted: true,
        });
        match kind {
            ScriptKind::ClassicInline => {
                if let Some(doc) = self.document.as_mut() {
                    doc.lifecycle.defer_queue.push_back(PendingScript {
                        node,
                        resource: None,
                        label: "inline XML script".into(),
                    });
                }
            }
            ScriptKind::ClassicBlocking | ScriptKind::ClassicDefer => {
                let src = src.unwrap_or_default();
                if let Some(rid) = self.start_script(node, &src, ScriptKind::ClassicDefer) {
                    if let Some(doc) = self.document.as_mut() {
                        doc.lifecycle.defer_queue.push_back(PendingScript {
                            node,
                            resource: Some(rid),
                            label: src,
                        });
                    }
                }
            }
            ScriptKind::ClassicAsync => {
                self.start_script(node, src.as_deref().unwrap_or_default(), kind);
            }
            ScriptKind::Module | ScriptKind::ModuleAsync => {
                self.start_module_script(node, src, kind)
            }
            ScriptKind::ImportMap => self.register_import_map(node, src.is_some()),
            ScriptKind::ClassicDynamic | ScriptKind::DataBlock => {}
        }
    }

    /// `<script type="importmap">`: merge its mappings into the document's. External
    /// import maps are not allowed.
    fn register_import_map(&mut self, node: NodeId, external: bool) {
        if external {
            log::warn!("external import maps are not supported");
            if let Some(doc) = self.document.as_mut() {
                doc.element_events.push_back((node, "error"));
            }
            return;
        }
        let Some(text) = self.page.inline_module_text(node) else {
            return;
        };
        if !self.inline_script_allowed(node, &text) {
            return;
        }
        let base = self.base_url_for_scripts();
        match ImportMap::parse(&text, &base) {
            Ok((map, warnings)) => {
                for w in warnings {
                    log::warn!("import map: {w}");
                }
                self.page.shared.import_map.borrow_mut().merge(map);
            }
            Err(err) => {
                let (syntax, message) = match err {
                    ImportMapError::Syntax(m) => (true, format!("invalid import map JSON: {m}")),
                    ImportMapError::Type(m) => (false, m),
                };
                log::warn!("import map rejected: {message}");
                self.page.report_script_error(syntax, &message);
                if let Some(doc) = self.document.as_mut() {
                    doc.script_errors.push(ScriptError {
                        resource: None,
                        label: "import map".into(),
                        message,
                    });
                }
            }
        }
    }

    /// CSP inline check for a `<script>` element's text (classic, module or import map).
    pub(super) fn inline_script_allowed(&self, node: NodeId, text: &str) -> bool {
        let nonce = crate::csp::element_nonce(&self.page.document.borrow(), node);
        self.page.shared.csp.check_inline(
            axiom_csp::InlineKind::Script,
            Some(node),
            text,
            nonce.as_deref(),
        )
    }

    fn base_url_for_scripts(&self) -> String {
        self.document
            .as_ref()
            .map(|d| d.base_url.clone())
            .unwrap_or_else(|| self.page.url.clone())
    }

    /// Run the external script `rid` (its source arrived) or report its failure.
    pub(super) fn run_external_script(&mut self, rid: ResourceId) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        let (node, label, final_url) = doc
            .registry
            .get(rid)
            .map(|r| (r.node, r.url.clone(), r.final_url.clone()))
            .unwrap_or((None, String::new(), None));
        let module_node = node.filter(|n| {
            doc.module_scripts
                .get(n)
                .is_some_and(|m| m.resource == Some(rid) && m.state == ModuleState::Fetching)
        });
        if let Some(node) = module_node {
            let Some(source) = doc.script_sources.remove(&rid) else {
                return;
            };
            doc.registry
                .transition(rid, axiom_document::ResourceState::Processing);
            let url = final_url.filter(|u| u != "data:").unwrap_or(label);
            self.start_module_job(node, &url, &source, false);
            return;
        }
        match doc.script_sources.remove(&rid) {
            Some(source) => self.run_script(node, Some(rid), &source, &label),
            None => {
                // Fetch failed (the error event was queued when it failed).
            }
        }
    }

    /// A module script element: fetch its source (external) or start its job (inline).
    /// Deferred ones also take their place in the defer queue.
    fn start_module_script(&mut self, node: NodeId, src: Option<String>, kind: ScriptKind) {
        let deferred = kind == ScriptKind::Module;
        let resource = match src.as_deref().filter(|s| !s.is_empty()) {
            Some(src) => match self.start_script(node, src, kind) {
                Some(rid) => Some(rid),
                None => return,
            },
            None => None,
        };
        let inline_text = match resource {
            Some(_) => None,
            None => match self.page.inline_module_text(node) {
                Some(text) if self.inline_script_allowed(node, &text) => Some(text),
                _ => return,
            },
        };
        let nonce = crate::csp::element_nonce(&self.page.document.borrow(), node);
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        let label = src.unwrap_or_else(|| "inline module".into());
        // A fetch can fail before `start_script` returns (bad URL, policy).
        let failed = resource
            .is_some_and(|rid| !doc.is_pending(rid) && !doc.script_sources.contains_key(&rid));
        if !failed && doc.module_nonce.is_none() {
            doc.module_nonce = nonce;
        }
        doc.module_scripts.insert(
            node,
            ModuleScript {
                resource,
                label: label.clone(),
                deferred,
                state: if failed {
                    ModuleState::Done
                } else {
                    ModuleState::Fetching
                },
            },
        );
        if deferred {
            doc.lifecycle.defer_queue.push_back(PendingScript {
                node,
                resource,
                label,
            });
        }
        if let Some(text) = inline_text {
            let base = doc.base_url.clone();
            self.start_module_job(node, &base, &text, true);
        }
    }

    fn start_module_job(&mut self, node: NodeId, url: &str, source: &str, inline: bool) {
        let t0 = Instant::now();
        let job = self.page.start_module(url, source, inline);
        self.page.hud.js_ms += t0.elapsed().as_secs_f64() * 1000.0;
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        let Some(module) = doc.module_scripts.get_mut(&node) else {
            return;
        };
        match job {
            Some(job) => {
                module.state = ModuleState::Loading(job);
                doc.module_jobs.insert(job, node);
            }
            // Scripting is off: the script never runs.
            None => {
                module.state = ModuleState::Done;
                if let Some(rid) = module.resource {
                    doc.registry
                        .transition(rid, axiom_document::ResourceState::Ready);
                    doc.lifecycle.resource_settled(rid);
                }
            }
        }
    }

    /// Evaluate a module script whose graph loaded.
    fn run_module_script(&mut self, node: NodeId) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        let Some(module) = doc.module_scripts.get_mut(&node) else {
            return;
        };
        let ModuleState::Ready(job) = module.state else {
            return;
        };
        module.state = ModuleState::Done;
        let (rid, label) = (module.resource, module.label.clone());
        if !self.scripts_enabled {
            if let Some(rid) = rid {
                doc.registry
                    .transition(rid, axiom_document::ResourceState::Ready);
            }
            doc.events.record(
                DocumentEventKind::ScriptExecutionFinished,
                rid,
                None,
                "script blocked by sandbox",
            );
            return;
        }
        let doc_id = doc.info.id;
        doc.events.record(
            DocumentEventKind::ScriptExecutionStarted,
            rid,
            None,
            label.clone(),
        );
        let t0 = Instant::now();
        self.page.run_module(job, &label);
        self.page.hud.js_ms += t0.elapsed().as_secs_f64() * 1000.0;
        if self.document.as_ref().is_some_and(|d| d.info.id == doc_id) {
            self.finish_module_script(node, None);
        }
    }

    /// A module script ran (`error: None`) or its graph failed to parse or link.
    fn finish_module_script(&mut self, node: NodeId, error: Option<String>) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        let Some(module) = doc.module_scripts.get_mut(&node) else {
            return;
        };
        module.state = ModuleState::Done;
        let (rid, label) = (module.resource, module.label.clone());
        match &error {
            None => {
                if let Some(rid) = rid {
                    if doc
                        .registry
                        .transition(rid, axiom_document::ResourceState::Ready)
                    {
                        doc.events.record(
                            DocumentEventKind::ResourceReady,
                            Some(rid),
                            None,
                            "module",
                        );
                    }
                }
            }
            Some(e) => {
                log::warn!("script error ({label}): {e}");
                doc.script_errors.push(ScriptError {
                    resource: rid,
                    label: label.clone(),
                    message: e.clone(),
                });
                if let Some(rid) = rid {
                    doc.registry.fail(
                        rid,
                        axiom_document::ResourceError::of(
                            ResourceType::Script,
                            axiom_document::FailureStage::Execution,
                            e.clone(),
                        ),
                    );
                }
            }
        }
        doc.events.record(
            DocumentEventKind::ScriptExecutionFinished,
            rid,
            None,
            match &error {
                None => label,
                Some(e) => format!("{label}: {e}"),
            },
        );
        if let Some(rid) = rid {
            doc.lifecycle.resource_settled(rid);
            doc.element_events.push_back((node, "load"));
        }
    }

    /// Fetch the imports module graphs asked for and act on module job progress.
    fn service_modules(&mut self) -> bool {
        let fetches = self.page.take_module_fetches();
        let mut progressed = !fetches.is_empty();
        for url in fetches {
            if !self.start_module_import(&url) {
                self.page
                    .module_fetched(&url, Err("the request could not be started".into()));
            }
        }
        for (job, outcome) in self.page.poll_modules() {
            progressed = true;
            let Some(doc) = self.document.as_mut() else {
                break;
            };
            let Some(&node) = doc.module_jobs.get(&job) else {
                continue;
            };
            let Some(module) = doc.module_scripts.get_mut(&node) else {
                continue;
            };
            let before_run = module.state == ModuleState::Loading(job);
            let (resource, label, deferred) =
                (module.resource, module.label.clone(), module.deferred);
            if outcome == axiom_js::ModuleOutcome::Ready {
                module.state = ModuleState::Ready(job);
                if !deferred {
                    self.run_module_script(node);
                }
                continue;
            }
            doc.module_jobs.remove(&job);
            match outcome {
                axiom_js::ModuleOutcome::Ready | axiom_js::ModuleOutcome::Evaluated => {}
                axiom_js::ModuleOutcome::FetchFailed(message) => {
                    module.state = ModuleState::Done;
                    match resource {
                        Some(rid) => self.fail_resource(
                            rid,
                            axiom_document::ResourceError::of(
                                ResourceType::Script,
                                axiom_document::FailureStage::Network,
                                message,
                            ),
                        ),
                        None => {
                            log::warn!("{label}: {message}");
                            doc.element_events.push_back((node, "error"));
                        }
                    }
                }
                axiom_js::ModuleOutcome::Error(message) if before_run => {
                    self.finish_module_script(node, Some(message));
                }
                axiom_js::ModuleOutcome::Error(message) => {
                    log::warn!("script error ({label}): {message}");
                    doc.script_errors.push(ScriptError {
                        resource,
                        label,
                        message,
                    });
                }
            }
        }
        progressed
    }

    /// Execute a classic script at the current point. Errors are recorded against the
    /// script's resource; the document keeps running.
    pub(super) fn run_script(
        &mut self,
        node: Option<NodeId>,
        rid: Option<ResourceId>,
        source: &str,
        label: &str,
    ) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        if !self.scripts_enabled {
            if let Some(rid) = rid {
                doc.registry
                    .transition(rid, axiom_document::ResourceState::Ready);
            }
            doc.events.record(
                DocumentEventKind::ScriptExecutionFinished,
                rid,
                None,
                "script blocked by sandbox",
            );
            return;
        }
        let doc_id = doc.info.id;
        if let Some(rid) = rid {
            doc.registry
                .transition(rid, axiom_document::ResourceState::Processing);
        }
        doc.events.record(
            DocumentEventKind::ScriptExecutionStarted,
            rid,
            None,
            label.to_string(),
        );
        let t0 = Instant::now();
        let result = self.page.execute_script(node, source, label);
        self.page.hud.js_ms += t0.elapsed().as_secs_f64() * 1000.0;
        let Some(doc) = self.document.as_mut().filter(|d| d.info.id == doc_id) else {
            return;
        };
        match &result {
            Ok(()) => {
                if let Some(rid) = rid {
                    if doc
                        .registry
                        .transition(rid, axiom_document::ResourceState::Ready)
                    {
                        doc.events.record(
                            DocumentEventKind::ResourceReady,
                            Some(rid),
                            None,
                            "script",
                        );
                    }
                }
            }
            Err(e) => {
                doc.script_errors.push(ScriptError {
                    resource: rid,
                    label: label.to_string(),
                    message: e.clone(),
                });
                if let Some(rid) = rid {
                    doc.registry.fail(
                        rid,
                        axiom_document::ResourceError::of(
                            ResourceType::Script,
                            axiom_document::FailureStage::Execution,
                            e.clone(),
                        ),
                    );
                }
            }
        }
        doc.events.record(
            DocumentEventKind::ScriptExecutionFinished,
            rid,
            None,
            match &result {
                Ok(()) => label.to_string(),
                Err(e) => format!("{label}: {e}"),
            },
        );
        if let Some(rid) = rid {
            doc.lifecycle.resource_settled(rid);
            if let Some(node) = node {
                doc.element_events.push_back((node, "load"));
            }
        }
    }

    fn finish_parsing(&mut self) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        doc.lifecycle.finish_parsing(Instant::now());
        doc.events.record(
            DocumentEventKind::ParsingCompleted,
            None,
            None,
            format!("{} bytes", doc.parser.bytes_parsed()),
        );
        self.page.update_title();
        self.history.update_title(&self.page.title);
        self.page.set_ready(DocumentReadyState::Interactive);
        self.page.dispatch_document_event("readystatechange", false);
    }

    fn fire_dom_content_loaded(&mut self) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        doc.lifecycle.fire_dom_content_loaded(Instant::now());
        doc.events
            .record(DocumentEventKind::DomContentLoadedFired, None, None, "");
        self.loading = false;
        self.page.dispatch_document_event("DOMContentLoaded", true);
    }

    fn fire_load(&mut self) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        doc.lifecycle.begin_load(Instant::now());
        doc.events
            .record(DocumentEventKind::LoadFired, None, None, "");
        self.page.set_ready(DocumentReadyState::Complete);
        self.page.dispatch_document_event("readystatechange", false);
        self.page.dispatch_window_event("load");
        if let Some(doc) = self.document.as_mut() {
            doc.lifecycle.finish_load(Instant::now());
        }
    }

    /// Resources script connected to the document since the last turn.
    fn drain_insertions(&mut self) -> bool {
        let items: Vec<DynamicInsertion> =
            self.page.shared.insertions.borrow_mut().drain(..).collect();
        if items.is_empty() {
            return false;
        }
        for item in items {
            match item {
                DynamicInsertion::Script(node) => self.start_dynamic_script(node),
                DynamicInsertion::Stylesheet(node) => self.start_link_stylesheet(node, false),
                DynamicInsertion::Image(node) => {
                    self.start_image(node, axiom_document::Initiator::Script)
                }
                DynamicInsertion::Style(node) => self.process_inline_style(node, false),
                DynamicInsertion::Frame(node) => self.start_frame(node),
            }
        }
        true
    }

    /// Start or replace a genuine nested browsing context for an `<iframe src>`.
    /// The child receives a distinct DOM/JS/document loader and ResourceLoader context,
    /// while sharing the embedder's profile scheduler, cookie authority and storage
    /// binder. That keeps privacy/profile ownership at the normal browser boundary.
    fn start_frame(&mut self, node: NodeId) {
        let (src, srcdoc, sandbox, base) = {
            let dom = self.page.document.borrow();
            (
                dom.attr(node, "src")
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                dom.attr(node, "srcdoc").map(str::to_string),
                dom.attr(node, "sandbox").map(str::to_string),
                self.document.as_ref().map(|d| d.base_url.clone()),
            )
        };
        let Some(base) = base else { return };
        let mut child = BrowsingContext::headless(
            self.viewport_width,
            self.content_height(),
            std::sync::Arc::clone(self.loader.scheduler()),
        );
        child.set_blocking_navigation(false);
        child.set_owner(self.owner_tab, self.owner_profile.clone());
        child.set_cookie_jar(self.cookie_jar.clone());
        child.set_storage_binder(self.storage_binder.clone());
        child.set_content_policy(std::sync::Arc::clone(&self.content_policy));
        let scripts_allowed = sandbox.as_ref().is_none_or(|value| {
            value
                .split_ascii_whitespace()
                .any(|token| token.eq_ignore_ascii_case("allow-scripts"))
        });
        child.set_scripts_enabled(scripts_allowed);
        let forms_allowed = sandbox.as_ref().is_none_or(|value| {
            value
                .split_ascii_whitespace()
                .any(|token| token.eq_ignore_ascii_case("allow-forms"))
        });
        child.set_forms_enabled(forms_allowed);
        let same_origin_allowed = sandbox.as_ref().is_none_or(|value| {
            value
                .split_ascii_whitespace()
                .any(|token| token.eq_ignore_ascii_case("allow-same-origin"))
        });
        child.set_sandboxed_origin(!same_origin_allowed);
        match srcdoc {
            // srcdoc intentionally wins over src and gets the embedder URL/origin for
            // storage and relative-URL semantics, while retaining its own DOM and realm.
            Some(markup) => {
                if let Err(error) = child.load_local_html(&base, &markup, false) {
                    log::warn!("iframe srcdoc parse failed: {error}");
                }
            }
            None => match src {
                Some(src) => match axiom_document::resolve_subresource(&base, &src) {
                    axiom_document::SubresourceTarget::Network(url) => {
                        child.navigate_to(&url.as_str());
                    }
                    axiom_document::SubresourceTarget::Local(path) => {
                        child.navigate_to(&path.display().to_string());
                    }
                    axiom_document::SubresourceTarget::Data { .. } => {
                        // `data:` frame documents are not supported by this loader yet.
                        // Keep the child boundary, but complete it as a controlled frame
                        // failure so the embedder receives `error`, never a false `load`.
                        child.last_error = Some("data: iframe documents are unsupported".into());
                    }
                    axiom_document::SubresourceTarget::Invalid(error) => {
                        child.last_error = Some(format!("invalid iframe URL: {error}"));
                    }
                },
                // A source-less iframe is a separate, same-origin about:blank document.
                None => {
                    if let Err(error) = child.load_local_html(&base, "", false) {
                        log::warn!("iframe about:blank creation failed: {error}");
                    }
                }
            },
        }
        self.frame_load_fired.remove(&node);
        self.frame_error_fired.remove(&node);
        self.frames.insert(node, child);
    }

    fn start_dynamic_script(&mut self, node: NodeId) {
        let (src, type_attr) = {
            let d = self.page.document.borrow();
            (
                d.attr(node, "src").map(|s| s.trim().to_string()),
                d.attr(node, "type").map(str::to_string),
            )
        };
        let kind = classify_script(ScriptElement {
            src: src.as_deref(),
            is_async: false,
            defer: false,
            type_attr: type_attr.as_deref(),
            parser_inserted: false,
        });
        match kind {
            ScriptKind::ClassicDynamic => {
                self.start_script(node, src.as_deref().unwrap_or_default(), kind);
            }
            ScriptKind::ModuleAsync => self.start_module_script(node, src, kind),
            ScriptKind::ImportMap => self.register_import_map(node, src.is_some()),
            _ => {}
        }
    }

    /// Parser-inserted stylesheets and `blocking="render"` resources hold up rendering
    /// (with a safety cap).
    pub(super) fn sync_render_blocking(&mut self) {
        let blocked = self.document.as_ref().is_some_and(|d| {
            let lc = &d.lifecycle;
            !lc.canceled
                && d.committed_at.elapsed() < RENDER_BLOCK_TIMEOUT
                && (!lc.render_blocking_resources.is_empty()
                    || lc
                        .blocking_stylesheets
                        .iter()
                        .any(|r| d.registry.get(*r).is_some_and(|rec| rec.blocking.render)))
        });
        self.page.render_blocked = blocked;
    }

    /// Cancel every pending resource of the active document (Stop, network swap). The
    /// parser finishes with the bytes it has, so the lifecycle still completes.
    pub(super) fn cancel_document_resources(&mut self, reason: &str) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        let pending: Vec<ResourceId> = doc.registry.pending().map(|r| r.id).collect();
        doc.registry.cancel_pending(reason);
        for rid in pending {
            doc.lifecycle.resource_settled(rid);
            doc.bodies.remove(&rid);
            doc.events.record(
                DocumentEventKind::ResourceCanceled,
                Some(rid),
                None,
                reason.to_string(),
            );
        }
        doc.buffered_bytes = 0;
        if doc.body_request.take().is_some() {
            let tail = doc.decoder.finish();
            feed_text(doc, &tail, true);
        }
    }
}

/// Feed decoded document text to the parser (escaped inside the `<pre>` wrapper for
/// non-HTML text documents). `last` ends the stream.
pub(super) fn feed_text(doc: &mut DocumentLoad, text: &str, last: bool) {
    if !text.is_empty() {
        if doc.text_wrapper {
            doc.parser.feed(&super::html_escape(text));
        } else {
            doc.parser.feed(text);
        }
    }
    if last {
        if doc.text_wrapper {
            doc.parser.feed("</pre></body></html>");
        }
        doc.parser.finish();
    }
}
