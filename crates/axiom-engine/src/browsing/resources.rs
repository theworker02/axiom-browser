//! Subresources of the active document: discovery, requests through the resource loader
//! (and so the profile scheduler and network service), processing and failure.

use std::fs;
use std::sync::Arc;

use axiom_csp::{CspUrl, FetchDirective};
use axiom_document::{
    absolutize_urls, classify_mixed_content, css_imports, font_faces, font_families_used,
    priority_for, resolve_subresource, scan_for_preloads, strip_css_imports, Blocking,
    CorsSettings, DocumentEventKind, DocumentKind, FailureStage, ImageInfo, Initiator,
    MixedContent, NewResource, PolicyDecision, PolicyDirective, PriorityHint, ResourceError,
    ResourceId, ResourceState, ScriptKind, SubresourceTarget,
};
use axiom_dom::NodeId;
use axiom_loader::{
    decode_image, LoaderEvent, MimeType, NetworkError, RedirectCheck, ResourceRequest,
};
use axiom_net::{decode_text, CredentialsMode, RequestMode, ResourceType};
use axiom_url::Url;

use super::document::{DocumentLoad, ModuleState, PendingFace, SheetState};
use super::{BrowsingContext, SecurityState};
use crate::csp::element_nonce;
use crate::page::DecodedImage;

/// Nesting limit for `@import` chains (also stops import cycles).
const MAX_IMPORT_DEPTH: u8 = 8;
/// Subresource body bytes one document may hold in memory while they are processed.
const MAX_DOCUMENT_BUFFER: u64 = 64 * 1024 * 1024;
/// `@font-face` requests per document.
const MAX_FONTS_PER_DOCUMENT: usize = 32;
const MAX_CSS_IMAGES_PER_DOCUMENT: usize = 256;

/// A complete subresource body, from the network, a `data:` URL or a local file.
struct Body {
    bytes: Vec<u8>,
    mime: Option<MimeType>,
    status: u16,
    final_url: String,
}

struct ResourceStart<'a> {
    kind: ResourceType,
    raw_url: &'a str,
    base: String,
    initiator: Initiator,
    node: Option<NodeId>,
    hint: PriorityHint,
    blocking: Blocking,
    script_kind: Option<ScriptKind>,
    lazy: bool,
    sheet: Option<SheetState>,
    /// A module import: the URL the module map asked for.
    module_import: Option<String>,
    /// Fetch in `cors` mode (`None`: `no-cors`).
    cors: Option<CorsSettings>,
}

impl<'a> ResourceStart<'a> {
    fn new(kind: ResourceType, raw_url: &'a str, base: String, initiator: Initiator) -> Self {
        Self {
            kind,
            raw_url,
            base,
            initiator,
            node: None,
            hint: PriorityHint::Normal,
            blocking: Blocking::default(),
            script_kind: None,
            lazy: false,
            sheet: None,
            module_import: None,
            cors: None,
        }
    }
}

/// The CSP fetch directive a subresource of `kind` is checked against.
fn csp_directive(kind: ResourceType) -> FetchDirective {
    match kind {
        ResourceType::Script => FetchDirective::ScriptSrcElem,
        ResourceType::Stylesheet => FetchDirective::StyleSrcElem,
        ResourceType::Image => FetchDirective::ImgSrc,
        ResourceType::Font => FetchDirective::FontSrc,
        ResourceType::Media => FetchDirective::MediaSrc,
        ResourceType::Manifest => FetchDirective::ManifestSrc,
        _ => FetchDirective::ConnectSrc,
    }
}

/// The URL CSP matches for a subresource target (`None`: an invalid URL, never fetched).
fn csp_url(target: &SubresourceTarget) -> Option<CspUrl> {
    match target {
        SubresourceTarget::Network(u) => Some(CspUrl::from_url(u)),
        SubresourceTarget::Data { mime, .. } => CspUrl::parse(&format!("data:{mime},")),
        SubresourceTarget::Local(p) => {
            let path = p.display().to_string().replace('\\', "/");
            CspUrl::parse(&format!("file:///{}", path.trim_start_matches('/')))
        }
        SubresourceTarget::Invalid(_) => None,
    }
}

fn register_blocking(doc: &mut DocumentLoad, rid: ResourceId, kind: ResourceType, b: Blocking) {
    if kind == ResourceType::Stylesheet && b.script {
        doc.lifecycle.blocking_stylesheets.insert(rid);
    }
    if b.load && !doc.lifecycle.load_fired {
        doc.lifecycle.load_blocking_resources.insert(rid);
    }
    if b.render {
        doc.lifecycle.render_blocking_resources.insert(rid);
    }
}

impl BrowsingContext {
    fn crossorigin(&self, node: NodeId) -> Option<CorsSettings> {
        CorsSettings::from_attribute(self.page.document.borrow().attr(node, "crossorigin"))
    }

    fn base_url(&self) -> String {
        self.document
            .as_ref()
            .map(|d| d.base_url.clone())
            .unwrap_or_default()
    }

