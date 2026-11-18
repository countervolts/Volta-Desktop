//! Persistent storage. The web's `localStorage` becomes a SQLite `kv` table
//! with the same key names and JSON value shapes. `sessionStorage` becomes
//! an in-memory map that lives for the process.
//!
//! The database lives in the platform data directory
//! (`ProjectDirs::from("xyz", "voltamusic", "Volta")`), or in
//! `$VOLTA_DATA_DIR` when that is set (tests and screenshots use it).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

const SCHEMA_VERSION: i64 = 1;

/// The SQLite library bundled into this build.
pub fn sqlite_version() -> &'static str {
    rusqlite::version()
}

/// The database layout version Volta writes.
pub fn schema_version() -> i64 {
    SCHEMA_VERSION
}

#[derive(Clone)]
pub struct Storage {
    db: Arc<Mutex<Option<Connection>>>,
    session: Arc<Mutex<HashMap<String, String>>>,
    path: Option<PathBuf>,
}

pub fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("VOLTA_DATA_DIR") {
        return Some(PathBuf::from(dir));
    }
    directories::ProjectDirs::from("xyz", "voltamusic", "Volta").map(|dirs| dirs.data_dir().to_path_buf())
}

fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version < 1 {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);",
        )?;
    }
    connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

impl Storage {
    /// Opens the default database. Storage is optional on the web, so a
    /// failure here degrades to an in-memory database instead of aborting.
    pub fn open_default() -> Self {
        match data_dir() {
            Some(dir) => Self::open_in(&dir),
            None => Self::in_memory(),
        }
    }

    pub fn open_in(dir: &Path) -> Self {
        let path = dir.join("volta.sqlite3");
        let opened = std::fs::create_dir_all(dir)
            .map_err(|e| e.to_string())
            .and_then(|_| Connection::open(&path).map_err(|e| e.to_string()))
            .and_then(|c| migrate(&c).map(|_| c).map_err(|e| e.to_string()));
        match opened {
            Ok(connection) => Self {
                db: Arc::new(Mutex::new(Some(connection))),
                session: Default::default(),
                path: Some(path),
            },
            Err(error) => {
                log::warn!("storage unavailable ({error}); settings will not persist");
                Self::in_memory()
            }
        }
    }

    pub fn in_memory() -> Self {
        let connection = Connection::open_in_memory().ok().filter(|c| migrate(c).is_ok());
        Self {
            db: Arc::new(Mutex::new(connection)),
            session: Default::default(),
            path: None,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// `safeRead(localStorage, key)`: `""` when missing.
    pub fn get(&self, key: &str) -> String {
        let guard = self.db.lock();
        let Some(db) = guard.as_ref() else { return String::new() };
        db.query_row("SELECT value FROM kv WHERE key = ?1", params![key], |row| row.get(0))
            .optional()
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    pub fn set(&self, key: &str, value: &str) {
        let guard = self.db.lock();
        if let Some(db) = guard.as_ref()
            && let Err(error) = db.execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            ) {
                log::warn!("storage write failed for {key}: {error}");
            }
    }

    pub fn remove(&self, key: &str) {
        let guard = self.db.lock();
        if let Some(db) = guard.as_ref() {
            let _ = db.execute("DELETE FROM kv WHERE key = ?1", params![key]);
        }
    }

    /// `safeRead(sessionStorage, key)`
    pub fn session_get(&self, key: &str) -> String {
        self.session.lock().get(key).cloned().unwrap_or_default()
    }

    pub fn session_set(&self, key: &str, value: &str) {
        self.session.lock().insert(key.to_string(), value.to_string());
    }

    pub fn session_remove(&self, key: &str) {
        self.session.lock().remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_round_trip_and_session_isolation() {
        let dir = std::env::temp_dir().join(format!("volta-storage-test-{}", std::process::id()));
        let storage = Storage::open_in(&dir);
        assert_eq!(storage.get("volta-theme"), "");
        storage.set("volta-theme", "light");
        storage.set("volta-theme", "high-contrast");
        assert_eq!(storage.get("volta-theme"), "high-contrast");
        storage.session_set("volta-session-credentials", "{}");
        assert_eq!(storage.get("volta-session-credentials"), "");
        drop(storage);
        let reopened = Storage::open_in(&dir);
        assert_eq!(reopened.get("volta-theme"), "high-contrast");
        assert_eq!(reopened.session_get("volta-session-credentials"), "");
        reopened.remove("volta-theme");
        assert_eq!(reopened.get("volta-theme"), "");
        let _ = std::fs::remove_dir_all(dir);
    }
}
