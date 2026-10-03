//! Durable browsing history repository.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::time_util::{host_of_url, now_ms, TimestampMs};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisitTransition {
    Typed,
    Link,
    Reload,
    Redirect,
    FormSubmit,
    Other,
}

impl VisitTransition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Typed => "typed",
            Self::Link => "link",
            Self::Reload => "reload",
            Self::Redirect => "redirect",
            Self::FormSubmit => "form_submit",
            Self::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "typed" => Self::Typed,
            "link" => Self::Link,
            "reload" => Self::Reload,
            "redirect" => Self::Redirect,
            "form_submit" => Self::FormSubmit,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRecord {
    pub id: String,
    pub url: String,
    pub title: String,
    pub visit_time_ms: TimestampMs,
    pub visit_count: i64,
    pub transition: VisitTransition,
    pub host: String,
}

pub struct HistoryRepository<'a> {
    conn: &'a Connection,
}

impl<'a> HistoryRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn record_visit(
        &self,
        url: &str,
        title: &str,
        transition: VisitTransition,
    ) -> rusqlite::Result<HistoryRecord> {
        let host = host_of_url(url);
        let now = now_ms();
        if let Some(mut existing) = self.by_url(url)? {
            existing.visit_count += 1;
            existing.visit_time_ms = now;
            if !title.is_empty() {
                existing.title = title.to_string();
            }
            existing.transition = transition;
            self.conn.execute(
                "UPDATE history_entries SET title=?1, visit_time_ms=?2, visit_count=?3, transition=?4, host=?5 WHERE id=?6",
                params![
                    existing.title,
                    existing.visit_time_ms,
                    existing.visit_count,
                    existing.transition.as_str(),
                    existing.host,
                    existing.id
                ],
            )?;
            return Ok(existing);
        }
        let rec = HistoryRecord {
            id: Uuid::new_v4().to_string(),
            url: url.to_string(),
            title: if title.is_empty() {
                url.to_string()
            } else {
                title.to_string()
            },
            visit_time_ms: now,
            visit_count: 1,
            transition,
            host,
        };
        self.conn.execute(
            "INSERT INTO history_entries (id, url, title, visit_time_ms, visit_count, transition, host)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                rec.id,
                rec.url,
                rec.title,
                rec.visit_time_ms,
                rec.visit_count,
                rec.transition.as_str(),
                rec.host
            ],
        )?;
        Ok(rec)
    }

    pub fn by_url(&self, url: &str) -> rusqlite::Result<Option<HistoryRecord>> {
        self.conn
            .query_row(
                "SELECT id, url, title, visit_time_ms, visit_count, transition, host
                 FROM history_entries WHERE url = ?1 LIMIT 1",
                [url],
                map_row,
            )
            .optional()
    }

    pub fn recent(&self, limit: usize) -> rusqlite::Result<Vec<HistoryRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, url, title, visit_time_ms, visit_count, transition, host
             FROM history_entries ORDER BY visit_time_ms DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], map_row)?;
        rows.collect()
    }

    pub fn search(&self, query: &str, limit: usize) -> rusqlite::Result<Vec<HistoryRecord>> {
        let q = format!("%{}%", query.trim().to_ascii_lowercase());
        let mut stmt = self.conn.prepare(
            "SELECT id, url, title, visit_time_ms, visit_count, transition, host
             FROM history_entries
             WHERE lower(url) LIKE ?1 OR lower(title) LIKE ?1 OR lower(host) LIKE ?1
             ORDER BY visit_count DESC, visit_time_ms DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![q, limit as i64], map_row)?;
        rows.collect()
    }

    pub fn visits_for_url(&self, url: &str) -> rusqlite::Result<Vec<HistoryRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, url, title, visit_time_ms, visit_count, transition, host
             FROM history_entries WHERE url = ?1 ORDER BY visit_time_ms DESC",
        )?;
        let rows = stmt.query_map([url], map_row)?;
        rows.collect()
    }

    pub fn delete_entry(&self, id: &str) -> rusqlite::Result<usize> {
        self.conn
            .execute("DELETE FROM history_entries WHERE id = ?1", [id])
    }

    pub fn delete_url(&self, url: &str) -> rusqlite::Result<usize> {
        self.conn
            .execute("DELETE FROM history_entries WHERE url = ?1", [url])
    }

    pub fn delete_range(
        &self,
        start_ms: TimestampMs,
        end_ms: TimestampMs,
    ) -> rusqlite::Result<usize> {
        self.conn.execute(
            "DELETE FROM history_entries WHERE visit_time_ms >= ?1 AND visit_time_ms <= ?2",
            params![start_ms, end_ms],
        )
    }

    pub fn clear(&self) -> rusqlite::Result<usize> {
        self.conn.execute("DELETE FROM history_entries", [])
    }

    pub fn count(&self) -> rusqlite::Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM history_entries", [], |r| r.get(0))
    }
}

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryRecord> {
    let transition: String = r.get(5)?;
    Ok(HistoryRecord {
        id: r.get(0)?,
        url: r.get(1)?,
        title: r.get(2)?,
        visit_time_ms: r.get(3)?,
        visit_count: r.get(4)?,
        transition: VisitTransition::parse(&transition),
        host: r.get(6)?,
    })
}

/// Ranking weights for history omnibox suggestions (centralized).
pub mod ranking {
    pub const TEXT_MATCH: i32 = 40;
    pub const RECENCY_BONUS_MAX: i32 = 30;
    pub const FREQUENCY_BONUS_MAX: i32 = 25;
    pub const TYPED_BONUS: i32 = 15;

    pub fn score(text_hit: bool, age_hours: f64, visit_count: i64, typed: bool) -> i32 {
        let mut s = 0;
        if text_hit {
            s += TEXT_MATCH;
        }
        let recency = (RECENCY_BONUS_MAX as f64 * (1.0 / (1.0 + age_hours / 24.0))) as i32;
        s += recency;
        s += (visit_count as i32).min(FREQUENCY_BONUS_MAX);
        if typed {
            s += TYPED_BONUS;
        }
        s
    }
}
