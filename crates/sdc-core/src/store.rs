//! SQLite persistence. Same schema and JSON shape as the Go `store.go`, so the
//! Rust app opens the database the Go app already wrote.

use crate::model::{Candidate, Settings};
use crate::snapshot::Snapshot;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;

pub struct Store {
    db: Mutex<Connection>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListInfo {
    pub id: i64,
    pub name: String,
    pub count: usize,
}

pub const DEFAULT_LISTS: [&str; 2] = ["App Large Files", "Document Large Files"];

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
        db.execute_batch("PRAGMA foreign_keys=ON;").map_err(e)?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(e)?;
        if version < 1 {
            // Bookmark lists. Defaults are seeded once, so deleting them sticks.
            db.execute_batch(&format!(
                "BEGIN;
                 CREATE TABLE IF NOT EXISTS lists (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, position INTEGER NOT NULL);
                 CREATE TABLE IF NOT EXISTS bookmarks (
                   list_id INTEGER NOT NULL REFERENCES lists(id) ON DELETE CASCADE,
                   file_id TEXT NOT NULL, json TEXT NOT NULL, added_at TEXT NOT NULL,
                   PRIMARY KEY(list_id, file_id));
                 INSERT OR IGNORE INTO lists(name, position) VALUES ('{}', 0), ('{}', 1);
                 PRAGMA user_version = 1;
                 COMMIT;",
                DEFAULT_LISTS[0], DEFAULT_LISTS[1]
            ))
            .map_err(e)?;
        }
        if version < 2 {
            db.execute_batch(
                "BEGIN;
                 CREATE TABLE IF NOT EXISTS snapshots (day TEXT PRIMARY KEY, json TEXT NOT NULL);
                 PRAGMA user_version = 2;
                 COMMIT;",
            )
            .map_err(e)?;
        }
        if version < 3 {
            db.execute_batch(
                "BEGIN;
                 CREATE TABLE IF NOT EXISTS usage (scope TEXT PRIMARY KEY, json TEXT NOT NULL);
                 PRAGMA user_version = 3;
                 COMMIT;",
            )
            .map_err(e)?;
        }
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

    // ------------------------------------------------------------ bookmark lists

