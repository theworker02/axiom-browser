//! Session restore foundations with durable checkpoints (Wave C).

use serde::{Deserialize, Serialize};

use crate::profile::ProfileId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTab {
    pub url: String,
    pub title: String,
    pub pinned: bool,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub profile_id: ProfileId,
    pub tabs: Vec<SessionTab>,
}

impl SessionSnapshot {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    pub fn from_json(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }
}
