//! Web Storage host bridge (implemented by browser ProfileStorage).

/// localStorage / sessionStorage access for the current document origin.
pub trait WebStorageHost: Send + Sync {
    fn local_get(&self, key: &str) -> Option<String>;
    fn local_set(&self, key: &str, value: &str) -> Result<(), String>;
    fn local_remove(&self, key: &str);
    fn local_clear(&self);
    fn local_length(&self) -> usize;
    fn local_key(&self, index: usize) -> Option<String>;

    fn session_get(&self, key: &str) -> Option<String>;
    fn session_set(&self, key: &str, value: &str) -> Result<(), String>;
    fn session_remove(&self, key: &str);
    fn session_clear(&self);
    fn session_length(&self) -> usize;
    fn session_key(&self, index: usize) -> Option<String>;
}

/// Creates an origin-scoped [`WebStorageHost`] for a document URL.
pub type StorageBinder =
    std::sync::Arc<dyn Fn(&str) -> Option<std::sync::Arc<dyn WebStorageHost>> + Send + Sync>;
