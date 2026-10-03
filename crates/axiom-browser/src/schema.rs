//! Schema versioning and migrations for profile SQLite databases.

use rusqlite::{Connection, OptionalExtension};
use thiserror::Error;

pub const SCHEMA_VERSION: i64 = 3;

#[derive(Debug, Error)]
pub enum SchemaError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("unsupported newer schema version {found} (this build supports {SCHEMA_VERSION})")]
    NewerUnsupported { found: i64 },
    #[error("corrupt schema metadata")]
    Corrupt,
    #[error("migration failed from v{from} to v{to}: {detail}")]
    MigrationFailed { from: i64, to: i64, detail: String },
}

pub fn migrate(conn: &Connection) -> Result<i64, SchemaError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_metadata (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            version INTEGER NOT NULL
        );",
    )?;

    let current: Option<i64> = conn
        .query_row(
            "SELECT version FROM schema_metadata WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .optional()?;

    match current {
        None => {
            apply_v1(conn)?;
            apply_v2(conn).map_err(|e| SchemaError::MigrationFailed {
                from: 0,
                to: 2,
                detail: e.to_string(),
            })?;
            apply_v3(conn).map_err(|e| SchemaError::MigrationFailed {
                from: 2,
                to: 3,
                detail: e.to_string(),
            })?;
            conn.execute(
                "INSERT INTO schema_metadata (id, version) VALUES (1, ?1)",
                [SCHEMA_VERSION],
            )?;
            log::info!(target: "axiom_persist", "schema initialized at v{SCHEMA_VERSION}");
            Ok(SCHEMA_VERSION)
        }
        Some(v) if v == SCHEMA_VERSION => Ok(v),
        Some(v) if v < SCHEMA_VERSION => {
            let mut ver = v;
            while ver < SCHEMA_VERSION {
                let next = ver + 1;
                match next {
                    2 => apply_v2(conn).map_err(|e| SchemaError::MigrationFailed {
                        from: ver,
                        to: next,
                        detail: e.to_string(),
                    })?,
                    3 => apply_v3(conn).map_err(|e| SchemaError::MigrationFailed {
                        from: ver,
                        to: next,
                        detail: e.to_string(),
                    })?,
                    _ => {
                        return Err(SchemaError::MigrationFailed {
                            from: ver,
                            to: next,
                            detail: "unknown migration step".into(),
                        });
                    }
                }
                conn.execute(
                    "UPDATE schema_metadata SET version = ?1 WHERE id = 1",
                    [next],
                )?;
                log::info!(target: "axiom_persist", "schema migrated v{ver} → v{next}");
                ver = next;
            }
            Ok(SCHEMA_VERSION)
        }
        Some(v) => Err(SchemaError::NewerUnsupported { found: v }),
    }
}

fn apply_v1(conn: &Connection) -> Result<(), SchemaError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS history_entries (
            id TEXT PRIMARY KEY NOT NULL,
            url TEXT NOT NULL,
            title TEXT NOT NULL,
            visit_time_ms INTEGER NOT NULL,
            visit_count INTEGER NOT NULL DEFAULT 1,
            transition TEXT NOT NULL DEFAULT 'other',
            host TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_history_visit_time ON history_entries(visit_time_ms DESC);
        CREATE INDEX IF NOT EXISTS idx_history_url ON history_entries(url);
        CREATE INDEX IF NOT EXISTS idx_history_host ON history_entries(host);
        CREATE INDEX IF NOT EXISTS idx_history_title ON history_entries(title);

        CREATE TABLE IF NOT EXISTS bookmark_folders (
            id TEXT PRIMARY KEY NOT NULL,
            title TEXT NOT NULL,
            parent_id TEXT,
            created_at_ms INTEGER NOT NULL,
            FOREIGN KEY(parent_id) REFERENCES bookmark_folders(id)
        );

        CREATE TABLE IF NOT EXISTS bookmarks (
            id TEXT PRIMARY KEY NOT NULL,
            url TEXT NOT NULL,
            title TEXT NOT NULL,
            folder_id TEXT,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            FOREIGN KEY(folder_id) REFERENCES bookmark_folders(id)
        );
        CREATE INDEX IF NOT EXISTS idx_bookmarks_url ON bookmarks(url);
        CREATE INDEX IF NOT EXISTS idx_bookmarks_title ON bookmarks(title);

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY NOT NULL,
            value_json TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY NOT NULL,
            json TEXT NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            clean_shutdown INTEGER NOT NULL DEFAULT 0
        );
        ",
    )?;
    Ok(())
}

