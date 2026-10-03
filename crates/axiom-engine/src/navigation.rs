//! Navigation lifecycle as reported to the embedder (the browser layer).
//!
//! A [`BrowsingContext`](crate::BrowsingContext) records one [`NavigationEvent`] per
//! transition. The browser drains them with `take_navigation_events` to update history,
//! the omnibox and the session, whether the navigation was started by the browser, a link
//! click, reload or back/forward, and whether it ran blocking or in the background.

use std::collections::VecDeque;

use axiom_document::NavigationId;

/// Events kept when the embedder does not drain them (oldest are dropped first).
const MAX_EVENTS: usize = 256;

/// Why a navigation started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationCause {
    /// The embedder called `navigate` / `navigate_to` / `start_navigation`.
    Embedder,
    /// The user activated a link in the page.
    Link,
    /// A form in the page was submitted.
    FormSubmission,
    Reload,
    /// Back or forward through session history.
    History,
    /// Script navigated the document (`location.assign`, `location.href = …`).
    Script,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationEventKind {
    /// The request was issued (or a local document is being read).
    Started,
    /// A response was accepted and a new document replaced the old one. `url` is the final
    /// URL after redirects.
    Committed { redirected: bool },
    /// `DOMContentLoaded` fired: the document is parsed and its title is known.
    Interactive,
    /// The `load` event fired.
    Completed,
    /// The navigation failed; a trusted error page is shown. `kind` is the stable error
    /// kind name (`dns`, `certificate`, `redirect_loop`, …).
    Failed { kind: &'static str },
    /// Stopped by the user or superseded by a newer navigation.
    Cancelled,
    /// The response is a download; the document was not replaced.
    Download,
    /// A link targeted an internal `axiom:` page. The context never loads these itself;
    /// the embedder decides whether `initiator` may open it.
    InternalRequested { initiator: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationEvent {
    pub id: NavigationId,
    pub url: String,
    pub cause: NavigationCause,
    pub kind: NavigationEventKind,
}

impl NavigationEvent {
    /// No further events follow for this navigation id.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.kind,
            NavigationEventKind::Completed
                | NavigationEventKind::Failed { .. }
                | NavigationEventKind::Cancelled
                | NavigationEventKind::Download
                | NavigationEventKind::InternalRequested { .. }
        )
    }
}

/// Where the navigation currently shown or in flight stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationState {
    /// No navigation has run in this context.
    Idle,
    /// Request issued, waiting for response headers (DNS, connect, TLS and the server's
    /// think time are not distinguished by the transport).
    Connecting,
    /// Committed; the body is still streaming into the parser.
    Receiving,
    /// Body complete; parsing or parser-blocking scripts still running.
    Parsing,
    /// `DOMContentLoaded` fired; subresources may still be loading.
    Interactive,
    /// `load` fired.
    Completed,
    Failed,
    Cancelled,
}

impl NavigationState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Connecting => "connecting",
            Self::Receiving => "receiving",
            Self::Parsing => "parsing",
            Self::Interactive => "interactive",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct NavigationEvents {
    queue: VecDeque<NavigationEvent>,
    dropped: u64,
}

impl NavigationEvents {
    pub(crate) fn push(
        &mut self,
        id: NavigationId,
        url: &str,
        cause: NavigationCause,
        kind: NavigationEventKind,
    ) {
        if self.queue.len() == MAX_EVENTS {
            self.queue.pop_front();
            self.dropped += 1;
        }
        self.queue.push_back(NavigationEvent {
            id,
            url: url.to_string(),
            cause,
            kind,
        });
    }

    pub(crate) fn take(&mut self) -> Vec<NavigationEvent> {
        self.queue.drain(..).collect()
    }

    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }
}