    fn load_fired(&self) -> bool {
        self.document
            .as_ref()
            .is_some_and(|d| d.lifecycle.load_fired)
    }

    /// `<link rel=stylesheet>`: parser-inserted ones block scripts and rendering.
    pub(super) fn start_link_stylesheet(&mut self, node: NodeId, parser_inserted: bool) {
        let href = {
            let d = self.page.document.borrow();
            let rel = d.attr(node, "rel").unwrap_or("").to_ascii_lowercase();
            let is_sheet = rel.split_ascii_whitespace().any(|t| t == "stylesheet");
            let alternate = rel.split_ascii_whitespace().any(|t| t == "alternate");
            match d.attr(node, "href").map(str::trim) {
                Some(h) if is_sheet && !alternate && !h.is_empty() => h.to_string(),
                _ => return,
            }
        };
        let blocking = if parser_inserted {
            Blocking {
                script: true,
                render: true,
                load: true,
            }
        } else {
            Blocking {
                load: !self.load_fired(),
                render: self.explicitly_render_blocking(node),
                ..Blocking::default()
            }
        };
        let initiator = if parser_inserted {
            Initiator::Parser
        } else {
            Initiator::Script
        };
        let mut s = ResourceStart::new(ResourceType::Stylesheet, &href, self.base_url(), initiator);
        s.node = Some(node);
        s.hint = PriorityHint::Blocking;
        s.blocking = blocking;
        s.sheet = Some(SheetState::default());
        s.cors = self.crossorigin(node);
        self.start_resource(s);
    }

    /// `blocking="render"` on an element inserted while the document still "allows
    /// adding render-blocking elements" (an HTML document whose body is not parsed yet).
    fn explicitly_render_blocking(&self, node: NodeId) -> bool {
        let d = self.page.document.borrow();
        let requested = d.attr(node, "blocking").is_some_and(|b| {
            b.split_ascii_whitespace()
                .any(|t| t.eq_ignore_ascii_case("render"))
        });
        requested && !self.page.xhtml && d.body().is_none()
    }

    pub(super) fn start_image(&mut self, node: NodeId, initiator: Initiator) {
        let (src, lazy) = {
            let d = self.page.document.borrow();
            let Some(src) = d.attr(node, "src").map(str::trim).filter(|s| !s.is_empty()) else {
                return;
            };
            let lazy = d
                .attr(node, "loading")
                .is_some_and(|l| l.trim().eq_ignore_ascii_case("lazy"));
            (src.to_string(), lazy)
        };
        let mut s = ResourceStart::new(ResourceType::Image, &src, self.base_url(), initiator);
        s.node = Some(node);
        s.lazy = lazy;
        s.cors = self.crossorigin(node);
        s.hint = if lazy {
            PriorityHint::Lazy
        } else {
            PriorityHint::Normal
        };
        s.blocking.load = !lazy && !self.load_fired();
        self.start_resource(s);
    }

    pub(super) fn start_script(
        &mut self,
        node: NodeId,
        src: &str,
        kind: ScriptKind,
    ) -> Option<ResourceId> {
        let (hint, initiator) = match kind {
            ScriptKind::ClassicBlocking => (PriorityHint::Blocking, Initiator::Parser),
            ScriptKind::ClassicDefer | ScriptKind::Module => {
                (PriorityHint::Deferred, Initiator::Parser)
            }
            ScriptKind::ClassicAsync => (PriorityHint::Async, Initiator::Parser),
            ScriptKind::ModuleAsync => (PriorityHint::Async, Initiator::Script),
            _ => (PriorityHint::Dynamic, Initiator::Script),
        };
        let mut s = ResourceStart::new(ResourceType::Script, src, self.base_url(), initiator);
        s.node = Some(node);
        s.hint = hint;
        s.script_kind = Some(kind);
        s.cors = self.crossorigin(node);
        // Module scripts are always fetched with CORS (HTML "fetch a single module script").
        if matches!(kind, ScriptKind::Module | ScriptKind::ModuleAsync) {
            s.cors = s.cors.or(Some(CorsSettings::Anonymous));
        }
        s.blocking = Blocking {
            script: kind == ScriptKind::ClassicBlocking,
            render: false,
            load: !self.load_fired(),
        };
        self.start_resource(s)
    }

    /// Fetch an import of a module graph (the URL is already absolute). Blocks `load`
    /// while the document is loading.
    pub(super) fn start_module_import(&mut self, url: &str) -> bool {
        let mut s = ResourceStart::new(
            ResourceType::Script,
            url,
            url.to_string(),
            Initiator::Script,
        );
        s.hint = PriorityHint::Async;
        s.blocking.load = !self.load_fired();
        s.module_import = Some(url.to_string());
        s.cors = Some(CorsSettings::Anonymous);
        self.start_resource(s).is_some()
    }

