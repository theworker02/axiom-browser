#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpProtocol {
    Http11,
    Http2,
    /// Reserved — not implemented in Wave F.
    Http3,
    Unknown,
}

impl HttpProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http11 => "http/1.1",
            Self::Http2 => "h2",
            Self::Http3 => "h3",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_alpn(s: &str) -> Self {
        match s {
            "h2" | "HTTP/2.0" | "http/2" => Self::Http2,
            "http/1.1" | "HTTP/1.1" => Self::Http11,
            "h3" => Self::Http3,
            _ => Self::Unknown,
        }
    }
}
