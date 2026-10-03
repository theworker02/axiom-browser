//! JavaScript runtime abstraction for Axiom.
//!
//! The rest of the engine depends on these types — not on Boa directly.
//! Backend: embeddable Boa engine.
//!
//! Event listeners and timer callbacks are stored **in the JS realm** so
//! closures remain live under Boa's GC. Rust only schedules and dispatches.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use boa_engine::builtins::promise::{OperationType, PromiseState};
use boa_engine::context::{ContextBuilder, HostHooks};
use boa_engine::object::builtins::{JsArray, JsArrayBuffer, JsPromise};
use boa_engine::{
    js_string, Context, JsArgs, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue,
    NativeFunction, Source,
};
use thiserror::Error;

mod modules;

pub use modules::{
    is_url_like_specifier, ModuleOutcome, RESOLVE_FAILURE as MODULE_RESOLVE_FAILURE,
};
use modules::{AxiomModuleLoader, ModuleJob, ModuleMap};

/// A `fetch()` call as normalized by the JS bindings. The host re-validates everything:
/// the JS layer is convenience, the host is the security boundary.
#[derive(Debug, Clone, Default)]
pub struct FetchInit {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    /// The body is a `ReadableStream`: bytes arrive later through
    /// [`JsHost::fetch_upload_write`] and end with [`JsHost::fetch_upload_close`].
    pub body_stream: bool,
    pub mode: String,
    pub credentials: String,
    pub cache: String,
    pub redirect: String,
    /// `"about:client"` (default), `""` (no referrer) or a same-origin URL.
    pub referrer: String,
    pub referrer_policy: String,
    pub integrity: String,
    pub keepalive: bool,
    pub priority: String,
}

/// Response head delivered to a pending `fetch()`, already filtered by the host.
#[derive(Debug, Clone)]
pub struct FetchResponseHead {
    /// `"basic"`, `"cors"`, `"opaque"` or `"opaqueredirect"`.
    pub response_type: &'static str,
    pub status: u16,
    pub status_text: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub redirected: bool,
}

#[derive(Debug, Error)]
pub enum JsEngineError {
    #[error("javascript error: {0}")]
    Runtime(String),
}

#[derive(Debug, Clone)]
pub struct JsValueHandle {
    pub display: String,
}

/// One element attribute as seen by script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomAttr {
    pub namespace: Option<String>,
    pub prefix: Option<String>,
    pub local_name: String,
    pub value: String,
}

/// An element's layout geometry for CSSOM View, in CSS px.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ElementGeometry {
    /// Border-box fragments (one per line for inline boxes) in viewport coordinates:
    /// `[x, y, width, height]`.
    pub client_rects: Vec<[f64; 4]>,
    /// The `offsetParent` element; `-1` for none.
    pub offset_parent: i32,
    /// `offsetLeft`, `offsetTop`, `offsetWidth`, `offsetHeight`.
    pub offset: [f64; 4],
    /// `clientLeft`, `clientTop`, `clientWidth`, `clientHeight`.
    pub client: [f64; 4],
    /// `scrollWidth`, `scrollHeight`.
    pub scroll_size: [f64; 2],
}

/// A child browsing realm as exposed through an embedding element. The numeric IDs are
/// stable runtime identities, never DOM node IDs: child node identifiers remain local
/// to their owning document arena.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameInfo {
    pub browsing_context_id: u64,
    pub document_id: u64,
    pub same_origin: bool,
    /// Present only when the caller is same-origin with the child.
    pub url: Option<String>,
    /// MIME type of a same-origin child document. Cross-origin callers receive no
    /// document proxy and therefore no content-type disclosure.
    pub content_type: Option<String>,
}

/// Shadow root option bits shared by [`JsHost::attach_shadow`] and the DOM prelude.
pub mod shadow_flags {
    pub const OPEN: u32 = 1;
    pub const DELEGATES_FOCUS: u32 = 2;
    pub const MANUAL_SLOTS: u32 = 4;
    pub const CLONABLE: u32 = 8;
    pub const SERIALIZABLE: u32 = 16;
}

