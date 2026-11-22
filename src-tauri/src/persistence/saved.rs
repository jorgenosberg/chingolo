//! Saved items: replies, selections and reader passages kept with a note. Each item copies its
//! text and the sources it cites, so it outlives the conversation and the document.

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::domain::{Id, SavedFilter, SavedItem, SavedKind, SavedSource, new_id, now_ms};
use crate::error::{AppError, Result};
use crate::persistence::{chat, library, sources};

/// A selection longer than this is not a quote worth keeping.
const MAX_TEXT_CHARS: usize = 20_000;
const MAX_NOTE_CHARS: usize = 10_000;

fn kind_str(k: SavedKind) -> &'static str {
    match k {
        SavedKind::Reply => "reply",
        SavedKind::Selection => "selection",
        SavedKind::Passage => "passage",
    }
}

fn kind_from(s: &str) -> SavedKind {
    match s {
        "selection" => SavedKind::Selection,
        "passage" => SavedKind::Passage,
        _ => SavedKind::Reply,
    }
}

const COLS: &str = "id, kind, project_id, conversation_id, message_id, conversation_title, body, \
                    note, created_at, updated_at";

fn from_row(r: &Row) -> rusqlite::Result<SavedItem> {
    Ok(SavedItem {
        id: r.get(0)?,
        kind: kind_from(&r.get::<_, String>(1)?),
        project_id: r.get(2)?,
        conversation_id: r.get(3)?,
        message_id: r.get(4)?,
        conversation_title: r.get(5)?,
        body: r.get(6)?,
        note: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
        sources: Vec::new(),
    })
}

fn source_from_row(r: &Row) -> rusqlite::Result<SavedSource> {
    Ok(SavedSource {
        handle: r.get(0)?,
        passage_id: r.get(1)?,
        document_id: r.get(2)?,
        title: r.get(3)?,
        label: r.get(4)?,
        page: r.get(5)?,
        text: r.get(6)?,
        doi: r.get(7)?,
    })
}

const SOURCE_COLS: &str = "handle, passage_id, document_id, title, label, page, text, doi";

fn load_sources(conn: &Connection, item_id: &str) -> Result<Vec<SavedSource>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {SOURCE_COLS} FROM saved_item_sources WHERE saved_item_id = ?1 ORDER BY handle"
    ))?;
    let rows = stmt.query_map([item_id], source_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn get(conn: &Connection, id: &str) -> Result<SavedItem> {
    let mut item = conn
        .query_row(
            &format!("SELECT {COLS} FROM saved_items WHERE id = ?1"),
            [id],
            from_row,
        )
        .optional()?
        .ok_or(AppError::NotFound)?;
    item.sources = load_sources(conn, id)?;
    Ok(item)
}

/// Newest first.
pub fn list(conn: &Connection, filter: &SavedFilter) -> Result<Vec<SavedItem>> {
    let (clause, arg): (&str, Option<&str>) = match filter {
        SavedFilter::All => ("1 = 1", None),
        SavedFilter::Project { id } => ("project_id = ?1", Some(id.as_str())),
        SavedFilter::Unassigned => ("project_id IS NULL", None),
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM saved_items WHERE {clause} ORDER BY created_at DESC, id DESC"
    ))?;
    let mut items: Vec<SavedItem> = match arg {
        Some(a) => stmt.query_map([a], from_row)?,
        None => stmt.query_map([], from_row)?,
    }
    .collect::<rusqlite::Result<_>>()?;

    // One query for every item's sources, then hand them out.
    let mut src_stmt = conn.prepare(&format!(
        "SELECT saved_item_id, {SOURCE_COLS} FROM saved_item_sources ORDER BY saved_item_id, handle"
    ))?;
    let mut by_item: std::collections::HashMap<String, Vec<SavedSource>> = Default::default();
    let rows = src_stmt.query_map([], |r| {
        let id: String = r.get(0)?;
        Ok((
            id,
            SavedSource {
                handle: r.get(1)?,
                passage_id: r.get(2)?,
                document_id: r.get(3)?,
                title: r.get(4)?,
                label: r.get(5)?,
                page: r.get(6)?,
                text: r.get(7)?,
                doi: r.get(8)?,
            },
        ))
    })?;
    for row in rows {
        let (id, s) = row?;
        by_item.entry(id).or_default().push(s);
    }
    for item in &mut items {
        item.sources = by_item.remove(&item.id).unwrap_or_default();
    }
    Ok(items)
}