    pub fn lists(&self) -> Res<Vec<ListInfo>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT l.id, l.name, COUNT(b.file_id) FROM lists l LEFT JOIN bookmarks b ON b.list_id=l.id
                 GROUP BY l.id ORDER BY l.position, l.id",
            )
            .map_err(e)?;
        let rows = stmt
            .query_map([], |r| Ok(ListInfo { id: r.get(0)?, name: r.get(1)?, count: r.get::<_, i64>(2)? as usize }))
            .map_err(e)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(e)
    }

    fn list_name(name: &str) -> Res<String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("list name cannot be empty".into());
        }
        Ok(name.chars().take(80).collect())
    }

    pub fn create_list(&self, name: &str) -> Res<i64> {
        let name = Self::list_name(name)?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO lists(name, position) VALUES(?1, (SELECT COALESCE(MAX(position), -1) + 1 FROM lists))",
            params![name],
        )
        .map_err(|err| if err.to_string().contains("UNIQUE") { "a list with that name already exists".into() } else { e(err) })?;
        Ok(conn.last_insert_rowid())
    }

    pub fn rename_list(&self, id: i64, name: &str) -> Res<()> {
        let name = Self::list_name(name)?;
        self.conn()
            .execute("UPDATE lists SET name=?1 WHERE id=?2", params![name, id])
            .map_err(|err| if err.to_string().contains("UNIQUE") { "a list with that name already exists".into() } else { e(err) })?;
        Ok(())
    }

    pub fn delete_list(&self, id: i64) -> Res<()> {
        self.conn().execute("DELETE FROM lists WHERE id=?1", params![id]).map_err(e)?;
        Ok(())
    }

    /// Store a snapshot of each file so the bookmark survives later scans.
    pub fn add_bookmarks(&self, list_id: i64, items: &[Candidate], added_at: &str) -> Res<()> {
        let mut conn = self.conn();
        let tx = conn.transaction().map_err(e)?;
        for c in items {
            let mut c = c.clone();
            c.ignored = false;
            let json = serde_json::to_string(&c).map_err(e)?;
            tx.execute(
                "INSERT INTO bookmarks(list_id, file_id, json, added_at) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(list_id, file_id) DO UPDATE SET json=excluded.json",
                params![list_id, c.id, json, added_at],
            )
            .map_err(e)?;
        }
        tx.commit().map_err(e)
    }

    pub fn remove_bookmarks(&self, list_id: i64, file_ids: &[String]) -> Res<()> {
        let mut conn = self.conn();
        let tx = conn.transaction().map_err(e)?;
        for id in file_ids {
            tx.execute("DELETE FROM bookmarks WHERE list_id=?1 AND file_id=?2", params![list_id, id]).map_err(e)?;
        }
        tx.commit().map_err(e)
    }

    /// Drop the files from every list (they were cleaned or no longer exist).
    pub fn remove_bookmarks_for_files(&self, file_ids: &[String]) -> Res<()> {
        self.batch("DELETE FROM bookmarks WHERE file_id=?1", file_ids)
    }

    pub fn bookmarks(&self, list_id: i64) -> Res<Vec<Candidate>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT json FROM bookmarks WHERE list_id=?1 ORDER BY added_at DESC").map_err(e)?;
        let rows = stmt.query_map(params![list_id], |r| r.get::<_, String>(0)).map_err(e)?;
        let mut out = Vec::new();
        for json in rows {
            out.push(serde_json::from_str(&json.map_err(e)?).map_err(e)?);
        }
        Ok(out)
    }

    /// Every bookmarked file across all lists (one entry per file).
    pub fn all_bookmarks(&self) -> Res<Vec<Candidate>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT json FROM bookmarks GROUP BY file_id").map_err(e)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(e)?;
        let mut out = Vec::new();
        for json in rows {
            out.push(serde_json::from_str(&json.map_err(e)?).map_err(e)?);
        }
        Ok(out)
    }

    /// file id -> names of the lists holding it.
    pub fn bookmark_index(&self) -> Res<std::collections::HashMap<String, Vec<String>>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT b.file_id, l.name FROM bookmarks b JOIN lists l ON l.id=b.list_id ORDER BY l.position, l.id")
            .map_err(e)?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(e)?;
        let mut out: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
        for row in rows {
            let (id, name) = row.map_err(e)?;
            out.entry(id).or_default().push(name);
        }
        Ok(out)
    }

    // ------------------------------------------------------------ daily snapshots

    /// Insert or replace the snapshot for its day; keeps the newest `keep` days.
    pub fn save_snapshot(&self, snap: &Snapshot, keep: usize) -> Res<()> {
        let json = serde_json::to_string(snap).map_err(e)?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO snapshots(day, json) VALUES(?1, ?2) ON CONFLICT(day) DO UPDATE SET json=excluded.json",
            params![snap.day, json],
        )
        .map_err(e)?;
        conn.execute(
            "DELETE FROM snapshots WHERE day NOT IN (SELECT day FROM snapshots ORDER BY day DESC LIMIT ?1)",
            params![keep as i64],
        )
        .map_err(e)?;
        Ok(())
    }

    /// Newest first.
    pub fn snapshots(&self, limit: usize) -> Res<Vec<Snapshot>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT json FROM snapshots ORDER BY day DESC LIMIT ?1").map_err(e)?;
        let rows = stmt.query_map(params![limit as i64], |r| r.get::<_, String>(0)).map_err(e)?;
        let mut out = Vec::new();
        for json in rows {
            out.push(serde_json::from_str(&json.map_err(e)?).map_err(e)?);
        }
        Ok(out)
    }

    // ------------------------------------------------------------ folder usage

    pub fn save_usage(&self, scope: &str, usage: &crate::usage::Usage) -> Res<()> {
        let json = serde_json::to_string(usage).map_err(e)?;
        self.conn()
            .execute(
                "INSERT INTO usage(scope, json) VALUES(?1, ?2) ON CONFLICT(scope) DO UPDATE SET json=excluded.json",
                params![scope, json],
            )
            .map_err(e)?;
        Ok(())
    }

    pub fn load_usage(&self, scope: &str) -> Res<Option<crate::usage::Usage>> {
        let json: Option<String> = self
            .conn()
            .query_row("SELECT json FROM usage WHERE scope=?1", params![scope], |r| r.get(0))
            .optional()
            .map_err(e)?;
        json.map(|j| serde_json::from_str(&j).map_err(e)).transpose()
    }
}
