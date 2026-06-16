use rusqlite::{Connection, Result, params};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct UsageStats {
    pub global_count: i64,
    pub global_last_used: i64,
    pub query_count: i64,
    pub query_last_used: i64,
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
            ",
        )?;

        Ok(Self { conn })
    }

    pub fn stats_for(&self, query: &str, desktop_file: &str) -> Result<UsageStats> {
        let (global_count, global_last_used) = self
            .conn
            .query_row(
                "SELECT launch_count, last_used FROM app_usage WHERE desktop_file = ?1",
                [desktop_file],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((0, 0));

        let normalized_query = normalize_query(query);
        let (query_count, query_last_used) = if normalized_query.is_empty() {
            (0, 0)
        } else {
            self.conn
                .query_row(
                    "SELECT launch_count, last_used FROM query_usage WHERE query = ?1 AND desktop_file = ?2",
                    params![normalized_query, desktop_file],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap_or((0, 0))
        };

        Ok(UsageStats {
            global_count,
            global_last_used,
            query_count,
            query_last_used,
        })
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

        let stats = db.stats_for("TERM", "/tmp/terminal.desktop").unwrap();
        assert_eq!(stats.global_count, 2);
        assert_eq!(stats.query_count, 2);
        assert!(stats.global_last_used > 0);
        assert!(stats.query_last_used > 0);
    }

    #[test]
    fn ignores_empty_query_usage() {
        let db = HistoryDb::open_in_memory().unwrap();

        db.record_launch("   ", "/tmp/terminal.desktop").unwrap();

        let stats = db.stats_for("", "/tmp/terminal.desktop").unwrap();
        assert_eq!(stats.global_count, 1);
        assert_eq!(stats.query_count, 0);
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
