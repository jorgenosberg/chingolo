//! SQLite access. Repository functions take a `&Connection` so they can be tested directly;
//! `Db::call` runs them on a blocking thread for the async commands.

pub mod chat;
pub mod library;
pub mod projects;
pub mod settings;
pub mod vectors;

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use crate::error::{AppError, Result};

/// Applied in order. The number of applied entries is stored in `PRAGMA user_version`.
const MIGRATIONS: &[&str] = &[
    include_str!("../../migrations/0001_init.sql"),
    include_str!("../../migrations/0002_cached_tokens.sql"),
    include_str!("../../migrations/0003_fork_marker.sql"),
    include_str!("../../migrations/0004_library.sql"),
    include_str!("../../migrations/0005_embeddings.sql"),
];

#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(&mut conn)?;
        Ok(Db(Arc::new(Mutex::new(conn))))
    }

    fn init(conn: &mut Connection) -> Result<()> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // The import worker writes through its own connection, so wait for it instead of failing.
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        migrate(conn)?;
        // A previous run may have been killed mid-stream.
        chat::recover_interrupted(conn)?;
        Ok(())
    }

    /// A second connection to the same file for a background worker, so long writes
    /// don't hold up the shared connection (WAL lets readers and one writer overlap).
    pub fn open_secondary(path: &Path) -> Result<Connection> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        Ok(conn)
    }

    pub async fn call<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| AppError::Internal("database lock poisoned".into()))?;
            f(&mut guard)
        })
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
    }
}

pub fn migrate(conn: &mut Connection) -> Result<()> {
    let applied = conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as usize;
    if applied > MIGRATIONS.len() {
        return Err(AppError::Invalid(
            "This data was created by a newer version of Chingui. Update the app to open it."
                .into(),
        ));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(applied) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
pub fn test_conn() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    migrate(&mut conn).unwrap();
    conn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_and_are_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        let v: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
    }

    #[test]
    fn newer_database_is_refused() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 999).unwrap();
        assert!(migrate(&mut conn).is_err());
    }

    #[test]
    fn fts5_is_available() {
        let conn = test_conn();
        conn.execute("SELECT count(*) FROM messages_fts", []).ok();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM messages_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }
}
