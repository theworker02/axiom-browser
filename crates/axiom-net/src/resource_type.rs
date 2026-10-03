#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceType {
    Document,
    Stylesheet,
    Script,
    Image,
    Font,
    Media,
    Manifest,
    Fetch,
    Xhr,
    WebSocket,
    /// A file saved by the download manager (never rendered).
    Download,
    Other,
}

impl ResourceType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Stylesheet => "stylesheet",
            Self::Script => "script",
            Self::Image => "image",
            Self::Font => "font",
            Self::Media => "media",
            Self::Manifest => "manifest",
            Self::Fetch => "fetch",
            Self::Xhr => "xhr",
            Self::WebSocket => "websocket",
            Self::Download => "download",
            Self::Other => "other",
        }
    }

    pub fn default_priority(self) -> crate::priority::RequestPriority {
        use crate::priority::RequestPriority;
        match self {
            Self::Document => RequestPriority::VeryHigh,
            Self::Stylesheet | Self::Script => RequestPriority::High,
            Self::Image | Self::Font | Self::Media => RequestPriority::Medium,
            Self::Manifest | Self::Fetch | Self::Xhr => RequestPriority::Medium,
            Self::WebSocket | Self::Download | Self::Other => RequestPriority::Low,
        }
    }
}
