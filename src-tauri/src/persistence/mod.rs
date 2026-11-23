//! SQLite access. Repository functions take a `&Connection` so they can be tested directly;
//! `Db::call` runs them on a blocking thread for the async commands.

pub mod chat;
pub mod library;
pub mod projects;
pub mod saved;
pub mod settings;
pub mod sources;
pub mod usage;
pub mod vectors;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use crate::error::{AppError, Result};

/// Applied in order. The number of applied entries is stored in `PRAGMA user_version`.
const MIGRATIONS: &[&str] = &[
    include_str!("../../migrations/0001_init.sql"),
    include_str!("../../migrations/0002_drafts.sql"),
    include_str!("../../migrations/0003_saved.sql"),
    include_str!("../../migrations/0004_saved_notes.sql"),
    include_str!("../../migrations/0005_layout_errors.sql"),
];

#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        back_up_before_migrating(&conn, path, MIGRATIONS.len())?;
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

/// How many backups of older schema versions to keep.
const KEEP_BACKUPS: usize = 2;

/// Copies an existing database to `<file>.bak-v<version>` before migrations change it, so an
/// update that goes wrong can be undone by hand. A new database (version 0) needs none. If the
/// copy fails the app doesn't start, rather than migrating without a backup.
fn back_up_before_migrating(conn: &Connection, path: &Path, latest: usize) -> Result<()> {
    let version = conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as usize;
    if version == 0 || version >= latest {
        return Ok(());
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("database");
    let target = path.with_file_name(format!("{name}.bak-v{version}"));
    if target.exists() {
        std::fs::remove_file(&target)
            .map_err(|e| AppError::Internal(format!("could not replace the old backup: {e}")))?;
    }
    // VACUUM INTO writes a consistent copy that includes what is still in the WAL.
    conn.execute("VACUUM INTO ?1", [target.to_string_lossy().as_ref()])?;
    log::info!(
        "database backed up to {} before migrating from version {version}",
        target.display()
    );
    prune_backups(path, name);
    Ok(())
}

fn prune_backups(path: &Path, name: &str) {
    let Some(dir) = path.parent() else { return };
    let prefix = format!("{name}.bak-v");
    let mut found: Vec<(u64, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let v = e
                .file_name()
                .to_str()?
                .strip_prefix(&prefix)?
                .parse()
                .ok()?;
            Some((v, e.path()))
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (_, old) in found.into_iter().skip(KEEP_BACKUPS) {
        let _ = std::fs::remove_file(old);
    }
}

pub fn migrate(conn: &mut Connection) -> Result<()> {
    let applied = conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as usize;
    if applied > MIGRATIONS.len() {
        return Err(AppError::Invalid(
            "This data was created by a newer version of Chingolo. Update the app to open it."
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
    fn single_notes_become_the_first_note_of_their_item() {
        // A database as 0.2.0 development left it: schema version 3, one item with a note.
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..3] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 3i64).unwrap();
        for (id, note) in [("a", "  my note "), ("b", ""), ("c", "   ")] {
            conn.execute(
                "INSERT INTO saved_items (id, kind, body, note, created_at, updated_at)
                 VALUES (?1, 'reply', 'text', ?2, 10, 20)",
                rusqlite::params![id, note],
            )
            .unwrap();
        }
        migrate(&mut conn).unwrap();
        let notes: Vec<(String, String, i64)> = conn
            .prepare("SELECT saved_item_id, body, created_at FROM saved_notes")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(notes, [("a".to_string(), "my note".to_string(), 20)]);
    }

    #[test]
    fn documents_from_schema_four_start_with_no_layout_failures() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..4] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 4i64).unwrap();
        let doc =
            library::insert_document(&conn, "h1", "a.pdf", crate::domain::library::DocKind::Pdf)
                .unwrap();
        migrate(&mut conn).unwrap();
        let (error, attempts): (Option<String>, i64) = conn
            .query_row(
                "SELECT layout_error, layout_attempts FROM documents WHERE id = ?1",
                [&doc.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((error, attempts), (None, 0));
    }

    #[test]
    fn an_older_database_is_backed_up_before_migrating_and_old_backups_are_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite");
        let mut conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrate(&mut conn).unwrap();
        conn.execute("INSERT INTO settings (key, value) VALUES ('k', 'v')", [])
            .unwrap();

        // The copy is named after the schema version the database is at.
        let backup = format!("test.sqlite.bak-v{}", MIGRATIONS.len());

        // Up to date: nothing to back up.
        back_up_before_migrating(&conn, &path, MIGRATIONS.len()).unwrap();
        assert!(!dir.path().join(&backup).exists());

        // A newer schema is coming: the copy has the data, including what sits in the WAL.
        back_up_before_migrating(&conn, &path, MIGRATIONS.len() + 1).unwrap();
        let copy = Connection::open(dir.path().join(&backup)).unwrap();
        let v: String = copy
            .query_row("SELECT value FROM settings WHERE key = 'k'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(v, "v");

        // Only the newest two stay.
        for n in [
            MIGRATIONS.len() + 1,
            MIGRATIONS.len() + 2,
            MIGRATIONS.len() + 3,
        ] {
            std::fs::write(dir.path().join(format!("test.sqlite.bak-v{n}")), b"x").unwrap();
        }
        prune_backups(&path, "test.sqlite");
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(String::from))
            .filter(|n| n.contains(".bak-v"))
            .collect();
        left.sort();
        let (a, b) = (MIGRATIONS.len() + 2, MIGRATIONS.len() + 3);
        assert_eq!(
            left,
            [
                format!("test.sqlite.bak-v{a}"),
                format!("test.sqlite.bak-v{b}")
            ]
        );
    }

    #[test]
    fn a_new_database_needs_no_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.sqlite");
        let conn = Connection::open(&path).unwrap();
        back_up_before_migrating(&conn, &path, 5).unwrap();
        assert!(!dir.path().join("new.sqlite.bak-v0").exists());
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
