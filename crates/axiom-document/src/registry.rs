//! Per-document resource registry.

use std::collections::HashMap;
use std::time::Instant;

use axiom_dom::NodeId;
use axiom_net::{NetworkRequestId, RequestPriority, ResourceType};

use crate::error::{FailureStage, ResourceError};
use crate::ids::{DocumentId, ResourceId};
use crate::resource::{Blocking, CorsSettings, Initiator, ResourceRecord, ResourceState};
use crate::script::ScriptKind;

/// Resources one document may register before further discoveries are rejected.
pub const DEFAULT_MAX_RESOURCES: usize = 2048;

/// Parameters of a newly discovered resource.
#[derive(Debug, Clone)]
pub struct NewResource {
    pub url: String,
    pub kind: ResourceType,
    pub initiator: Initiator,
    pub node: Option<NodeId>,
    pub source_offset: Option<usize>,
    pub priority: RequestPriority,
    pub blocking: Blocking,
    pub script_kind: Option<ScriptKind>,
    pub lazy: bool,
    pub cors: Option<CorsSettings>,
}

impl NewResource {
    pub fn new(url: impl Into<String>, kind: ResourceType, initiator: Initiator) -> Self {
        Self {
            url: url.into(),
            kind,
            initiator,
            node: None,
            source_offset: None,
            priority: kind.default_priority(),
            blocking: Blocking::default(),
            script_kind: None,
            lazy: false,
            cors: None,
        }
    }
}

/// Every resource a document discovered, in discovery order. Resources are identified by
/// [`ResourceId`] (never by URL) and map to at most one network request each.
#[derive(Debug)]
pub struct ResourceRegistry {
    document: DocumentId,
    records: Vec<ResourceRecord>,
    index: HashMap<ResourceId, usize>,
    by_request: HashMap<NetworkRequestId, ResourceId>,
    max_resources: usize,
    rejected: usize,
}

impl ResourceRegistry {
    pub fn new(document: DocumentId) -> Self {
        Self::with_limit(document, DEFAULT_MAX_RESOURCES)
    }

    pub fn with_limit(document: DocumentId, max_resources: usize) -> Self {
        Self {
            document,
            records: Vec::new(),
            index: HashMap::new(),
            by_request: HashMap::new(),
            max_resources,
            rejected: 0,
        }
    }

    pub fn document(&self) -> DocumentId {
        self.document
    }

    /// Register a discovered resource. `None` when the per-document limit is reached.
    pub fn add(&mut self, r: NewResource) -> Option<ResourceId> {
        if self.records.len() >= self.max_resources {
            self.rejected += 1;
            return None;
        }
        let id = ResourceId::next();
        self.index.insert(id, self.records.len());
        self.records.push(ResourceRecord {
            id,
            document: self.document,
            request: None,
            url: r.url,
            final_url: None,
            kind: r.kind,
            initiator: r.initiator,
            node: r.node,
            source_offset: r.source_offset,
            state: ResourceState::Discovered,
            priority: r.priority,
            blocking: r.blocking,
            script_kind: r.script_kind,
            lazy: r.lazy,
            cors: r.cors,
            claimed: false,
            mixed_content: false,
            status: None,
            mime: None,
            cache: None,
            transferred_bytes: 0,
            decoded_bytes: 0,
            discovered_at: Instant::now(),
            queued_at: None,
            response_at: None,
            finished_at: None,
            failure: None,
            image: None,
            font_family: None,
        });
        Some(id)
    }

    pub fn get(&self, id: ResourceId) -> Option<&ResourceRecord> {
        self.index.get(&id).map(|&i| &self.records[i])
    }

    pub fn get_mut(&mut self, id: ResourceId) -> Option<&mut ResourceRecord> {
        self.index.get(&id).map(|&i| &mut self.records[i])
    }

    /// Record the network request carrying the resource (state becomes `Queued`).
    pub fn attach_request(&mut self, id: ResourceId, request: NetworkRequestId) {
        self.by_request.insert(request, id);
        if let Some(r) = self.get_mut(id) {
            r.request = Some(request);
            r.queued_at = Some(Instant::now());
        }
        self.transition(id, ResourceState::Queued);
    }

