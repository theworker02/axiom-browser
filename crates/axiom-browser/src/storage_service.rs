//! StorageService — localStorage (durable) + sessionStorage helpers (Wave E).

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::storage_repo::StorageRepository;
use crate::storage_types::{StorageError, StorageOrigin, StorageQuota};
use crate::store::{BrowserDataStore, StoreError};
use crate::time_util;

/// In-memory sessionStorage for one browsing context (never durable).
#[derive(Debug, Default)]
pub struct SessionStorageMap {
    /// origin → (key → value)
    map: HashMap<String, HashMap<String, String>>,
}

impl SessionStorageMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, origin: &str, key: &str) -> Option<String> {
        self.map.get(origin)?.get(key).cloned()
    }

    pub fn set(
        &mut self,
        origin: &str,
        key: &str,
        value: &str,
        quota: &StorageQuota,
    ) -> Result<(), StorageError> {
        check_item_limits(key, value, quota)?;
        let entry = self.map.entry(origin.to_string()).or_default();
        let old_len = entry.get(key).map(|v| key.len() + v.len()).unwrap_or(0);
        let new_len = key.len() + value.len();
        let current: usize = entry.iter().map(|(k, v)| k.len() + v.len()).sum();
        let next = current - old_len + new_len;
        if next > quota.max_bytes_per_origin {
            return Err(StorageError::QuotaExceeded);
        }
        entry.insert(key.to_string(), value.to_string());
        Ok(())
    }

    pub fn remove(&mut self, origin: &str, key: &str) {
        if let Some(m) = self.map.get_mut(origin) {
            m.remove(key);
        }
    }

    pub fn clear(&mut self, origin: &str) {
        self.map.remove(origin);
    }

    pub fn length(&self, origin: &str) -> usize {
        self.map.get(origin).map(|m| m.len()).unwrap_or(0)
    }

    pub fn key_at(&self, origin: &str, index: usize) -> Option<String> {
        let mut keys: Vec<_> = self.map.get(origin)?.keys().cloned().collect();
        keys.sort();
        keys.into_iter().nth(index)
    }
}

pub struct StorageService {
    quota: StorageQuota,
}

impl Default for StorageService {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageService {
    pub fn new() -> Self {
        Self {
            quota: StorageQuota::default(),
        }
    }

    pub fn with_quota(quota: StorageQuota) -> Self {
        Self { quota }
    }

    pub fn quota(&self) -> &StorageQuota {
        &self.quota
    }

    pub fn local_get(
        &self,
        store: &BrowserDataStore,
        origin: &StorageOrigin,
        key: &str,
    ) -> Result<Option<String>, StoreError> {
        store.with_conn(|c| StorageRepository::new(c).get(origin.as_str(), key))
    }

    pub fn local_set(
        &self,
        store: &BrowserDataStore,
        origin: &StorageOrigin,
        key: &str,
        value: &str,
    ) -> Result<(), StorageError> {
        check_item_limits(key, value, &self.quota)?;
        let now = time_util::now_ms();
        let (old_len, current) = store
            .with_conn(|c| {
                let repo = StorageRepository::new(c);
                let old = repo.get(origin.as_str(), key)?;
                let old_len = old.as_ref().map(|v| key.len() + v.len()).unwrap_or(0);
                let current = repo.byte_size(origin.as_str())?;
                Ok((old_len, current))
            })
            .map_err(|_| StorageError::Denied)?;
        let next = current.saturating_sub(old_len) + key.len() + value.len();
        if next > self.quota.max_bytes_per_origin {
            return Err(StorageError::QuotaExceeded);
        }
        store
            .with_conn(|c| {
                StorageRepository::new(c).set(origin.as_str(), key, value, now)?;
                log::info!(
                    target: "axiom_storage",
                    "localStorage set origin_len={} key_len={}",
                    origin.as_str().len(),
                    key.len()
                );
                Ok(())
            })
            .map_err(|_| StorageError::Denied)
    }

    pub fn local_remove(
        &self,
        store: &BrowserDataStore,
        origin: &StorageOrigin,
        key: &str,
    ) -> Result<(), StoreError> {
        store.with_conn(|c| StorageRepository::new(c).remove(origin.as_str(), key))
    }

    pub fn local_clear(
        &self,
        store: &BrowserDataStore,
        origin: &StorageOrigin,
    ) -> Result<u64, StoreError> {
        store.with_conn(|c| StorageRepository::new(c).clear_origin(origin.as_str()))
    }

    pub fn local_length(
        &self,
        store: &BrowserDataStore,
        origin: &StorageOrigin,
    ) -> Result<usize, StoreError> {
        store.with_conn(|c| StorageRepository::new(c).length(origin.as_str()))
    }

    pub fn local_key(
        &self,
        store: &BrowserDataStore,
        origin: &StorageOrigin,
        index: usize,
    ) -> Result<Option<String>, StoreError> {
        store.with_conn(|c| StorageRepository::new(c).key_at(origin.as_str(), index))
    }

    pub fn clear_all_local(&self, store: &BrowserDataStore) -> Result<(), StoreError> {
        store.with_conn(|c| StorageRepository::new(c).clear_all())
    }