/// Host callbacks from JS into the engine/DOM.
pub trait JsHost {
    fn get_element_by_id(&mut self, id: &str) -> i32;
    fn query_selector(&mut self, sel: &str) -> i32;
    /// Metadata for the child frame embedded by `iframe`. Cross-origin callers receive
    /// identity only; the host must not disclose its document or URL.
    fn frame_info(&mut self, _iframe: i32) -> Option<FrameInfo> {
        None
    }
    /// Same-origin child-document queries. `iframe` identifies the embedding element;
    /// `node` values returned here are meaningful only with the matching iframe.
    fn frame_get_element_by_id(&mut self, _iframe: i32, _id: &str) -> i32 {
        -1
    }
    fn frame_query_selector(&mut self, _iframe: i32, _selector: &str) -> i32 {
        -1
    }
    fn frame_document_node(&mut self, _iframe: i32, _which: &str) -> i32 {
        -1
    }
    fn frame_node_text(&mut self, _iframe: i32, _node: i32) -> String {
        String::new()
    }
    fn frame_node_tag_name(&mut self, _iframe: i32, _node: i32) -> String {
        String::new()
    }
    fn frame_node_namespace_uri(&mut self, _iframe: i32, _node: i32) -> Option<String> {
        None
    }
    /// Create a detached element owned by a same-origin child document. The returned
    /// handle is meaningful only together with the embedding iframe.
    fn frame_create_element(&mut self, _iframe: i32, _tag: &str) -> i32 {
        -1
    }
    /// Queue a structured-clone JSON payload for the child frame. The host validates
    /// target-origin matching before it enqueues anything.
    fn frame_post_message(
        &mut self,
        _iframe: i32,
        _data_json: &str,
        _target_origin: &str,
    ) -> Result<(), String> {
        Err("frame messaging is not available in this context".into())
    }
    fn create_element(&mut self, tag: &str) -> i32;
    fn set_text_content(&mut self, node: i32, text: &str);
    fn get_text_content(&mut self, node: i32) -> String;
    fn set_attribute(&mut self, node: i32, name: &str, value: &str);
    fn append_child(&mut self, parent: i32, child: i32);
    fn log(&mut self, msg: &str);
    /// Notify host that a JS listener exists (for diagnostics / optional rust-side routing).
    fn add_event_listener(&mut self, node: i32, type_: &str);
    /// The `User-Agent` the document's requests carry (`navigator.userAgent`).
    fn user_agent(&mut self) -> String {
        "Axiom".into()
    }
    /// The user's preferred languages, most preferred first (`navigator.languages`).
    fn languages(&mut self) -> Vec<String> {
        vec!["en-US".into()]
    }
    /// Whether the document can use cookies (`navigator.cookieEnabled`).
    fn cookie_enabled(&mut self) -> bool {
        false
    }
    /// Whether media query list `query` matches the current viewport (`matchMedia`).
    fn match_media(&mut self, _query: &str) -> bool {
        false
    }
    /// The viewport size in CSS px (`innerWidth` / `innerHeight`).
    fn viewport_size(&mut self) -> (f32, f32) {
        (0.0, 0.0)
    }
    /// The connected element that has focus; `-1` when none does.
    fn focused_element(&mut self) -> i32 {
        -1
    }
    /// Move focus to element `node`, or clear it when `node` is `-1` (`focus()` /
    /// `blur()`; script fires the focus events).
    fn set_focus(&mut self, _node: i32) {}
    /// Whether the style engine supports a `@supports` condition (`CSS.supports`).
    fn css_supports(&mut self, _condition: &str) -> bool {
        false
    }
    /// Document metadata for the `Document` getters: `"referrer"`, `"contentType"`,
    /// `"characterSet"`, `"compatMode"` (`CSS1Compat` / `BackCompat`) and `"lastModified"`
    /// (Unix seconds, `""` when unknown). Unknown keys are `""`.
    fn document_info(&mut self, _key: &str) -> String {
        String::new()
    }
    /// Number of entries in the session history (`history.length`).
    fn history_length(&mut self) -> u32 {
        1
    }
    /// Script state id of the current session history entry (`history.state`).
    fn history_state(&mut self) -> Option<u64> {
        None
    }
    /// `pushState` / `replaceState`: `url` (resolved against the document URL; `None`
    /// keeps the current one) becomes the document URL of a new or the current entry
    /// with script state `state`. Returns the new URL, or the DOMException name to throw.
    fn history_push(
        &mut self,
        _url: Option<&str>,
        _state: u64,
        _replace: bool,
    ) -> Result<String, &'static str> {
        Err("SecurityError")
    }
    /// `history.go(delta)`: queue a traversal (`0` reloads).
    fn history_traverse(&mut self, _delta: i32) {}
    /// Navigate to `url` (resolved against the document URL) for `location`; `false`
    /// when it does not parse.
    fn navigate(&mut self, _url: &str, _replace: bool) -> bool {
        false
    }
    /// `location.reload()`.
    fn reload(&mut self) {}
    /// The resolved value of `property` on element `node` (`getComputedStyle`), after
    /// flushing style and layout; `None` for unknown properties and non-elements.
    /// `pseudo` is `""` or a pseudo-element such as `"::before"`.
    fn computed_value(&mut self, _node: i32, _pseudo: &str, _property: &str) -> Option<String> {
        None
    }
    /// The longhands a computed style declaration enumerates (`length` / `item`).
    fn computed_property_names(&mut self) -> Vec<String> {
        Vec::new()
    }
    /// Element `node`'s layout geometry after flushing style and layout; `None` when it
    /// is not an element.
    fn element_geometry(&mut self, _node: i32) -> Option<ElementGeometry> {
        None
    }
    /// The viewport's scroll position (`scrollX`, `scrollY`).
    fn scroll_position(&mut self) -> (f64, f64) {
        (0.0, 0.0)
    }
    /// Scroll the viewport to (`x`, `y`), clamped to the scrollable area.
    fn scroll_to(&mut self, _x: f64, _y: f64) {}
    /// Intrinsic size of image element `node`'s decoded image; `None` until it decodes.
    fn image_natural_size(&mut self, _node: i32) -> Option<(u32, u32)> {
        None
    }
    fn get_document_cookie(&mut self) -> String {
        String::new()
    }
    fn set_document_cookie(&mut self, _value: &str) {}
    fn storage_get(&mut self, _session: bool, _key: &str) -> Option<String> {
        None
    }
    fn storage_set(&mut self, _session: bool, _key: &str, _value: &str) -> Result<(), String> {
        Err("SecurityError".into())
    }
    fn storage_remove(&mut self, _session: bool, _key: &str) {}
    fn storage_clear(&mut self, _session: bool) {}
    fn storage_length(&mut self, _session: bool) -> usize {
        0
    }
    fn storage_key(&mut self, _session: bool, _index: usize) -> Option<String> {
        None
    }
    /// Resolve `input` against the document URL; `None` if it is not a valid URL.
    fn resolve_url(&mut self, _input: &str) -> Option<String> {
        None
    }
    /// Resolve `input` against the absolute URL `base`; `None` if it is not a valid URL.
    fn resolve_url_against(&mut self, _base: &str, _input: &str) -> Option<String> {
        None
    }
    /// HTML "resolve a module specifier" for an import in the module at `base` (`None`:
    /// a document script). Without import maps only URL-like specifiers resolve.
    fn resolve_module_specifier(
        &mut self,
        base: Option<&str>,
        specifier: &str,
    ) -> Result<String, String> {
        if !is_url_like_specifier(specifier) {
            return Err(modules::bare_specifier_error(specifier));
        }
        let resolved = match base {
            Some(base) => self.resolve_url_against(base, specifier),
            None => self.resolve_url(specifier),
        };
        resolved.ok_or_else(|| format!("{MODULE_RESOLVE_FAILURE} \"{specifier}\""))
    }
    /// Start a fetch. Returns the request id; the outcome is delivered through
    /// [`JsContext::fetch_response`] and friends. `Err` rejects with a `TypeError`.
    fn fetch_start(&mut self, _init: FetchInit) -> Result<u64, String> {
        Err("fetch is not available in this context".into())
    }
    fn fetch_abort(&mut self, _id: u64) {}
    /// Append a chunk to fetch `id`'s streaming request body. `Ok(false)` asks script to
    /// wait for [`JsContext::fetch_upload_drained`]; `Err` means the request is gone.
    fn fetch_upload_write(&mut self, _id: u64, _bytes: Vec<u8>) -> Result<bool, String> {
        Err("no such upload".into())
    }
    fn fetch_upload_close(&mut self, _id: u64) {}
    fn fetch_upload_error(&mut self, _id: u64, _message: &str) {}
    /// Script consumed `bytes` of fetch `id`'s response body (flow-control credit).
    fn fetch_consumed(&mut self, _id: u64, _bytes: u64) {}
    /// Referrer for a request `referrer` URL: the URL itself when same-origin with the
    /// document, otherwise `"about:client"`.
    fn fetch_referrer(&mut self, _url: &str) -> String {
        "about:client".into()
    }
    /// The first attribute whose qualified name is exactly `name` (the bindings lowercase
    /// names for HTML elements).
    fn get_attribute(&mut self, _node: i32, _name: &str) -> Option<String> {
        None
    }
    /// Removes the first attribute whose qualified name is exactly `name`.
    fn remove_attribute(&mut self, _node: i32, _name: &str) {}
    /// Element local name, doctype name or processing-instruction target.
    fn tag_name(&mut self, _node: i32) -> String {
        String::new()
    }
    fn create_text_node(&mut self, _data: &str) -> i32 {
        -1
    }
    fn create_cdata_section(&mut self, _data: &str) -> i32 {
        -1
    }
    fn create_comment(&mut self, _data: &str) -> i32 {
        -1
    }
    fn create_processing_instruction(&mut self, _target: &str, _data: &str) -> i32 {
        -1
    }
    fn create_document_fragment(&mut self) -> i32 {
        -1
    }
    /// A new inert Document node (`createHTMLDocument`, `new Document()`); `-1` if
    /// unsupported.
    fn create_document(&mut self) -> i32 {
        -1
    }
    /// `DOMParser` `text/html`: `markup` parsed as a whole document (scripting disabled,
    /// scripts never run) under a new inert Document node; `-1` if unsupported.
    fn parse_html_document(&mut self, _markup: &str) -> i32 {
        -1
    }
    /// `DOMParser` XML types: `markup` parsed as XML under a new inert Document node; a
    /// well-formedness error yields a `parsererror` document. `-1` if unsupported.
    fn parse_xml_document(&mut self, _markup: &str) -> i32 {
        -1
    }
    /// `createElementNS`. `Err` is the `DOMException` name.
    fn create_element_ns(
        &mut self,
        _namespace: Option<&str>,
        _qualified_name: &str,
    ) -> Result<i32, String> {
        Err("NotSupportedError".into())
    }
    /// Pre-insert `node` into `parent` before `child` (`-1` appends). `Err` is the
    /// `DOMException` name.
    fn insert_before(&mut self, _parent: i32, _node: i32, _child: i32) -> Result<(), String> {
        Err("NotSupportedError".into())
    }
    /// Replace `child` of `parent` with `node`. `Err` is the `DOMException` name.
    fn replace_child(&mut self, _parent: i32, _node: i32, _child: i32) -> Result<(), String> {
        Err("NotSupportedError".into())
    }
    /// A detached copy of `node` (with descendants when `deep`); `-1` if absent.
    fn clone_node(&mut self, _node: i32, _deep: bool) -> i32 {
        -1
    }
    /// An element's namespace URI.
    fn namespace_uri(&mut self, _node: i32) -> Option<String> {
        None
    }
    /// An element's namespace prefix.
    fn prefix(&mut self, _node: i32) -> Option<String> {
        None
    }
    /// An element's attributes in order.
    fn attributes(&mut self, _node: i32) -> Vec<DomAttr> {
        Vec::new()
    }
    fn get_attribute_ns(
        &mut self,
        _node: i32,
        _namespace: Option<&str>,
        _local_name: &str,
    ) -> Option<String> {
        None
    }
    /// `setAttributeNS`. `Err` is the `DOMException` name.
    fn set_attribute_ns(
        &mut self,
        _node: i32,
        _namespace: Option<&str>,
        _qualified_name: &str,
        _value: &str,
    ) -> Result<(), String> {
        Err("NotSupportedError".into())
    }
    fn remove_attribute_ns(&mut self, _node: i32, _namespace: Option<&str>, _local_name: &str) {}
    /// Sets `attr` on element `node`, replacing the attribute with the same namespace
    /// and local name in place (prefix included) or appending it (`setAttributeNode`).
    fn replace_attribute(&mut self, _node: i32, _attr: &DomAttr) {}
    /// "Validate and extract" for an attribute name: `(namespace, prefix, local name)`.
    /// `Err` is the `DOMException` name.
    #[allow(clippy::type_complexity)]
    fn validate_attribute_name(
        &mut self,
        _namespace: Option<&str>,
        _qualified_name: &str,
    ) -> Result<(Option<String>, Option<String>, String), String> {
        Err("NotSupportedError".into())
    }
    /// Whether element `node` matches `selectors`; `None` when they do not parse.
    fn matches_selectors(&mut self, _node: i32, _selectors: &str) -> Option<bool> {
        None
    }
    /// The nearest inclusive ancestor element of `node` matching `selectors` (-1 when
    /// none); `None` when they do not parse.
    fn closest(&mut self, _node: i32, _selectors: &str) -> Option<i32> {
        None
    }
    /// The contents fragment of HTML `template` element `node` (created on first use);
    /// `None` for other nodes.
    fn template_content(&mut self, _node: i32) -> Option<i32> {
        None
    }
    /// Attaches a shadow root to `host` (DOM §4.2.2.3). `flags` is a [`shadow_flags`]
    /// bit set. Returns the root, or `-1` when `host` cannot host one (NotSupportedError).
    fn attach_shadow(&mut self, _host: i32, _flags: u32) -> i32 {
        -1
    }
    /// `host`'s shadow root and its [`shadow_flags`], open or closed.
    fn shadow_root_info(&mut self, _host: i32) -> Option<(i32, u32)> {
        None
    }
    /// The host of shadow root `root`; `-1` when `root` is not a shadow root.
    fn shadow_host(&mut self, _root: i32) -> i32 {
        -1
    }
    /// Shadow roots whose hosts are connected, in shadow-including tree order.
    fn connected_shadow_roots(&mut self) -> Vec<i32> {
        Vec::new()
    }
    /// Whether any shadow root exists (connected or not).
    fn has_shadow_trees(&mut self) -> bool {
        false
    }
    /// A slot's assigned nodes, optionally flattened (HTML `assignedNodes`).
    fn assigned_nodes(&mut self, _slot: i32, _flatten: bool) -> Vec<i32> {
        Vec::new()
    }
    /// The slot `node` is assigned to, open or closed; `-1` for none.
    fn assigned_slot(&mut self, _node: i32) -> i32 {
        -1
    }
    /// Manual slot assignment (`HTMLSlotElement.assign`).
    fn assign_slot(&mut self, _slot: i32, _nodes: Vec<i32>) {}
    /// Marks `node` as an upgraded custom element (for `:defined`).
    fn set_custom_element_defined(&mut self, _node: i32) {}
    /// HTML serialization of `node`'s children.
    fn inner_html(&mut self, _node: i32) -> String {
        String::new()
    }
    /// HTML serialization of `node`.
    fn outer_html(&mut self, _node: i32) -> String {
        String::new()
    }
    /// Replaces `node`'s children with `markup` parsed as a fragment in its context.
    /// Parsed scripts never run.
    fn set_inner_html(&mut self, _node: i32, _markup: &str) -> Result<(), String> {
        Err("NotSupportedError".into())
    }
    /// Replaces `node` with `markup` parsed as a fragment in its parent's context.
    fn set_outer_html(&mut self, _node: i32, _markup: &str) -> Result<(), String> {
        Err("NotSupportedError".into())
    }
    /// `insertAdjacentHTML`; `position` is not yet validated.
    fn insert_adjacent_html(
        &mut self,
        _node: i32,
        _position: &str,
        _markup: &str,
    ) -> Result<(), String> {
        Err("NotSupportedError".into())
    }
    /// A doctype's public and system identifiers.
    fn doctype_ids(&mut self, _node: i32) -> (String, String) {
        (String::new(), String::new())
    }
    /// A detached doctype node (`DOMImplementation.createDocumentType`).
    fn create_doctype(&mut self, _name: &str, _public_id: &str, _system_id: &str) -> i32 {
        -1
    }
    /// A value that changes whenever the tree, an attribute or character data changes;
    /// `None` when the host cannot tell, so script must not cache tree queries.
    fn dom_version(&mut self) -> Option<u64> {
        None
    }
    /// `"body"`, `"head"` or `"documentElement"`; `-1` when absent.
    fn document_node(&mut self, _which: &str) -> i32 {
        -1
    }
    /// `document.readyState`.
    fn ready_state(&mut self) -> String {
        "complete".into()
    }
    /// A form control's current value (the `value` IDL attribute).
    fn control_value(&mut self, _node: i32) -> String {
        String::new()
    }
    fn set_control_value(&mut self, _node: i32, _value: &str) {}
    /// Checkedness of a checkbox or radio button; selectedness of an option.
    fn control_checked(&mut self, _node: i32) -> bool {
        false
    }
    fn set_control_checked(&mut self, _node: i32, _checked: bool) {}
    /// A select's selected option index; `-1` when none is selected.
    fn selected_index(&mut self, _select: i32) -> i32 {
        -1
    }
    /// A listed element's form owner; `-1` when it has none.
    fn form_owner(&mut self, _node: i32) -> i32 {
        -1
    }
    /// A form's listed elements in tree order.
    fn form_controls(&mut self, _form: i32) -> Vec<i32> {
        Vec::new()
    }
    /// `(willValidate, validity flag name or "", validationMessage)`.
    fn control_validity(&mut self, _node: i32) -> (bool, String, String) {
        (false, String::new(), String::new())
    }
    fn set_custom_validity(&mut self, _node: i32, _message: &str) {}
    /// A form's controls that fail constraint validation, in tree order.
    fn invalid_controls(&mut self, _form: i32) -> Vec<i32> {
        Vec::new()
    }
    /// The form data set as `(name, value, is_file)`; `submitter` is `-1` for none.
    fn form_entries(&mut self, _form: i32, _submitter: i32) -> Vec<(String, String, bool)> {
        Vec::new()
    }
    /// Plan the form's navigation (capturing its entry list) and queue it for the
    /// browsing context. `Err` says why nothing will navigate.
    fn submit_form(&mut self, _form: i32, _submitter: i32) -> Result<(), String> {
        Err("form submission is not available in this context".into())
    }
    /// Restore the form's controls to their defaults.
    fn reset_form(&mut self, _form: i32) {}
    /// Inline classic scripts connected by the last DOM insertion (node, source), in tree
    /// order. They run synchronously inside the inserting call.
    fn take_inserted_scripts(&mut self) -> Vec<(i32, String)> {
        Vec::new()
    }
    /// Content Security Policy check for the event handler content attribute `source` on
    /// element `node` (`-1` for none); `false` leaves the handler `null`.
    fn csp_allows_handler(&mut self, _node: i32, _source: &str) -> bool {
        true
    }
    /// The Document node itself; `-1` when there is none.
    fn document_root(&mut self) -> i32 {
        -1
    }
    /// DOM `nodeType` (1 element, 3 text, 8 comment, 9 document, 10 doctype); 0 if absent.
    fn node_type(&mut self, _node: i32) -> u16 {
        0
    }
    fn parent_node(&mut self, _node: i32) -> i32 {
        -1
    }
    /// Children in tree order.
    fn child_nodes(&mut self, _node: i32) -> Vec<i32> {
        Vec::new()
    }
    /// Detach `child` from `parent`; `false` when `child` is not a child of `parent`.
    fn remove_child(&mut self, _parent: i32, _child: i32) -> bool {
        false
    }
    /// Descendant elements of `root` (not `root` itself) in tree order whose qualified
    /// name is `name` (ASCII-lowercased for HTML elements), or all of them for `"*"`.
    fn elements_by_tag_name(&mut self, _root: i32, _name: &str) -> Vec<i32> {
        Vec::new()
    }
    /// Descendant elements of `root` matching `selectors`, in tree order.
    fn query_selector_all(&mut self, _root: i32, _selectors: &str) -> Vec<i32> {
        Vec::new()
    }
}

type EvalPolicy = Box<dyn Fn(&str) -> bool>;

/// HostEnsureCanCompileStrings state of a realm (CSP `'unsafe-eval'`), kept in the
/// realm's host-defined slot where script cannot reach it.
struct EvalGate {
    policy: RefCell<Option<EvalPolicy>>,
    /// Engine-internal compilation (event handler attributes, already checked against
    /// `script-src-attr`) is in progress.
    internal: Cell<u32>,
}

impl boa_engine::gc::Finalize for EvalGate {}
// SAFETY: holds no GC-managed values.
unsafe impl boa_engine::gc::Trace for EvalGate {
    boa_engine::gc::empty_trace!();
}
impl boa_engine::JsData for EvalGate {}

fn with_eval_gate<R>(context: &Context, f: impl FnOnce(&EvalGate) -> R) -> Option<R> {
    let realm = context.realm().clone();
    let defined = realm.host_defined();
    defined.get::<EvalGate>().map(f)
}

/// Forwards HostPromiseRejectionTracker to the prelude, which implements the HTML
/// "notify about rejected promises" bookkeeping in the realm, and gates `eval` /
/// `Function` on the realm's [`EvalGate`].
struct RejectionHooks;

impl HostHooks for RejectionHooks {
    fn ensure_can_compile_strings(
        &self,
        realm: boa_engine::realm::Realm,
        _parameters: &[JsString],
        body: &JsString,
        _direct: bool,
        _context: &mut Context,
    ) -> JsResult<()> {
        let defined = realm.host_defined();
        let Some(gate) = defined.get::<EvalGate>() else {
            return Ok(());
        };
        if gate.internal.get() > 0 {
            return Ok(());
        }
        let allowed = match &*gate.policy.borrow() {
            Some(policy) => policy(&body.to_std_string_escaped()),
            None => true,
        };
        if allowed {
            Ok(())
        } else {
            Err(JsNativeError::eval()
                .with_message(
                    "Refused to evaluate a string as JavaScript because 'unsafe-eval' is not an \
                     allowed source of script in the Content Security Policy",
                )
                .into())
        }
    }