fn doi_of(conn: &Connection, document_id: &str) -> Option<String> {
    library::get_document(conn, document_id)
        .ok()
        .and_then(|d| d.doi)
        .filter(|d| !d.trim().is_empty())
}

fn insert_item(
    conn: &mut Connection,
    kind: SavedKind,
    project_id: Option<&str>,
    conversation_id: Option<&str>,
    message_id: Option<&str>,
    conversation_title: &str,
    body: &str,
    srcs: &[SavedSource],
) -> Result<SavedItem> {
    let id: Id = new_id();
    let now = now_ms();
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO saved_items
           (id, kind, project_id, conversation_id, message_id, conversation_title, body, note,
            created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '', ?8, ?8)",
        params![
            id,
            kind_str(kind),
            project_id,
            conversation_id,
            message_id,
            conversation_title,
            body,
            now
        ],
    )?;
    for s in srcs {
        tx.execute(
            "INSERT INTO saved_item_sources
               (saved_item_id, handle, passage_id, document_id, title, label, page, text, doi)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                s.handle,
                s.passage_id,
                s.document_id,
                s.title,
                s.label,
                s.page,
                s.text,
                s.doi
            ],
        )?;
    }
    tx.commit()?;
    get(conn, &id)
}

/// The sources of a conversation for the given handles. A handle the conversation never gave out
/// is dropped, as it is for citations.
fn conversation_sources(
    conn: &Connection,
    conversation_id: &str,
    handles: &[u32],
) -> Result<Vec<SavedSource>> {
    let mut out: Vec<SavedSource> = Vec::new();
    for &h in handles {
        if out.iter().any(|s| s.handle == h) {
            continue;
        }
        if let Some(s) = sources::get(conn, conversation_id, h)? {
            out.push(SavedSource {
                doi: doi_of(conn, &s.document_id),
                handle: s.handle,
                passage_id: s.passage_id,
                document_id: s.document_id,
                title: s.title,
                label: s.label,
                page: s.page,
                text: s.text,
            });
        }
    }
    out.sort_by_key(|s| s.handle);
    Ok(out)
}

/// Saves a reply. Saving the same reply again returns the item that is already there.
pub fn save_reply(conn: &mut Connection, message_id: &str) -> Result<SavedItem> {
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM saved_items WHERE message_id = ?1 AND kind = 'reply'",
            [message_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        return get(conn, &id);
    }
    let msg = chat::get_message(conn, message_id)?;
    if msg.role != crate::domain::Role::Assistant || msg.content.trim().is_empty() {
        return Err(AppError::Invalid(
            "Only a reply with text can be saved.".into(),
        ));
    }
    let conv = chat::get_conversation(conn, &msg.conversation_id)?;
    let handles: Vec<u32> = msg.citations.iter().map(|c| c.handle).collect();
    let srcs = conversation_sources(conn, &conv.id, &handles)?;
    insert_item(
        conn,
        SavedKind::Reply,
        conv.project_id.as_deref(),
        Some(&conv.id),
        Some(&msg.id),
        &conv.title,
        &msg.content,
        &srcs,
    )
}

pub fn save_selection(
    conn: &mut Connection,
    message_id: &str,
    text: &str,
    handles: &[u32],
) -> Result<SavedItem> {
    let text = clean_text(text)?;
    let msg = chat::get_message(conn, message_id)?;
    let conv = chat::get_conversation(conn, &msg.conversation_id)?;
    let srcs = conversation_sources(conn, &conv.id, handles)?;
    insert_item(
        conn,
        SavedKind::Selection,
        conv.project_id.as_deref(),
        Some(&conv.id),
        Some(&msg.id),
        &conv.title,
        &text,
        &srcs,
    )
}

/// Saves text selected in the reader. The passage it came from is found by text, so a selection
/// that crosses a passage boundary or differs in hyphenation is kept without one and opens at
/// the page.
pub fn save_passage(
    conn: &mut Connection,
    document_id: &str,
    page: u32,
    text: &str,
    project_id: Option<&str>,
) -> Result<SavedItem> {
    let text = clean_text(text)?;
    let doc = library::get_document(conn, document_id)?;
    let passage_id = match_passage(conn, document_id, page, &text)?.unwrap_or_default();
    let src = SavedSource {
        handle: 1,
        passage_id,
        document_id: doc.id.clone(),
        title: doc
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| doc.original_name.clone()),
        label: doc.label(),
        page,
        text: text.clone(),
        doi: doc.doi.clone().filter(|d| !d.trim().is_empty()),
    };
    insert_item(
        conn,
        SavedKind::Passage,
        project_id,
        None,
        None,
        "",
        &text,
        &[src],
    )
}

