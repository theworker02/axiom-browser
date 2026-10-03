//! First-class browser profiles and profile manager.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::paths::ProfilePaths;
use crate::store::{BrowserDataStore, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProfileId(pub Uuid);

impl ProfileId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ProfileId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ProfileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    Persistent,
    Private,
}

/// Back-compat note: Wave A used `Normal`; Wave C uses `Persistent`.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileMetadata {
    pub id: ProfileId,
    pub name: String,
    pub kind: ProfileKind,
    pub created_at_ms: i64,
}

pub struct Profile {
    pub meta: ProfileMetadata,
    pub paths: Option<ProfilePaths>,
    pub store: Arc<BrowserDataStore>,
}

impl Profile {
    pub fn id(&self) -> ProfileId {
        self.meta.id
    }

    pub fn kind(&self) -> ProfileKind {
        self.meta.kind
    }

    pub fn is_private(&self) -> bool {
        self.meta.kind == ProfileKind::Private
    }

    pub fn persists(&self) -> bool {
        self.meta.kind == ProfileKind::Persistent
    }

    pub fn name(&self) -> &str {
        &self.meta.name
    }

    /// Wave A compatibility: Normal → Persistent.
    pub fn normal(name: impl Into<String>, data_dir: PathBuf) -> Result<Self, StoreError> {
        Self::open_persistent(name, data_dir)
    }

    pub fn private(name: impl Into<String>) -> Result<Self, StoreError> {
        let store = Arc::new(BrowserDataStore::open_private()?);
        Ok(Self {
            meta: ProfileMetadata {
                id: ProfileId::new(),
                name: name.into(),
                kind: ProfileKind::Private,
                created_at_ms: crate::time_util::now_ms(),
            },
            paths: None,
            store,
        })
    }

    pub fn open_persistent(name: impl Into<String>, data_dir: PathBuf) -> Result<Self, StoreError> {
        let paths = ProfilePaths::new(data_dir);
        paths.ensure_layout().map_err(StoreError::Io)?;
        let id = load_or_create_id(&paths)?;
        let store = Arc::new(BrowserDataStore::open_persistent(paths.root.clone())?);
        let meta = ProfileMetadata {
            id,
            name: name.into(),
            kind: ProfileKind::Persistent,
            created_at_ms: crate::time_util::now_ms(),
        };
        save_metadata(&paths, &meta)?;
        Ok(Self {
            meta,
            paths: Some(paths),
            store,
        })
    }
}

fn load_or_create_id(paths: &ProfilePaths) -> Result<ProfileId, StoreError> {
    let meta_path = paths.root.join("profile_id.json");
    if meta_path.exists() {
        if let Ok(s) = fs::read_to_string(&meta_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
                    if let Ok(u) = Uuid::parse_str(id) {
                        return Ok(ProfileId(u));
                    }
                }
            }
        }
    }
    let id = ProfileId::new();
    let body = serde_json::json!({ "id": id.0.to_string() }).to_string();
    crate::store::atomic_write_json(&meta_path, &body)?;
    Ok(id)
}

fn save_metadata(paths: &ProfilePaths, meta: &ProfileMetadata) -> Result<(), StoreError> {
    let body = serde_json::to_string_pretty(meta).unwrap_or_else(|_| "{}".into());
    crate::store::atomic_write_json(&paths.root.join("profile_meta.json"), &body)?;
    Ok(())
}

pub struct ProfileManager {
    pub user_data_root: PathBuf,
}

impl ProfileManager {
    pub fn new(user_data_root: impl Into<PathBuf>) -> Self {
        Self {
            user_data_root: user_data_root.into(),
        }
    }

    pub fn profile_dir(&self, name: &str) -> PathBuf {
        self.user_data_root.join("profiles").join(sanitize(name))
    }

    pub fn open_or_create(&self, name: &str) -> Result<Profile, StoreError> {
        let dir = self.profile_dir(name);
        fs::create_dir_all(&dir).map_err(StoreError::Io)?;
        Profile::open_persistent(name, dir)
    }

    pub fn open_private(&self, name: &str) -> Result<Profile, StoreError> {
        Profile::private(name)
    }
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
