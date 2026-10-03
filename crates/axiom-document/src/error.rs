//! Error classes for document and subresource failures.

use std::fmt;

use axiom_net::ResourceType;

/// Which part of the page a failure belongs to. Only `FatalDocument` replaces the page
/// (with the trusted error page); every other class leaves the document running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorClass {
    FatalDocument,
    Subresource,
    Script,
    Stylesheet,
    Image,
    Font,
    PolicyRejection,
}

impl ErrorClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FatalDocument => "fatal-document",
            Self::Subresource => "subresource",
            Self::Script => "script",
            Self::Stylesheet => "stylesheet",
            Self::Image => "image",
            Self::Font => "font",
            Self::PolicyRejection => "policy-rejection",
        }
    }

    pub fn for_kind(kind: ResourceType) -> Self {
        match kind {
            ResourceType::Document => Self::FatalDocument,
            ResourceType::Script => Self::Script,
            ResourceType::Stylesheet => Self::Stylesheet,
            ResourceType::Image => Self::Image,
            ResourceType::Font => Self::Font,
            _ => Self::Subresource,
        }
    }
}

/// Where in the pipeline the failure happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureStage {
    /// Rejected before any request (content policy, mixed content, internal page).
    Policy,
    /// Rejected by a memory or count limit.
    Limit,
    /// Invalid or unsupported URL.
    Url,
    Network,
    HttpStatus,
    Mime,
    Decode,
    Execution,
    Canceled,
}

impl FailureStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Policy => "policy",
            Self::Limit => "limit",
            Self::Url => "url",
            Self::Network => "network",
            Self::HttpStatus => "http-status",
            Self::Mime => "mime",
            Self::Decode => "decode",
            Self::Execution => "execution",
            Self::Canceled => "canceled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceError {
    pub class: ErrorClass,
    pub stage: FailureStage,
    pub message: String,
}

impl ResourceError {
    pub fn new(class: ErrorClass, stage: FailureStage, message: impl Into<String>) -> Self {
        Self {
            class,
            stage,
            message: message.into(),
        }
    }

    /// A failure of a resource of type `kind` (class derived from the type).
    pub fn of(kind: ResourceType, stage: FailureStage, message: impl Into<String>) -> Self {
        Self::new(ErrorClass::for_kind(kind), stage, message)
    }

    pub fn policy(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::PolicyRejection, FailureStage::Policy, message)
    }
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} failure at {}: {}",
            self.class.as_str(),
            self.stage.as_str(),
            self.message
        )
    }
}
