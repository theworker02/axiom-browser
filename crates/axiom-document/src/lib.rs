//! Document loading vocabulary shared by the engine and the browser layer.
//!
//! Everything here is plain data and pure logic (no network, no JS): document and
//! resource identity ([`DocumentId`], [`ResourceId`]), the per-document
//! [`ResourceRegistry`] and its [`ResourceState`] machine, the [`DocumentLifecycle`]
//! coordinator (readyState, `DOMContentLoaded`, `load`), script classification, request
//! priorities, structured [`DocumentEvent`]s, error classes, policy hooks (CSP-style
//! directives and mixed content), base-URL and CSS `url()` resolution, and a conservative
//! preload scanner. The engine's document loader drives these; nothing here performs I/O.

mod base;
mod css;
mod diagnostics;
mod error;
mod events;
mod ids;
mod info;
mod lifecycle;
mod policy;
mod preload;
mod priority;
mod registry;
mod resource;
mod script;

pub use base::{resolve_base_href, resolve_subresource, SubresourceTarget};
pub use css::{
    absolutize_urls, css_imports, font_faces, font_families_used, strip_css_imports, FontFace,
};
pub use diagnostics::{DocumentDiagnostics, ResourceDiagnostics};
pub use error::{ErrorClass, FailureStage, ResourceError};
pub use events::{DocumentEvent, DocumentEventKind, DocumentEventLog, DEFAULT_EVENT_CAPACITY};
pub use ids::{BrowsingContextId, DocumentId, NavigationId, ResourceId};
pub use info::{DocumentInfo, DocumentKind};
pub use lifecycle::{DocumentLifecycle, ParsingState, PendingScript, ReadyState, Timeline};
pub use policy::{
    classify_mixed_content, mixed_content_decision, AllowAllPolicy, BlockHostsPolicy,
    ContentPolicy, MixedContent, PolicyDecision, PolicyDirective,
};
pub use preload::{scan_for_preloads, PreloadHint, MAX_PRELOAD_HINTS};
pub use priority::{priority_for, PriorityHint};
pub use registry::{NewResource, ResourceRegistry, DEFAULT_MAX_RESOURCES};
pub use resource::{Blocking, CorsSettings, ImageInfo, Initiator, ResourceRecord, ResourceState};
pub use script::{
    classify_script, is_classic_script_type, unwrap_cdata, ScriptElement, ScriptKind,
};