fn clean_text(raw: &str) -> Result<String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(AppError::Invalid("Nothing is selected.".into()));
    }
    if t.chars().count() > MAX_TEXT_CHARS {
        return Err(AppError::Invalid(
            "The selection is too long to save.".into(),
        ));
    }
    Ok(t.to_string())
}

fn squash(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The passage on this page that contains the selected text, if exactly one does.
pub fn match_passage(
    conn: &Connection,
    document_id: &str,
    page: u32,
    text: &str,
) -> Result<Option<String>> {
    let needle = squash(text);
    if needle.is_empty() {
        return Ok(None);
    }
    let mut hits = library::passages_on_page(conn, document_id, page, 200)?
        .into_iter()
        .filter(|h| squash(&h.text).contains(&needle));
    let first = hits.next();
    Ok(match (first, hits.next()) {
        (Some(h), None) => Some(h.passage_id),
        _ => None,
    })
}

pub fn update_note(conn: &Connection, id: &str, note: &str) -> Result<SavedItem> {
    let note = note.trim();
    if note.chars().count() > MAX_NOTE_CHARS {
        return Err(AppError::Invalid("The note is too long.".into()));
    }
    let n = conn.execute(
        "UPDATE saved_items SET note = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, note, now_ms()],
    )?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    get(conn, id)
}

pub fn set_project(conn: &Connection, id: &str, project_id: Option<&str>) -> Result<SavedItem> {
    let n = conn.execute(
        "UPDATE saved_items SET project_id = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, project_id, now_ms()],
    )?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    get(conn, id)
}

pub fn delete(conn: &Connection, id: &str) -> Result<()> {
    let n = conn.execute("DELETE FROM saved_items WHERE id = ?1", [id])?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

pub fn get_source(conn: &Connection, item_id: &str, handle: u32) -> Result<Option<SavedSource>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {SOURCE_COLS} FROM saved_item_sources WHERE saved_item_id = ?1 AND handle = ?2"
            ),
            params![item_id, handle],
            source_from_row,
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Citation, CitationStatus, MessageStatus, Role};
    use crate::persistence::{projects, test_conn};

    fn cite(handle: u32) -> Citation {
        Citation {
            handle,
            status: CitationStatus::Ok,
            quote_verified: None,
            passage_id: format!("p{handle}"),
            document_id: "d1".into(),
            title: "A paper".into(),
            label: "Boyd 2004".into(),
            page: 3,
            snippet: String::new(),
        }
    }

    /// A project, a conversation in it, and one reply citing handles 1 and 2.
    fn setup(conn: &Connection) -> (Id, Id, Id) {
        let p = projects::create_project(conn, "Thesis").unwrap();
        let c = chat::create_conversation(conn, Some(&p.id), None, "gemini", Some("m")).unwrap();
        for (i, text) in ["first source", "second source"].iter().enumerate() {
            sources::handle_for(
                conn,
                &c.id,
                &sources::NewSource {
                    passage_id: &format!("p{}", i + 1),
                    document_id: "d1",
                    title: "A paper",
                    label: "Boyd 2004",
                    page: 3,
                    text,
                },
            )
            .unwrap();
        }
        let m = chat::insert_message(
            conn,
            &c.id,
            Role::Assistant,
            "Growth rose [S1]. It fell later [S2].",
            MessageStatus::Complete,
            None,
            None,
        )
        .unwrap();
        chat::set_citations(
            conn,
            &m.id,
            &serde_json::to_string(&[cite(1), cite(2)]).unwrap(),
        )
        .unwrap();
        (p.id, c.id, m.id)
    }

    #[test]
    fn a_reply_is_saved_once_with_its_sources_and_project() {
        let mut conn = test_conn();
        let (project, conv, msg) = setup(&conn);
        let a = save_reply(&mut conn, &msg).unwrap();
        let b = save_reply(&mut conn, &msg).unwrap();
        assert_eq!(a.id, b.id, "saving twice keeps one item");
        assert_eq!(a.kind, SavedKind::Reply);
        assert_eq!(a.project_id.as_deref(), Some(project.as_str()));
        assert_eq!(a.conversation_id.as_deref(), Some(conv.as_str()));
        assert_eq!(a.body, "Growth rose [S1]. It fell later [S2].");
        assert_eq!(a.sources.len(), 2);
        assert_eq!(list(&conn, &SavedFilter::All).unwrap().len(), 1);
        assert_eq!(
            list(&conn, &SavedFilter::Project { id: project }).unwrap()[0]
                .sources
                .len(),
            2
        );
    }

    #[test]
    fn a_selection_keeps_only_its_own_handles() {
        let mut conn = test_conn();
        let (_, _, msg) = setup(&conn);
        // 9 was never given out and 2 is listed twice.
        let item = save_selection(&mut conn, &msg, "  It fell later.  ", &[2, 9, 2]).unwrap();
        assert_eq!(item.kind, SavedKind::Selection);
        assert_eq!(item.body, "It fell later.");
        assert_eq!(item.sources.len(), 1);
        assert_eq!(item.sources[0].handle, 2);
        assert!(save_selection(&mut conn, &msg, "   ", &[1]).is_err());
    }

    #[test]
    fn an_item_survives_deleting_its_conversation_and_project() {
        let mut conn = test_conn();
        let (project, conv, msg) = setup(&conn);
        let item = save_reply(&mut conn, &msg).unwrap();

        chat::delete_conversation(&conn, &conv).unwrap();
        let after = get(&conn, &item.id).unwrap();
        assert_eq!(after.conversation_id, None);
        assert_eq!(after.message_id, None);
        assert_eq!(after.conversation_title, "New conversation");
        assert_eq!(after.sources.len(), 2, "sources are the item's own copy");
        assert_eq!(
            get_source(&conn, &item.id, 1).unwrap().unwrap().text,
            "first source"
        );

        projects::delete_project(&conn, &project).unwrap();
        let after = get(&conn, &item.id).unwrap();
        assert_eq!(after.project_id, None);
        assert_eq!(list(&conn, &SavedFilter::Unassigned).unwrap().len(), 1);
    }

    #[test]
    fn note_project_and_delete() {
        let mut conn = test_conn();
        let (project, _, msg) = setup(&conn);
        let item = save_reply(&mut conn, &msg).unwrap();
        assert_eq!(
            update_note(&conn, &item.id, "  use in chapter 2 ")
                .unwrap()
                .note,
            "use in chapter 2"
        );
        assert_eq!(set_project(&conn, &item.id, None).unwrap().project_id, None);
        assert_eq!(
            set_project(&conn, &item.id, Some(&project))
                .unwrap()
                .project_id
                .as_deref(),
            Some(project.as_str())
        );
        delete(&conn, &item.id).unwrap();
        assert!(matches!(get(&conn, &item.id), Err(AppError::NotFound)));
        assert!(matches!(delete(&conn, &item.id), Err(AppError::NotFound)));
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM saved_item_sources", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "sources go with the item"
        );
    }

    #[test]
    fn a_reader_selection_finds_its_passage_or_falls_back_to_the_page() {
        let mut conn = test_conn();
        let doc = library::insert_document(
            &conn,
            "h1",
            "paper.pdf",
            crate::domain::library::DocKind::Pdf,
        )
        .unwrap();
        for (i, text) in [
            "Iron limits\nplankton growth.",
            "Iron limits plankton growth.",
        ]
        .iter()
        .enumerate()
        {
            // Two passages on page 2 with the same words, one on page 1.
            conn.execute(
                "INSERT INTO passages (id, document_id, ordinal, page_start, page_end, text, text_folded)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?5)",
                params![format!("p{i}"), doc.id, i as i64, if i == 0 { 1 } else { 2 }, text],
            )
            .unwrap();
        }
        assert_eq!(
            match_passage(&conn, &doc.id, 1, "iron limits plankton")
                .unwrap()
                .as_deref(),
            Some("p0"),
            "case and line breaks do not matter"
        );
        assert_eq!(
            match_passage(&conn, &doc.id, 2, "not in the paper").unwrap(),
            None
        );

        let hit =
            save_passage(&mut conn, &doc.id, 2, "Iron limits plankton growth.", None).unwrap();
        assert_eq!(hit.kind, SavedKind::Passage);
        assert_eq!(
            (hit.sources.len(), hit.sources[0].passage_id.as_str()),
            (1, "p1")
        );
        let miss = save_passage(&mut conn, &doc.id, 2, "words from two passages", None).unwrap();
        assert_eq!(
            miss.sources[0].passage_id, "",
            "no match, the page is the fallback"
        );
        assert_eq!(miss.sources[0].page, 2);
        assert!(save_passage(&mut conn, "missing", 1, "x", None).is_err());
    }

    #[test]
    fn squash_ignores_case_and_line_breaks() {
        assert_eq!(squash("Growth\n  rose  Fast"), "growth rose fast");
    }
}
