//! Structured document loading events (bounded per-document log).

use std::collections::VecDeque;
use std::time::Instant;

use axiom_net::NetworkRequestId;

use crate::ids::{DocumentId, ResourceId};

pub const DEFAULT_EVENT_CAPACITY: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocumentEventKind {
    DocumentCreated,
    ParsingStarted,
    ResourceDiscovered,
    ResourceQueued,
    ResourceReady,
    ResourceFailed,
    ResourceCanceled,
    ScriptExecutionStarted,
    ScriptExecutionFinished,
    ParsingCompleted,
    DomContentLoadedFired,
    LoadFired,
    DocumentCanceled,
    /// A network, timer or script callback addressed to an older document was dropped.
    StaleEventDropped,
}

impl DocumentEventKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::DocumentCreated => "DocumentCreated",
            Self::ParsingStarted => "ParsingStarted",
            Self::ResourceDiscovered => "ResourceDiscovered",
            Self::ResourceQueued => "ResourceQueued",
            Self::ResourceReady => "ResourceReady",
            Self::ResourceFailed => "ResourceFailed",
            Self::ResourceCanceled => "ResourceCanceled",
            Self::ScriptExecutionStarted => "ScriptExecutionStarted",
            Self::ScriptExecutionFinished => "ScriptExecutionFinished",
            Self::ParsingCompleted => "ParsingCompleted",
            Self::DomContentLoadedFired => "DOMContentLoadedFired",
            Self::LoadFired => "LoadFired",
            Self::DocumentCanceled => "DocumentCanceled",
            Self::StaleEventDropped => "StaleEventDropped",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DocumentEvent {
    /// Position in the document's log (monotonic).
    pub seq: u64,
    pub at: Instant,
    pub document: DocumentId,
    pub kind: DocumentEventKind,
    pub resource: Option<ResourceId>,
    pub request: Option<NetworkRequestId>,
    /// Short, secret-free detail (resource type, script kind, error class).
    pub detail: String,
}

#[derive(Debug)]
pub struct DocumentEventLog {
    document: DocumentId,
    events: VecDeque<DocumentEvent>,
    capacity: usize,
    next_seq: u64,
    dropped: u64,
}

impl DocumentEventLog {
    pub fn new(document: DocumentId) -> Self {
        Self::with_capacity(document, DEFAULT_EVENT_CAPACITY)
    }

    pub fn with_capacity(document: DocumentId, capacity: usize) -> Self {
        Self {
            document,
            events: VecDeque::new(),
            capacity: capacity.max(1),
            next_seq: 0,
            dropped: 0,
        }
    }

    pub fn record(
        &mut self,
        kind: DocumentEventKind,
        resource: Option<ResourceId>,
        request: Option<NetworkRequestId>,
        detail: impl Into<String>,
    ) {
        if self.events.len() == self.capacity {
            self.events.pop_front();
            self.dropped += 1;
        }
        self.events.push_back(DocumentEvent {
            seq: self.next_seq,
            at: Instant::now(),
            document: self.document,
            kind,
            resource,
            request,
            detail: detail.into(),
        });
        self.next_seq += 1;
    }

    pub fn events(&self) -> impl Iterator<Item = &DocumentEvent> {
        self.events.iter()
    }

    pub fn count(&self, kind: DocumentEventKind) -> usize {
        self.events.iter().filter(|e| e.kind == kind).count()
    }

    /// Events evicted because the log was full.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_is_bounded_and_ordered() {
        let mut log = DocumentEventLog::with_capacity(DocumentId::next(), 2);
        log.record(DocumentEventKind::DocumentCreated, None, None, "");
        log.record(DocumentEventKind::ParsingStarted, None, None, "");
        log.record(DocumentEventKind::ParsingCompleted, None, None, "");
        let kinds: Vec<_> = log.events().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [
                DocumentEventKind::ParsingStarted,
                DocumentEventKind::ParsingCompleted
            ]
        );
        assert_eq!(log.dropped(), 1);
        assert_eq!(log.events().last().unwrap().seq, 2);
    }
}
