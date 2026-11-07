//! SQLite persistence. Same schema and JSON shape as the Go `store.go`, so the
//! Rust app opens the database the Go app already wrote.

use crate::model::{Candidate, Settings};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;

pub struct Store {
    db: Mutex<Connection>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionRecord {
    pub at: String,
    pub action: String,
    pub path: String,
    pub result: String,
}

type Res<T> = Result<T, String>;

fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

impl Store {
    pub fn open(directory: &Path) -> Res<Store> {
        std::fs::create_dir_all(directory).map_err(e)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700));
        }
        let db = Connection::open(directory.join("cleaner.db")).map_err(e)?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS settings (id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS candidates (id TEXT PRIMARY KEY, json TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS ignored (id TEXT PRIMARY KEY);
             CREATE TABLE IF NOT EXISTS actions (id INTEGER PRIMARY KEY, at TEXT NOT NULL, action TEXT NOT NULL, path TEXT NOT NULL, result TEXT NOT NULL);",
        )
        .map_err(e)?;
        Ok(Store { db: Mutex::new(db) })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn load_settings(&self, home: &Path) -> Res<Settings> {
        let json: Option<String> = self
            .conn()
            .query_row("SELECT json FROM settings WHERE id=1", [], |r| r.get(0))
            .optional()
            .map_err(e)?;
        let Some(json) = json else { return Ok(Settings::default_for(home)) };
        let s: Settings = serde_json::from_str(&json).map_err(e)?;
        s.validate()?;
        Ok(s)
    }

    pub fn save_settings(&self, s: &Settings) -> Res<()> {
        s.validate()?;
        let json = serde_json::to_string(s).map_err(e)?;
        self.conn()
            .execute(
                "INSERT INTO settings(id,json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=excluded.json",
                params![json],
            )
            .map_err(e)?;
        Ok(())
    }

    pub fn load_candidates(&self) -> Res<Vec<Candidate>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT c.json, i.id FROM candidates c LEFT JOIN ignored i ON c.id=i.id ORDER BY c.id")
            .map_err(e)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))
            .map_err(e)?;
        let mut out = Vec::new();
        for row in rows {
            let (json, ignored) = row.map_err(e)?;
            let mut c: Candidate = serde_json::from_str(&json).map_err(e)?;
            c.ignored = ignored.is_some();
            out.push(c);
        }
        Ok(out)
    }

    pub fn replace_candidates(&self, candidates: &[Candidate]) -> Res<()> {
        let mut conn = self.conn();
        let tx = conn.transaction().map_err(e)?;
        tx.execute("DELETE FROM candidates", []).map_err(e)?;
        for c in candidates {
            let json = serde_json::to_string(c).map_err(e)?;
            tx.execute("INSERT INTO candidates(id,json) VALUES(?1,?2)", params![c.id, json]).map_err(e)?;
        }
        tx.commit().map_err(e)
    }

    fn batch(&self, sql: &str, ids: &[String]) -> Res<()> {
        let mut conn = self.conn();
        let tx = conn.transaction().map_err(e)?;
        for id in ids {
            tx.execute(sql, params![id]).map_err(e)?;
        }
        tx.commit().map_err(e)
    }

    pub fn ignore(&self, ids: &[String]) -> Res<()> {
        self.batch("INSERT OR IGNORE INTO ignored(id) VALUES(?1)", ids)
    }

    pub fn unignore(&self, ids: &[String]) -> Res<()> {
        self.batch("DELETE FROM ignored WHERE id=?1", ids)
    }

    pub fn remove_candidates(&self, ids: &[String]) -> Res<()> {
        self.batch("DELETE FROM candidates WHERE id=?1", ids)
    }

    pub fn record_action(&self, at: &str, action: &str, path: &str, result: &str) -> Res<()> {
        let result: String = result.chars().take(2000).collect();
        self.conn()
            .execute("INSERT INTO actions(at,action,path,result) VALUES(?1,?2,?3,?4)", params![at, action, path, result])
            .map_err(e)?;
        Ok(())
    }

    pub fn recent_actions(&self, limit: usize) -> Res<Vec<ActionRecord>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT at, action, path, result FROM actions ORDER BY id DESC LIMIT ?1").map_err(e)?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(ActionRecord { at: r.get(0)?, action: r.get(1)?, path: r.get(2)?, result: r.get(3)? })
            })
            .map_err(e)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(e)
    }
}
