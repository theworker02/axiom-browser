//! Page lifecycle coordinator: parser state, script/stylesheet blocking, readyState,
//! `DOMContentLoaded` and `load`. State-driven — no timers and no "network quiet"
//! heuristics decide when an event fires.

use std::collections::{BTreeSet, VecDeque};
use std::time::Instant;

use axiom_dom::NodeId;

use crate::ids::ResourceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsingState {
    NotStarted,
    Parsing,
    /// Everything received so far is parsed; waiting for more bytes.
    WaitingForData,
    /// Paused at a script (waiting for it to arrive or for script-blocking stylesheets).
    BlockedOnScript,
    Complete,
}

impl ParsingState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "not-started",
            Self::Parsing => "parsing",
            Self::WaitingForData => "waiting-for-data",
            Self::BlockedOnScript => "blocked-on-script",
            Self::Complete => "complete",
        }
    }
}

/// `document.readyState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyState {
    Loading,
    Interactive,
    Complete,
}

impl ReadyState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::Interactive => "interactive",
            Self::Complete => "complete",
        }
    }
}

/// A script the parser or the defer queue waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingScript {
    pub node: NodeId,
    /// `None` for an inline script waiting only for script-blocking stylesheets.
    pub resource: Option<ResourceId>,
    pub label: String,
}

/// Performance milestones of one document. `None` = not reached (never estimated).
#[derive(Debug, Clone)]
pub struct Timeline {
    pub navigation_start: Instant,
    pub response_start: Option<Instant>,
    pub first_bytes: Option<Instant>,
    pub parse_start: Option<Instant>,
    pub parse_end: Option<Instant>,
    pub dom_content_loaded: Option<Instant>,
    /// `load` event dispatched.
    pub load: Option<Instant>,
    /// `load` handlers finished; the document is fully loaded.
    pub complete: Option<Instant>,
}

impl Timeline {
    pub fn new(navigation_start: Instant) -> Self {
        Self {
            navigation_start,
            response_start: None,
            first_bytes: None,
            parse_start: None,
            parse_end: None,
            dom_content_loaded: None,
            load: None,
            complete: None,
        }
    }

    /// Milestones in milliseconds since navigation start.
    pub fn milestones(&self) -> Vec<(&'static str, Option<f64>)> {
        let ms = |t: Option<Instant>| {
            t.map(|t| {
                t.saturating_duration_since(self.navigation_start)
                    .as_secs_f64()
                    * 1000.0
            })
        };
        vec![
            ("navigation_start", Some(0.0)),
            ("response_start", ms(self.response_start)),
            ("first_bytes", ms(self.first_bytes)),
            ("parse_start", ms(self.parse_start)),
            ("parse_end", ms(self.parse_end)),
            ("dom_content_loaded", ms(self.dom_content_loaded)),
            ("load", ms(self.load)),
            ("complete", ms(self.complete)),
        ]
    }
}

#[derive(Debug, Clone)]
pub struct DocumentLifecycle {
    pub parsing_state: ParsingState,
    /// Parser-inserted stylesheets (and their imports) still loading: they block scripts
    /// and rendering.
    pub blocking_stylesheets: BTreeSet<ResourceId>,
    /// The script the parser is paused at.
    pub parser_blocking_script: Option<PendingScript>,
    /// `defer` scripts, in document order.
    pub defer_queue: VecDeque<PendingScript>,
    /// Resources the `load` event waits for.
    pub load_blocking_resources: BTreeSet<ResourceId>,
    /// Render-blocking resources still loading (parser-inserted stylesheets and
    /// `blocking="render"` elements): no rendering opportunity, animation frames
    /// included, until they settle.
    pub render_blocking_resources: BTreeSet<ResourceId>,
    pub ready_state: ReadyState,
    pub dom_content_loaded_fired: bool,
    pub load_fired: bool,
    /// The document was replaced or stopped; nothing may advance it any more.
    pub canceled: bool,
    pub timeline: Timeline,
}

impl DocumentLifecycle {
    pub fn new(navigation_start: Instant) -> Self {
        Self {
            parsing_state: ParsingState::NotStarted,
            blocking_stylesheets: BTreeSet::new(),
            parser_blocking_script: None,
            defer_queue: VecDeque::new(),
            load_blocking_resources: BTreeSet::new(),
            render_blocking_resources: BTreeSet::new(),
            ready_state: ReadyState::Loading,
            dom_content_loaded_fired: false,
            load_fired: false,
            canceled: false,
            timeline: Timeline::new(navigation_start),
        }
    }

    pub fn parsing_complete(&self) -> bool {
        self.parsing_state == ParsingState::Complete
    }