    pub fn clear_local_for_origin(
        &self,
        store: &BrowserDataStore,
        origin: &str,
    ) -> Result<u64, StoreError> {
        store.with_conn(|c| StorageRepository::new(c).clear_origin(origin))
    }
}

fn check_item_limits(key: &str, value: &str, quota: &StorageQuota) -> Result<(), StorageError> {
    if key.len() > quota.max_key_bytes || value.len() > quota.max_value_bytes {
        return Err(StorageError::QuotaExceeded);
    }
    Ok(())
}

/// Shared localStorage handle + per-context session map factory.
pub struct ProfileStorage {
    store: std::sync::Weak<BrowserDataStore>,
    service: StorageService,
}

impl ProfileStorage {
    pub fn new(store: Arc<BrowserDataStore>) -> Arc<Self> {
        Arc::new(Self {
            store: Arc::downgrade(&store),
            service: StorageService::new(),
        })
    }

    fn upgrade(&self) -> Option<Arc<BrowserDataStore>> {
        self.store.upgrade()
    }

    pub fn service(&self) -> &StorageService {
        &self.service
    }

    pub fn new_session_map(&self) -> Arc<Mutex<SessionStorageMap>> {
        Arc::new(Mutex::new(SessionStorageMap::new()))
    }
}

/// Origin-scoped facade used by engine/JS for one document.
pub struct OriginStorageAccess {
    profile: Arc<ProfileStorage>,
    origin: StorageOrigin,
    session: Arc<Mutex<SessionStorageMap>>,
}

impl OriginStorageAccess {
    pub fn new(
        profile: Arc<ProfileStorage>,
        document_url: &str,
        session: Arc<Mutex<SessionStorageMap>>,
    ) -> Option<Self> {
        let origin = StorageOrigin::from_url_str(document_url)?;
        // Opaque / non-http(s) storage is denied for remote-like schemes we don't support.
        if !(origin.as_str().starts_with("http://") || origin.as_str().starts_with("https://")) {
            return None;
        }
        Some(Self {
            profile,
            origin,
            session,
        })
    }

    pub fn local_get(&self, key: &str) -> Option<String> {
        let store = self.profile.upgrade()?;
        self.profile
            .service
            .local_get(&store, &self.origin, key)
            .ok()
            .flatten()
    }

    pub fn local_set(&self, key: &str, value: &str) -> Result<(), StorageError> {
        let store = self.profile.upgrade().ok_or(StorageError::Denied)?;
        self.profile
            .service
            .local_set(&store, &self.origin, key, value)
    }

    pub fn local_remove(&self, key: &str) {
        if let Some(store) = self.profile.upgrade() {
            let _ = self.profile.service.local_remove(&store, &self.origin, key);
        }
    }

    pub fn local_clear(&self) {
        if let Some(store) = self.profile.upgrade() {
            let _ = self.profile.service.local_clear(&store, &self.origin);
        }
    }

    pub fn local_length(&self) -> usize {
        let Some(store) = self.profile.upgrade() else {
            return 0;
        };
        self.profile
            .service
            .local_length(&store, &self.origin)
            .unwrap_or(0)
    }

    pub fn local_key(&self, index: usize) -> Option<String> {
        let store = self.profile.upgrade()?;
        self.profile
            .service
            .local_key(&store, &self.origin, index)
            .ok()
            .flatten()
    }

    pub fn session_get(&self, key: &str) -> Option<String> {
        self.session.lock().get(self.origin.as_str(), key)
    }

    pub fn session_set(&self, key: &str, value: &str) -> Result<(), StorageError> {
        self.session.lock().set(
            self.origin.as_str(),
            key,
            value,
            self.profile.service.quota(),
        )
    }

    pub fn session_remove(&self, key: &str) {
        self.session.lock().remove(self.origin.as_str(), key);
    }

    pub fn session_clear(&self) {
        self.session.lock().clear(self.origin.as_str());
    }

    pub fn session_length(&self) -> usize {
        self.session.lock().length(self.origin.as_str())
    }

    pub fn session_key(&self, index: usize) -> Option<String> {
        self.session.lock().key_at(self.origin.as_str(), index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::BrowserDataStore;

    #[test]
    fn local_roundtrip_and_quota() {
        let store = BrowserDataStore::open_private().unwrap();
        let svc = StorageService::with_quota(StorageQuota {
            max_bytes_per_origin: 32,
            max_key_bytes: 16,
            max_value_bytes: 32,
        });
        let origin = StorageOrigin::from_url_str("https://example.test/").unwrap();
        svc.local_set(&store, &origin, "a", "hello").unwrap();
        assert_eq!(
            svc.local_get(&store, &origin, "a").unwrap().as_deref(),
            Some("hello")
        );
        let big = "x".repeat(40);
        assert_eq!(
            svc.local_set(&store, &origin, "b", &big),
            Err(StorageError::QuotaExceeded)
        );
    }

    #[test]
    fn session_isolated_per_map() {
        let q = StorageQuota::default();
        let mut a = SessionStorageMap::new();
        let b = SessionStorageMap::new();
        a.set("https://example.test", "k", "1", &q).unwrap();
        assert!(b.get("https://example.test", "k").is_none());
        assert_eq!(a.get("https://example.test", "k").as_deref(), Some("1"));
    }
}