    /// `<style>`: its `@import`s load through the loader and `@font-face` rules register
    /// fonts. Parser-inserted imports block scripts and rendering like `<link>`s.
    pub(super) fn process_inline_style(&mut self, node: NodeId, parser_inserted: bool) {
        let (text, nonce) = {
            let d = self.page.document.borrow();
            (d.text_content(node), element_nonce(&d, node))
        };
        if !self
            .page
            .shared
            .csp
            .inline_style_allowed(node, &text, nonce.as_deref())
        {
            return;
        }
        let base = self.base_url();
        self.register_fonts(&text, &base, None);
        let imports = css_imports(&text);
        if imports.is_empty() {
            return;
        }
        let blocking = if parser_inserted {
            Blocking {
                script: true,
                render: true,
                load: true,
            }
        } else {
            Blocking {
                load: !self.load_fired(),
                ..Blocking::default()
            }
        };
        let mut ids = Vec::new();
        for href in &imports {
            let mut s = ResourceStart::new(
                ResourceType::Stylesheet,
                href,
                base.clone(),
                Initiator::Parser,
            );
            s.hint = PriorityHint::Blocking;
            s.blocking = blocking;
            s.sheet = Some(SheetState {
                depth: 1,
                inline_owner: Some(node),
                ..SheetState::default()
            });
            if let Some(rid) = self.start_resource(s) {
                ids.push(rid);
            }
        }
        if let Some(doc) = self.document.as_mut() {
            doc.inline_style_imports.insert(node, ids);
        }
        self.check_inline_imports(node);
    }

    /// While the parser waits for a script, fetch what the rest of the received markup
    /// references (claimed by the elements once the parser reaches them).
    pub(super) fn scan_preloads(&mut self) {
        let hints = {
            let Some(doc) = self.document.as_mut() else {
                return;
            };
            if doc.info.kind != DocumentKind::Network {
                return;
            }
            let key = (doc.parser.bytes_parsed(), doc.parser.unparsed().len());
            if doc.last_preload_scan == Some(key) {
                return;
            }
            doc.last_preload_scan = Some(key);
            scan_for_preloads(doc.parser.unparsed())
        };
        let base = self.base_url();
        for hint in hints {
            let SubresourceTarget::Network(url) = resolve_subresource(&base, &hint.url) else {
                continue;
            };
            let Some(doc) = self.document.as_mut() else {
                return;
            };
            if !doc.preloaded.insert((hint.kind, url.as_str())) {
                continue;
            }
            let mut s = ResourceStart::new(
                hint.kind,
                &hint.url,
                base.clone(),
                Initiator::PreloadScanner,
            );
            s.hint = match hint.kind {
                ResourceType::Script | ResourceType::Stylesheet => PriorityHint::Blocking,
                _ => PriorityHint::Normal,
            };
            if hint.kind == ResourceType::Stylesheet {
                s.sheet = Some(SheetState::default());
            }
            s.cors = hint.cors;
            self.start_resource(s);
        }
    }