fn apply_v2(conn: &Connection) -> Result<(), SchemaError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS cookies (
            id TEXT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            value TEXT NOT NULL,
            domain TEXT NOT NULL,
            path TEXT NOT NULL,
            creation_time_ms INTEGER NOT NULL,
            last_access_time_ms INTEGER NOT NULL,
            expires_at_ms INTEGER,
            secure INTEGER NOT NULL DEFAULT 0,
            http_only INTEGER NOT NULL DEFAULT 0,
            same_site TEXT NOT NULL DEFAULT 'lax',
            host_only INTEGER NOT NULL DEFAULT 1,
            persistent INTEGER NOT NULL DEFAULT 0,
            priority TEXT NOT NULL DEFAULT 'medium',
            source TEXT NOT NULL DEFAULT 'network',
            UNIQUE(name, domain, path)
        );
        CREATE INDEX IF NOT EXISTS idx_cookies_domain ON cookies(domain);
        CREATE INDEX IF NOT EXISTS idx_cookies_domain_path ON cookies(domain, path);
        CREATE INDEX IF NOT EXISTS idx_cookies_expires ON cookies(expires_at_ms);
        CREATE INDEX IF NOT EXISTS idx_cookies_last_access ON cookies(last_access_time_ms);
        ",
    )?;
    Ok(())
}

fn apply_v3(conn: &Connection) -> Result<(), SchemaError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS web_storage (
            origin TEXT NOT NULL,
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            PRIMARY KEY (origin, key)
        );
        CREATE INDEX IF NOT EXISTS idx_web_storage_origin ON web_storage(origin);
        ",
    )?;
    Ok(())
}

#[allow(dead_code)]
pub fn read_version(conn: &Connection) -> Result<i64, SchemaError> {
    let v: Option<i64> = conn
        .query_row(
            "SELECT version FROM schema_metadata WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    v.ok_or(SchemaError::Corrupt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn fresh_is_v3() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(migrate(&conn).unwrap(), 3);
        assert_eq!(read_version(&conn).unwrap(), 3);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM web_storage", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn v1_to_v3() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_metadata (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                version INTEGER NOT NULL
            );",
        )
        .unwrap();
        apply_v1(&conn).unwrap();
        conn.execute(
            "INSERT INTO schema_metadata (id, version) VALUES (1, 1)",
            [],
        )
        .unwrap();
        assert_eq!(migrate(&conn).unwrap(), 3);
        conn.execute(
            "INSERT INTO web_storage (origin, key, value, updated_at_ms)
             VALUES ('https://example.test','a','b',0)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn v2_to_v3() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_metadata (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                version INTEGER NOT NULL
            );",
        )
        .unwrap();
        apply_v1(&conn).unwrap();
        apply_v2(&conn).unwrap();
        conn.execute(
            "INSERT INTO schema_metadata (id, version) VALUES (1, 2)",
            [],
        )
        .unwrap();
        assert_eq!(migrate(&conn).unwrap(), 3);
    }

    #[test]
    fn reopen_v3() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        assert_eq!(migrate(&conn).unwrap(), 3);
    }

    #[test]
    fn newer_unsupported() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute("UPDATE schema_metadata SET version = 99 WHERE id = 1", [])
            .unwrap();
        match migrate(&conn) {
            Err(SchemaError::NewerUnsupported { found: 99 }) => {}
            other => panic!("expected NewerUnsupported, got {other:?}"),
        }
    }
}