    /// Scripts (parser-blocking, inline and deferred) must wait.
    pub fn scripts_blocked_by_stylesheets(&self) -> bool {
        !self.blocking_stylesheets.is_empty()
    }

    pub fn start_parsing(&mut self, now: Instant) {
        if self.parsing_state == ParsingState::NotStarted {
            self.parsing_state = ParsingState::Parsing;
            self.timeline.parse_start = Some(now);
        }
    }

    /// The parser reached the end of input: readyState becomes `interactive`.
    pub fn finish_parsing(&mut self, now: Instant) {
        self.parsing_state = ParsingState::Complete;
        self.parser_blocking_script = None;
        self.timeline.parse_end = Some(now);
        self.ready_state = ReadyState::Interactive;
    }

    /// Parsing is done, no parser-blocking script is pending and every deferred script
    /// has run.
    pub fn ready_for_dom_content_loaded(&self) -> bool {
        !self.canceled
            && self.parsing_complete()
            && self.parser_blocking_script.is_none()
            && self.defer_queue.is_empty()
            && !self.dom_content_loaded_fired
    }

    pub fn fire_dom_content_loaded(&mut self, now: Instant) {
        self.dom_content_loaded_fired = true;
        self.timeline.dom_content_loaded = Some(now);
    }

    /// `DOMContentLoaded` fired and nothing load-blocking is pending.
    pub fn ready_for_load(&self) -> bool {
        !self.canceled
            && self.dom_content_loaded_fired
            && self.load_blocking_resources.is_empty()
            && !self.load_fired
    }

    /// readyState becomes `complete` right before the `load` event.
    pub fn begin_load(&mut self, now: Instant) {
        self.ready_state = ReadyState::Complete;
        self.load_fired = true;
        self.timeline.load = Some(now);
    }

    pub fn finish_load(&mut self, now: Instant) {
        self.timeline.complete = Some(now);
    }

    /// A resource reached a terminal state: it no longer blocks anything.
    pub fn resource_settled(&mut self, id: ResourceId) {
        self.blocking_stylesheets.remove(&id);
        self.load_blocking_resources.remove(&id);
        self.render_blocking_resources.remove(&id);
    }

    pub fn cancel(&mut self) {
        self.canceled = true;
        self.blocking_stylesheets.clear();
        self.load_blocking_resources.clear();
        self.render_blocking_resources.clear();
        self.parser_blocking_script = None;
        self.defer_queue.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dom_content_loaded_waits_for_parsing_and_defer_scripts_only() {
        let now = Instant::now();
        let mut lc = DocumentLifecycle::new(now);
        let image = ResourceId::next();
        lc.load_blocking_resources.insert(image);
        lc.start_parsing(now);
        assert_eq!(lc.ready_state, ReadyState::Loading);
        assert!(!lc.ready_for_dom_content_loaded());
        lc.defer_queue.push_back(PendingScript {
            node: NodeId(3),
            resource: Some(ResourceId::next()),
            label: "d.js".into(),
        });
        lc.finish_parsing(now);
        assert_eq!(lc.ready_state, ReadyState::Interactive);
        assert!(
            !lc.ready_for_dom_content_loaded(),
            "fired before defer scripts ran"
        );
        lc.defer_queue.pop_front();
        assert!(lc.ready_for_dom_content_loaded(), "waited for an image");
        lc.fire_dom_content_loaded(now);
        assert!(!lc.ready_for_dom_content_loaded(), "fires once");
        assert!(!lc.ready_for_load(), "load fired with an image pending");
        lc.resource_settled(image);
        assert!(lc.ready_for_load());
        lc.begin_load(now);
        assert_eq!(lc.ready_state, ReadyState::Complete);
        assert!(!lc.ready_for_load(), "load fires once");
    }

    #[test]
    fn canceled_documents_never_advance() {
        let now = Instant::now();
        let mut lc = DocumentLifecycle::new(now);
        lc.finish_parsing(now);
        lc.cancel();
        assert!(!lc.ready_for_dom_content_loaded());
        lc.dom_content_loaded_fired = true;
        assert!(!lc.ready_for_load());
    }

    #[test]
    fn timeline_reports_unreached_milestones_as_none() {
        let start = Instant::now();
        let mut t = Timeline::new(start);
        t.parse_start = Some(start);
        let m = t.milestones();
        assert_eq!(m[0], ("navigation_start", Some(0.0)));
        assert!(m.iter().any(|(n, v)| *n == "parse_start" && v.is_some()));
        assert!(m.iter().any(|(n, v)| *n == "load" && v.is_none()));
    }
}
