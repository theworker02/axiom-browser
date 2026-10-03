//! Strongly typed cookie domain model (Wave D).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable row identity (not the cookie replacement key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CookieId(pub Uuid);

impl CookieId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for CookieId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CookieSource {
    Network,
    Script,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CookieSameSite {
    Strict,
    Lax,
    None,
}

impl CookieSameSite {
    pub fn parse_attr(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "strict" => Some(Self::Strict),
            "lax" => Some(Self::Lax),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Lax => "lax",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CookiePriority {
    Low,
    #[default]
    Medium,
    High,
}

/// Expiration: session cookie or absolute UTC ms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CookieExpiration {
    Session,
    Absolute(i64),
}

impl CookieExpiration {
    pub fn is_expired(&self, now_ms: i64) -> bool {
        match self {
            Self::Session => false,
            Self::Absolute(t) => *t <= now_ms,
        }
    }

    pub fn absolute_ms(self) -> Option<i64> {
        match self {
            Self::Session => None,
            Self::Absolute(t) => Some(t),
        }
    }
}

/// How the cookie is being accessed (network vs script + SameSite context).
#[derive(Debug, Clone)]
pub struct CookieAccessContext {
    pub request_url: String,
    pub top_level_site: Site,
    pub initiator_site: Option<Site>,
    pub navigation: NavigationKind,
    pub method: HttpMethodKind,
    pub secure_context: bool,
    pub source: CookieSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationKind {
    /// Top-level document navigation.
    TopLevel,
    /// Subresource / iframe / fetch-like.
    Subresource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethodKind {
    SafeGet,
    Unsafe,
}

impl HttpMethodKind {
    pub fn from_method(m: &str) -> Self {
        match m.trim().to_ascii_uppercase().as_str() {
            "GET" | "HEAD" | "OPTIONS" | "TRACE" => Self::SafeGet,
            _ => Self::Unsafe,
        }
    }
}

/// Registrable-site oriented identity (schemeful-site foundation).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Site {
    pub scheme: String,
    pub host: String,
}

impl Site {
    pub fn from_url_str(url: &str) -> Option<Self> {
        let u = axiom_url::Url::parse(url).ok()?;
        Some(Self::from_url(&u))
    }

    pub fn from_url(url: &axiom_url::Url) -> Self {
        Self {
            scheme: url.scheme.clone(),
            host: url.host.to_ascii_lowercase(),
        }
    }

    /// Same-site for Wave D: same scheme + same registrable host heuristic.
    pub fn is_same_site(&self, other: &Site, etld1: impl Fn(&str) -> String) -> bool {
        if self.scheme != other.scheme {
            return false;
        }
        etld1(&self.host) == etld1(&other.host)
    }
}

#[derive(Debug, Clone)]
pub struct Cookie {
    pub id: CookieId,
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub creation_time_ms: i64,
    pub last_access_time_ms: i64,
    pub expiration: CookieExpiration,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: CookieSameSite,
    pub host_only: bool,
    pub persistent: bool,
    pub priority: CookiePriority,
    pub source: CookieSource,
}

impl Cookie {
    /// Replacement identity: name + domain + path (RFC-oriented).
    pub fn identity_key(&self) -> CookieIdentity {
        CookieIdentity {
            name: self.name.clone(),
            domain: self.domain.to_ascii_lowercase(),
            path: self.path.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CookieIdentity {
    pub name: String,
    pub domain: String,
    pub path: String,
}

/// Internal metadata for trusted UI / DevTools (never for webpage JS).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieInspectRecord {
    pub id: String,
    pub name: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: String,
    pub host_only: bool,
    pub session: bool,
    pub expires_at_ms: Option<i64>,
    pub creation_time_ms: i64,
    pub last_access_time_ms: i64,
    /// Value omitted by default for trusted UI unless explicitly requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

impl From<&Cookie> for CookieInspectRecord {
    fn from(c: &Cookie) -> Self {
        Self {
            id: c.id.0.to_string(),
            name: c.name.clone(),
            domain: c.domain.clone(),
            path: c.path.clone(),
            secure: c.secure,
            http_only: c.http_only,
            same_site: c.same_site.as_str().to_string(),
            host_only: c.host_only,
            session: matches!(c.expiration, CookieExpiration::Session),
            expires_at_ms: c.expiration.absolute_ms(),
            creation_time_ms: c.creation_time_ms,
            last_access_time_ms: c.last_access_time_ms,
            value: None,
        }
    }
}