    fn promise_rejection_tracker(
        &self,
        promise: &JsObject,
        operation: OperationType,
        context: &mut Context,
    ) {
        let reason = match JsPromise::from_object(promise.clone()).map(|p| p.state()) {
            Ok(PromiseState::Rejected(v)) => v,
            _ => JsValue::undefined(),
        };
        let global = context.global_object();
        let Ok(track) = global.get(js_string!("__axiom_trackRejection"), context) else {
            return;
        };
        if let Some(track) = track.as_callable() {
            let rejected = matches!(operation, OperationType::Reject);
            let _ = track.call(
                &JsValue::undefined(),
                &[promise.clone().into(), reason, JsValue::from(rejected)],
                context,
            );
        }
    }
}

static REJECTION_HOOKS: RejectionHooks = RejectionHooks;

/// Run promise jobs, then report rejections that are still unhandled.
fn microtask_checkpoint(context: &mut Context) {
    context.run_jobs();
    let global = context.global_object();
    if let Ok(notify) = global.get(js_string!("__axiom_notifyRejections"), context) {
        if let Some(notify) = notify.as_callable() {
            let _ = notify.call(&JsValue::undefined(), &[], context);
            context.run_jobs();
        }
    }
}

struct NullHost;
impl JsHost for NullHost {
    fn get_element_by_id(&mut self, _: &str) -> i32 {
        -1
    }
    fn query_selector(&mut self, _: &str) -> i32 {
        -1
    }
    fn create_element(&mut self, _: &str) -> i32 {
        -1
    }
    fn set_text_content(&mut self, _: i32, _: &str) {}
    fn get_text_content(&mut self, _: i32) -> String {
        String::new()
    }
    fn set_attribute(&mut self, _: i32, _: &str, _: &str) {}
    fn append_child(&mut self, _: i32, _: i32) {}
    fn log(&mut self, msg: &str) {
        log::info!("[console] {msg}");
    }
    fn add_event_listener(&mut self, _: i32, _: &str) {}
}

type HostCell = Rc<RefCell<Box<dyn JsHost>>>;

/// A timer change requested by script; the host's timer queue applies them in order.
/// Timer ids are the ids `setTimeout` / `setInterval` returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerRequest {
    /// Run timer `id` after `delay_ms`, then every `delay_ms` when `repeat`.
    Set {
        id: u32,
        delay_ms: u64,
        repeat: bool,
    },
    Clear {
        id: u32,
    },
}

pub struct JsContext {
    context: Context,
    host: HostCell,
    pub console_messages: Rc<RefCell<Vec<String>>>,
    pub timer_requests: Rc<RefCell<Vec<TimerRequest>>>,
    flags: Rc<ScriptFlags>,
    modules: Rc<RefCell<ModuleMap>>,
    module_jobs: Vec<ModuleJob>,
    /// Jobs that ended before they could start (the top-level module did not parse).
    finished_modules: Vec<(u32, ModuleOutcome)>,
    next_module_job: u32,
}

pub struct JsRuntime;

impl Default for JsRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl JsRuntime {
    pub fn new() -> Self {
        Self
    }

    pub fn is_enabled(&self) -> bool {
        true
    }

    pub fn create_context(&self) -> Result<JsContext, JsEngineError> {
        let host: HostCell = Rc::new(RefCell::new(Box::new(NullHost)));
        let console_messages = Rc::new(RefCell::new(Vec::new()));
        let timer_requests = Rc::new(RefCell::new(Vec::new()));
        let flags = Rc::new(ScriptFlags::default());
        let modules = Rc::new(RefCell::new(ModuleMap::default()));
        let loader = Rc::new(AxiomModuleLoader {
            map: modules.clone(),
            host: host.clone(),
        });
        let mut context = ContextBuilder::new()
            .host_hooks(&REJECTION_HOOKS)
            .module_loader(loader)
            .build()
            .map_err(|e| JsEngineError::Runtime(e.to_string()))?;
        context.realm().host_defined_mut().insert(EvalGate {
            policy: RefCell::new(None),
            internal: Cell::new(0),
        });
        register_natives(
            &mut context,
            host.clone(),
            console_messages.clone(),
            timer_requests.clone(),
            flags.clone(),
        )?;
        inject_prelude(&mut context)?;
        Ok(JsContext {
            context,
            host,
            console_messages,
            timer_requests,
            flags,
            modules,
            module_jobs: Vec::new(),
            finished_modules: Vec::new(),
            next_module_job: 0,
        })
    }
}

impl JsContext {
    pub fn set_host(&mut self, host: Box<dyn JsHost>) {
        *self.host.borrow_mut() = host;
    }

    /// Run a script, then perform a microtask checkpoint (promise jobs).
    pub fn eval(&mut self, source: &str) -> Result<JsValueHandle, JsEngineError> {
        self.eval_script(source, None)
    }

    /// [`Self::eval`] for a classic script fetched from `url`, which names its frames in
    /// `error.stack`.
    pub fn eval_script(
        &mut self,
        source: &str,
        url: Option<&str>,
    ) -> Result<JsValueHandle, JsEngineError> {
        let source = Source::from_bytes(source);
        let source = match url {
            Some(url) => source.with_path(std::path::Path::new(url)),
            None => source,
        };
        let result = match self.context.eval(source) {
            Ok(v) => Ok(JsValueHandle {
                display: js_to_string(&v, &mut self.context),
            }),
            Err(e) => {
                if log::log_enabled!(target: "axiom_js::stack", log::Level::Debug) {
                    let error = e.to_opaque(&mut self.context);
                    if let Some(stack) = error
                        .as_object()
                        .and_then(|o| o.get(js_string!("stack"), &mut self.context).ok())
                        .and_then(|s| s.as_string().map(JsString::to_std_string_escaped))
                    {
                        log::debug!(target: "axiom_js::stack", "{stack}");
                    }
                }
                Err(JsEngineError::Runtime(e.to_string()))
            }
        };
        microtask_checkpoint(&mut self.context);
        result
    }

    /// Call a prelude function by name, then perform a microtask checkpoint.
    fn call_global(&mut self, name: &str, args: &[JsValue]) -> Result<(), JsEngineError> {
        let ctx = &mut self.context;
        let f = ctx
            .global_object()
            .get(JsString::from(name), ctx)
            .map_err(|e| JsEngineError::Runtime(e.to_string()))?;
        let result = match f.as_callable() {
            Some(f) => f
                .call(&JsValue::undefined(), args, ctx)
                .map(|_| ())
                .map_err(|e| JsEngineError::Runtime(e.to_string())),
            None => Err(JsEngineError::Runtime(format!("{name} is not callable"))),
        };
        microtask_checkpoint(&mut self.context);
        result
    }

    /// The streaming request body of fetch `id` has room again: script resumes pumping.
    pub fn fetch_upload_drained(&mut self, id: u64) -> Result<(), JsEngineError> {
        self.call_global("__axiom_fetchOnUploadDrain", &[JsValue::from(id as f64)])
    }

    /// Resolve the pending `fetch()` promise `id` with a response.
    pub fn fetch_response(
        &mut self,
        id: u64,
        head: &FetchResponseHead,
    ) -> Result<(), JsEngineError> {
        let ctx = &mut self.context;
        let pairs: Vec<JsValue> = head
            .headers
            .iter()
            .map(|(n, v)| {
                JsArray::from_iter(
                    [
                        JsValue::from(JsString::from(n.as_str())),
                        JsValue::from(JsString::from(v.as_str())),
                    ],
                    ctx,
                )
                .into()
            })
            .collect();
        let headers: JsValue = JsArray::from_iter(pairs, ctx).into();
        let args = [
            JsValue::from(id as f64),
            JsValue::from(JsString::from(head.response_type)),
            JsValue::from(head.status),
            JsValue::from(JsString::from(head.status_text.as_str())),
            JsValue::from(JsString::from(head.url.as_str())),
            headers,
            JsValue::from(head.redirected),
        ];
        self.call_global("__axiom_fetchOnResponse", &args)
    }

    /// Append body bytes to the response of fetch `id`.
    pub fn fetch_chunk(&mut self, id: u64, bytes: Vec<u8>) -> Result<(), JsEngineError> {
        let buffer = JsArrayBuffer::from_byte_block(bytes, &mut self.context)
            .map_err(|e| JsEngineError::Runtime(e.to_string()))?;
        self.call_global(
            "__axiom_fetchOnChunk",
            &[JsValue::from(id as f64), buffer.into()],
        )
    }

    /// The body of fetch `id` is complete.
    pub fn fetch_complete(&mut self, id: u64) -> Result<(), JsEngineError> {
        self.call_global("__axiom_fetchOnComplete", &[JsValue::from(id as f64)])
    }

    /// Fetch `id` failed: rejects the promise (or errors the body stream) with a TypeError.
    pub fn fetch_fail(&mut self, id: u64) -> Result<(), JsEngineError> {
        self.call_global("__axiom_fetchOnError", &[JsValue::from(id as f64)])
    }

    /// Dispatch a trusted UI event at the last node of `path` (root → … → target).
    /// Returns `false` when a listener canceled it (the default action must not run).
    pub fn dispatch_event(
        &mut self,
        path: &[usize],
        type_: &str,
        client_x: f32,
        client_y: f32,
    ) -> Result<bool, JsEngineError> {
        let path_js = path
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let src =
            format!("__axiom_dispatchJsEvent([{path_js}], {type_:?}, {client_x}, {client_y});");
        Ok(self.eval(&src)?.display != "false")
    }

    /// A user click at the last node of `path` (root first), with the activation behavior
    /// of form controls and labels (checkbox toggling, submit and reset buttons). Returns
    /// false when a listener canceled it, so the engine skips its own default actions.
    pub fn dispatch_click(
        &mut self,
        path: &[usize],
        client_x: f32,
        client_y: f32,
    ) -> Result<bool, JsEngineError> {
        let path_js = path
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let src = format!("__axiom_userClick([{path_js}], {client_x}, {client_y});");
        Ok(self.eval(&src)?.display != "false")
    }

    /// Trusted, bubbling `input` or `change` event at form control `node`.
    pub fn fire_form_event(&mut self, node: usize, type_: &str) -> Result<(), JsEngineError> {
        self.call_global(
            "__axiom_fireFormEvent",
            &[
                JsValue::from(node as f64),
                JsValue::from(JsString::from(type_)),
            ],
        )
    }

    /// Enter in text control `node`: implicit submission of its form (HTML §4.10.21.2).
    pub fn implicit_submit(&mut self, node: usize) -> Result<(), JsEngineError> {
        self.call_global("__axiom_implicitSubmit", &[JsValue::from(node as f64)])
    }

    /// Run the handler of timer `id`. An uncaught exception is reported to the window
    /// (`error` event) and returned as `Ok(Some(description))`.
    pub fn fire_timeout(&mut self, id: u32) -> Result<Option<String>, JsEngineError> {
        let out = self.eval(&format!("__axiom_fireTimeout({id})"))?;
        Ok(uncaught(out))
    }

    /// The viewport scrolled: `scroll` / `scrollend` fire in the next frame's scroll steps.
    pub fn notify_scroll(&mut self) -> Result<(), JsEngineError> {
        self.eval("__axiom_notifyScroll()").map(|_| ())
    }

    /// A same-document session history traversal landed on an entry with script state
    /// `state`: fire `popstate`, then `hashchange` when only the fragment changed
    /// (`hash_change` holds the old and new URLs).
    pub fn fire_popstate(
        &mut self,
        state: Option<u64>,
        hash_change: Option<(&str, &str)>,
    ) -> Result<(), JsEngineError> {
        let state = state.map_or(JsValue::null(), |s| JsValue::from(s as f64));
        let (old, new) = hash_change.map_or((JsValue::null(), JsValue::null()), |(o, n)| {
            (js_string!(o).into(), js_string!(n).into())
        });
        self.call_global("__axiom_firePopstate", &[state, old, new])
    }

    /// A fragment navigation from `old` to `new` URL: fire `hashchange`.
    pub fn fire_hashchange(&mut self, old: &str, new: &str) -> Result<(), JsEngineError> {
        self.call_global(
            "__axiom_fireHashchange",
            &[js_string!(old).into(), js_string!(new).into()],
        )
    }

    /// Upgrade elements the parser created for defined custom elements (constructor,
    /// observed attributes, `connectedCallback`). A no-op until script defines one.
    pub fn upgrade_parsed_custom_elements(&mut self) -> Result<(), JsEngineError> {
        if !self.flags.custom_elements.get() {
            return Ok(());
        }
        self.eval("__axiom_upgradeCustomElements()").map(|_| ())
    }

    /// Layout changed: when intersection or resize observers exist, the next frame
    /// updates their observations.
    pub fn layout_changed(&self) {
        if self.flags.observers.get() {
            self.flags.frame_requested.set(true);
        }
    }

    /// Whether script has animation frame callbacks waiting for the next frame.
    pub fn animation_frame_requested(&self) -> bool {
        self.flags.frame_requested.get()
    }

