use crate::resource_type::ResourceType;

/// Axiom identifies as itself: no `Mozilla/5.0`, `AppleWebKit`, `Chrome` or `Safari`
/// tokens. Sites that sniff for those serve Axiom their fallback markup, which is what
/// an independent engine should be judged on (see `docs/NETWORKING.md`).
pub const AXIOM_USER_AGENT: &str = "Axiom/0.2 (+https://github.com/theworker02/axiom)";

pub const DEFAULT_ACCEPT_LANGUAGE: &str = "en-US,en;q=0.9";

/// Only advertise codings the service decodes itself (see `body::decoding`).
pub fn accept_encoding() -> &'static str {
    "gzip, deflate, br"
}

pub fn accept_for(resource: ResourceType) -> &'static str {
    match resource {
        ResourceType::Document => "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        ResourceType::Stylesheet => "text/css,*/*;q=0.1",
        ResourceType::Script => "*/*",
        ResourceType::Image => "image/avif,image/webp,image/apng,image/*,*/*;q=0.8",
        ResourceType::Font => "font/woff2,font/woff,font/ttf,*/*;q=0.1",
        ResourceType::Media => "*/*",
        _ => "*/*",
    }
}
