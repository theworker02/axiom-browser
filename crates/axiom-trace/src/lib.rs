//! Structured engine tracing (not just log lines).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceKind {
    Navigation,
    HtmlParse,
    CssParse,
    Style,
    Javascript,
    Layout,
    Paint,
    Composite,
    Gpu,
    Network,
    DomMutation,
    Event,
    Timer,
    Other,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceEvent {
    pub kind: TraceKind,
    pub name: String,
    pub start_ms: f64,
    pub duration_ms: f64,
    pub detail: Option<String>,
}

#[derive(Debug)]
pub struct TraceTimeline {
    events: Mutex<Vec<TraceEvent>>,
    epoch: Instant,
}

impl Default for TraceTimeline {
    fn default() -> Self {
        Self::new()
    }
}

impl TraceTimeline {
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            epoch: Instant::now(),
        }
    }

    pub fn clear(&self) {
        self.events.lock().clear();
        // keep epoch for relative consistency within a session
    }

    pub fn record(
        &self,
        kind: TraceKind,
        name: impl Into<String>,
        duration: Duration,
        detail: Option<String>,
    ) {
        let start_ms =
            self.epoch.elapsed().as_secs_f64() * 1000.0 - duration.as_secs_f64() * 1000.0;
        self.events.lock().push(TraceEvent {
            kind,
            name: name.into(),
            start_ms: start_ms.max(0.0),
            duration_ms: duration.as_secs_f64() * 1000.0,
            detail,
        });
    }

    pub fn span(&self, kind: TraceKind, name: impl Into<String>) -> TraceSpan<'_> {
        TraceSpan {
            timeline: self,
            kind,
            name: name.into(),
            start: Instant::now(),
            detail: None,
        }
    }

    pub fn events(&self) -> Vec<TraceEvent> {
        self.events.lock().clone()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.events()).unwrap_or_else(|_| "[]".into())
    }

    pub fn wall_clock_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

pub struct TraceSpan<'a> {
    timeline: &'a TraceTimeline,
    kind: TraceKind,
    name: String,
    start: Instant,
    detail: Option<String>,
}

impl TraceSpan<'_> {
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

impl Drop for TraceSpan<'_> {
    fn drop(&mut self) {
        self.timeline.record(
            self.kind,
            self.name.clone(),
            self.start.elapsed(),
            self.detail.clone(),
        );
    }
}
