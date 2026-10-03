//! Request priority per resource role.

use axiom_net::{RequestPriority, ResourceType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorityHint {
    Normal,
    /// Parser-blocking script or script-blocking stylesheet.
    Blocking,
    Deferred,
    Async,
    /// Script-inserted script.
    Dynamic,
    /// `loading=lazy` image.
    Lazy,
}

/// Document: very high; blocking scripts and stylesheets: high; images and fonts:
/// medium ("normal"); deferred scripts: medium; async and script-inserted scripts: low;
/// lazy images: low.
pub fn priority_for(kind: ResourceType, hint: PriorityHint) -> RequestPriority {
    match (kind, hint) {
        (ResourceType::Document, _) => RequestPriority::VeryHigh,
        (_, PriorityHint::Lazy) => RequestPriority::Low,
        (ResourceType::Script, PriorityHint::Async | PriorityHint::Dynamic) => RequestPriority::Low,
        (ResourceType::Script, PriorityHint::Deferred) => RequestPriority::Medium,
        (ResourceType::Script | ResourceType::Stylesheet, _) => RequestPriority::High,
        (ResourceType::Image | ResourceType::Font, _) => RequestPriority::Medium,
        (other, _) => other.default_priority(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priorities_follow_resource_roles() {
        use PriorityHint::*;
        assert_eq!(
            priority_for(ResourceType::Document, Normal),
            RequestPriority::VeryHigh
        );
        assert_eq!(
            priority_for(ResourceType::Script, Blocking),
            RequestPriority::High
        );
        assert_eq!(
            priority_for(ResourceType::Stylesheet, Blocking),
            RequestPriority::High
        );
        assert_eq!(
            priority_for(ResourceType::Script, Deferred),
            RequestPriority::Medium
        );
        assert_eq!(
            priority_for(ResourceType::Script, Async),
            RequestPriority::Low
        );
        assert_eq!(
            priority_for(ResourceType::Image, Normal),
            RequestPriority::Medium
        );
        assert_eq!(
            priority_for(ResourceType::Font, Normal),
            RequestPriority::Medium
        );
        assert_eq!(
            priority_for(ResourceType::Image, Lazy),
            RequestPriority::Low
        );
    }
}
