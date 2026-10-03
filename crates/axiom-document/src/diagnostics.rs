//! Queryable snapshots of a document and its resources (no UI required).

use crate::ids::{DocumentId, NavigationId, ResourceId};
use crate::resource::ResourceRecord;

/// One row per resource.
#[derive(Debug, Clone)]
pub struct ResourceDiagnostics {
    pub id: ResourceId,
    pub document: DocumentId,
    pub request: Option<u64>,
    pub kind: &'static str,
    pub url: String,
    pub initiator: &'static str,
    pub state: &'static str,
    pub priority: &'static str,
    pub script_kind: Option<&'static str>,
    pub render_blocking: bool,
    pub load_blocking: bool,
    pub status: Option<u16>,
    pub mime: Option<String>,
    pub cache: Option<&'static str>,
    pub transferred_bytes: u64,
    pub decoded_bytes: u64,
    /// Measured from queueing to the terminal state; `None` while pending.
    pub duration_ms: Option<f64>,
    pub error_class: Option<&'static str>,
    pub failure: Option<String>,
}

impl From<&ResourceRecord> for ResourceDiagnostics {
    fn from(r: &ResourceRecord) -> Self {
        Self {
            id: r.id,
            document: r.document,
            request: r.request.map(|q| q.0),
            kind: r.kind.as_str(),
            url: r.url.clone(),
            initiator: r.initiator.as_str(),
            state: r.state.as_str(),
            priority: r.priority.as_str(),
            script_kind: r.script_kind.map(|k| k.as_str()),
            render_blocking: r.blocking.render,
            load_blocking: r.blocking.load,
            status: r.status,
            mime: r.mime.clone(),
            cache: r.cache.map(|c| c.as_str()),
            transferred_bytes: r.transferred_bytes,
            decoded_bytes: r.decoded_bytes,
            duration_ms: r.duration().map(|d| d.as_secs_f64() * 1000.0),
            error_class: r.failure.as_ref().map(|f| f.class.as_str()),
            failure: r.failure.as_ref().map(|f| f.to_string()),
        }
    }
}

/// The document-level view.
#[derive(Debug, Clone)]
pub struct DocumentDiagnostics {
    pub document: DocumentId,
    pub navigation: Option<NavigationId>,
    pub url: String,
    pub origin: String,
    pub kind: &'static str,
    pub encoding: Option<String>,
    pub ready_state: &'static str,
    pub parsing_state: &'static str,
    pub bytes_parsed: usize,
    pub blocking_stylesheets: Vec<ResourceId>,
    pub parser_blocking_script: Option<String>,
    pub defer_queue: Vec<String>,
    pub load_blocking: usize,
    pub total_resources: usize,
    pub pending_resources: usize,
    pub failed_resources: usize,
    pub rejected_resources: usize,
    pub script_errors: usize,
    pub dom_content_loaded_fired: bool,
    pub load_fired: bool,
    pub canceled: bool,
    /// Milestones in ms since navigation start (`None` = not reached).
    pub timeline: Vec<(&'static str, Option<f64>)>,
    /// Callbacks addressed to older documents that this browsing context dropped.
    pub stale_events_dropped: u64,
    pub events_recorded: usize,
}
