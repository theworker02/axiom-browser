//! Profile paths — centralized layout; never scatter hardcoded paths.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ProfilePaths {
    pub root: PathBuf,
}

impl ProfilePaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn ensure_layout(&self) -> io::Result<()> {
        for p in [
            self.root.clone(),
            self.history_dir(),
            self.bookmarks_dir(),
            self.sessions_dir(),
            self.settings_dir(),
            self.site_data_dir(),
            self.cache_dir(),
            self.downloads_dir(),
            self.state_dir(),
        ] {
            fs::create_dir_all(p)?;
        }
        Ok(())
    }

    pub fn profile_json(&self) -> PathBuf {
        self.root.join("profile.json")
    }

    pub fn database(&self) -> PathBuf {
        self.root.join("browser.sqlite")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.root.join("profile.lock")
    }

    pub fn history_dir(&self) -> PathBuf {
        self.root.join("history")
    }

    pub fn bookmarks_dir(&self) -> PathBuf {
        self.root.join("bookmarks")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    pub fn settings_dir(&self) -> PathBuf {
        self.root.join("settings")
    }

    pub fn site_data_dir(&self) -> PathBuf {
        self.root.join("site-data")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// Disk-backed HTTP cache (`objects/` + `index/`), owned by the network service.
    pub fn network_cache_dir(&self) -> PathBuf {
        self.root.join("network-cache")
    }

    pub fn downloads_dir(&self) -> PathBuf {
        self.root.join("downloads")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.root.join("state")
    }

    pub fn session_checkpoint(&self) -> PathBuf {
        self.sessions_dir().join("current.json")
    }

    pub fn clean_shutdown_marker(&self) -> PathBuf {
        self.state_dir().join("clean_shutdown")
    }

    pub fn quarantine_dir(&self) -> PathBuf {
        self.root.join("quarantine")
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
