//! Small local SQLite store for Google Takeout / transfer imports.
//!
//! Tokens are never written to this database. It holds deduplicated metadata
//! only, with a row cap and a hard page-count cap so it cannot grow forever.

use crate::config::Config;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::sync::{Mutex, OnceLock};

const MAX_ROWS: i64 = 10_000;

pub struct Store {
    connection: Mutex<Connection>,
}

static STORE: OnceLock<Result<Store, String>> = OnceLock::new();

pub fn store() -> Result<&'static Store, String> {
    STORE
        .get_or_init(|| {
            let dir = Config::data_dir();
            std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
            let connection = Connection::open(dir.join("sourcetube.sqlite3"))
                .map_err(|error| error.to_string())?;
            connection
                .execute_batch(
                    "PRAGMA journal_mode=WAL;
                     PRAGMA max_page_count=32768;
                     PRAGMA user_version=1;
                     CREATE TABLE IF NOT EXISTS imported_items (
                       kind TEXT NOT NULL,
                       id TEXT NOT NULL,
                       title TEXT NOT NULL DEFAULT '',
                       channel TEXT NOT NULL DEFAULT '',
                       channel_id TEXT NOT NULL DEFAULT '',
                       at INTEGER NOT NULL DEFAULT 0,
                       extra TEXT NOT NULL DEFAULT '{}',
                       PRIMARY KEY (kind, id)
                     );
                     CREATE INDEX IF NOT EXISTS imported_items_at ON imported_items(at DESC);",
                )
                .map_err(|error| error.to_string())?;
            let count: i64 = connection
                .query_row("SELECT count(*) FROM imported_items", [], |row| row.get(0))
                .map_err(|error| error.to_string())?;
            if count > MAX_ROWS {
                connection
                    .execute(
                        "DELETE FROM imported_items WHERE rowid IN
                         (SELECT rowid FROM imported_items ORDER BY at DESC LIMIT -1 OFFSET ?1)",
                        [MAX_ROWS],
                    )
                    .map_err(|error| error.to_string())?;
            }
            Ok(Store {
                connection: Mutex::new(connection),
            })
        })
        .as_ref()
        .map_err(Clone::clone)
}

pub fn upsert(
    kind: &str,
    id: &str,
    title: &str,
    channel: &str,
    channel_id: &str,
    at: i64,
    extra: &Value,
) -> Result<(), String> {
    if id.is_empty() {
        return Ok(());
    }
    let store = store()?;
    let connection = store
        .connection
        .lock()
        .map_err(|_| "SQLite lock poisoned".to_string())?;
    connection
        .execute(
            "INSERT INTO imported_items (kind, id, title, channel, channel_id, at, extra)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(kind, id) DO UPDATE SET
               title=excluded.title, channel=excluded.channel,
               channel_id=excluded.channel_id, at=excluded.at, extra=excluded.extra",
            params![kind, id, title, channel, channel_id, at, extra.to_string()],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn summary() -> Result<Value, String> {
    let store = store()?;
    let connection = store
        .connection
        .lock()
        .map_err(|_| "SQLite lock poisoned".to_string())?;
    let mut statement = connection
        .prepare("SELECT kind, count(*) FROM imported_items GROUP BY kind ORDER BY kind")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| error.to_string())?;
    let mut counts = serde_json::Map::new();
    for row in rows {
        let (kind, count) = row.map_err(|error| error.to_string())?;
        counts.insert(kind, json!(count));
    }
    let total: i64 = connection
        .query_row("SELECT count(*) FROM imported_items", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    Ok(json!({ "counts": Value::Object(counts), "total": total }))
}

pub fn recent(kind: &str, limit: usize) -> Result<Vec<Value>, String> {
    let store = store()?;
    let connection = store
        .connection
        .lock()
        .map_err(|_| "SQLite lock poisoned".to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT id, title, channel, channel_id, at, extra FROM imported_items
             WHERE kind = ?1 ORDER BY at DESC LIMIT ?2",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![kind, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        let (id, title, channel, channel_id, at, extra) = row.map_err(|error| error.to_string())?;
        let extra: Value = serde_json::from_str(&extra).unwrap_or_else(|_| json!({}));
        out.push(json!({
            "id": id,
            "title": title,
            "channel": channel,
            "channelId": channel_id,
            "at": at,
            "extra": extra,
        }));
    }
    Ok(out)
}

#[cfg(test)]
pub fn delete(kind: &str, id: &str) -> Result<(), String> {
    let store = store()?;
    let connection = store
        .connection
        .lock()
        .map_err(|_| "SQLite lock poisoned".to_string())?;
    connection
        .execute(
            "DELETE FROM imported_items WHERE kind=?1 AND id=?2",
            params![kind, id],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn exists(kind: &str, id: &str) -> Result<bool, String> {
    let store = store()?;
    let connection = store
        .connection
        .lock()
        .map_err(|_| "SQLite lock poisoned".to_string())?;
    connection
        .query_row(
            "SELECT 1 FROM imported_items WHERE kind=?1 AND id=?2",
            params![kind, id],
            |_| Ok(true),
        )
        .optional()
        .map(|value| value.is_some())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_usable() {
        let _ = store().expect("store opens");
        assert!(summary().is_ok());
    }

    #[test]
    fn deduplicates_by_kind_and_id() {
        let _ = store().expect("store opens");
        let result = upsert(
            "test",
            "test-dedupe-id",
            "Title",
            "Channel",
            "UCtest",
            1,
            &json!({}),
        )
        .and_then(|_| {
            upsert(
                "test",
                "test-dedupe-id",
                "Title 2",
                "Channel",
                "UCtest",
                2,
                &json!({}),
            )
        });
        assert!(result.is_ok());
        assert!(exists("test", "test-dedupe-id").unwrap_or(false));
        let _ = delete("test", "test-dedupe-id");
    }
}
