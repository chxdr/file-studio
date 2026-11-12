//! SQLite for the File Studio core.
//!
//! One database file per user (`~/Library/Application Support/File Studio/fstudio.sqlite3`),
//! in WAL mode. This crate owns only the connection, the migration runner and a
//! small key-value table. **Each crate owns its own tables:** it declares a
//! [`Migrations`] list under its own module name (`"ops"`, `"path"`, `"keymap"`…)
//! and calls [`Db::migrate`] at start-up. Versions are tracked per module, so
//! crates never edit each other's schema.
//!
//! Rules for migrations:
//! - Versions start at 1 and are append-only; never edit a shipped migration.
//! - Prefix table names with the module (`ops_jobs`, `path_frecency`).
//! - A database written by a newer build is refused, not downgraded.

use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
pub use rusqlite;
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("module '{module}' schema v{found} is newer than this build supports (v{supported})")]
    TooNew {
        module: String,
        found: u32,
        supported: u32,
    },
}
pub type Result<T> = std::result::Result<T, DbError>;

/// One module's migrations: `(version, sql)`, versions 1, 2, 3… in order.
pub struct Migrations {
    pub module: &'static str,
    pub steps: &'static [(u32, &'static str)],
}

/// A shared handle to the database. Cheap to clone. Access is serialised by a
/// mutex: keep closures short and never do file I/O while holding it.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    /// Opens (or creates) the database at `path`, creating parent folders.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// An in-memory database, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             CREATE TABLE IF NOT EXISTS db_schema (
                 module  TEXT PRIMARY KEY,
                 version INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS db_kv (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Brings one module's tables up to date. Each step runs in its own transaction.
    pub fn migrate(&self, m: &Migrations) -> Result<()> {
        let mut conn = self.conn.lock();
        let current: u32 = conn
            .query_row(
                "SELECT version FROM db_schema WHERE module = ?1",
                [m.module],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let supported = m.steps.last().map(|s| s.0).unwrap_or(0);
        if current > supported {
            return Err(DbError::TooNew {
                module: m.module.into(),
                found: current,
                supported,
            });
        }
        for &(version, sql) in m.steps.iter().filter(|s| s.0 > current) {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO db_schema(module, version) VALUES (?1, ?2)
                 ON CONFLICT(module) DO UPDATE SET version = excluded.version",
                params![m.module, version],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    /// The schema version recorded for a module (0 if never migrated).
    pub fn version(&self, module: &str) -> Result<u32> {
        Ok(self
            .conn
            .lock()
            .query_row(
                "SELECT version FROM db_schema WHERE module = ?1",
                [module],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    /// Runs `f` with the connection locked.
    pub fn with<T>(&self, f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>) -> Result<T> {
        Ok(f(&mut self.conn.lock())?)
    }

    /// Reads a value from the key-value table (settings, small state).
    pub fn kv_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .lock()
            .query_row("SELECT value FROM db_kv WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO db_kv(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn kv_delete(&self, key: &str) -> Result<()> {
        self.conn
            .lock()
            .execute("DELETE FROM db_kv WHERE key = ?1", [key])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M1: Migrations = Migrations {
        module: "test",
        steps: &[(1, "CREATE TABLE test_a (x INTEGER);")],
    };
    const M2: Migrations = Migrations {
        module: "test",
        steps: &[
            (1, "CREATE TABLE test_a (x INTEGER);"),
            (2, "ALTER TABLE test_a ADD COLUMN y TEXT;"),
        ],
    };

    #[test]
    fn migrations_are_per_module_and_incremental() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/fs.sqlite3");
        let db = Db::open(&p).unwrap();
        db.migrate(&M1).unwrap();
        assert_eq!(db.version("test").unwrap(), 1);
        db.migrate(&M1).unwrap(); // idempotent
        drop(db);
        let db = Db::open(&p).unwrap();
        db.migrate(&M2).unwrap();
        assert_eq!(db.version("test").unwrap(), 2);
        db.with(|c| c.execute("INSERT INTO test_a(x, y) VALUES (1, 'a')", []))
            .unwrap();
        assert_eq!(db.version("other").unwrap(), 0);
    }

    #[test]
    fn refuses_newer_schema() {
        let db = Db::open_in_memory().unwrap();
        db.migrate(&M2).unwrap();
        assert!(matches!(
            db.migrate(&M1),
            Err(DbError::TooNew {
                found: 2,
                supported: 1,
                ..
            })
        ));
    }

    #[test]
    fn kv() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.kv_get("a").unwrap(), None);
        db.kv_set("a", "1").unwrap();
        db.kv_set("a", "2").unwrap();
        assert_eq!(db.kv_get("a").unwrap().as_deref(), Some("2"));
        db.kv_delete("a").unwrap();
        assert_eq!(db.kv_get("a").unwrap(), None);
    }
}
