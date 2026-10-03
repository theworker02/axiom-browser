//! Bridge: OriginStorageAccess → axiom_engine::WebStorageHost

use std::sync::Arc;

use parking_lot::Mutex;

use crate::storage_service::{OriginStorageAccess, ProfileStorage, SessionStorageMap};

impl axiom_engine::WebStorageHost for OriginStorageAccess {
    fn local_get(&self, key: &str) -> Option<String> {
        OriginStorageAccess::local_get(self, key)
    }

    fn local_set(&self, key: &str, value: &str) -> Result<(), String> {
        OriginStorageAccess::local_set(self, key, value).map_err(|e| e.to_string())
    }

    fn local_remove(&self, key: &str) {
        OriginStorageAccess::local_remove(self, key);
    }

    fn local_clear(&self) {
        OriginStorageAccess::local_clear(self);
    }

    fn local_length(&self) -> usize {
        OriginStorageAccess::local_length(self)
    }

    fn local_key(&self, index: usize) -> Option<String> {
        OriginStorageAccess::local_key(self, index)
    }

    fn session_get(&self, key: &str) -> Option<String> {
        OriginStorageAccess::session_get(self, key)
    }

    fn session_set(&self, key: &str, value: &str) -> Result<(), String> {
        OriginStorageAccess::session_set(self, key, value).map_err(|e| e.to_string())
    }

    fn session_remove(&self, key: &str) {
        OriginStorageAccess::session_remove(self, key);
    }

    fn session_clear(&self) {
        OriginStorageAccess::session_clear(self);
    }

    fn session_length(&self) -> usize {
        OriginStorageAccess::session_length(self)
    }

    fn session_key(&self, index: usize) -> Option<String> {
        OriginStorageAccess::session_key(self, index)
    }
}

pub fn make_storage_binder(
    profile: Arc<ProfileStorage>,
    session: Arc<Mutex<SessionStorageMap>>,
) -> axiom_engine::StorageBinder {
    Arc::new(move |url: &str| {
        OriginStorageAccess::new(Arc::clone(&profile), url, Arc::clone(&session))
            .map(|a| Arc::new(a) as Arc<dyn axiom_engine::WebStorageHost>)
    })
}
