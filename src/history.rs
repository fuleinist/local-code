//! F5 — Session history: persist each exchange to SQLite and query it back.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use rusqlite::Connection;

/// Default database location: ~/.local-code/history.db
/// Override with LOCAL_CODE_DB (used by tests).
pub fn db_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("LOCAL_CODE_DB") {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .context("could not determine home directory (set USERPROFILE or HOME)")?;
    Ok(PathBuf::from(home).join(".local-code").join("history.db"))
}

pub fn open(path: &std::path::Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("failed to create ~/.local-code")?;
    }
    let conn = Connection::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts TEXT NOT NULL,
            cwd TEXT NOT NULL,
            model TEXT NOT NULL,
            prompt TEXT NOT NULL,
            response TEXT NOT NULL
        );",
    )?;
    Ok(conn)
}

fn now() -> String {
    // Seconds since epoch as a simple sortable timestamp (no chrono dep).
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

pub fn record(
    conn: &Connection,
    cwd: &str,
    model: &str,
    prompt: &str,
    response: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions (ts, cwd, model, prompt, response) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![now(), cwd, model, prompt, response],
    )?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub struct SessionRow {
    pub id: i64,
    pub ts: String,
    pub cwd: String,
    pub model: String,
    pub prompt: String,
    pub response: String,
}

pub fn recent(conn: &Connection, limit: usize) -> Result<Vec<SessionRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, cwd, model, prompt, response FROM sessions ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit as i64], |r| {
            Ok(SessionRow {
                id: r.get(0)?,
                ts: r.get(1)?,
                cwd: r.get(2)?,
                model: r.get(3)?,
                prompt: r.get(4)?,
                response: r.get(5)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn last(conn: &Connection) -> Result<Option<SessionRow>> {
    Ok(recent(conn, 1)?.into_iter().next())
}

pub fn format_unix_ts(ts: &str) -> String {
    // Best-effort human-readable rendering without a datetime crate:
    // show the raw epoch plus nothing else; CLI prints are fine with ISO-ish.
    ts.parse::<u64>()
        .map(|secs| {
            let days = secs / 86400;
            let time = secs % 86400;
            // Civil-from-days algorithm (Howard Hinnant) for UTC date.
            let z = days + 719468;
            let era = z / 146097;
            let doe = z - era * 146097;
            let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
            let y = yoe + era * 400;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let d = doy - (153 * mp + 2) / 5 + 1;
            let m = if mp < 10 { mp + 3 } else { mp - 9 };
            let y = if m <= 2 { y + 1 } else { y };
            format!(
                "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z",
                time / 3600,
                (time % 3600) / 60,
                time % 60
            )
        })
        .unwrap_or_else(|_| ts.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn temp_conn() -> (tempfile::TempDir, Connection) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.db");
        let conn = open(&path).unwrap();
        (dir, conn)
    }

    #[test]
    fn record_and_roundtrip_last() {
        let (_d, conn) = temp_conn();
        record(&conn, "/repo", "qwen2.5-coder:7b", "fix the bug", "done: patched x.rs").unwrap();
        let row = last(&conn).unwrap().expect("expected a session");
        assert_eq!(row.cwd, "/repo");
        assert_eq!(row.model, "qwen2.5-coder:7b");
        assert_eq!(row.prompt, "fix the bug");
        assert_eq!(row.response, "done: patched x.rs");
    }

    #[test]
    fn recent_orders_newest_first_with_limit() {
        let (_d, conn) = temp_conn();
        for i in 0..5 {
            record(&conn, "/repo", "m", &format!("p{i}"), &format!("r{i}")).unwrap();
        }
        let rows = recent(&conn, 3).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].prompt, "p4");
        assert_eq!(rows[2].prompt, "p2");
        assert!(rows[0].id > rows[2].id);
    }

    #[test]
    fn last_on_empty_db() {
        let (_d, conn) = temp_conn();
        assert_eq!(last(&conn).unwrap(), None);
    }

    #[test]
    fn schema_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.db");
        let conn = open(&path).unwrap();
        record(&conn, "/repo", "m", "p", "r").unwrap();
        drop(conn);
        let conn = open(&path).unwrap(); // reopen must not fail or wipe
        assert_eq!(recent(&conn, 10).unwrap().len(), 1);
    }

    #[test]
    fn ts_formatting() {
        assert_eq!(format_unix_ts("0"), "1970-01-01 00:00:00Z");
        assert_eq!(format_unix_ts("1767225600"), "2026-01-01 00:00:00Z");
        assert_eq!(format_unix_ts("garbage"), "garbage");
    }
}
