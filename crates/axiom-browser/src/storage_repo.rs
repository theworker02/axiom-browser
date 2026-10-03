//! localStorage repository — profile SQLite / in-memory (Wave E).

use rusqlite::{params, Connection, OptionalExtension};

use crate::storage_types::StorageEntry;
use crate::store::StoreError;

pub struct StorageRepository<'a> {
    conn: &'a Connection,
}

impl<'a> StorageRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn get(&self, origin: &str, key: &str) -> Result<Option<String>, StoreError> {
        self.conn
            .query_row(
                "SELECT value FROM web_storage WHERE origin=?1 AND key=?2",
                params![origin, key],
                |r| r.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn set(&self, origin: &str, key: &str, value: &str, now_ms: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO web_storage (origin, key, value, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(origin, key) DO UPDATE SET
               value=excluded.value,
               updated_at_ms=excluded.updated_at_ms",
            params![origin, key, value, now_ms],
        )?;
        Ok(())
    }

    pub fn remove(&self, origin: &str, key: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM web_storage WHERE origin=?1 AND key=?2",
            params![origin, key],
        )?;
        Ok(())
    }

    pub fn clear_origin(&self, origin: &str) -> Result<u64, StoreError> {
        let n = self
            .conn
            .execute("DELETE FROM web_storage WHERE origin=?1", params![origin])?;
        Ok(n as u64)
    }

    pub fn clear_all(&self) -> Result<(), StoreError> {
        self.conn.execute("DELETE FROM web_storage", [])?;
        Ok(())
    }

    pub fn length(&self, origin: &str) -> Result<usize, StoreError> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM web_storage WHERE origin=?1",
            params![origin],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    pub fn key_at(&self, origin: &str, index: usize) -> Result<Option<String>, StoreError> {
        self.conn
            .query_row(
                "SELECT key FROM web_storage WHERE origin=?1 ORDER BY key ASC LIMIT 1 OFFSET ?2",
                params![origin, index as i64],
                |r| r.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn byte_size(&self, origin: &str) -> Result<usize, StoreError> {
        let n: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(LENGTH(key) + LENGTH(value)), 0) FROM web_storage WHERE origin=?1",
            params![origin],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    #[allow(dead_code)]
    pub fn list_origin(&self, origin: &str) -> Result<Vec<StorageEntry>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT origin, key, value, updated_at_ms FROM web_storage
             WHERE origin=?1 ORDER BY key ASC",
        )?;
        let rows = stmt.query_map(params![origin], |r| {
            Ok(StorageEntry {
                origin: r.get(0)?,
                key: r.get(1)?,
                value: r.get(2)?,
                updated_at_ms: r.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    #[allow(dead_code)]
    pub fn list_origins(&self) -> Result<Vec<String>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT origin FROM web_storage ORDER BY origin ASC")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}
