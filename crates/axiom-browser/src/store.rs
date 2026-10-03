//! Browser data store — profile-owned SQLite (or in-memory for private).

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use rusqlite::Connection;
use thiserror::Error;

use crate::bookmarks_repo::BookmarkRepository;
use crate::cookie_repo::CookieRepository;
use crate::cookie_service::CookieService;
use crate::cookie_types::CookieInspectRecord;
use crate::history_repo::HistoryRepository;
use crate::lock::{LockError, ProfileLock};
use crate::paths::ProfilePaths;
use crate::schema::{self, SchemaError, SCHEMA_VERSION};
use crate::session::SessionSnapshot;
use crate::session_repo::SessionRepository;
use crate::settings_repo::{BrowserSettings, SettingsRepository};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Schema(#[from] SchemaError),
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("private profile does not persist: {0}")]
    Private(&'static str),
    #[error("store closed")]
    Closed,
    #[error("corrupt persistence: {0}")]
    Corrupt(String),
}

pub struct BrowserDataStore {
    paths: Option<ProfilePaths>,
    conn: Mutex<Connection>,
    _lock: Option<ProfileLock>,
    private: bool,
    schema_version: i64,
}

impl BrowserDataStore {
    pub fn open_persistent(root: PathBuf) -> Result<Self, StoreError> {
        let paths = ProfilePaths::new(root);
        paths.ensure_layout()?;
        let lock = ProfileLock::try_acquire(&paths.lock_file())?;
        let db_path = paths.database();
        let conn = match Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => {
                quarantine_file(&db_path)?;
                log::error!(target: "axiom_persist", "database open failed, quarantined: {e}");
                return Err(StoreError::Corrupt(e.to_string()));
            }
        };
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        let schema_version = match schema::migrate(&conn) {
            Ok(v) => v,
            Err(SchemaError::NewerUnsupported { found }) => {
                return Err(StoreError::Schema(SchemaError::NewerUnsupported { found }));
            }
            Err(SchemaError::MigrationFailed { from, to, detail }) => {
                // Never silently recreate the user database.
                log::error!(
                    target: "axiom_persist",
                    "migration failed v{from}→v{to}: {detail} (database preserved)"
                );
                return Err(StoreError::Schema(SchemaError::MigrationFailed {
                    from,
                    to,
                    detail,
                }));
            }
            Err(e) => {
                quarantine_file(&db_path)?;
                return Err(StoreError::Corrupt(e.to_string()));
            }
        };
        write_profile_json(&paths)?;
        log::info!(
            target: "axiom_persist",
            "profile opened schema=v{schema_version} path={}",
            paths.root().display()
        );
        let store = Self {
            paths: Some(paths),
            conn: Mutex::new(conn),
            _lock: Some(lock),
            private: false,
            schema_version,
        };
        // Axiom session semantics: session cookies do not survive process restart.
        let _ = CookieService::new().purge_session_cookies(&store);
        Ok(store)
    }

    /// Settings of the persistent profile at `root`, read without taking the profile lock
    /// or writing anything (a private window inherits them). `None` when the profile has
    /// no database yet.
    pub fn read_settings(root: PathBuf) -> Result<Option<BrowserSettings>, StoreError> {
        let db_path = ProfilePaths::new(root).database();
        if !db_path.exists() {
            return Ok(None);
        }
        let conn = Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Ok(Some(SettingsRepository::new(&conn).load()?))
    }

    pub fn open_private() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        let schema_version = schema::migrate(&conn)?;
        log::info!(target: "axiom_persist", "private in-memory profile opened");
        Ok(Self {
            paths: None,
            conn: Mutex::new(conn),
            _lock: None,
            private: true,
            schema_version,
        })
    }

    pub fn is_private(&self) -> bool {
        self.private
    }

    pub fn schema_version(&self) -> i64 {
        self.schema_version
    }

    pub fn paths(&self) -> Option<&ProfilePaths> {
        self.paths.as_ref()
    }

    pub fn with_conn<R>(
        &self,
        f: impl FnOnce(&Connection) -> Result<R, StoreError>,
    ) -> Result<R, StoreError> {
        let guard = self.conn.lock().map_err(|_| StoreError::Closed)?;
        f(&guard)
    }

    pub fn history_count(&self) -> Result<i64, StoreError> {
        self.with_conn(|c| Ok(HistoryRepository::new(c).count()?))
    }

    pub fn bookmark_count(&self) -> Result<i64, StoreError> {
        self.with_conn(|c| Ok(BookmarkRepository::new(c).count()?))
    }

    pub fn record_history(
        &self,
        url: &str,
        title: &str,
        transition: crate::history_repo::VisitTransition,
    ) -> Result<(), StoreError> {
        if self.private {
            // Private: keep in-memory only for the session (still in sqlite memory).
        }
        if should_skip_history_url(url) {
            return Ok(());
        }
        self.with_conn(|c| {
            let tx = c.unchecked_transaction()?;
            HistoryRepository::new(&tx).record_visit(url, title, transition)?;
            tx.commit()?;
            log::debug!(target: "axiom_persist", "history write");
            Ok(())
        })
    }

    pub fn search_history(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::history_repo::HistoryRecord>, StoreError> {
        self.with_conn(|c| Ok(HistoryRepository::new(c).search(query, limit)?))
    }

    pub fn recent_history(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::history_repo::HistoryRecord>, StoreError> {
        self.with_conn(|c| Ok(HistoryRepository::new(c).recent(limit)?))
    }

    pub fn clear_history(&self) -> Result<(), StoreError> {
        self.with_conn(|c| {
            HistoryRepository::new(c).clear()?;
            Ok(())
        })
    }

    pub fn add_bookmark(
        &self,
        url: &str,
        title: &str,
    ) -> Result<crate::bookmarks_repo::Bookmark, StoreError> {
        self.with_conn(|c| {
            let tx = c.unchecked_transaction()?;
            let repo = BookmarkRepository::new(&tx);
            let folder = repo.ensure_root_folder()?;
            let bm = repo.create(url, title, Some(&folder.id))?;
            tx.commit()?;
            log::info!(target: "axiom_persist", "bookmark created");
            Ok(bm)
        })
    }

    pub fn remove_bookmark_url(&self, url: &str) -> Result<(), StoreError> {
        self.with_conn(|c| {
            BookmarkRepository::new(c).delete_url(url)?;
            Ok(())
        })
    }

    pub fn bookmark_for_url(
        &self,
        url: &str,
    ) -> Result<Option<crate::bookmarks_repo::Bookmark>, StoreError> {
        self.with_conn(|c| Ok(BookmarkRepository::new(c).by_url(url)?))
    }

    pub fn search_bookmarks(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::bookmarks_repo::Bookmark>, StoreError> {
        self.with_conn(|c| Ok(BookmarkRepository::new(c).search(query, limit)?))
    }

    pub fn list_bookmarks(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::bookmarks_repo::Bookmark>, StoreError> {
        self.with_conn(|c| Ok(BookmarkRepository::new(c).list(limit)?))
    }

    pub fn clear_bookmarks(&self) -> Result<(), StoreError> {
        self.with_conn(|c| {
            BookmarkRepository::new(c).clear()?;
            Ok(())
        })
    }

    pub fn load_settings(&self) -> Result<BrowserSettings, StoreError> {
        self.with_conn(|c| Ok(SettingsRepository::new(c).load()?))
    }

    pub fn save_settings(&self, settings: &BrowserSettings) -> Result<(), StoreError> {
        self.with_conn(|c| {
            SettingsRepository::new(c).save(settings)?;
            Ok(())
        })
    }

    pub fn checkpoint_session(
        &self,
        snap: &SessionSnapshot,
        clean_shutdown: bool,
    ) -> Result<(), StoreError> {
        if self.private {
            // Private sessions must never restore after profile ends.
            return Ok(());
        }
        self.with_conn(|c| {
            SessionRepository::new(c).save_checkpoint(snap, clean_shutdown)?;
            log::debug!(target: "axiom_persist", "session checkpoint");
            Ok(())
        })?;
        // Also atomic JSON sidecar for recovery tooling.
        if let Some(paths) = &self.paths {
            atomic_write_json(&paths.session_checkpoint(), &snap.to_json())?;
            if clean_shutdown {
                let _ = fs::write(paths.clean_shutdown_marker(), b"1");
            } else {
                let _ = fs::remove_file(paths.clean_shutdown_marker());
            }
        }
        Ok(())
    }

    pub fn load_session(&self) -> Result<Option<(SessionSnapshot, bool)>, StoreError> {
        if self.private {
            return Ok(None);
        }
        self.with_conn(|c| Ok(SessionRepository::new(c).load_checkpoint()?))
    }

    pub fn clear_session_state(&self) -> Result<(), StoreError> {
        self.with_conn(|c| {
            SessionRepository::new(c).clear()?;
            Ok(())
        })?;
        if let Some(paths) = &self.paths {
            let _ = fs::remove_file(paths.session_checkpoint());
            let _ = fs::remove_file(paths.clean_shutdown_marker());
        }
        Ok(())
    }

    // --- Cookies (Wave D) ---

    pub fn cookie_count(&self) -> Result<i64, StoreError> {
        self.with_conn(|c| CookieRepository::new(c).count())
    }

    pub fn clear_cookies(&self) -> Result<(), StoreError> {
        CookieService::new().clear(self)
    }

    pub fn clear_cookies_for_site(&self, host: &str) -> Result<u64, StoreError> {
        CookieService::new().clear_for_site(self, host)
    }

    pub fn list_cookies_inspect(&self) -> Result<Vec<CookieInspectRecord>, StoreError> {
        CookieService::new().list_all_inspect(self)
    }

    pub fn list_cookies_for_site(
        &self,
        host: &str,
    ) -> Result<Vec<CookieInspectRecord>, StoreError> {
        CookieService::new().list_for_site(self, host)
    }

    pub fn delete_cookie(&self, id: &str) -> Result<u64, StoreError> {
        CookieService::new().delete_by_id(self, id)
    }

    // --- Web Storage (Wave E) ---

    pub fn clear_local_storage(&self) -> Result<(), StoreError> {
        crate::storage_service::StorageService::new().clear_all_local(self)
    }

    pub fn clear_local_storage_for_origin(&self, origin: &str) -> Result<u64, StoreError> {
        crate::storage_service::StorageService::new().clear_local_for_origin(self, origin)
    }

    pub fn mark_clean_shutdown(&self) -> Result<(), StoreError> {
        if self.private {
            return Ok(());
        }
        self.with_conn(|c| {
            SessionRepository::new(c).mark_clean_shutdown()?;
            Ok(())
        })?;
        if let Some(paths) = &self.paths {
            let _ = fs::write(paths.clean_shutdown_marker(), b"1");
        }
        Ok(())
    }

    pub fn appears_crashed(&self) -> bool {
        if self.private {
            return false;
        }
        let Some(paths) = &self.paths else {
            return false;
        };
        let has_session = paths.session_checkpoint().exists();
        let clean = paths.clean_shutdown_marker().exists();
        has_session && !clean
    }
}

fn should_skip_history_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("axiom://") || lower.starts_with("about:")
}

fn quarantine_file(path: &std::path::Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let parent = path
        .parent()
        .map(|p| p.join("quarantine"))
        .unwrap_or_else(|| PathBuf::from("quarantine"));
    fs::create_dir_all(&parent)?;
    let dest = parent.join(format!(
        "{}.{}",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("db"),
        crate::time_util::now_ms()
    ));
    fs::rename(path, dest)?;
    Ok(())
}

fn write_profile_json(paths: &ProfilePaths) -> Result<(), StoreError> {
    let meta = serde_json::json!({
        "format": 1,
        "schema_version": SCHEMA_VERSION,
        "created_engine": env!("CARGO_PKG_VERSION"),
    });
    atomic_write_json(&paths.profile_json(), &meta.to_string())?;
    Ok(())
}

/// Write temp → rename for crash safety.
pub fn atomic_write_json(path: &std::path::Path, contents: &str) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // best-effort fsync via reopen
        let f = fs::OpenOptions::new().write(true).open(&tmp)?;
        let _ = f.sync_all();
    }
    fs::rename(&tmp, path)?;
    Ok(())
}
