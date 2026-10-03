//! Persistent bookmarks + folders.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::time_util::{now_ms, TimestampMs};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookmarkFolder {
    pub id: String,
    pub title: String,
    pub parent_id: Option<String>,
    pub created_at_ms: TimestampMs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: String,
    pub url: String,
    pub title: String,
    pub folder_id: Option<String>,
    pub created_at_ms: TimestampMs,
    pub updated_at_ms: TimestampMs,
}

pub struct BookmarkRepository<'a> {
    conn: &'a Connection,
}

impl<'a> BookmarkRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn ensure_root_folder(&self) -> rusqlite::Result<BookmarkFolder> {
        if let Some(f) = self.folder_by_title("Bookmarks Bar")? {
            return Ok(f);
        }
        self.create_folder("Bookmarks Bar", None)
    }

    pub fn create_folder(
        &self,
        title: &str,
        parent_id: Option<&str>,
    ) -> rusqlite::Result<BookmarkFolder> {
        if let Some(pid) = parent_id {
            if self.assert_no_cycle(pid, None).is_err() {
                return Err(rusqlite::Error::InvalidQuery);
            }
        }
        let folder = BookmarkFolder {
            id: Uuid::new_v4().to_string(),
            title: title.to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
            created_at_ms: now_ms(),
        };
        self.conn.execute(
            "INSERT INTO bookmark_folders (id, title, parent_id, created_at_ms) VALUES (?1,?2,?3,?4)",
            params![folder.id, folder.title, folder.parent_id, folder.created_at_ms],
        )?;
        Ok(folder)
    }

    fn assert_no_cycle(&self, parent_id: &str, moving_id: Option<&str>) -> Result<(), ()> {
        let mut cur = Some(parent_id.to_string());
        let mut guard = 0;
        while let Some(id) = cur {
            if moving_id == Some(id.as_str()) {
                return Err(());
            }
            cur = self
                .conn
                .query_row(
                    "SELECT parent_id FROM bookmark_folders WHERE id = ?1",
                    [&id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()
                .map_err(|_| ())?
                .flatten();
            guard += 1;
            if guard > 64 {
                return Err(());
            }
        }
        Ok(())
    }

    pub fn folder_by_title(&self, title: &str) -> rusqlite::Result<Option<BookmarkFolder>> {
        self.conn
            .query_row(
                "SELECT id, title, parent_id, created_at_ms FROM bookmark_folders WHERE title = ?1 LIMIT 1",
                [title],
                map_folder,
            )
            .optional()
    }

    pub fn create(
        &self,
        url: &str,
        title: &str,
        folder_id: Option<&str>,
    ) -> rusqlite::Result<Bookmark> {
        let now = now_ms();
        let bm = Bookmark {
            id: Uuid::new_v4().to_string(),
            url: url.to_string(),
            title: if title.is_empty() {
                url.to_string()
            } else {
                title.to_string()
            },
            folder_id: folder_id.map(|s| s.to_string()),
            created_at_ms: now,
            updated_at_ms: now,
        };
        self.conn.execute(
            "INSERT INTO bookmarks (id, url, title, folder_id, created_at_ms, updated_at_ms)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                bm.id,
                bm.url,
                bm.title,
                bm.folder_id,
                bm.created_at_ms,
                bm.updated_at_ms
            ],
        )?;
        Ok(bm)
    }

    pub fn update(
        &self,
        id: &str,
        title: &str,
        url: &str,
        folder_id: Option<&str>,
    ) -> rusqlite::Result<usize> {
        self.conn.execute(
            "UPDATE bookmarks SET title=?1, url=?2, folder_id=?3, updated_at_ms=?4 WHERE id=?5",
            params![title, url, folder_id, now_ms(), id],
        )
    }

    pub fn delete(&self, id: &str) -> rusqlite::Result<usize> {
        self.conn
            .execute("DELETE FROM bookmarks WHERE id = ?1", [id])
    }

    pub fn delete_url(&self, url: &str) -> rusqlite::Result<usize> {
        self.conn
            .execute("DELETE FROM bookmarks WHERE url = ?1", [url])
    }

    pub fn by_url(&self, url: &str) -> rusqlite::Result<Option<Bookmark>> {
        self.conn
            .query_row(
                "SELECT id, url, title, folder_id, created_at_ms, updated_at_ms FROM bookmarks WHERE url = ?1 LIMIT 1",
                [url],
                map_bookmark,
            )
            .optional()
    }

    pub fn lookup(&self, id: &str) -> rusqlite::Result<Option<Bookmark>> {
        self.conn
            .query_row(
                "SELECT id, url, title, folder_id, created_at_ms, updated_at_ms FROM bookmarks WHERE id = ?1",
                [id],
                map_bookmark,
            )
            .optional()
    }

    pub fn list(&self, limit: usize) -> rusqlite::Result<Vec<Bookmark>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, url, title, folder_id, created_at_ms, updated_at_ms FROM bookmarks
             ORDER BY updated_at_ms DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], map_bookmark)?;
        rows.collect()
    }

    pub fn search(&self, query: &str, limit: usize) -> rusqlite::Result<Vec<Bookmark>> {
        let q = format!("%{}%", query.trim().to_ascii_lowercase());
        let mut stmt = self.conn.prepare(
            "SELECT id, url, title, folder_id, created_at_ms, updated_at_ms FROM bookmarks
             WHERE lower(url) LIKE ?1 OR lower(title) LIKE ?1
             ORDER BY updated_at_ms DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![q, limit as i64], map_bookmark)?;
        rows.collect()
    }

    pub fn clear(&self) -> rusqlite::Result<usize> {
        self.conn.execute("DELETE FROM bookmarks", [])
    }

    pub fn count(&self) -> rusqlite::Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM bookmarks", [], |r| r.get(0))
    }
}

fn map_folder(r: &rusqlite::Row<'_>) -> rusqlite::Result<BookmarkFolder> {
    Ok(BookmarkFolder {
        id: r.get(0)?,
        title: r.get(1)?,
        parent_id: r.get(2)?,
        created_at_ms: r.get(3)?,
    })
}

fn map_bookmark(r: &rusqlite::Row<'_>) -> rusqlite::Result<Bookmark> {
    Ok(Bookmark {
        id: r.get(0)?,
        url: r.get(1)?,
        title: r.get(2)?,
        folder_id: r.get(3)?,
        created_at_ms: r.get(4)?,
        updated_at_ms: r.get(5)?,
    })
}