    fn start_resource(&mut self, s: ResourceStart) -> Option<ResourceId> {
        let mut target = resolve_subresource(&s.base, s.raw_url);
        let csp = &self.page.shared.csp;
        if let SubresourceTarget::Network(u) = &target {
            if let Some(upgraded) = csp.upgrade(u) {
                target = SubresourceTarget::Network(upgraded);
            }
        }
        let directive = csp_directive(s.kind);
        let csp_target = csp_url(&target);
        let parser_inserted = matches!(s.initiator, Initiator::Parser | Initiator::PreloadScanner);
        // A preload the policy would block is not started; the element's own request
        // (with its nonce) is checked and reported when the parser reaches it.
        if s.initiator == Initiator::PreloadScanner {
            if let Some(t) = &csp_target {
                let info = axiom_csp::RequestInfo {
                    parser_inserted: true,
                    ..Default::default()
                };
                if csp.list().check_request(directive, t, info).is_blocked() {
                    return None;
                }
            }
        }
        let nonce = match s.node {
            Some(node) => element_nonce(&self.page.document.borrow(), node),
            None if s.module_import.is_some() => {
                self.document.as_ref().and_then(|d| d.module_nonce.clone())
            }
            None => None,
        };
        let doc = self.document.as_mut()?;
        if doc.lifecycle.canceled || doc.info.kind == DocumentKind::Internal {
            return None;
        }
        let doc_kind = doc.info.kind;
        let url = match &target {
            SubresourceTarget::Network(u) => u.as_str(),
            SubresourceTarget::Local(p) => p.display().to_string(),
            SubresourceTarget::Data { mime, .. } => format!("data:{mime},…"),
            SubresourceTarget::Invalid(_) => s.raw_url.trim().to_string(),
        };
        if s.initiator == Initiator::Parser {
            if let Some(rid) = doc.registry.find_unclaimed_preload(s.kind, &url, s.cors) {
                self.adopt_preload(rid, s);
                return Some(rid);
            }
        }
        let blocked_label = super::redacted_url(&url);
        let mut spec = NewResource::new(url, s.kind, s.initiator);
        spec.node = s.node;
        if matches!(s.initiator, Initiator::Parser | Initiator::PreloadScanner) {
            spec.source_offset = Some(doc.parser.bytes_parsed());
        }
        spec.priority = priority_for(s.kind, s.hint);
        spec.blocking = s.blocking;
        spec.script_kind = s.script_kind;
        spec.lazy = s.lazy;
        spec.cors = s.cors;
        let priority = spec.priority;
        let Some(rid) = doc.registry.add(spec) else {
            log::warn!(
                "{}: resource limit reached; {} {} not loaded",
                doc.info.id,
                s.kind.as_str(),
                s.raw_url
            );
            doc.events.record(
                DocumentEventKind::ResourceFailed,
                None,
                None,
                format!("limit: {} not registered", s.kind.as_str()),
            );
            return None;
        };
        doc.events.record(
            DocumentEventKind::ResourceDiscovered,
            Some(rid),
            None,
            format!("{} via {}", s.kind.as_str(), s.initiator.as_str()),
        );
        if let Some(node) = s.node {
            doc.node_resource.insert((node, s.kind), rid);
        }
        if let Some(url) = s.module_import {
            doc.module_imports.insert(rid, url);
        }
        if let Some(sheet) = s.sheet {
            if let Some(parent) = sheet.parent.and_then(|p| doc.sheets.get_mut(&p)) {
                parent.children.push(rid);
            }
            doc.sheets.insert(rid, sheet);
        }
        register_blocking(doc, rid, s.kind, s.blocking);
        if let Some(t) = &csp_target {
            let csp = &self.page.shared.csp;
            if !csp.check_request(directive, t, nonce.as_deref(), parser_inserted, s.node) {
                let msg = format!(
                    "csp: {blocked_label} blocked by the Content Security Policy ({})",
                    directive.name()
                );
                self.fail_resource(rid, ResourceError::policy(msg));
                return Some(rid);
            }
        }
        match target {
            SubresourceTarget::Invalid(why) => {
                self.fail_resource(rid, ResourceError::of(s.kind, FailureStage::Url, why));
            }
            SubresourceTarget::Data { mime, bytes } => {
                let body = Body {
                    bytes,
                    mime: MimeType::parse(&mime),
                    status: 200,
                    final_url: "data:".into(),
                };
                self.complete_resource(rid, body);
            }
            SubresourceTarget::Local(path) => {
                if doc_kind != DocumentKind::LocalFile {
                    let err =
                        ResourceError::policy("local files are only readable by local documents");
                    self.fail_resource(rid, err);
                } else {
                    match fs::read(&path) {
                        Ok(bytes) => {
                            let body = Body {
                                bytes,
                                mime: None,
                                status: 200,
                                final_url: path.display().to_string(),
                            };
                            self.complete_resource(rid, body);
                        }
                        Err(e) => {
                            let err =
                                ResourceError::of(s.kind, FailureStage::Network, e.to_string());
                            self.fail_resource(rid, err);
                        }
                    }
                }
            }
            SubresourceTarget::Network(url) => {
                let redirect_check =
                    self.page
                        .shared
                        .csp
                        .redirect_check(directive, nonce, parser_inserted, s.node);
                self.request_resource(rid, url, s.kind, priority, s.cors, redirect_check);
            }
        }
        Some(rid)
    }

    /// Policy checks, then a request through the resource loader. With `cors` set the
    /// network service applies the CORS check to every cross-origin hop, and a failure
    /// fails the resource like any other network error. `redirect_check` applies the
    /// document's Content Security Policy to redirect targets.
    fn request_resource(
        &mut self,
        rid: ResourceId,
        url: Url,
        kind: ResourceType,
        priority: axiom_net::RequestPriority,
        cors: Option<CorsSettings>,
        redirect_check: Option<RedirectCheck>,
    ) {
        let secure = matches!(self.security, SecurityState::Secure(_));
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        let directive = PolicyDirective::for_resource(kind);
        if let PolicyDecision::Block(why) = self.content_policy.check(directive, &url, &doc.info) {
            let msg = format!(
                "{} blocked by {} ({}): {why}",
                url.as_str(),
                self.content_policy.name(),
                directive.as_str()
            );
            self.fail_resource(rid, ResourceError::policy(msg));
            return;
        }
        match classify_mixed_content(secure, &url, kind) {
            MixedContent::Blockable => {
                let msg = format!("mixed content: {} blocked on a secure page", url.as_str());
                self.fail_resource(rid, ResourceError::policy(msg));
                return;
            }
            MixedContent::Passive => {
                if let Some(r) = doc.registry.get_mut(rid) {
                    r.mixed_content = true;
                }
                self.mixed_content = true;
            }
            MixedContent::NotMixed => {}
        }
        let mut request = ResourceRequest::new(url, kind).with_priority(priority);
        if let Ok(doc_url) = Url::parse(&doc.info.url) {
            request.top_level_url = Some(doc_url.clone());
            request = request.from_document(&doc_url);
        }
        if let Some(cors) = cors {
            request.mode = RequestMode::Cors;
            request.credentials_mode = match cors {
                CorsSettings::Anonymous => CredentialsMode::SameOrigin,
                CorsSettings::UseCredentials => CredentialsMode::Include,
            };
        }
        request.stream_body = true;
        request.redirect_check = redirect_check;
        let id = self.loader.start(request);
        doc.registry.attach_request(rid, id);
        self.request_owner.insert(id, doc.info.id);
        doc.events.record(
            DocumentEventKind::ResourceQueued,
            Some(rid),
            Some(id),
            priority.as_str(),
        );
    }