    pub fn by_request(&self, request: NetworkRequestId) -> Option<ResourceId> {
        self.by_request.get(&request).copied()
    }

    /// Apply a state transition. Illegal transitions are refused (returns `false`).
    pub fn transition(&mut self, id: ResourceId, next: ResourceState) -> bool {
        let Some(r) = self.get_mut(id) else {
            return false;
        };
        if !r.state.can_transition_to(next) {
            return false;
        }
        r.state = next;
        match next {
            ResourceState::Fetching => r.response_at = Some(Instant::now()),
            s if s.is_terminal() => r.finished_at = Some(Instant::now()),
            _ => {}
        }
        true
    }

    /// Terminal failure (`Canceled` when the stage is `Canceled`, otherwise `Failed`).
    pub fn fail(&mut self, id: ResourceId, error: ResourceError) -> bool {
        let next = if error.stage == FailureStage::Canceled {
            ResourceState::Canceled
        } else {
            ResourceState::Failed
        };
        if !self.transition(id, next) {
            return false;
        }
        if let Some(r) = self.get_mut(id) {
            r.failure = Some(error);
        }
        true
    }

    /// Cancel every pending resource (document replaced or stopped). Returns the
    /// requests that were still attached.
    pub fn cancel_pending(&mut self, reason: &str) -> Vec<NetworkRequestId> {
        let ids: Vec<ResourceId> = self.pending().map(|r| r.id).collect();
        let mut requests = Vec::new();
        for id in ids {
            let kind = self.get(id).map(|r| r.kind).unwrap_or(ResourceType::Other);
            if let Some(req) = self.get(id).and_then(|r| r.request) {
                requests.push(req);
            }
            self.fail(id, ResourceError::of(kind, FailureStage::Canceled, reason));
        }
        requests
    }

    /// A preload-scanner resource not yet adopted by an element.
    /// A preload the parser can adopt: same type, URL and CORS mode (a `no-cors` response
    /// cannot stand in for a `cors` one, or the reverse).
    pub fn find_unclaimed_preload(
        &self,
        kind: ResourceType,
        url: &str,
        cors: Option<CorsSettings>,
    ) -> Option<ResourceId> {
        self.records
            .iter()
            .find(|r| {
                r.initiator == Initiator::PreloadScanner
                    && !r.claimed
                    && r.kind == kind
                    && r.url == url
                    && r.cors == cors
                    && r.state != ResourceState::Canceled
            })
            .map(|r| r.id)
    }

    pub fn all(&self) -> &[ResourceRecord] {
        &self.records
    }

    pub fn pending(&self) -> impl Iterator<Item = &ResourceRecord> {
        self.records.iter().filter(|r| r.is_pending())
    }

    pub fn failed(&self) -> impl Iterator<Item = &ResourceRecord> {
        self.records
            .iter()
            .filter(|r| r.state == ResourceState::Failed)
    }

    pub fn of_kind(&self, kind: ResourceType) -> impl Iterator<Item = &ResourceRecord> {
        self.records.iter().filter(move |r| r.kind == kind)
    }

    pub fn stylesheets(&self) -> impl Iterator<Item = &ResourceRecord> {
        self.of_kind(ResourceType::Stylesheet)
    }

    pub fn scripts(&self) -> impl Iterator<Item = &ResourceRecord> {
        self.of_kind(ResourceType::Script)
    }

    pub fn images(&self) -> impl Iterator<Item = &ResourceRecord> {
        self.of_kind(ResourceType::Image)
    }

