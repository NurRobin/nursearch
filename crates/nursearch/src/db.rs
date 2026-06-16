use rusqlite::{Connection, Result, params};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct UsageStats {
    pub global_count: i64,
    pub global_last_used: i64,
    pub query_count: i64,
    pub query_last_used: i64,
}

/// All usage counts relevant to a single query, loaded in one pass so ranking
/// can score every result without issuing per-item SQL queries on each keystroke.
#[derive(Default)]
pub struct StatsSnapshot {
    global: HashMap<String, (i64, i64)>,
    per_query: HashMap<String, (i64, i64)>,
}

impl StatsSnapshot {
    /// Usage stats for one history key (a desktop-file path or a synthetic id).
    pub fn stats_for(&self, key: &str) -> UsageStats {
        let (global_count, global_last_used) = self.global.get(key).copied().unwrap_or((0, 0));
        let (query_count, query_last_used) = self.per_query.get(key).copied().unwrap_or((0, 0));
        UsageStats {
            global_count,
            global_last_used,
            query_count,
            query_last_used,
        }
    }
}

pub struct HistoryDb {
    conn: Connection,
}

impl HistoryDb {
    pub fn open() -> Result<Self> {
        let path = data_dir().join("history.sqlite");
        Self::open_at(path)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn)
    }

    fn open_at(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
        }

        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS app_usage (
                desktop_file TEXT PRIMARY KEY,
                launch_count INTEGER NOT NULL DEFAULT 0,
                last_used INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS query_usage (
                query TEXT NOT NULL,
                desktop_file TEXT NOT NULL,
                launch_count INTEGER NOT NULL DEFAULT 0,
                last_used INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (query, desktop_file)
            );

            CREATE TABLE IF NOT EXISTS plugin_storage (
                plugin_id TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                PRIMARY KEY (plugin_id, key)
            );
            ",
        )?;

        Ok(Self { conn })
    }

    /// Persist a value for a plugin (reboot-safe). Upserts on (plugin_id, key).
    pub fn storage_set(&self, plugin_id: &str, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO plugin_storage (plugin_id, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT(plugin_id, key) DO UPDATE SET value = excluded.value",
            params![plugin_id, key, value],
        )?;
        Ok(())
    }

    pub fn storage_get(&self, plugin_id: &str, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM plugin_storage WHERE plugin_id = ?1 AND key = ?2",
                params![plugin_id, key],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
    }

    pub fn storage_delete(&self, plugin_id: &str, key: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM plugin_storage WHERE plugin_id = ?1 AND key = ?2",
            params![plugin_id, key],
        )?;
        Ok(())
    }

    /// All key/value pairs for a plugin, optionally filtered by a literal key
    /// prefix. LIKE metacharacters in the prefix are escaped so it matches
    /// literally rather than as a pattern.
    pub fn storage_list(
        &self,
        plugin_id: &str,
        prefix: Option<&str>,
    ) -> Result<Vec<(String, String)>> {
        let escaped = prefix
            .unwrap_or("")
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("{escaped}%");
        let mut stmt = self.conn.prepare(
            "SELECT key, value FROM plugin_storage WHERE plugin_id = ?1 AND key LIKE ?2 ESCAPE '\\' ORDER BY key",
        )?;
        let rows = stmt.query_map(params![plugin_id, pattern], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        Ok(rows.flatten().collect())
    }

    /// Load all global usage plus the usage for one query in two SQL statements.
    pub fn snapshot(&self, query: &str) -> StatsSnapshot {
        let mut snapshot = StatsSnapshot::default();
        load_counts(
            &self.conn,
            "SELECT desktop_file, launch_count, last_used FROM app_usage",
            [],
            &mut snapshot.global,
        );

        let normalized_query = normalize_query(query);
        if !normalized_query.is_empty() {
            load_counts(
                &self.conn,
                "SELECT desktop_file, launch_count, last_used FROM query_usage WHERE query = ?1",
                [normalized_query.as_str()],
                &mut snapshot.per_query,
            );
        }

        snapshot
    }

    pub fn record_launch(&self, query: &str, desktop_file: &str) -> Result<()> {
        let now = unix_time();
        self.conn.execute(
            "
            INSERT INTO app_usage (desktop_file, launch_count, last_used)
            VALUES (?1, 1, ?2)
            ON CONFLICT(desktop_file) DO UPDATE SET
                launch_count = launch_count + 1,
                last_used = excluded.last_used
            ",
            params![desktop_file, now],
        )?;

        let normalized_query = normalize_query(query);
        if !normalized_query.is_empty() {
            self.conn.execute(
                "
                INSERT INTO query_usage (query, desktop_file, launch_count, last_used)
                VALUES (?1, ?2, 1, ?3)
                ON CONFLICT(query, desktop_file) DO UPDATE SET
                    launch_count = launch_count + 1,
                    last_used = excluded.last_used
                ",
                params![normalized_query, desktop_file, now],
            )?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_global_and_query_usage() {
        let db = HistoryDb::open_in_memory().unwrap();

        db.record_launch("  Term  ", "/tmp/terminal.desktop")
            .unwrap();
        db.record_launch("term", "/tmp/terminal.desktop").unwrap();

        let stats = db.snapshot("TERM").stats_for("/tmp/terminal.desktop");
        assert_eq!(stats.global_count, 2);
        assert_eq!(stats.query_count, 2);
        assert!(stats.global_last_used > 0);
        assert!(stats.query_last_used > 0);
    }

    #[test]
    fn ignores_empty_query_usage() {
        let db = HistoryDb::open_in_memory().unwrap();

        db.record_launch("   ", "/tmp/terminal.desktop").unwrap();

        let stats = db.snapshot("").stats_for("/tmp/terminal.desktop");
        assert_eq!(stats.global_count, 1);
        assert_eq!(stats.query_count, 0);
    }

    #[test]
    fn plugin_storage_round_trips_and_isolates_by_plugin() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.storage_set("plugin.a", "entry:1", "hello").unwrap();
        db.storage_set("plugin.a", "entry:2", "world").unwrap();
        db.storage_set("plugin.b", "entry:1", "other").unwrap();

        assert_eq!(
            db.storage_get("plugin.a", "entry:1").unwrap().as_deref(),
            Some("hello")
        );
        assert_eq!(db.storage_get("plugin.a", "missing").unwrap(), None);

        let listed = db.storage_list("plugin.a", Some("entry:")).unwrap();
        assert_eq!(listed.len(), 2);

        db.storage_delete("plugin.a", "entry:1").unwrap();
        assert_eq!(db.storage_get("plugin.a", "entry:1").unwrap(), None);
        // plugin.b is unaffected.
        assert_eq!(
            db.storage_get("plugin.b", "entry:1").unwrap().as_deref(),
            Some("other")
        );
    }

    #[test]
    fn storage_list_prefix_matches_literally() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.storage_set("p", "a%b", "1").unwrap();
        db.storage_set("p", "axb", "2").unwrap();
        db.storage_set("p", "a_c", "3").unwrap();

        // "a%" is a literal prefix, so it must match only "a%b", not "axb".
        let listed = db.storage_list("p", Some("a%")).unwrap();
        assert_eq!(listed, vec![("a%b".to_string(), "1".to_string())]);
    }
}

fn load_counts<P: rusqlite::Params>(
    conn: &Connection,
    sql: &str,
    params: P,
    target: &mut HashMap<String, (i64, i64)>,
) {
    let Ok(mut stmt) = conn.prepare(sql) else {
        return;
    };
    let Ok(rows) = stmt.query_map(params, |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    }) else {
        return;
    };
    for (key, count, last_used) in rows.flatten() {
        target.insert(key, (count, last_used));
    }
}

pub fn normalize_query(query: &str) -> String {
    query.trim().to_lowercase()
}

fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".local/share/nursearch")
}

fn unix_time() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}