    fn adopt_preload(&mut self, rid: ResourceId, s: ResourceStart) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        let Some(rec) = doc.registry.get_mut(rid) else {
            return;
        };
        rec.claimed = true;
        rec.node = s.node;
        rec.blocking = s.blocking;
        rec.script_kind = s.script_kind;
        rec.lazy = s.lazy;
        let state = rec.state;
        if let Some(node) = s.node {
            doc.node_resource.insert((node, s.kind), rid);
        }
        doc.events.record(
            DocumentEventKind::ResourceDiscovered,
            Some(rid),
            None,
            "claimed preload",
        );
        if state.is_pending() {
            register_blocking(doc, rid, s.kind, s.blocking);
            let runs_when_ready = s.script_kind.is_some_and(runs_when_available);
            if runs_when_ready && doc.script_sources.contains_key(&rid) {
                doc.async_ready.push_back(rid);
            }
        } else if state == ResourceState::Ready {
            self.apply_to_node(rid);
        } else if let Some(node) = s.node {
            doc.element_events.push_back((node, "error"));
        }
    }

    /// A loader event for one of the active document's subresources.
    pub(super) fn on_resource_event(&mut self, rid: ResourceId, ev: LoaderEvent) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        let kind = doc
            .registry
            .get(rid)
            .map(|r| r.kind)
            .unwrap_or(ResourceType::Other);
        match ev {
            LoaderEvent::Response(resp) => {
                if let Some(r) = doc.registry.get_mut(rid) {
                    r.status = Some(resp.status);
                    r.mime = resp.mime.as_ref().map(MimeType::essence);
                    r.cache = Some(resp.cache_state);
                    r.final_url = Some(resp.url.as_str());
                }
                doc.registry.transition(rid, ResourceState::Fetching);
            }
            LoaderEvent::Chunk { id, bytes } => {
                if doc.buffered_bytes + bytes.len() as u64 > MAX_DOCUMENT_BUFFER {
                    self.loader.cancel(id);
                    let err = ResourceError::of(
                        kind,
                        FailureStage::Limit,
                        format!("document buffer limit ({MAX_DOCUMENT_BUFFER} bytes) exceeded"),
                    );
                    self.fail_resource(rid, err);
                    return;
                }
                doc.buffered_bytes += bytes.len() as u64;
                doc.bodies.entry(rid).or_default().extend_from_slice(&bytes);
            }
            LoaderEvent::Completed(resp) => {
                self.count_response(&resp);
                let Some(doc) = self.document.as_mut() else {
                    return;
                };
                if let Some(r) = doc.registry.get_mut(rid) {
                    r.transferred_bytes = resp.transferred_bytes;
                    r.status = Some(resp.status);
                    r.cache = Some(resp.cache_state);
                    r.final_url = Some(resp.url.as_str());
                }
                let bytes = match doc.bodies.remove(&rid) {
                    Some(b) => {
                        doc.buffered_bytes = doc.buffered_bytes.saturating_sub(b.len() as u64);
                        b
                    }
                    None => resp.body.clone(),
                };
                let body = Body {
                    bytes,
                    mime: resp.mime.clone(),
                    status: resp.status,
                    final_url: resp.url.as_str(),
                };
                self.complete_resource(rid, body);
            }
            LoaderEvent::Failed { error, .. } => {
                let stage = if error == NetworkError::Cancelled {
                    FailureStage::Canceled
                } else {
                    FailureStage::Network
                };
                self.fail_resource(rid, ResourceError::of(kind, stage, error.to_string()));
            }
        }
    }

    fn complete_resource(&mut self, rid: ResourceId, body: Body) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        let Some(rec) = doc.registry.get_mut(rid) else {
            return;
        };
        let kind = rec.kind;
        rec.decoded_bytes = body.bytes.len() as u64;
        rec.status = Some(body.status);
        if rec.mime.is_none() {
            rec.mime = body.mime.as_ref().map(MimeType::essence);
        }
        if !doc.registry.transition(rid, ResourceState::Available) {
            return;
        }
        if !(200..300).contains(&body.status) {
            let err = ResourceError::of(
                kind,
                FailureStage::HttpStatus,
                format!("HTTP {}", body.status),
            );
            self.fail_resource(rid, err);
            return;
        }
        let charset = body.mime.as_ref().and_then(|m| m.charset.clone());
        match kind {
            ResourceType::Stylesheet => {
                if body.mime.as_ref().is_some_and(|m| !m.is_css()) {
                    let err = ResourceError::of(kind, FailureStage::Mime, "not text/css");
                    self.fail_resource(rid, err);
                    return;
                }
                let text = decode_text(&body.bytes, charset.as_deref());
                doc.registry.transition(rid, ResourceState::Processing);
                self.stylesheet_loaded(rid, &text, &body.final_url);
            }
            ResourceType::Script => {
                let script_kind = doc.registry.get(rid).and_then(|r| r.script_kind);
                let module = doc.module_imports.contains_key(&rid)
                    || script_kind.is_some_and(ScriptKind::is_module);
                // Module scripts need a JavaScript MIME type; classic ones only a
                // non-blocked one.
                let rejected = match &body.mime {
                    Some(mime) if module => !mime.is_javascript(),
                    Some(mime) => mime.is_blocked_for_script(),
                    None => false,
                };
                if rejected {
                    let reason = if module {
                        "not a JavaScript MIME type (required for modules)"
                    } else {
                        "MIME type not executable"
                    };
                    self.fail_resource(rid, ResourceError::of(kind, FailureStage::Mime, reason));
                    return;
                }
                let text = decode_text(&body.bytes, charset.as_deref());
                if let Some(url) = doc.module_imports.remove(&rid) {
                    let response_url = if body.final_url == "data:" {
                        url.clone()
                    } else {
                        body.final_url.clone()
                    };
                    self.settle_ready(rid);
                    self.page.module_fetched(&url, Ok((response_url, text)));
                    return;
                }
                doc.script_sources.insert(rid, text);
                if script_kind.is_some_and(runs_when_available) {
                    doc.async_ready.push_back(rid);
                }
            }
            ResourceType::Image => match decode_image(&body.bytes) {
                Ok((width, height, rgba)) => {
                    if let Some(r) = doc.registry.get_mut(rid) {
                        r.image = Some(ImageInfo {
                            width,
                            height,
                            decoded: true,
                        });
                    }
                    doc.decoded_images.insert(
                        rid,
                        DecodedImage {
                            width,
                            height,
                            rgba,
                        },
                    );
                    self.settle_ready(rid);
                }
                Err(e) => {
                    self.fail_resource(
                        rid,
                        ResourceError::of(kind, FailureStage::Decode, e.to_string()),
                    );
                }
            },
            ResourceType::Font => {
                let scope = doc.registry.document().0;
                let registered = match doc.font_loads.get(&rid) {
                    Some((family, bold, italic, _)) => {
                        // The key is scoped to this document so its faces never
                        // resolve for another page using the same family name.
                        let family = family.to_ascii_lowercase();
                        let key = format!("{family}\u{1}{scope}");
                        axiom_text::register_web_font(&key, body.bytes.to_vec(), *bold, *italic)
                            .map(|()| (family, key))
                    }
                    None => Err("font was not requested by @font-face".to_string()),
                };
                match registered {
                    Ok((family, key)) => {
                        self.page.render_mut().web_fonts.insert(family, key);
                        mark_style_dirty(&self.page);
                        self.settle_ready(rid);
                    }
                    Err(e) => {
                        self.fail_resource(rid, ResourceError::of(kind, FailureStage::Decode, e));
                    }
                }
            }
            _ => self.settle_ready(rid),
        }
    }

    /// Stylesheet text arrived: start its `@import`s (resolved against the sheet's own
    /// final URL) and register its fonts; it becomes ready once every import settled.
    fn stylesheet_loaded(&mut self, rid: ResourceId, text: &str, sheet_url: &str) {
        let own = absolutize_urls(&strip_css_imports(text), sheet_url);
        let imports = css_imports(text);
        self.register_fonts(&own, sheet_url, Some(rid));
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        let depth = doc.sheets.get(&rid).map_or(0, |s| s.depth);
        let blocking = doc
            .registry
            .get(rid)
            .map(|r| r.blocking)
            .unwrap_or_default();
        if depth >= MAX_IMPORT_DEPTH && !imports.is_empty() {
            log::warn!("{sheet_url}: @import nesting deeper than {MAX_IMPORT_DEPTH} ignored");
        } else {
            for href in &imports {
                let mut s = ResourceStart::new(
                    ResourceType::Stylesheet,
                    href,
                    sheet_url.to_string(),
                    Initiator::Stylesheet(rid),
                );
                s.hint = PriorityHint::Blocking;
                s.blocking = blocking;
                s.sheet = Some(SheetState {
                    parent: Some(rid),
                    depth: depth + 1,
                    ..SheetState::default()
                });
                self.start_resource(s);
            }
        }
        if let Some(sheet) = self.document.as_mut().and_then(|d| d.sheets.get_mut(&rid)) {
            sheet.own_text = own;
            sheet.children_started = true;
        }
        self.try_finish_sheet(rid);
    }

    fn try_finish_sheet(&mut self, rid: ResourceId) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        let Some(sheet) = doc.sheets.get(&rid) else {
            return;
        };
        let processing = doc
            .registry
            .get(rid)
            .is_some_and(|r| r.state == ResourceState::Processing);
        if !sheet.children_started || !processing {
            return;
        }
        if sheet.children.iter().any(|c| doc.is_pending(*c)) {
            return;
        }
        let mut css = String::new();
        for child in &sheet.children {
            if let Some(text) = doc.sheets.get(child).and_then(|s| s.css.as_deref()) {
                css.push_str(text);
                css.push('\n');
            }
        }
        css.push_str(&sheet.own_text);
        let (parent, owner) = (sheet.parent, sheet.inline_owner);
        if let Some(sheet) = doc.sheets.get_mut(&rid) {
            sheet.css = Some(css);
        }
        self.settle_ready(rid);
        if let Some(parent) = parent {
            self.try_finish_sheet(parent);
        }
        if let Some(style) = owner {
            self.check_inline_imports(style);
        }
    }

    /// Every `@import` of a `<style>` settled: expose the imported CSS before its text.
    fn check_inline_imports(&mut self, style: NodeId) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        let Some(ids) = doc.inline_style_imports.get(&style) else {
            return;
        };
        if ids.iter().any(|r| doc.is_pending(*r)) {
            return;
        }
        let mut css = String::new();
        for rid in ids {
            if let Some(text) = doc.sheets.get(rid).and_then(|s| s.css.as_deref()) {
                css.push_str(text);
                css.push('\n');
            }
        }
        doc.inline_style_imports.remove(&style);
        self.page.render_mut().inline_imports.insert(style, css);
        mark_style_dirty(&self.page);
    }

    fn register_fonts(&mut self, css: &str, base: &str, sheet: Option<ResourceId>) {
        let faces = font_faces(css);
        let families = font_families_used(css);
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        if faces.is_empty() && families.is_empty() {
            return;
        }
        doc.font_faces
            .extend(faces.into_iter().map(|face| PendingFace {
                face,
                base: base.to_string(),
                sheet,
                requested: false,
                source: 0,
            }));
        doc.font_families.extend(families);
        self.request_used_fonts();
    }

    /// Fetch `@font-face` sources whose family some rule uses. Fonts load through the
    /// resource loader and block `load`; each becomes usable (and restyles the page) once
    /// it has been registered with the text system.
    fn request_used_fonts(&mut self) {
        loop {
            let Some(doc) = self.document.as_mut() else {
                return;
            };
            if doc.fonts_requested >= MAX_FONTS_PER_DOCUMENT {
                return;
            }
            let Some((index, face)) = doc.font_faces.iter_mut().enumerate().find(|(_, f)| {
                !f.requested
                    && doc
                        .font_families
                        .contains(&f.face.family.to_ascii_lowercase())
            }) else {
                return;
            };
            face.requested = true;
            let Some(src) = face.face.sources.get(face.source).cloned() else {
                continue;
            };
            let (family, base, sheet) = (face.face.family.clone(), face.base.clone(), face.sheet);
            let (bold, italic) = (face.face.bold, face.face.italic);
            doc.fonts_requested += 1;
            let initiator = sheet.map_or(Initiator::Parser, Initiator::Stylesheet);
            let mut s = ResourceStart::new(ResourceType::Font, &src, base, initiator);
            s.blocking.load = !doc.lifecycle.load_fired;
            // CSS Fonts 4 §4.9: fonts are fetched in `cors` mode, credentials same-origin.
            s.cors = Some(CorsSettings::Anonymous);
            if let Some(rid) = self.start_resource(s) {
                if let Some(d) = self.document.as_mut() {
                    if let Some(r) = d.registry.get_mut(rid) {
                        r.font_family = Some(family.clone());
                    }
                    d.font_loads.insert(rid, (family, bold, italic, index));
                }
            }
        }
    }

    /// Fetch the `url()` background and mask images the last style pass found on rendered
    /// elements. Each is requested once per document; like `<img>`, they block `load`.
    pub(super) fn request_css_images(&mut self) {
        let wanted = std::mem::take(&mut self.page.render_mut().css_image_urls);
        let base = self.base_url();
        for url in wanted {
            let Some(doc) = self.document.as_mut() else {
                return;
            };
            if doc.css_images_requested.len() >= MAX_CSS_IMAGES_PER_DOCUMENT {
                return;
            }
            if !doc.css_images_requested.insert(url.clone()) {
                continue;
            }
            let load_fired = doc.lifecycle.load_fired;
            let mut s =
                ResourceStart::new(ResourceType::Image, &url, base.clone(), Initiator::Parser);
            s.blocking.load = !load_fired;
            let Some(rid) = self.start_resource(s) else {
                continue;
            };
            let Some(d) = self.document.as_mut() else {
                return;
            };
            d.css_image_loads.insert(rid, url);
            // A claimed preload may already have loaded.
            if d.registry.get(rid).map(|r| r.state) == Some(ResourceState::Ready) {
                self.apply_to_node(rid);
            }
        }
    }

    fn settle_ready(&mut self, rid: ResourceId) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        if !doc.registry.transition(rid, ResourceState::Ready) {
            return;
        }
        doc.events
            .record(DocumentEventKind::ResourceReady, Some(rid), None, "");
        doc.lifecycle.resource_settled(rid);
        self.apply_to_node(rid);
    }

    /// Hand a ready stylesheet or image to its element, unless a newer resource replaced
    /// it there.
    fn apply_to_node(&mut self, rid: ResourceId) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        if let Some(url) = doc.css_image_loads.remove(&rid) {
            if let Some(image) = doc.decoded_images.remove(&rid) {
                self.page
                    .render_mut()
                    .css_images
                    .insert(url, Arc::new(image));
                self.page.document.borrow_mut().dirty.paint = true;
            }
            return;
        }
        let Some((kind, Some(node))) = doc.registry.get(rid).map(|r| (r.kind, r.node)) else {
            return;
        };
        if doc.node_resource.get(&(node, kind)) != Some(&rid) {
            return;
        }
        match kind {
            ResourceType::Stylesheet => {
                if let Some(css) = doc.sheets.get(&rid).and_then(|s| s.css.clone()) {
                    self.page.render_mut().external_css.insert(node, css);
                    mark_style_dirty(&self.page);
                    doc.element_events.push_back((node, "load"));
                }
            }
            ResourceType::Image => {
                if let Some(image) = doc.decoded_images.remove(&rid) {
                    self.page.render_mut().images.insert(node, Arc::new(image));
                    let mut d = self.page.document.borrow_mut();
                    d.dirty.layout = true;
                    d.dirty.paint = true;
                    doc.element_events.push_back((node, "load"));
                }
            }
            _ => {}
        }
    }

    pub(super) fn fail_resource(&mut self, rid: ResourceId, err: ResourceError) {
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        let canceled = err.stage == FailureStage::Canceled;
        let message = err.to_string();
        if !doc.registry.fail(rid, err) {
            return;
        }
        if let Some(b) = doc.bodies.remove(&rid) {
            doc.buffered_bytes = doc.buffered_bytes.saturating_sub(b.len() as u64);
        }
        doc.script_sources.remove(&rid);
        doc.decoded_images.remove(&rid);
        doc.css_image_loads.remove(&rid);
        let kind = if canceled {
            DocumentEventKind::ResourceCanceled
        } else {
            DocumentEventKind::ResourceFailed
        };
        doc.events.record(kind, Some(rid), None, message.clone());
        doc.lifecycle.resource_settled(rid);
        let (rkind, node, url) = doc
            .registry
            .get(rid)
            .map(|r| (r.kind, r.node, r.url.clone()))
            .unwrap_or((ResourceType::Other, None, String::new()));
        if let Some(module) = node.and_then(|n| doc.module_scripts.get_mut(&n)) {
            if module.resource == Some(rid) && module.state == ModuleState::Fetching {
                module.state = ModuleState::Done;
            }
        }
        if let Some(import) = doc.module_imports.remove(&rid) {
            if !canceled {
                log::warn!("{url}: {message}");
            }
            self.page.module_fetched(&import, Err(message));
            return;
        }
        if !canceled {
            log::warn!("{url}: {message}");
            if let Some(node) = node.filter(|n| doc.node_resource.get(&(*n, rkind)) == Some(&rid)) {
                doc.element_events.push_back((node, "error"));
            }
        }
        // CSS Fonts 4 §4.3: a source that fails to load or decode falls back to the next.
        let font_face = doc.font_loads.remove(&rid).map(|(.., index)| index);
        if let Some(face) = font_face
            .filter(|_| !canceled)
            .and_then(|i| doc.font_faces.get_mut(i))
        {
            if face.source + 1 < face.face.sources.len() {
                face.source += 1;
                face.requested = false;
                self.request_used_fonts();
                return;
            }
        }
        if rkind == ResourceType::Stylesheet {
            let (parent, owner) = doc
                .sheets
                .get(&rid)
                .map_or((None, None), |s| (s.parent, s.inline_owner));
            if let Some(parent) = parent {
                self.try_finish_sheet(parent);
            }
            if let Some(style) = owner {
                self.check_inline_imports(style);
            }
        }
    }
}

/// Scripts processed as soon as their source arrives (classic ones run, module ones
/// start loading their graph).
fn runs_when_available(kind: ScriptKind) -> bool {
    matches!(
        kind,
        ScriptKind::ClassicAsync
            | ScriptKind::ClassicDynamic
            | ScriptKind::Module
            | ScriptKind::ModuleAsync
    )
}

fn mark_style_dirty(page: &crate::page::Page) {
    let mut d = page.document.borrow_mut();
    d.dirty.style = true;
    d.dirty.layout = true;
    d.dirty.paint = true;
}
