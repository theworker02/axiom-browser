//! Web Storage domain types (Wave E).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    Local,
    Session,
}

/// Serialized security origin key: `scheme://host:port` (or opaque).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StorageOrigin(pub String);

impl StorageOrigin {
    pub fn from_url_str(url: &str) -> Option<Self> {
        let u = axiom_url::Url::parse(url).ok()?;
        Some(Self::from_url(&u))
    }

    pub fn from_url(url: &axiom_url::Url) -> Self {
        let port = url.effective_port();
        let default = match url.scheme.as_str() {
            "https" => 443,
            "http" => 80,
            _ => port,
        };
        let s = if port == default {
            format!("{}://{}", url.scheme, url.host)
        } else {
            format!("{}://{}:{}", url.scheme, url.host, port)
        };
        Self(s)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageEntry {
    pub origin: String,
    pub key: String,
    pub value: String,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct StorageQuota {
    /// Max UTF-16-ish byte budget per origin (key+value lengths summed as UTF-8 bytes).
    pub max_bytes_per_origin: usize,
    pub max_key_bytes: usize,
    pub max_value_bytes: usize,
}

impl Default for StorageQuota {
    fn default() -> Self {
        Self {
            max_bytes_per_origin: 5 * 1024 * 1024,
            max_key_bytes: 1024 * 1024,
            max_value_bytes: 5 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageError {
    QuotaExceeded,
    InvalidOrigin,
    Denied,
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QuotaExceeded => write!(f, "QuotaExceededError"),
            Self::InvalidOrigin => write!(f, "invalid_origin"),
            Self::Denied => write!(f, "security_error"),
        }
    }
}
