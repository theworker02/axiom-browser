//! Profile lock — prevent two processes writing the same profile.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

use fs2::FileExt;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LockError {
    #[error("profile already in use by another process")]
    AlreadyLocked,
    #[error("io: {0}")]
    Io(#[from] io::Error),
}

pub struct ProfileLock {
    file: File,
}

impl ProfileLock {
    pub fn try_acquire(path: &Path) -> Result<Self, LockError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        match file.try_lock_exclusive() {
            Ok(()) => {
                log::info!(target: "axiom_persist", "profile lock acquired");
                Ok(Self { file })
            }
            Err(_) => Err(LockError::AlreadyLocked),
        }
    }
}

impl Drop for ProfileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        log::info!(target: "axiom_persist", "profile lock released");
    }
}
