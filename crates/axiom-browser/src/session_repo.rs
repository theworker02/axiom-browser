//! Session persistence repository.

use rusqlite::{params, Connection, OptionalExtension};

use crate::session::SessionSnapshot;
use crate::time_util::now_ms;

pub const SESSION_ID_CURRENT: &str = "current";

pub struct SessionRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SessionRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn save_checkpoint(
        &self,
        snap: &SessionSnapshot,
        clean_shutdown: bool,
    ) -> rusqlite::Result<()> {
        let json = snap.to_json();
        self.conn.execute(
            "INSERT INTO sessions (id, json, updated_at_ms, clean_shutdown) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET json=excluded.json, updated_at_ms=excluded.updated_at_ms,
             clean_shutdown=excluded.clean_shutdown",
            params![
                SESSION_ID_CURRENT,
                json,
                now_ms(),
                if clean_shutdown { 1 } else { 0 }
            ],
        )?;
        Ok(())
    }

    pub fn load_checkpoint(&self) -> rusqlite::Result<Option<(SessionSnapshot, bool)>> {
        let row: Option<(String, i64)> = self
            .conn
            .query_row(
                "SELECT json, clean_shutdown FROM sessions WHERE id = ?1",
                [SESSION_ID_CURRENT],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(
            row.and_then(|(json, clean)| {
                SessionSnapshot::from_json(&json).map(|s| (s, clean != 0))
            }),
        )
    }

    pub fn clear(&self) -> rusqlite::Result<usize> {
        self.conn.execute("DELETE FROM sessions", [])
    }

    pub fn mark_clean_shutdown(&self) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE sessions SET clean_shutdown = 1 WHERE id = ?1",
            [SESSION_ID_CURRENT],
        )?;
        Ok(())
    }
}
