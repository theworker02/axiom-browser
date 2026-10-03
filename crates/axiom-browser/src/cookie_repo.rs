//! Cookie repository — SQLite / in-memory via profile connection (Wave D).

use rusqlite::{params, Connection, OptionalExtension};

use crate::cookie_types::{
    Cookie, CookieExpiration, CookieId, CookiePriority, CookieSameSite, CookieSource,
};
use crate::store::StoreError;
use uuid::Uuid;

pub struct CookieRepository<'a> {
    conn: &'a Connection,
}

impl<'a> CookieRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn upsert(&self, cookie: &Cookie) -> Result<(), StoreError> {
        let expires = cookie.expiration.absolute_ms();
        let persistent = if cookie.persistent { 1 } else { 0 };
        let secure = if cookie.secure { 1 } else { 0 };
        let http_only = if cookie.http_only { 1 } else { 0 };
        let host_only = if cookie.host_only { 1 } else { 0 };
        self.conn.execute(
            "INSERT INTO cookies (
                id, name, value, domain, path,
                creation_time_ms, last_access_time_ms, expires_at_ms,
                secure, http_only, same_site, host_only, persistent,
                priority, source
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
            ON CONFLICT(name, domain, path) DO UPDATE SET
                id=excluded.id,
                value=excluded.value,
                creation_time_ms=cookies.creation_time_ms,
                last_access_time_ms=excluded.last_access_time_ms,
                expires_at_ms=excluded.expires_at_ms,
                secure=excluded.secure,
                http_only=excluded.http_only,
                same_site=excluded.same_site,
                host_only=excluded.host_only,
                persistent=excluded.persistent,
                priority=excluded.priority,
                source=excluded.source
            ",
            params![
                cookie.id.0.to_string(),
                cookie.name,
                cookie.value,
                cookie.domain.to_ascii_lowercase(),
                cookie.path,
                cookie.creation_time_ms,
                cookie.last_access_time_ms,
                expires,
                secure,
                http_only,
                cookie.same_site.as_str(),
                host_only,
                persistent,
                priority_str(cookie.priority),
                source_str(cookie.source),
            ],
        )?;
        Ok(())
    }

    pub fn delete_identity(&self, name: &str, domain: &str, path: &str) -> Result<u64, StoreError> {
        let n = self.conn.execute(
            "DELETE FROM cookies WHERE name=?1 AND domain=?2 AND path=?3",
            params![name, domain.to_ascii_lowercase(), path],
        )?;
        Ok(n as u64)
    }

    pub fn delete_by_id(&self, id: &str) -> Result<u64, StoreError> {
        let n = self
            .conn
            .execute("DELETE FROM cookies WHERE id=?1", params![id])?;
        Ok(n as u64)
    }

    pub fn clear(&self) -> Result<(), StoreError> {
        self.conn.execute("DELETE FROM cookies", [])?;
        Ok(())
    }

    pub fn clear_for_domain_suffix(&self, host: &str) -> Result<u64, StoreError> {
        let host = host.to_ascii_lowercase();
        let n = self.conn.execute(
            "DELETE FROM cookies WHERE domain = ?1 OR ?1 LIKE '%.' || domain",
            params![host],
        )?;
        Ok(n as u64)
    }

    pub fn delete_expired(&self, now_ms: i64) -> Result<u64, StoreError> {
        let n = self.conn.execute(
            "DELETE FROM cookies WHERE expires_at_ms IS NOT NULL AND expires_at_ms <= ?1",
            params![now_ms],
        )?;
        Ok(n as u64)
    }

    pub fn delete_session_cookies(&self) -> Result<u64, StoreError> {
        let n = self
            .conn
            .execute("DELETE FROM cookies WHERE persistent = 0", [])?;
        Ok(n as u64)
    }

    pub fn count(&self) -> Result<i64, StoreError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM cookies", [], |r| r.get(0))?;
        Ok(n)
    }

    pub fn count_for_domain(&self, domain: &str) -> Result<i64, StoreError> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM cookies WHERE domain = ?1",
            params![domain.to_ascii_lowercase()],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    /// Candidates that might match host (indexed by domain equality or suffix).
    pub fn candidates_for_host(&self, host: &str) -> Result<Vec<Cookie>, StoreError> {
        let host = host.to_ascii_lowercase();
        let mut stmt = self.conn.prepare(
            "SELECT id, name, value, domain, path, creation_time_ms, last_access_time_ms,
                    expires_at_ms, secure, http_only, same_site, host_only, persistent,
                    priority, source
             FROM cookies
             WHERE domain = ?1 OR ?1 LIKE '%.' || domain
             ORDER BY length(path) DESC, creation_time_ms ASC",
        )?;
        let rows = stmt.query_map(params![host], row_to_cookie)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn list_all(&self) -> Result<Vec<Cookie>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, value, domain, path, creation_time_ms, last_access_time_ms,
                    expires_at_ms, secure, http_only, same_site, host_only, persistent,
                    priority, source
             FROM cookies
             ORDER BY domain ASC, length(path) DESC, creation_time_ms ASC",
        )?;
        let rows = stmt.query_map([], row_to_cookie)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_by_identity(
        &self,
        name: &str,
        domain: &str,
        path: &str,
    ) -> Result<Option<Cookie>, StoreError> {
        self.conn
            .query_row(
                "SELECT id, name, value, domain, path, creation_time_ms, last_access_time_ms,
                        expires_at_ms, secure, http_only, same_site, host_only, persistent,
                        priority, source
                 FROM cookies WHERE name=?1 AND domain=?2 AND path=?3",
                params![name, domain.to_ascii_lowercase(), path],
                row_to_cookie,
            )
            .optional()
            .map_err(StoreError::from)
    }

    /// Evict oldest (by creation) cookie for a domain. Returns whether a row was removed.
    pub fn evict_oldest_for_domain(&self, domain: &str) -> Result<bool, StoreError> {
        let id: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM cookies WHERE domain=?1 ORDER BY creation_time_ms ASC LIMIT 1",
                params![domain.to_ascii_lowercase()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = id {
            self.delete_by_id(&id)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn evict_oldest_global(&self) -> Result<bool, StoreError> {
        let id: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM cookies ORDER BY last_access_time_ms ASC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = id {
            self.delete_by_id(&id)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn touch_access(&self, id: &str, now_ms: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE cookies SET last_access_time_ms=?1 WHERE id=?2",
            params![now_ms, id],
        )?;
        Ok(())
    }
}

fn row_to_cookie(r: &rusqlite::Row<'_>) -> rusqlite::Result<Cookie> {
    let id_s: String = r.get(0)?;
    let expires: Option<i64> = r.get(7)?;
    let same_site_s: String = r.get(10)?;
    let priority_s: String = r.get(13)?;
    let source_s: String = r.get(14)?;
    Ok(Cookie {
        id: CookieId(Uuid::parse_str(&id_s).unwrap_or_else(|_| Uuid::nil())),
        name: r.get(1)?,
        value: r.get(2)?,
        domain: r.get(3)?,
        path: r.get(4)?,
        creation_time_ms: r.get(5)?,
        last_access_time_ms: r.get(6)?,
        expiration: match expires {
            None => CookieExpiration::Session,
            Some(t) => CookieExpiration::Absolute(t),
        },
        secure: r.get::<_, i64>(8)? != 0,
        http_only: r.get::<_, i64>(9)? != 0,
        same_site: CookieSameSite::parse_attr(&same_site_s).unwrap_or(CookieSameSite::Lax),
        host_only: r.get::<_, i64>(11)? != 0,
        persistent: r.get::<_, i64>(12)? != 0,
        priority: parse_priority(&priority_s),
        source: parse_source(&source_s),
    })
}

fn priority_str(p: CookiePriority) -> &'static str {
    match p {
        CookiePriority::Low => "low",
        CookiePriority::Medium => "medium",
        CookiePriority::High => "high",
    }
}

fn parse_priority(s: &str) -> CookiePriority {
    match s {
        "low" => CookiePriority::Low,
        "high" => CookiePriority::High,
        _ => CookiePriority::Medium,
    }
}

fn source_str(s: CookieSource) -> &'static str {
    match s {
        CookieSource::Network => "network",
        CookieSource::Script => "script",
    }
}

fn parse_source(s: &str) -> CookieSource {
    match s {
        "script" => CookieSource::Script,
        _ => CookieSource::Network,
    }
}
