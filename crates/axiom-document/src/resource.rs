//! One subresource of one document.

use std::time::{Duration, Instant};

use axiom_dom::NodeId;
use axiom_net::{CacheState, NetworkRequestId, RequestPriority, ResourceType};

use crate::error::ResourceError;
use crate::ids::{DocumentId, ResourceId};
use crate::script::ScriptKind;

/// Resource lifecycle. Transitions are validated by [`ResourceState::can_transition_to`];
/// the three terminal states never change again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceState {
    /// Found by the parser, preload scanner, a stylesheet or script; no request yet.
    Discovered,
    /// Handed to the resource loader (waiting in the profile scheduler).
    Queued,
    /// Response headers received; body arriving.
    Fetching,
    /// The complete body is available.
    Available,
    /// Being parsed, decoded, executed or waiting for dependent resources (`@import`).
    Processing,
    Ready,
    Failed,
    Canceled,
}

impl ResourceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "DISCOVERED",
            Self::Queued => "QUEUED",
            Self::Fetching => "FETCHING",
            Self::Available => "AVAILABLE",
            Self::Processing => "PROCESSING",
            Self::Ready => "READY",
            Self::Failed => "FAILED",
            Self::Canceled => "CANCELED",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Ready | Self::Failed | Self::Canceled)
    }

    pub fn is_pending(self) -> bool {
        !self.is_terminal()
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        use ResourceState::*;
        if matches!(next, Failed | Canceled) {
            return !self.is_terminal();
        }
        matches!(
            (self, next),
            (Discovered, Queued)
                | (Discovered, Available)
                | (Queued, Fetching)
                | (Queued, Available)
                | (Fetching, Available)
                | (Available, Processing)
                | (Available, Ready)
                | (Processing, Ready)
        )
    }
}

/// What caused the resource to be requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Initiator {
    Navigation,
    Parser,
    PreloadScanner,
    /// Inserted or changed by script (dynamic insertion).
    Script,
    /// Referenced by a stylesheet (`@import`, `@font-face`).
    Stylesheet(ResourceId),
}

impl Initiator {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Navigation => "navigation",
            Self::Parser => "parser",
            Self::PreloadScanner => "preload-scanner",
            Self::Script => "script",
            Self::Stylesheet(_) => "stylesheet",
        }
    }
}

/// What the resource holds up while pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Blocking {
    /// Script-blocking stylesheet or parser-blocking script.
    pub script: bool,
    /// Rendering waits for it (parser-inserted stylesheets).
    pub render: bool,
    /// The `load` event waits for it.
    pub load: bool,
}

/// HTML "CORS settings attribute" (`crossorigin`), or the fixed mode of a resource type
/// that is always fetched with CORS (module scripts, fonts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CorsSettings {
    /// `cors` mode, credentials only to the document's own origin.
    Anonymous,
    /// `cors` mode, credentials always (`crossorigin="use-credentials"`).
    UseCredentials,
}

impl CorsSettings {
    /// The state of a `crossorigin` attribute value: absent means no CORS; any value
    /// other than `use-credentials` (including an empty or invalid one) is anonymous.
    pub fn from_attribute(value: Option<&str>) -> Option<Self> {
        value.map(|v| {
            if v.trim().eq_ignore_ascii_case("use-credentials") {
                Self::UseCredentials
            } else {
                Self::Anonymous
            }
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anonymous => "anonymous",
            Self::UseCredentials => "use-credentials",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    pub decoded: bool,
}

#[derive(Debug, Clone)]
pub struct ResourceRecord {
    pub id: ResourceId,
    pub document: DocumentId,
    pub request: Option<NetworkRequestId>,
    /// Resolved URL as requested.
    pub url: String,
    /// Final URL after redirects (when a response arrived).
    pub final_url: Option<String>,
    pub kind: ResourceType,
    pub initiator: Initiator,
    /// The element that references it (`None` for `@import`/fonts and unclaimed preloads).
    pub node: Option<NodeId>,
    /// Byte offset of the referencing markup in the HTML stream, when known.
    pub source_offset: Option<usize>,
    pub state: ResourceState,
    pub priority: RequestPriority,
    pub blocking: Blocking,
    pub script_kind: Option<ScriptKind>,
    /// `loading=lazy` image.
    pub lazy: bool,
    /// Fetched in `cors` mode (`None`: `no-cors`).
    pub cors: Option<CorsSettings>,
    /// Started by the preload scanner and later adopted by its element.
    pub claimed: bool,
    /// Loaded over plain HTTP by a secure document (passive mixed content).
    pub mixed_content: bool,
    pub status: Option<u16>,
    pub mime: Option<String>,
    pub cache: Option<CacheState>,
    pub transferred_bytes: u64,
    pub decoded_bytes: u64,
    pub discovered_at: Instant,
    pub queued_at: Option<Instant>,
    pub response_at: Option<Instant>,
    pub finished_at: Option<Instant>,
    pub failure: Option<ResourceError>,
    pub image: Option<ImageInfo>,
    /// `@font-face` family a font resource serves.
    pub font_family: Option<String>,
}

impl ResourceRecord {
    pub fn is_pending(&self) -> bool {
        self.state.is_pending()
    }

    /// From queueing (or discovery) to the terminal state.
    pub fn duration(&self) -> Option<Duration> {
        let start = self.queued_at.unwrap_or(self.discovered_at);
        self.finished_at
            .map(|end| end.saturating_duration_since(start))
    }
}

#[cfg(test)]
mod tests {
    use super::ResourceState::*;

    #[test]
    fn crossorigin_attribute_states() {
        use super::CorsSettings;
        assert_eq!(CorsSettings::from_attribute(None), None);
        assert_eq!(
            CorsSettings::from_attribute(Some("")),
            Some(CorsSettings::Anonymous)
        );
        assert_eq!(
            CorsSettings::from_attribute(Some("bogus")),
            Some(CorsSettings::Anonymous)
        );
        assert_eq!(
            CorsSettings::from_attribute(Some(" Use-Credentials ")),
            Some(CorsSettings::UseCredentials)
        );
    }

    #[test]
    fn state_machine_rejects_illegal_transitions() {
        assert!(Discovered.can_transition_to(Queued));
        assert!(Queued.can_transition_to(Fetching));
        assert!(Fetching.can_transition_to(Available));
        assert!(Available.can_transition_to(Processing));
        assert!(Processing.can_transition_to(Ready));
        assert!(Fetching.can_transition_to(Canceled));
        assert!(!Ready.can_transition_to(Failed));
        assert!(!Canceled.can_transition_to(Queued));
        assert!(!Fetching.can_transition_to(Queued));
        assert!(!Discovered.can_transition_to(Ready));
        assert!(Canceled.is_terminal() && !Processing.is_terminal());
    }
}