    pub fn fonts(&self) -> impl Iterator<Item = &ResourceRecord> {
        self.of_kind(ResourceType::Font)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Discoveries refused because the limit was reached.
    pub fn rejected_count(&self) -> usize {
        self.rejected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> ResourceRegistry {
        ResourceRegistry::new(DocumentId::next())
    }

    #[test]
    fn tracks_states_and_answers_queries() {
        let mut reg = registry();
        let css = reg
            .add(NewResource::new(
                "http://a/s.css",
                ResourceType::Stylesheet,
                Initiator::Parser,
            ))
            .unwrap();
        let img = reg
            .add(NewResource::new(
                "http://a/i.png",
                ResourceType::Image,
                Initiator::Parser,
            ))
            .unwrap();
        let js = reg
            .add(NewResource::new(
                "http://a/s.js",
                ResourceType::Script,
                Initiator::Parser,
            ))
            .unwrap();
        let req = NetworkRequestId::new();
        reg.attach_request(css, req);
        assert_eq!(reg.by_request(req), Some(css));
        assert_eq!(reg.get(css).unwrap().state, ResourceState::Queued);
        assert!(reg.transition(css, ResourceState::Fetching));
        assert!(reg.transition(css, ResourceState::Available));
        assert!(reg.transition(css, ResourceState::Ready));
        assert!(
            !reg.transition(css, ResourceState::Queued),
            "left a terminal state"
        );
        assert!(reg.fail(
            img,
            ResourceError::of(ResourceType::Image, FailureStage::HttpStatus, "404")
        ));
        assert_eq!(reg.stylesheets().count(), 1);
        assert_eq!(reg.images().count(), 1);
        assert_eq!(reg.scripts().count(), 1);
        assert_eq!(reg.fonts().count(), 0);
        assert_eq!(reg.failed().map(|r| r.id).collect::<Vec<_>>(), [img]);
        assert_eq!(reg.pending().map(|r| r.id).collect::<Vec<_>>(), [js]);
        assert!(reg.get(css).unwrap().duration().is_some());
    }

    #[test]
    fn same_url_twice_is_two_resources() {
        let mut reg = registry();
        let a = reg.add(NewResource::new(
            "http://a/i.png",
            ResourceType::Image,
            Initiator::Parser,
        ));
        let b = reg.add(NewResource::new(
            "http://a/i.png",
            ResourceType::Image,
            Initiator::Parser,
        ));
        assert_ne!(a, b);
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn cancel_pending_marks_canceled_and_returns_requests() {
        let mut reg = registry();
        let a = reg
            .add(NewResource::new(
                "http://a/1",
                ResourceType::Image,
                Initiator::Parser,
            ))
            .unwrap();
        let req = NetworkRequestId::new();
        reg.attach_request(a, req);
        assert_eq!(reg.cancel_pending("replaced"), vec![req]);
        let r = reg.get(a).unwrap();
        assert_eq!(r.state, ResourceState::Canceled);
        assert_eq!(r.failure.as_ref().unwrap().stage, FailureStage::Canceled);
        assert_eq!(reg.pending().count(), 0);
    }

    #[test]
    fn limit_rejects_further_discoveries() {
        let mut reg = ResourceRegistry::with_limit(DocumentId::next(), 1);
        assert!(reg
            .add(NewResource::new(
                "http://a/1",
                ResourceType::Image,
                Initiator::Parser
            ))
            .is_some());
        assert!(reg
            .add(NewResource::new(
                "http://a/2",
                ResourceType::Image,
                Initiator::Parser
            ))
            .is_none());
        assert_eq!(reg.rejected_count(), 1);
    }

    #[test]
    fn preloads_are_claimed_once() {
        let mut reg = registry();
        let p = reg
            .add(NewResource::new(
                "http://a/s.js",
                ResourceType::Script,
                Initiator::PreloadScanner,
            ))
            .unwrap();
        assert_eq!(
            reg.find_unclaimed_preload(ResourceType::Script, "http://a/s.js", None),
            Some(p)
        );
        assert_eq!(
            reg.find_unclaimed_preload(ResourceType::Image, "http://a/s.js", None),
            None
        );
        // A preload fetched in another CORS mode is a different request.
        assert_eq!(
            reg.find_unclaimed_preload(
                ResourceType::Script,
                "http://a/s.js",
                Some(CorsSettings::Anonymous)
            ),
            None
        );
        reg.get_mut(p).unwrap().claimed = true;
        assert_eq!(
            reg.find_unclaimed_preload(ResourceType::Script, "http://a/s.js", None),
            None
        );
    }
}