    /// Run the animation frame callbacks requested before this frame. Returns the
    /// descriptions of uncaught exceptions (each also reported to the window).
    pub fn run_animation_frames(&mut self) -> Result<Vec<String>, JsEngineError> {
        self.flags.frame_requested.set(false);
        let out = self.eval("__axiom_runAnimationFrames()")?;
        Ok(out
            .display
            .split('\n')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Start the module script `source`, fetched from `url` or (`inline`) written in a
    /// document at `url`. Its imports are requested through [`Self::take_module_fetches`];
    /// progress arrives from [`Self::poll_modules`] under the returned job id.
    pub fn start_module(&mut self, url: &str, source: &str, inline: bool) -> u32 {
        self.next_module_job += 1;
        let id = self.next_module_job;
        match modules::parse_module(&self.modules, url, source, !inline, &mut self.context) {
            Ok(module) => {
                let job = modules::start_job(id, module, &mut self.context);
                self.module_jobs.push(job);
                microtask_checkpoint(&mut self.context);
            }
            Err(e) => {
                let err = e.to_opaque(&mut self.context);
                let _ = self.call_global("__axiom_reportError", &[err]);
                self.finished_modules
                    .push((id, ModuleOutcome::Error(format!("Uncaught {e}"))));
            }
        }
        id
    }

    /// "Report an exception" the host raised on script's behalf (such as an invalid
    /// import map): a `SyntaxError` when `syntax`, else a `TypeError`.
    pub fn report_error(&mut self, syntax: bool, message: &str) {
        let native = if syntax {
            JsNativeError::syntax()
        } else {
            JsNativeError::typ()
        };
        let err = JsValue::from(
            native
                .with_message(message.to_string())
                .to_opaque(&mut self.context),
        );
        let _ = self.call_global("__axiom_reportError", &[err]);
    }

    /// Module URLs script needs that the host has not been asked for yet.
    pub fn take_module_fetches(&mut self) -> Vec<String> {
        self.modules.borrow_mut().take_fetches()
    }

    /// The host fetched module `url`: `Ok((response_url, source))`, or `Err` with a
    /// reason (network error, HTTP status, non-JavaScript MIME type).
    pub fn module_fetched(&mut self, url: &str, result: Result<(String, String), String>) {
        modules::deliver(&self.modules, url, result, &mut self.context);
        microtask_checkpoint(&mut self.context);
    }

    /// Run a job that reported [`ModuleOutcome::Ready`]: link and evaluate its graph.
    pub fn run_module(&mut self, id: u32) {
        let Some(i) = self.module_jobs.iter().position(|j| j.id == id) else {
            return;
        };
        if let Some(outcome) = modules::run_job(&mut self.module_jobs[i], &mut self.context) {
            self.module_jobs.remove(i);
            self.finished_modules.push((id, outcome));
        }
        microtask_checkpoint(&mut self.context);
    }

    /// Progress of top-level module scripts since the last call.
    pub fn poll_modules(&mut self) -> Vec<(u32, ModuleOutcome)> {
        microtask_checkpoint(&mut self.context);
        let mut done = std::mem::take(&mut self.finished_modules);
        let mut i = 0;
        while i < self.module_jobs.len() {
            match modules::poll_job(&mut self.module_jobs[i], &mut self.context) {
                Some(ModuleOutcome::Ready) => {
                    done.push((self.module_jobs[i].id, ModuleOutcome::Ready));
                    i += 1;
                }
                Some(outcome) => {
                    let job = self.module_jobs.remove(i);
                    done.push((job.id, outcome));
                }
                None => i += 1,
            }
        }
        done
    }

    /// Module graphs are loading or waiting to run, or imports are being fetched. (A
    /// module suspended in top-level `await` is not pending work by itself.)
    pub fn has_module_work(&self) -> bool {
        self.module_jobs.iter().any(|j| j.is_pending())
            || !self.finished_modules.is_empty()
            || self.modules.borrow().is_waiting()
    }

    /// The viewport changed: live `MediaQueryList`s re-evaluate and fire `change`.
    pub fn evaluate_media_queries(&mut self) -> Result<(), JsEngineError> {
        self.call_global("__axiom_evaluateMediaQueries", &[])
    }

    /// `document.currentScript` for the script about to run (`None` afterwards).
    pub fn set_current_script(&mut self, node: Option<usize>) {
        let id = node.map_or(-1.0, |n| n as f64);
        let global = self.context.global_object();
        let _ = global.set(
            js_string!("__axiom_currentScriptId"),
            JsValue::from(id),
            false,
            &mut self.context,
        );
    }

    /// Trusted event at the document (`readystatechange`, `DOMContentLoaded`); bubbling
    /// events continue to window listeners.
    pub fn dispatch_document_event(
        &mut self,
        type_: &str,
        bubbles: bool,
    ) -> Result<(), JsEngineError> {
        self.call_global(
            "__axiom_fireDocumentEvent",
            &[JsValue::from(JsString::from(type_)), JsValue::from(bubbles)],
        )
    }

    /// Trusted event at the window (`load`), including its `on<type>` handler.
    pub fn dispatch_window_event(&mut self, type_: &str) -> Result<(), JsEngineError> {
        self.call_global(
            "__axiom_fireWindowEvent",
            &[JsValue::from(JsString::from(type_))],
        )
    }

    /// Deliver a JSON structured-clone payload from a parent/frame proxy on this
    /// realm's event-loop turn. The payload remains JSON across the Rust boundary so
    /// no Boa heap object can be shared between realms.
    pub fn dispatch_frame_message(
        &mut self,
        data_json: &str,
        origin: &str,
    ) -> Result<(), JsEngineError> {
        self.call_global(
            "__axiom_receiveFrameMessage",
            &[
                JsValue::from(JsString::from(data_json)),
                JsValue::from(JsString::from(origin)),
            ],
        )
    }

    /// Non-bubbling trusted event at an element (`load` / `error` of a resource).
    pub fn dispatch_element_event(
        &mut self,
        node: usize,
        type_: &str,
    ) -> Result<(), JsEngineError> {
        self.call_global(
            "__axiom_fireElementEvent",
            &[
                JsValue::from(node as f64),
                JsValue::from(JsString::from(type_)),
            ],
        )
    }

    pub fn take_console(&self) -> Vec<String> {
        self.console_messages.borrow_mut().drain(..).collect()
    }

    /// Decide whether script may compile strings (`eval`, `new Function`, string timers);
    /// `policy` gets the source and returns `false` to throw an `EvalError`.
    pub fn set_eval_policy(&mut self, policy: impl Fn(&str) -> bool + 'static) {
        with_eval_gate(&self.context, |gate| {
            *gate.policy.borrow_mut() = Some(Box::new(policy));
        });
    }

    /// Fire a trusted `securitypolicyviolation` event (bubbling, composed) at element
    /// `node`, or at the document when it is `None` or no longer connected.
    pub fn dispatch_csp_violation(
        &mut self,
        node: Option<usize>,
        violation: &CspViolationInit,
    ) -> Result<(), JsEngineError> {
        let init = JsObject::with_object_proto(self.context.intrinsics());
        let strings = [
            ("documentURI", &violation.document_uri),
            ("blockedURI", &violation.blocked_uri),
            ("violatedDirective", &violation.effective_directive),
            ("effectiveDirective", &violation.effective_directive),
            ("originalPolicy", &violation.original_policy),
            ("sourceFile", &violation.source_file),
            ("sample", &violation.sample),
            ("disposition", &violation.disposition),
        ];
        for (key, value) in strings {
            init.set(
                JsString::from(key),
                JsValue::from(JsString::from(value.as_str())),
                false,
                &mut self.context,
            )
            .map_err(|e| JsEngineError::Runtime(e.to_string()))?;
        }
        let id = node.map_or(-1.0, |n| n as f64);
        self.call_global(
            "__axiom_fireCspViolation",
            &[JsValue::from(id), JsValue::from(init)],
        )
    }
}

/// The fields of a `SecurityPolicyViolationEvent`.
#[derive(Debug, Clone, Default)]
pub struct CspViolationInit {
    pub document_uri: String,
    pub blocked_uri: String,
    pub effective_directive: String,
    pub original_policy: String,
    pub source_file: String,
    pub sample: String,
    /// `"enforce"` or `"report"`.
    pub disposition: String,
}

/// `[href, origin, protocol, username, password, host, hostname, port, pathname, search,
/// hash]`, the `URL` getters in order.
fn url_components(u: &url::Url, ctx: &mut Context) -> JsValue {
    use url::quirks;
    let parts = [
        quirks::href(u).to_string(),
        quirks::origin(u),
        quirks::protocol(u).to_string(),
        quirks::username(u).to_string(),
        quirks::password(u).to_string(),
        quirks::host(u).to_string(),
        quirks::hostname(u).to_string(),
        quirks::port(u).to_string(),
        quirks::pathname(u).to_string(),
        quirks::search(u).to_string(),
        quirks::hash(u).to_string(),
    ];
    let values = parts.map(|p| JsValue::from(JsString::from(p.as_str())));
    boa_engine::object::builtins::JsArray::from_iter(values, ctx).into()
}

fn js_to_string(v: &JsValue, ctx: &mut Context) -> String {
    v.to_string(ctx)
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_else(|_| "<value>".into())
}

/// Signals script raises for the event loop.
#[derive(Debug, Default)]
struct ScriptFlags {
    /// Animation frame callbacks wait for the next frame.
    frame_requested: Cell<bool>,
    /// A custom element definition exists (parser-created elements need upgrades).
    custom_elements: Cell<bool>,
    /// An intersection or resize observer has observed a target (layout changes need a
    /// frame to update observations).
    observers: Cell<bool>,
}

/// A prelude callback's result: the uncaught exception's description, if any.
fn uncaught(out: JsValueHandle) -> Option<String> {
    (out.display != "undefined" && !out.display.is_empty()).then_some(out.display)
}

fn register_natives(
    context: &mut Context,
    host: HostCell,
    console_messages: Rc<RefCell<Vec<String>>>,
    timer_requests: Rc<RefCell<Vec<TimerRequest>>>,
    flags: Rc<ScriptFlags>,
) -> Result<(), JsEngineError> {
    // SAFETY: captures are Rc/RefCell host state, not Boa GC-managed values.
    macro_rules! native {
        ($name:expr, $arity:expr, $closure:expr) => {{
            context
                .register_global_callable(js_string!($name), $arity, unsafe {
                    NativeFunction::from_closure($closure)
                })
                .map_err(|e| JsEngineError::Runtime(e.to_string()))?;
        }};
    }

    {
        let h = host.clone();
        native!("__axiom_getElementById", 1, move |_this, args, _ctx| {
            let id = args
                .get_or_undefined(0)
                .as_string()
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            Ok(JsValue::from(h.borrow_mut().get_element_by_id(&id)))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_querySelector", 1, move |_this, args, _ctx| {
            let sel = args
                .get_or_undefined(0)
                .as_string()
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            Ok(JsValue::from(h.borrow_mut().query_selector(&sel)))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_frameInfo", 1, move |_this, args, ctx| {
            let iframe = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let Some(info) = h.borrow_mut().frame_info(iframe) else {
                return Ok(JsValue::null());
            };
            let values = [
                JsValue::from(info.browsing_context_id as f64),
                JsValue::from(info.document_id as f64),
                JsValue::from(info.same_origin),
                info.url
                    .map(|url| JsValue::from(JsString::from(url.as_str())))
                    .unwrap_or_else(JsValue::null),
                info.content_type
                    .map(|content_type| JsValue::from(JsString::from(content_type.as_str())))
                    .unwrap_or_else(JsValue::null),
            ];
            Ok(JsArray::from_iter(values, ctx).into())
        });
    }
    {
        let h = host.clone();
        native!(
            "__axiom_frameGetElementById",
            2,
            move |_this, args, _ctx| {
                let iframe = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
                let id = arg_string(args, 1);
                Ok(JsValue::from(
                    h.borrow_mut().frame_get_element_by_id(iframe, &id),
                ))
            }
        );
    }
    {
        let h = host.clone();
        native!("__axiom_frameQuerySelector", 2, move |_this, args, _ctx| {
            let iframe = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let selector = arg_string(args, 1);
            Ok(JsValue::from(
                h.borrow_mut().frame_query_selector(iframe, &selector),
            ))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_frameDocumentNode", 2, move |_this, args, _ctx| {
            let iframe = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let which = arg_string(args, 1);
            Ok(JsValue::from(
                h.borrow_mut().frame_document_node(iframe, &which),
            ))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_frameNodeText", 2, move |_this, args, _ctx| {
            let iframe = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let node = args.get_or_undefined(1).as_number().unwrap_or(-1.0) as i32;
            let text = h.borrow_mut().frame_node_text(iframe, node);
            Ok(JsValue::from(JsString::from(text.as_str())))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_frameNodeTagName", 2, move |_this, args, _ctx| {
            let iframe = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let node = args.get_or_undefined(1).as_number().unwrap_or(-1.0) as i32;
            let tag = h.borrow_mut().frame_node_tag_name(iframe, node);
            Ok(JsValue::from(JsString::from(tag.as_str())))
        });
    }
    {
        let h = host.clone();
        native!(
            "__axiom_frameNodeNamespaceUri",
            2,
            move |_this, args, _ctx| {
                let iframe = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
                let node = args.get_or_undefined(1).as_number().unwrap_or(-1.0) as i32;
                Ok(h.borrow_mut()
                    .frame_node_namespace_uri(iframe, node)
                    .map(|uri| JsValue::from(JsString::from(uri.as_str())))
                    .unwrap_or_else(JsValue::null))
            }
        );
    }
    {
        let h = host.clone();
        native!("__axiom_frameCreateElement", 2, move |_this, args, _ctx| {
            let iframe = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let tag = arg_string(args, 1);
            Ok(JsValue::from(
                h.borrow_mut().frame_create_element(iframe, &tag),
            ))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_framePostMessage", 3, move |_this, args, _ctx| {
            let iframe = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let data = arg_string(args, 1);
            let target = arg_string(args, 2);
            if let Err(error) = h.borrow_mut().frame_post_message(iframe, &data, &target) {
                return Err(JsNativeError::typ().with_message(error).into());
            }
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_createElement", 1, move |_this, args, _ctx| {
            let tag = args
                .get_or_undefined(0)
                .as_string()
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_else(|| "div".into());
            Ok(JsValue::from(h.borrow_mut().create_element(&tag)))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_setText", 2, move |_this, args, _ctx| {
            let node = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let text = args
                .get_or_undefined(1)
                .as_string()
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            h.borrow_mut().set_text_content(node, &text);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_getText", 1, move |_this, args, _ctx| {
            let node = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let text = h.borrow_mut().get_text_content(node);
            Ok(JsValue::from(js_string!(text.as_str())))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_setAttr", 3, move |_this, args, _ctx| {
            let node = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let name = args
                .get_or_undefined(1)
                .as_string()
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let value = args
                .get_or_undefined(2)
                .as_string()
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            h.borrow_mut().set_attribute(node, &name, &value);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        let msgs = console_messages.clone();
        native!("__axiom_appendChild", 2, move |_this, args, ctx| {
            let parent = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let child = args.get_or_undefined(1).as_number().unwrap_or(-1.0) as i32;
            let scripts = {
                let mut host = h.borrow_mut();
                host.append_child(parent, child);
                host.take_inserted_scripts()
            };
            // The host borrow is released: the inserted scripts may call back into it.
            for (node, source) in scripts {
                run_inserted_script(ctx, node, &source, &msgs, &h);
            }
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_removeChild", 2, move |_this, args, _ctx| {
            let parent = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let child = args.get_or_undefined(1).as_number().unwrap_or(-1.0) as i32;
            Ok(JsValue::from(h.borrow_mut().remove_child(parent, child)))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_getAttr", 2, move |_this, args, _ctx| {
            let node = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let name = arg_string(args, 1);
            match h.borrow_mut().get_attribute(node, &name) {
                Some(v) => Ok(JsValue::from(JsString::from(v.as_str()))),
                None => Ok(JsValue::null()),
            }
        });
    }
    {
        let h = host.clone();
        native!("__axiom_controlValue", 1, move |_this, args, _ctx| {
            let v = h.borrow_mut().control_value(arg_node(args, 0));
            Ok(JsValue::from(JsString::from(v.as_str())))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_setControlValue", 2, move |_this, args, _ctx| {
            let value = arg_string(args, 1);
            h.borrow_mut().set_control_value(arg_node(args, 0), &value);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_controlChecked", 1, move |_this, args, _ctx| {
            Ok(JsValue::from(
                h.borrow_mut().control_checked(arg_node(args, 0)),
            ))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_setControlChecked", 2, move |_this, args, _ctx| {
            let checked = args.get_or_undefined(1).to_boolean();
            h.borrow_mut()
                .set_control_checked(arg_node(args, 0), checked);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_selectedIndex", 1, move |_this, args, _ctx| {
            Ok(JsValue::from(
                h.borrow_mut().selected_index(arg_node(args, 0)),
            ))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_formOwner", 1, move |_this, args, _ctx| {
            Ok(JsValue::from(h.borrow_mut().form_owner(arg_node(args, 0))))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_formControls", 1, move |_this, args, ctx| {
            let ids = h.borrow_mut().form_controls(arg_node(args, 0));
            Ok(node_id_array(ids, ctx))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_invalidControls", 1, move |_this, args, ctx| {
            let ids = h.borrow_mut().invalid_controls(arg_node(args, 0));
            Ok(node_id_array(ids, ctx))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_controlValidity", 1, move |_this, args, ctx| {
            let (will, flag, message) = h.borrow_mut().control_validity(arg_node(args, 0));
            let items = [
                JsValue::from(will),
                JsValue::from(JsString::from(flag.as_str())),
                JsValue::from(JsString::from(message.as_str())),
            ];
            Ok(JsArray::from_iter(items, ctx).into())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_setCustomValidity", 2, move |_this, args, _ctx| {
            let message = arg_string(args, 1);
            h.borrow_mut()
                .set_custom_validity(arg_node(args, 0), &message);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_formEntries", 2, move |_this, args, ctx| {
            let entries = h
                .borrow_mut()
                .form_entries(arg_node(args, 0), arg_node(args, 1));
            let rows: Vec<JsValue> = entries
                .into_iter()
                .map(|(name, value, file)| {
                    let row = [
                        JsValue::from(JsString::from(name.as_str())),
                        JsValue::from(JsString::from(value.as_str())),
                        JsValue::from(file),
                    ];
                    JsArray::from_iter(row, ctx).into()
                })
                .collect();
            Ok(JsArray::from_iter(rows, ctx).into())
        });
    }
    {
        let h = host.clone();
        // Returns null, or why the form will not navigate.
        native!("__axiom_submitForm", 2, move |_this, args, _ctx| {
            let result = h
                .borrow_mut()
                .submit_form(arg_node(args, 0), arg_node(args, 1));
            Ok(error_name(result))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_resetForm", 1, move |_this, args, _ctx| {
            h.borrow_mut().reset_form(arg_node(args, 0));
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_removeAttr", 2, move |_this, args, _ctx| {
            let node = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let name = arg_string(args, 1);
            h.borrow_mut().remove_attribute(node, &name);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_tagName", 1, move |_this, args, _ctx| {
            let node = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let tag = h.borrow_mut().tag_name(node);
            Ok(JsValue::from(JsString::from(tag.as_str())))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_documentNode", 1, move |_this, args, _ctx| {
            let which = arg_string(args, 0);
            Ok(JsValue::from(h.borrow_mut().document_node(&which)))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_documentRoot", 0, move |_this, _args, _ctx| {
            Ok(JsValue::from(h.borrow_mut().document_root()))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_nodeType", 1, move |_this, args, _ctx| {
            let node = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            Ok(JsValue::from(h.borrow_mut().node_type(node)))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_parentNode", 1, move |_this, args, _ctx| {
            let node = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            Ok(JsValue::from(h.borrow_mut().parent_node(node)))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_childNodes", 1, move |_this, args, ctx| {
            let node = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let ids = h.borrow_mut().child_nodes(node);
            Ok(node_id_array(ids, ctx))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_elementsByTagName", 2, move |_this, args, ctx| {
            let root = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let name = arg_string(args, 1);
            let ids = h.borrow_mut().elements_by_tag_name(root, &name);
            Ok(node_id_array(ids, ctx))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_querySelectorAll", 2, move |_this, args, ctx| {
            let root = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let selectors = arg_string(args, 1);
            let ids = h.borrow_mut().query_selector_all(root, &selectors);
            Ok(node_id_array(ids, ctx))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_createTextNode", 1, move |_this, args, _ctx| {
            let data = arg_string(args, 0);
            Ok(JsValue::from(h.borrow_mut().create_text_node(&data)))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_createCDATASection", 1, move |_this, args, _ctx| {
            let data = arg_string(args, 0);
            Ok(JsValue::from(h.borrow_mut().create_cdata_section(&data)))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_createComment", 1, move |_this, args, _ctx| {
            let data = arg_string(args, 0);
            Ok(JsValue::from(h.borrow_mut().create_comment(&data)))
        });
    }
    {
        let h = host.clone();
        native!(
            "__axiom_createProcessingInstruction",
            2,
            move |_this, args, _ctx| {
                let (target, data) = (arg_string(args, 0), arg_string(args, 1));
                let id = h.borrow_mut().create_processing_instruction(&target, &data);
                Ok(JsValue::from(id))
            }
        );
    }
    {
        let h = host.clone();
        native!(
            "__axiom_createDocumentFragment",
            0,
            move |_this, _args, _ctx| {
                Ok(JsValue::from(h.borrow_mut().create_document_fragment()))
            }
        );
    }
    native!("__axiom_debugStack", 1, |_this, args, ctx| {
        if log::log_enabled!(target: "axiom_js::stack", log::Level::Debug) {
            let stack = args.get_or_undefined(0).to_string(ctx)?;
            log::debug!(target: "axiom_js::stack", "{}", stack.to_std_string_escaped());
        }
        Ok(JsValue::undefined())
    });
    {
        let h = host.clone();
        native!("__axiom_createDocument", 0, move |_this, _args, _ctx| {
            Ok(JsValue::from(h.borrow_mut().create_document()))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_parseHtmlDocument", 1, move |_this, args, _ctx| {
            let markup = arg_string(args, 0);
            Ok(JsValue::from(h.borrow_mut().parse_html_document(&markup)))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_parseXmlDocument", 1, move |_this, args, _ctx| {
            let markup = arg_string(args, 0);
            Ok(JsValue::from(h.borrow_mut().parse_xml_document(&markup)))
        });
    }
    {
        let h = host.clone();
        // Returns the new node id, or the DOMException name.
        native!("__axiom_createElementNS", 2, move |_this, args, _ctx| {
            let namespace = arg_opt_string(args, 0);
            let name = arg_string(args, 1);
            let created = h
                .borrow_mut()
                .create_element_ns(namespace.as_deref(), &name);
            Ok(match created {
                Ok(id) => JsValue::from(id),
                Err(e) => JsValue::from(JsString::from(e.as_str())),
            })
        });
    }
    {
        let h = host.clone();
        let msgs = console_messages.clone();
        // Returns null, or the DOMException name.
        native!("__axiom_insertBefore", 3, move |_this, args, ctx| {
            let (parent, node, child) = (arg_node(args, 0), arg_node(args, 1), arg_node(args, 2));
            let result = h.borrow_mut().insert_before(parent, node, child);
            run_inserted_scripts(ctx, &h, &msgs);
            Ok(error_name(result))
        });
    }
    {
        let h = host.clone();
        let msgs = console_messages.clone();
        // Returns null, or the DOMException name.
        native!("__axiom_replaceChild", 3, move |_this, args, ctx| {
            let (parent, node, child) = (arg_node(args, 0), arg_node(args, 1), arg_node(args, 2));
            let result = h.borrow_mut().replace_child(parent, node, child);
            run_inserted_scripts(ctx, &h, &msgs);
            Ok(error_name(result))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_innerHTML", 1, move |_this, args, _ctx| {
            let html = h.borrow_mut().inner_html(arg_node(args, 0));
            Ok(JsValue::from(js_string!(html)))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_outerHTML", 1, move |_this, args, _ctx| {
            let html = h.borrow_mut().outer_html(arg_node(args, 0));
            Ok(JsValue::from(js_string!(html)))
        });
    }
    {
        let h = host.clone();
        // Returns null, or the DOMException name.
        native!("__axiom_setInnerHTML", 2, move |_this, args, _ctx| {
            let markup = arg_string(args, 1);
            Ok(error_name(
                h.borrow_mut().set_inner_html(arg_node(args, 0), &markup),
            ))
        });
    }
    {
        let h = host.clone();
        // Returns null, or the DOMException name.
        native!("__axiom_setOuterHTML", 2, move |_this, args, _ctx| {
            let markup = arg_string(args, 1);
            Ok(error_name(
                h.borrow_mut().set_outer_html(arg_node(args, 0), &markup),
            ))
        });
    }
    {
        let h = host.clone();
        // Returns null, or the DOMException name.
        native!("__axiom_insertAdjacentHTML", 3, move |_this, args, _ctx| {
            let (position, markup) = (arg_string(args, 1), arg_string(args, 2));
            let result = h
                .borrow_mut()
                .insert_adjacent_html(arg_node(args, 0), &position, &markup);
            Ok(error_name(result))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_cloneNode", 2, move |_this, args, _ctx| {
            let deep = args.get_or_undefined(1).to_boolean();
            Ok(JsValue::from(
                h.borrow_mut().clone_node(arg_node(args, 0), deep),
            ))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_namespaceURI", 1, move |_this, args, _ctx| {
            Ok(opt_js_string(
                h.borrow_mut().namespace_uri(arg_node(args, 0)),
            ))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_prefix", 1, move |_this, args, _ctx| {
            Ok(opt_js_string(h.borrow_mut().prefix(arg_node(args, 0))))
        });
    }
    {
        let h = host.clone();
        // [[namespace, prefix, localName, value], ...]
        native!("__axiom_attributes", 1, move |_this, args, ctx| {
            let attrs = h.borrow_mut().attributes(arg_node(args, 0));
            let rows: Vec<JsValue> = attrs
                .into_iter()
                .map(|a| {
                    let row = [
                        opt_js_string(a.namespace),
                        opt_js_string(a.prefix),
                        JsValue::from(JsString::from(a.local_name.as_str())),
                        JsValue::from(JsString::from(a.value.as_str())),
                    ];
                    JsArray::from_iter(row, ctx).into()
                })
                .collect();
            Ok(JsArray::from_iter(rows, ctx).into())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_getAttrNS", 3, move |_this, args, _ctx| {
            let namespace = arg_opt_string(args, 1);
            let local = arg_string(args, 2);
            let value =
                h.borrow_mut()
                    .get_attribute_ns(arg_node(args, 0), namespace.as_deref(), &local);
            Ok(opt_js_string(value))
        });
    }
    {
        let h = host.clone();
        // Returns null, or the DOMException name.
        native!("__axiom_setAttrNS", 4, move |_this, args, _ctx| {
            let namespace = arg_opt_string(args, 1);
            let (name, value) = (arg_string(args, 2), arg_string(args, 3));
            let result = h.borrow_mut().set_attribute_ns(
                arg_node(args, 0),
                namespace.as_deref(),
                &name,
                &value,
            );
            Ok(error_name(result))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_removeAttrNS", 3, move |_this, args, _ctx| {
            let namespace = arg_opt_string(args, 1);
            let local = arg_string(args, 2);
            h.borrow_mut()
                .remove_attribute_ns(arg_node(args, 0), namespace.as_deref(), &local);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_replaceAttrNS", 5, move |_this, args, _ctx| {
            let attr = DomAttr {
                namespace: arg_opt_string(args, 1),
                prefix: arg_opt_string(args, 2),
                local_name: arg_string(args, 3),
                value: arg_string(args, 4),
            };
            h.borrow_mut().replace_attribute(arg_node(args, 0), &attr);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        // [namespace, prefix, localName], or the DOMException name.
        native!("__axiom_validateAttrName", 2, move |_this, args, ctx| {
            let namespace = arg_opt_string(args, 0);
            let name = arg_string(args, 1);
            let result = h
                .borrow_mut()
                .validate_attribute_name(namespace.as_deref(), &name);
            Ok(match result {
                Ok((namespace, prefix, local)) => {
                    let row = [
                        opt_js_string(namespace),
                        opt_js_string(prefix),
                        JsValue::from(JsString::from(local.as_str())),
                    ];
                    JsArray::from_iter(row, ctx).into()
                }
                Err(e) => JsValue::from(JsString::from(e.as_str())),
            })
        });
    }
    {
        let h = host.clone();
        // true / false, or null when the selectors do not parse.
        native!("__axiom_matches", 2, move |_this, args, _ctx| {
            let selectors = arg_string(args, 1);
            Ok(
                match h
                    .borrow_mut()
                    .matches_selectors(arg_node(args, 0), &selectors)
                {
                    Some(m) => JsValue::from(m),
                    None => JsValue::null(),
                },
            )
        });
    }
    {
        let h = host.clone();
        // Element id, -1 when nothing matches, or null when the selectors do not parse.
        native!("__axiom_closest", 2, move |_this, args, _ctx| {
            let selectors = arg_string(args, 1);
            Ok(
                match h.borrow_mut().closest(arg_node(args, 0), &selectors) {
                    Some(id) => JsValue::from(id),
                    None => JsValue::null(),
                },
            )
        });
    }
    {
        let h = host.clone();
        native!("__axiom_templateContent", 1, move |_this, args, _ctx| {
            Ok(match h.borrow_mut().template_content(arg_node(args, 0)) {
                Some(id) => JsValue::from(id),
                None => JsValue::null(),
            })
        });
    }
    {
        let h = host.clone();
        native!("__axiom_attachShadow", 2, move |_this, args, _ctx| {
            let flags = args.get_or_undefined(1).as_number().unwrap_or(0.0) as u32;
            Ok(JsValue::from(
                h.borrow_mut().attach_shadow(arg_node(args, 0), flags),
            ))
        });
    }
    {
        let h = host.clone();
        // [root, flags] or null.
        native!("__axiom_shadowRootInfo", 1, move |_this, args, ctx| {
            Ok(match h.borrow_mut().shadow_root_info(arg_node(args, 0)) {
                Some((root, flags)) => {
                    let pair = [JsValue::from(root), JsValue::from(flags)];
                    JsArray::from_iter(pair, ctx).into()
                }
                None => JsValue::null(),
            })
        });
    }
    {
        let h = host.clone();
        native!("__axiom_shadowHost", 1, move |_this, args, _ctx| {
            Ok(JsValue::from(h.borrow_mut().shadow_host(arg_node(args, 0))))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_shadowRoots", 0, move |_this, _args, ctx| {
            let ids = h.borrow_mut().connected_shadow_roots();
            Ok(node_id_array(ids, ctx))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_hasShadowTrees", 0, move |_this, _args, _ctx| {
            Ok(JsValue::from(h.borrow_mut().has_shadow_trees()))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_assignedNodes", 2, move |_this, args, ctx| {
            let flatten = args.get_or_undefined(1).to_boolean();
            let ids = h.borrow_mut().assigned_nodes(arg_node(args, 0), flatten);
            Ok(node_id_array(ids, ctx))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_assignedSlot", 1, move |_this, args, _ctx| {
            Ok(JsValue::from(
                h.borrow_mut().assigned_slot(arg_node(args, 0)),
            ))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_assignSlot", 2, move |_this, args, ctx| {
            let mut ids = Vec::new();
            if let Some(obj) = args.get_or_undefined(1).as_object() {
                let list = JsArray::from_object(obj.clone())?;
                for i in 0..list.length(ctx)? {
                    ids.push(list.get(i, ctx)?.to_i32(ctx)?);
                }
            }
            h.borrow_mut().assign_slot(arg_node(args, 0), ids);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!(
            "__axiom_setCustomElementDefined",
            1,
            move |_this, args, _ctx| {
                h.borrow_mut().set_custom_element_defined(arg_node(args, 0));
                Ok(JsValue::undefined())
            }
        );
    }
    {
        let h = host.clone();
        native!("__axiom_doctypeIds", 1, move |_this, args, ctx| {
            let (public_id, system_id) = h.borrow_mut().doctype_ids(arg_node(args, 0));
            let pair = [
                JsValue::from(JsString::from(public_id.as_str())),
                JsValue::from(JsString::from(system_id.as_str())),
            ];
            Ok(JsArray::from_iter(pair, ctx).into())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_createDocumentType", 3, move |_this, args, _ctx| {
            let (name, public_id, system_id) = (
                arg_string(args, 0),
                arg_string(args, 1),
                arg_string(args, 2),
            );
            let id = h.borrow_mut().create_doctype(&name, &public_id, &system_id);
            Ok(JsValue::from(id))
        });
    }
    {
        let h = host.clone();
        // -1 when the host cannot version its tree.
        native!("__axiom_domVersion", 0, move |_this, _args, _ctx| {
            let version = h.borrow_mut().dom_version();
            Ok(JsValue::from(version.map_or(-1.0, |v| v as f64)))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_readyState", 0, move |_this, _args, _ctx| {
            let state = h.borrow_mut().ready_state();
            Ok(JsValue::from(JsString::from(state.as_str())))
        });
    }
    {
        let msgs = console_messages.clone();
        let h = host.clone();
        native!("__axiom_log", 1, move |_this, args, ctx| {
            let mut parts = Vec::new();
            for i in 0..args.len() {
                let v = args.get_or_undefined(i);
                parts.push(
                    v.to_string(ctx)
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default(),
                );
            }
            let msg = parts.join(" ");
            msgs.borrow_mut().push(msg.clone());
            h.borrow_mut().log(&msg);
            Ok(JsValue::undefined())
        });
    }
    {
        // Milliseconds since the realm was created, coarsened to 0.1 ms against timing
        // side channels.
        let origin = std::time::Instant::now();
        native!("__axiom_now", 0, move |_this, _args, _ctx| {
            let ms = origin.elapsed().as_secs_f64() * 1000.0;
            Ok(JsValue::from((ms * 10.0).floor() / 10.0))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_registerListener", 2, move |_this, args, _ctx| {
            let node = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let type_ = args
                .get_or_undefined(1)
                .as_string()
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            h.borrow_mut().add_event_listener(node, &type_);
            Ok(JsValue::undefined())
        });
    }
    {
        let requests = timer_requests.clone();
        native!("__axiom_scheduleTimer", 3, move |_this, args, _ctx| {
            let id = args.get_or_undefined(0).as_number().unwrap_or(0.0) as u32;
            let delay_ms = args.get_or_undefined(1).as_number().unwrap_or(0.0).max(0.0) as u64;
            let repeat = args.get_or_undefined(2).to_boolean();
            requests.borrow_mut().push(TimerRequest::Set {
                id,
                delay_ms,
                repeat,
            });
            Ok(JsValue::undefined())
        });
    }
    {
        let requests = timer_requests;
        native!("__axiom_clearTimer", 1, move |_this, args, _ctx| {
            let id = args.get_or_undefined(0).as_number().unwrap_or(0.0) as u32;
            requests.borrow_mut().push(TimerRequest::Clear { id });
            Ok(JsValue::undefined())
        });
    }
    {
        let requested = flags.clone();
        native!("__axiom_requestFrame", 0, move |_this, _args, _ctx| {
            requested.frame_requested.set(true);
            Ok(JsValue::undefined())
        });
    }
    {
        let defined = flags.clone();
        native!(
            "__axiom_customElementsDefined",
            0,
            move |_this, _args, _ctx| {
                defined.custom_elements.set(true);
                Ok(JsValue::undefined())
            }
        );
    }
    {
        let active = flags;
        native!("__axiom_observersActive", 0, move |_this, _args, _ctx| {
            active.observers.set(true);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_userAgent", 0, move |_this, _args, _ctx| {
            let ua = h.borrow_mut().user_agent();
            Ok(JsValue::from(js_string!(ua.as_str())))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_languages", 0, move |_this, _args, ctx| {
            let langs = h.borrow_mut().languages();
            let values = langs.iter().map(|l| JsValue::from(js_string!(l.as_str())));
            Ok(JsArray::from_iter(values, ctx).into())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_cookieEnabled", 0, move |_this, _args, _ctx| {
            Ok(JsValue::from(h.borrow_mut().cookie_enabled()))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_matchMedia", 1, move |_this, args, ctx| {
            let query = args
                .get_or_undefined(0)
                .to_string(ctx)?
                .to_std_string_escaped();
            Ok(JsValue::from(h.borrow_mut().match_media(&query)))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_viewportSize", 0, move |_this, _args, ctx| {
            let (w, h) = h.borrow_mut().viewport_size();
            let values = [JsValue::from(w as f64), JsValue::from(h as f64)];
            Ok(JsArray::from_iter(values, ctx).into())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_focusedElement", 0, move |_this, _args, _ctx| {
            Ok(JsValue::from(h.borrow_mut().focused_element()))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_setFocus", 1, move |_this, args, ctx| {
            let node = args.get_or_undefined(0).to_i32(ctx)?;
            h.borrow_mut().set_focus(node);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_cssSupports", 1, move |_this, args, ctx| {
            let condition = args
                .get_or_undefined(0)
                .to_string(ctx)?
                .to_std_string_escaped();
            Ok(JsValue::from(h.borrow_mut().css_supports(&condition)))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_historyLength", 0, move |_this, _args, _ctx| {
            Ok(JsValue::from(h.borrow_mut().history_length()))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_historyState", 0, move |_this, _args, _ctx| {
            let state = h.borrow_mut().history_state();
            Ok(state.map_or(JsValue::null(), |s| JsValue::from(s as f64)))
        });
    }
    {
        let h = host.clone();
        // [newUrl] on success, [null, DOMException name] on failure.
        native!("__axiom_historyPush", 3, move |_this, args, ctx| {
            let url = match args.get_or_undefined(0) {
                v if v.is_null_or_undefined() => None,
                v => Some(v.to_string(ctx)?.to_std_string_escaped()),
            };
            let state = args.get_or_undefined(1).to_number(ctx)? as u64;
            let replace = args.get_or_undefined(2).to_boolean();
            let result = h.borrow_mut().history_push(url.as_deref(), state, replace);
            let values = match result {
                Ok(url) => vec![js_string!(url.as_str()).into()],
                Err(name) => vec![JsValue::null(), js_string!(name).into()],
            };
            Ok(JsArray::from_iter(values, ctx).into())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_historyTraverse", 1, move |_this, args, ctx| {
            let delta = args.get_or_undefined(0).to_i32(ctx)?;
            h.borrow_mut().history_traverse(delta);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_navigate", 2, move |_this, args, ctx| {
            let url = args
                .get_or_undefined(0)
                .to_string(ctx)?
                .to_std_string_escaped();
            let replace = args.get_or_undefined(1).to_boolean();
            Ok(JsValue::from(h.borrow_mut().navigate(&url, replace)))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_reload", 0, move |_this, _args, _ctx| {
            h.borrow_mut().reload();
            Ok(JsValue::undefined())
        });
    }
    // OS randomness for crypto.getRandomValues / randomUUID (at most 64 KiB per call).
    native!("__axiom_randomBytes", 1, move |_this, args, ctx| {
        let len = (args.get_or_undefined(0).to_u32(ctx)? as usize).min(65536);
        let mut bytes = vec![0u8; len];
        getrandom::fill(&mut bytes).map_err(|e| {
            JsNativeError::error().with_message(format!("no OS randomness available: {e}"))
        })?;
        let values = bytes.into_iter().map(JsValue::from);
        Ok(JsArray::from_iter(values, ctx).into())
    });
    {
        let h = host.clone();
        native!("__axiom_computedValue", 3, move |_this, args, ctx| {
            let node = args.get_or_undefined(0).to_i32(ctx)?;
            let pseudo = args
                .get_or_undefined(1)
                .to_string(ctx)?
                .to_std_string_escaped();
            let property = args
                .get_or_undefined(2)
                .to_string(ctx)?
                .to_std_string_escaped();
            let value = h.borrow_mut().computed_value(node, &pseudo, &property);
            Ok(value.map_or(JsValue::null(), |v| js_string!(v.as_str()).into()))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_computedNames", 0, move |_this, _args, ctx| {
            let names = h.borrow_mut().computed_property_names();
            let values = names.iter().map(|n| JsValue::from(js_string!(n.as_str())));
            Ok(JsArray::from_iter(values, ctx).into())
        });
    }
    {
        let h = host.clone();
        // [offsetParent, offset×4, client×4, scroll×2, then x, y, w, h per client rect].
        native!("__axiom_elementGeometry", 1, move |_this, args, ctx| {
            let node = args.get_or_undefined(0).to_i32(ctx)?;
            let Some(g) = h.borrow_mut().element_geometry(node) else {
                return Ok(JsValue::null());
            };
            let mut values = vec![JsValue::from(g.offset_parent)];
            values.extend(g.offset.iter().map(|&v| JsValue::from(v)));
            values.extend(g.client.iter().map(|&v| JsValue::from(v)));
            values.extend(g.scroll_size.iter().map(|&v| JsValue::from(v)));
            values.extend(g.client_rects.iter().flatten().map(|&v| JsValue::from(v)));
            Ok(JsArray::from_iter(values, ctx).into())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_scrollPosition", 0, move |_this, _args, ctx| {
            let (x, y) = h.borrow_mut().scroll_position();
            Ok(JsArray::from_iter([JsValue::from(x), JsValue::from(y)], ctx).into())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_scrollTo", 2, move |_this, args, ctx| {
            let x = args.get_or_undefined(0).to_number(ctx)?;
            let y = args.get_or_undefined(1).to_number(ctx)?;
            let finite = |v: f64| if v.is_finite() { v } else { 0.0 };
            h.borrow_mut().scroll_to(finite(x), finite(y));
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_imageNaturalSize", 1, move |_this, args, ctx| {
            let node = args.get_or_undefined(0).to_i32(ctx)?;
            let Some((w, h)) = h.borrow_mut().image_natural_size(node) else {
                return Ok(JsValue::null());
            };
            Ok(JsArray::from_iter([JsValue::from(w), JsValue::from(h)], ctx).into())
        });
    }
    native!("__axiom_platformInfo", 0, move |_this, _args, ctx| {
        let platform = if cfg!(target_os = "windows") {
            "Win32"
        } else if cfg!(target_os = "macos") {
            "MacIntel"
        } else if cfg!(target_arch = "aarch64") {
            "Linux aarch64"
        } else {
            "Linux x86_64"
        };
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let values = [
            JsValue::from(js_string!(platform)),
            JsValue::from(threads as f64),
        ];
        Ok(JsArray::from_iter(values, ctx).into())
    });
    {
        let h = host.clone();
        native!("__axiom_getCookie", 0, move |_this, _args, _ctx| {
            let c = h.borrow_mut().get_document_cookie();
            Ok(JsValue::from(js_string!(c.as_str())))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_setCookie", 1, move |_this, args, _ctx| {
            let v = args
                .get_or_undefined(0)
                .as_string()
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            h.borrow_mut().set_document_cookie(&v);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_storageGet", 2, move |_this, args, _ctx| {
            let session = args.get_or_undefined(0).as_boolean().unwrap_or(false);
            let key = args
                .get_or_undefined(1)
                .as_string()
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            match h.borrow_mut().storage_get(session, &key) {
                Some(v) => Ok(JsValue::from(js_string!(v.as_str()))),
                None => Ok(JsValue::null()),
            }
        });
    }
    {
        let h = host.clone();
        native!("__axiom_storageSet", 3, move |_this, args, _ctx| {
            let session = args.get_or_undefined(0).as_boolean().unwrap_or(false);
            let key = args
                .get_or_undefined(1)
                .as_string()
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let value = args
                .get_or_undefined(2)
                .as_string()
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            match h.borrow_mut().storage_set(session, &key, &value) {
                Ok(()) => Ok(JsValue::undefined()),
                Err(e) => Err(JsNativeError::typ().with_message(e).into()),
            }
        });
    }
    {
        let h = host.clone();
        native!("__axiom_storageRemove", 2, move |_this, args, _ctx| {
            let session = args.get_or_undefined(0).as_boolean().unwrap_or(false);
            let key = args
                .get_or_undefined(1)
                .as_string()
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            h.borrow_mut().storage_remove(session, &key);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_storageClear", 1, move |_this, args, _ctx| {
            let session = args.get_or_undefined(0).as_boolean().unwrap_or(false);
            h.borrow_mut().storage_clear(session);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_storageLength", 1, move |_this, args, _ctx| {
            let session = args.get_or_undefined(0).as_boolean().unwrap_or(false);
            Ok(JsValue::from(h.borrow_mut().storage_length(session) as i32))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_storageKey", 2, move |_this, args, _ctx| {
            let session = args.get_or_undefined(0).as_boolean().unwrap_or(false);
            let index = args.get_or_undefined(1).as_number().unwrap_or(0.0) as usize;
            match h.borrow_mut().storage_key(session, index) {
                Some(k) => Ok(JsValue::from(js_string!(k.as_str()))),
                None => Ok(JsValue::null()),
            }
        });
    }
    {
        // Event handler content attributes: the CSP inline check for the attribute, then
        // compilation that is not subject to the realm's `'unsafe-eval'` gate.
        let h = host.clone();
        native!("__axiom_compileHandler", 4, move |_this, args, ctx| {
            let node = args.get_or_undefined(0).as_number().unwrap_or(-1.0) as i32;
            let params = arg_string(args, 1);
            let body = arg_string(args, 2);
            let scopes = args.get_or_undefined(3).clone();
            if !h.borrow_mut().csp_allows_handler(node, &body) {
                return Ok(JsValue::null());
            }
            with_eval_gate(ctx, |g| g.internal.set(g.internal.get() + 1));
            let result = compile_handler(&params, &body, scopes, ctx);
            with_eval_gate(ctx, |g| g.internal.set(g.internal.get().saturating_sub(1)));
            result
        });
    }
    {
        let h = host.clone();
        native!("__axiom_documentInfo", 1, move |_this, args, _ctx| {
            let info = h.borrow_mut().document_info(&arg_string(args, 0));
            Ok(JsValue::from(JsString::from(info.as_str())))
        });
    }
    {
        let h = host.clone();
        native!("__axiom_resolveUrl", 1, move |_this, args, _ctx| {
            let input = arg_string(args, 0);
            match h.borrow_mut().resolve_url(&input) {
                Some(u) => Ok(JsValue::from(JsString::from(u.as_str()))),
                None => Ok(JsValue::null()),
            }
        });
    }
    // WHATWG URL parsing for the `URL` interface: components as an array (see
    // `url_components`), or null when the input does not parse.
    native!("__axiom_urlParse", 2, move |_this, args, ctx| {
        let input = arg_string(args, 0);
        let base = match args.get_or_undefined(1) {
            v if v.is_undefined() => None,
            v => match url::Url::parse(&js_to_string(v, ctx)) {
                Ok(b) => Some(b),
                Err(_) => return Ok(JsValue::null()),
            },
        };
        match url::Url::options().base_url(base.as_ref()).parse(&input) {
            Ok(u) => Ok(url_components(&u, ctx)),
            Err(_) => Ok(JsValue::null()),
        }
    });
    // Apply one `URL` setter to `href`; returns the new components (unchanged on failure,
    // as the setters specify) or throws for an invalid `href`.
    native!("__axiom_urlSet", 3, move |_this, args, ctx| {
        let href = arg_string(args, 0);
        let setter = arg_string(args, 1);
        let value = arg_string(args, 2);
        let Ok(mut u) = url::Url::parse(&href) else {
            return Err(JsNativeError::typ()
                .with_message(format!("Invalid URL: {href}"))
                .into());
        };
        use url::quirks;
        match setter.as_str() {
            "href" => {
                quirks::set_href(&mut u, &value).map_err(|_| {
                    JsNativeError::typ().with_message(format!("Invalid URL: {value}"))
                })?;
            }
            "protocol" => {
                let _ = quirks::set_protocol(&mut u, &value);
            }
            "username" => {
                let _ = quirks::set_username(&mut u, &value);
            }
            "password" => {
                let _ = quirks::set_password(&mut u, &value);
            }
            "host" => {
                let _ = quirks::set_host(&mut u, &value);
            }
            "hostname" => {
                let _ = quirks::set_hostname(&mut u, &value);
            }
            "port" => {
                let _ = quirks::set_port(&mut u, &value);
            }
            "pathname" => quirks::set_pathname(&mut u, &value),
            "search" => quirks::set_search(&mut u, &value),
            "hash" => quirks::set_hash(&mut u, &value),
            _ => {}
        }
        Ok(url_components(&u, ctx))
    });
    {
        let h = host.clone();
        native!("__axiom_fetchStart", 1, move |_this, args, ctx| {
            let desc = args
                .get_or_undefined(0)
                .as_object()
                .cloned()
                .ok_or_else(|| JsNativeError::typ().with_message("invalid fetch descriptor"))?;
            let init = fetch_init_from_js(&desc, ctx)?;
            match h.borrow_mut().fetch_start(init) {
                Ok(id) => Ok(JsValue::from(id as f64)),
                Err(msg) => Err(JsNativeError::typ().with_message(msg).into()),
            }
        });
    }
    {
        let h = host.clone();
        native!("__axiom_fetchAbort", 1, move |_this, args, _ctx| {
            let id = args.get_or_undefined(0).as_number().unwrap_or(0.0) as u64;
            h.borrow_mut().fetch_abort(id);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_fetchUploadWrite", 2, move |_this, args, _ctx| {
            let id = args.get_or_undefined(0).as_number().unwrap_or(0.0) as u64;
            let bytes = array_buffer_bytes(args.get_or_undefined(1))?;
            match h.borrow_mut().fetch_upload_write(id, bytes) {
                Ok(more) => Ok(JsValue::from(more)),
                Err(msg) => Err(JsNativeError::typ().with_message(msg).into()),
            }
        });
    }
    {
        let h = host.clone();
        native!("__axiom_fetchUploadClose", 1, move |_this, args, _ctx| {
            let id = args.get_or_undefined(0).as_number().unwrap_or(0.0) as u64;
            h.borrow_mut().fetch_upload_close(id);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_fetchUploadError", 2, move |_this, args, _ctx| {
            let id = args.get_or_undefined(0).as_number().unwrap_or(0.0) as u64;
            let message = arg_string(args, 1);
            h.borrow_mut().fetch_upload_error(id, &message);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_fetchConsumed", 2, move |_this, args, _ctx| {
            let id = args.get_or_undefined(0).as_number().unwrap_or(0.0) as u64;
            let bytes = args.get_or_undefined(1).as_number().unwrap_or(0.0).max(0.0) as u64;
            h.borrow_mut().fetch_consumed(id, bytes);
            Ok(JsValue::undefined())
        });
    }
    {
        let h = host.clone();
        native!("__axiom_fetchReferrer", 1, move |_this, args, _ctx| {
            let url = arg_string(args, 0);
            let referrer = h.borrow_mut().fetch_referrer(&url);
            Ok(JsValue::from(JsString::from(referrer.as_str())))
        });
    }
    native!("__axiom_utf8Encode", 1, move |_this, args, ctx| {
        let s = args
            .get_or_undefined(0)
            .to_string(ctx)?
            .to_std_string_escaped();
        Ok(JsArrayBuffer::from_byte_block(s.into_bytes(), ctx)?.into())
    });
    native!("__axiom_utf8Decode", 1, move |_this, args, _ctx| {
        let bytes = array_buffer_bytes(args.get_or_undefined(0))?;
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
        Ok(JsValue::from(JsString::from(
            String::from_utf8_lossy(bytes).as_ref(),
        )))
    });

    Ok(())
}

/// Run an inline script that script just inserted into the document. Errors are reported
/// to the console, never thrown to the inserting call.
fn run_inserted_script(
    ctx: &mut Context,
    node: i32,
    source: &str,
    msgs: &Rc<RefCell<Vec<String>>>,
    host: &HostCell,
) {
    let global = ctx.global_object();
    let key = js_string!("__axiom_currentScriptId");
    let previous = global
        .get(key.clone(), ctx)
        .unwrap_or_else(|_| JsValue::from(-1));
    let _ = global.set(key.clone(), JsValue::from(node), false, ctx);
    if let Err(e) = ctx.eval(Source::from_bytes(source.as_bytes())) {
        let msg = format!("Uncaught {e}");
        msgs.borrow_mut().push(msg.clone());
        host.borrow_mut().log(&msg);
    }
    let _ = global.set(key, previous, false, ctx);
}

/// Compile an event handler body into `function (params) { body }` with `scopes` (an
/// array) pushed as `with` scopes, outermost first.
fn compile_handler(
    params: &str,
    body: &str,
    scopes: JsValue,
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let function = ctx.intrinsics().constructors().function().constructor();
    let undefined = JsValue::undefined();
    // A FunctionBody on its own, so it cannot close the wrapper.
    function.call(
        &undefined,
        &[
            JsValue::from(JsString::from(params)),
            JsValue::from(JsString::from(body)),
        ],
        ctx,
    )?;
    let count = match scopes.as_object() {
        Some(o) => o.get(js_string!("length"), ctx)?.to_u32(ctx)?,
        None => 0,
    };
    let mut src = String::new();
    for i in 0..count {
        src.push_str(&format!("with (arguments[0][{i}]) "));
    }
    // Anonymous: the handler has no binding for its own name (and Boa 0.20 panics on
    // named function expressions inside `with`).
    src.push_str(&format!("return function ({params}) {{\n{body}\n}};"));
    let outer = function.call(&undefined, &[JsValue::from(JsString::from(src))], ctx)?;
    let outer = outer
        .as_callable()
        .ok_or_else(|| JsNativeError::typ().with_message("handler wrapper is not callable"))?;
    outer.call(&undefined, &[scopes], ctx)
}

fn arg_string(args: &[JsValue], i: usize) -> String {
    args.get_or_undefined(i)
        .as_string()
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_default()
}

/// A string argument where `null` / `undefined` mean "none".
fn arg_opt_string(args: &[JsValue], i: usize) -> Option<String> {
    args.get_or_undefined(i)
        .as_string()
        .map(|s| s.to_std_string_escaped())
}

/// A node id argument; `-1` for anything that is not a number.
fn arg_node(args: &[JsValue], i: usize) -> i32 {
    args.get_or_undefined(i).as_number().unwrap_or(-1.0) as i32
}

fn opt_js_string(s: Option<String>) -> JsValue {
    match s {
        Some(s) => JsValue::from(JsString::from(s.as_str())),
        None => JsValue::null(),
    }
}

/// `null` on success, otherwise the `DOMException` name.
fn error_name(result: Result<(), String>) -> JsValue {
    match result {
        Ok(()) => JsValue::null(),
        Err(e) => JsValue::from(JsString::from(e.as_str())),
    }
}

/// Run the inline scripts the last insertion connected. The host must not be borrowed:
/// the scripts may call back into it.
fn run_inserted_scripts(ctx: &mut Context, host: &HostCell, msgs: &Rc<RefCell<Vec<String>>>) {
    let scripts = host.borrow_mut().take_inserted_scripts();
    for (node, source) in scripts {
        run_inserted_script(ctx, node, &source, msgs, host);
    }
}

fn node_id_array(ids: Vec<i32>, ctx: &mut Context) -> JsValue {
    JsArray::from_iter(ids.into_iter().map(JsValue::from), ctx).into()
}

/// Copy of an `ArrayBuffer`'s bytes (a detached or non-buffer value is a TypeError).
fn array_buffer_bytes(v: &JsValue) -> JsResult<Vec<u8>> {
    let obj = v
        .as_object()
        .cloned()
        .ok_or_else(|| JsNativeError::typ().with_message("expected an ArrayBuffer"))?;
    let buffer = JsArrayBuffer::from_object(obj)?;
    let data = buffer
        .data()
        .ok_or_else(|| JsNativeError::typ().with_message("ArrayBuffer is detached"))?;
    Ok(data.to_vec())
}

fn fetch_init_from_js(desc: &JsObject, ctx: &mut Context) -> JsResult<FetchInit> {
    let get_str = |key: &str, ctx: &mut Context| -> JsResult<String> {
        let v = desc.get(JsString::from(key), ctx)?;
        if v.is_undefined() || v.is_null() {
            return Ok(String::new());
        }
        Ok(v.to_string(ctx)?.to_std_string_escaped())
    };
    let mut init = FetchInit {
        method: get_str("method", ctx)?,
        url: get_str("url", ctx)?,
        mode: get_str("mode", ctx)?,
        credentials: get_str("credentials", ctx)?,
        cache: get_str("cache", ctx)?,
        redirect: get_str("redirect", ctx)?,
        referrer: get_str("referrer", ctx)?,
        referrer_policy: get_str("referrerPolicy", ctx)?,
        integrity: get_str("integrity", ctx)?,
        priority: get_str("priority", ctx)?,
        ..FetchInit::default()
    };
    init.keepalive = desc.get(js_string!("keepalive"), ctx)?.to_boolean();
    init.body_stream = desc.get(js_string!("bodyStream"), ctx)?.to_boolean();
    let headers = desc.get(js_string!("headers"), ctx)?;
    if let Some(obj) = headers.as_object() {
        let list = JsArray::from_object(obj.clone())?;
        for i in 0..list.length(ctx)? {
            let pair = list.get(i, ctx)?;
            let Some(pair) = pair.as_object() else {
                continue;
            };
            let name = pair.get(0, ctx)?.to_string(ctx)?.to_std_string_escaped();
            let value = pair.get(1, ctx)?.to_string(ctx)?.to_std_string_escaped();
            init.headers.push((name, value));
        }
    }
    let body = desc.get(js_string!("body"), ctx)?;
    if !body.is_undefined() && !body.is_null() {
        init.body = Some(array_buffer_bytes(&body)?);
    }
    Ok(init)
}

fn inject_prelude(context: &mut Context) -> Result<(), JsEngineError> {
    let prelude = r#"
var __axiom_nodeWrappers = [];
// One wrapper per node; the document node is the `document` object. Wrappers get the
// prototype of their node type from the DOM prelude.
function ElementRef(id) {
  id = id|0;
  var existing = __axiom_nodeWrappers[id];
  if (existing) return existing;
  if (id === (__axiom_documentRoot()|0)) return document;
  this.__id = id;
  __axiom_nodeWrappers[id] = this;
  if (typeof __axiom_protoFor === 'function') Object.setPrototypeOf(this, __axiom_protoFor(id));
}

var document = {
  getElementById: function(id) {
    var n = __axiom_getElementById(String(id));
    return n < 0 ? null : new ElementRef(n);
  },
  querySelector: function(sel) {
    var n = __axiom_querySelector(String(sel));
    return n < 0 ? null : new ElementRef(n);
  },
  createElement: function(tag) { return new ElementRef(__axiom_createElement(String(tag))); },
  body: null
};
Object.defineProperty(document, 'cookie', {
  get: function() { return __axiom_getCookie(); },
  set: function(v) { __axiom_setCookie(String(v)); },
  enumerable: true,
  configurable: true
});
function __axiom_makeStorage(session) {
  return {
    getItem: function(k) {
      var v = __axiom_storageGet(!!session, String(k));
      return v === null ? null : String(v);
    },
    setItem: function(k, v) { __axiom_storageSet(!!session, String(k), String(v)); },
    removeItem: function(k) { __axiom_storageRemove(!!session, String(k)); },
    clear: function() { __axiom_storageClear(!!session); },
    key: function(i) {
      var k = __axiom_storageKey(!!session, i|0);
      return k === null ? null : String(k);
    },
    get length() { return __axiom_storageLength(!!session)|0; }
  };
}
var localStorage = __axiom_makeStorage(false);
var sessionStorage = __axiom_makeStorage(true);
var window = this;
var console = { log: function() { __axiom_log.apply(null, arguments); } };
"#;
    context
        .eval(Source::from_bytes(prelude))
        .map_err(|e: JsError| JsEngineError::Runtime(e.to_string()))?;
    context
        .eval(Source::from_bytes(EVENTS_PRELUDE))
        .map_err(|e: JsError| JsEngineError::Runtime(e.to_string()))?;
    context
        .eval(Source::from_bytes(PLATFORM_PRELUDE))
        .map_err(|e: JsError| JsEngineError::Runtime(e.to_string()))?;
    context
        .eval(Source::from_bytes(FETCH_PRELUDE))
        .map_err(|e: JsError| JsEngineError::Runtime(e.to_string()))?;
    context
        .eval(Source::from_bytes(DOM_PRELUDE))
        .map_err(|e: JsError| JsEngineError::Runtime(e.to_string()))?;
    context
        .eval(Source::from_bytes(DOCUMENT_PRELUDE))
        .map_err(|e: JsError| JsEngineError::Runtime(e.to_string()))?;
    context
        .eval(Source::from_bytes(WINDOW_PRELUDE))
        .map_err(|e: JsError| JsEngineError::Runtime(e.to_string()))?;
    context
        .eval(Source::from_bytes(CSSOM_PRELUDE))
        .map_err(|e: JsError| JsEngineError::Runtime(e.to_string()))?;
    context
        .eval(Source::from_bytes(VIEW_PRELUDE))
        .map_err(|e: JsError| JsEngineError::Runtime(e.to_string()))?;
    context
        .eval(Source::from_bytes(TIMERS_PRELUDE))
        .map_err(|e: JsError| JsEngineError::Runtime(e.to_string()))?;
    context
        .eval(Source::from_bytes(OBSERVERS_PRELUDE))
        .map_err(|e: JsError| JsEngineError::Runtime(e.to_string()))?;
    context
        .eval(Source::from_bytes(WEBAPI_PRELUDE))
        .map_err(|e: JsError| JsEngineError::Runtime(e.to_string()))?;
    context
        .eval(Source::from_bytes(INTL_PRELUDE))
        .map_err(|e: JsError| JsEngineError::Runtime(e.to_string()))?;
    context
        .eval(Source::from_bytes(FORMS_PRELUDE))
        .map_err(|e: JsError| JsEngineError::Runtime(e.to_string()))?;
    context
        .eval(Source::from_bytes(CUSTOM_ELEMENTS_PRELUDE))
        .map_err(|e: JsError| JsEngineError::Runtime(e.to_string()))?;
    context
        .eval(Source::from_bytes(INTERFACES_PRELUDE))
        .map_err(|e: JsError| JsEngineError::Runtime(e.to_string()))?;
    Ok(())
}

/// Moves the document's attribute accessors onto `Document.prototype`. Runs after every
/// other prelude has defined them.
const INTERFACES_PRELUDE: &str = include_str!("interfaces_prelude.js");

/// customElements, constructible HTMLElement, upgrades and lifecycle reactions. Runs last:
/// it wraps the mutation natives the other preludes set up.
const CUSTOM_ELEMENTS_PRELUDE: &str = include_str!("custom_elements_prelude.js");

/// Form control IDL attributes, HTMLFormElement, activation behavior and submission.
const FORMS_PRELUDE: &str = include_str!("forms_prelude.js");

/// CSSStyleDeclaration, element.style over the style attribute, getComputedStyle.
const CSSOM_PRELUDE: &str = include_str!("cssom_prelude.js");

/// DOMRect, element geometry, viewport scrolling, the Image constructor and screen.
const VIEW_PRELUDE: &str = include_str!("view_prelude.js");

/// Timers, queueMicrotask, animation frames, navigator and matchMedia.
const TIMERS_PRELUDE: &str = include_str!("timers_prelude.js");

/// IntersectionObserver, ResizeObserver and requestIdleCallback.
const OBSERVERS_PRELUDE: &str = include_str!("observers_prelude.js");

/// Performance timeline, atob / btoa, crypto, structuredClone, dataset, CSS, focus,
/// MessageChannel / postMessage, TreeWalker / NodeIterator, XMLHttpRequest and History.
const WEBAPI_PRELUDE: &str = include_str!("webapi_prelude.js");

/// Intl (en-US) and the locale-sensitive Date / Number / String methods.
const INTL_PRELUDE: &str = include_str!("intl_prelude.js");

/// document.readyState / currentScript / lifecycle events, element load/error events and
/// attribute reflection.
const DOCUMENT_PRELUDE: &str = include_str!("document_prelude.js");

/// self / parent / top / location, document.URL / title, performance.
const WINDOW_PRELUDE: &str = include_str!("window_prelude.js");

/// The DOM core: node interfaces and wrappers, factories, checked mutation, attributes,
/// DOMTokenList, live collections, document.implementation.
const DOM_PRELUDE: &str = include_str!("dom_prelude.js");

/// EventTarget, Event and its subclasses, dispatch, event handlers, Window.
const EVENTS_PRELUDE: &str = include_str!("events_prelude.js");

/// Promise rejection tracking, Blob, File, URLSearchParams, FormData.
const PLATFORM_PRELUDE: &str = include_str!("platform_prelude.js");

/// DOMException, AbortController/AbortSignal, ReadableStream (default readers),
/// TextEncoder/TextDecoder (UTF-8), Headers, Request, Response and fetch().
const FETCH_PRELUDE: &str = include_str!("fetch_prelude.js");
