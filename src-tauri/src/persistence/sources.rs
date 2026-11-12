//! Handles (`[S12]`) for the passages a conversation has shown to the model.

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::now_ms;
use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub handle: u32,
    pub passage_id: String,
    pub document_id: String,
    pub title: String,
    /// Author and year, empty if the document had neither.
    pub label: String,
    /// First page, 0 for DOCX.
    pub page: u32,
    pub text: String,
}

/// What a tool knows about a passage it is about to show.
pub struct NewSource<'a> {
    pub passage_id: &'a str,
    pub document_id: &'a str,
    pub title: &'a str,
    pub label: &'a str,
    pub page: u32,
    pub text: &'a str,
}

const COLS: &str = "handle, passage_id, document_id, title, page, text, label";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Source> {
    Ok(Source {
        handle: r.get(0)?,
        passage_id: r.get(1)?,
        document_id: r.get(2)?,
        title: r.get(3)?,
        page: r.get(4)?,
        text: r.get(5)?,
        label: r.get(6)?,
    })
}

/// The handle for a passage, assigning the next number the first time it is shown. Call it before
/// the model sees the passage. The same passage always keeps its handle within a conversation.
pub fn handle_for(conn: &Connection, conversation_id: &str, s: &NewSource) -> Result<u32> {
    if let Some(h) = conn
        .query_row(
            "SELECT handle FROM conversation_sources WHERE conversation_id = ?1 AND passage_id = ?2",
            params![conversation_id, s.passage_id],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(h);
    }
    conn.execute(
        "INSERT INTO conversation_sources
           (conversation_id, handle, passage_id, document_id, title, page, text, created_at, label)
         VALUES (?1,
                 (SELECT COALESCE(MAX(handle), 0) + 1 FROM conversation_sources WHERE conversation_id = ?1),
                 ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![conversation_id, s.passage_id, s.document_id, s.title, s.page, s.text, now_ms(), s.label],
    )?;
    Ok(conn.query_row(
        "SELECT handle FROM conversation_sources WHERE conversation_id = ?1 AND passage_id = ?2",
        params![conversation_id, s.passage_id],
        |r| r.get(0),
    )?)
}

pub fn get(conn: &Connection, conversation_id: &str, handle: u32) -> Result<Option<Source>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLS} FROM conversation_sources WHERE conversation_id = ?1 AND handle = ?2"),
            params![conversation_id, handle],
            from_row,
        )
        .optional()?)
}

pub fn list(conn: &Connection, conversation_id: &str) -> Result<Vec<Source>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM conversation_sources WHERE conversation_id = ?1 ORDER BY handle"
    ))?;
    let rows = stmt.query_map([conversation_id], from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Gives a fork the same handles as its source, so copied citations keep resolving.
pub fn copy_all(conn: &Connection, from: &str, to: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO conversation_sources
           (conversation_id, handle, passage_id, document_id, title, page, text, created_at, label)
         SELECT ?2, handle, passage_id, document_id, title, page, text, created_at, label
           FROM conversation_sources WHERE conversation_id = ?1",
        params![from, to],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{chat, test_conn};

    fn src<'a>(passage: &'a str, text: &'a str) -> NewSource<'a> {
        NewSource { passage_id: passage, document_id: "d1", title: "A paper", label: "Boyd 2004", page: 3, text }
    }

    #[test]
    fn handles_count_up_per_conversation_and_stay_stable() {
        let conn = test_conn();
        let a = chat::create_conversation(&conn, None, None, "gemini", Some("m")).unwrap();
        let b = chat::create_conversation(&conn, None, None, "gemini", Some("m")).unwrap();
        assert_eq!(handle_for(&conn, &a.id, &src("p1", "one")).unwrap(), 1);
        assert_eq!(handle_for(&conn, &a.id, &src("p2", "two")).unwrap(), 2);
        assert_eq!(handle_for(&conn, &a.id, &src("p1", "changed")).unwrap(), 1, "same passage, same handle");
        assert_eq!(handle_for(&conn, &b.id, &src("p2", "two")).unwrap(), 1, "numbering is per conversation");
        let first = get(&conn, &a.id, 1).unwrap().unwrap();
        assert_eq!(first.text, "one", "the snapshot is not overwritten");
        assert_eq!((first.title.as_str(), first.page), ("A paper", 3));
        assert_eq!(get(&conn, &a.id, 9).unwrap(), None);
        assert_eq!(list(&conn, &a.id).unwrap().len(), 2);
    }

    #[test]
    fn sources_go_with_the_conversation_and_copy_to_forks() {
        let conn = test_conn();
        let a = chat::create_conversation(&conn, None, None, "gemini", Some("m")).unwrap();
        handle_for(&conn, &a.id, &src("p1", "one")).unwrap();
        handle_for(&conn, &a.id, &src("p2", "two")).unwrap();
        let b = chat::create_conversation(&conn, None, None, "gemini", Some("m")).unwrap();
        copy_all(&conn, &a.id, &b.id).unwrap();
        assert_eq!(list(&conn, &b.id).unwrap(), list(&conn, &a.id).unwrap());
        assert_eq!(handle_for(&conn, &b.id, &src("p3", "three")).unwrap(), 3, "numbering continues in the copy");

        chat::delete_conversation(&conn, &a.id).unwrap();
        assert!(list(&conn, &a.id).unwrap().is_empty());
        assert_eq!(list(&conn, &b.id).unwrap().len(), 3);
    }
}
