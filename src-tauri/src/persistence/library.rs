//! Documents, pages and passages. The ingestion worker and the library commands both use these.

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::domain::library::{
    DocKind, DocStatus, Document, MetadataPatch, SearchHit, fold,
};
use crate::domain::{Id, clean_name, new_id, now_ms};
use crate::error::{AppError, Result};

const DOC_COLS: &str = "id, content_hash, original_name, kind, title, authors, year, doi, page_count,
    status, error, created_at, updated_at,
    (SELECT count(*) FROM passages p WHERE p.document_id = documents.id)";

fn document_from_row(r: &Row) -> rusqlite::Result<Document> {
    let kind: String = r.get(3)?;
    let authors: String = r.get(5)?;
    let status: String = r.get(9)?;
    Ok(Document {
        id: r.get(0)?,
        content_hash: r.get(1)?,
        original_name: r.get(2)?,
        kind: DocKind::parse(&kind).ok_or(rusqlite::Error::InvalidQuery)?,
        title: r.get(4)?,
        authors: serde_json::from_str(&authors).unwrap_or_default(),
        year: r.get(6)?,
        doi: r.get(7)?,
        page_count: r.get(8)?,
        status: DocStatus::parse(&status).ok_or(rusqlite::Error::InvalidQuery)?,
        error: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
        passage_count: r.get(13)?,
    })
}

pub fn get_document(conn: &Connection, id: &str) -> Result<Document> {
    conn.query_row(
        &format!("SELECT {DOC_COLS} FROM documents WHERE id = ?1"),
        [id],
        document_from_row,
    )
    .optional()?
    .ok_or(AppError::NotFound)
}

pub fn find_by_hash(conn: &Connection, hash: &str) -> Result<Option<Document>> {
    Ok(conn
        .query_row(
            &format!("SELECT {DOC_COLS} FROM documents WHERE content_hash = ?1"),
            [hash],
            document_from_row,
        )
        .optional()?)
}

/// All documents, newest first, or only those attached to a project.
pub fn list_documents(conn: &Connection, project_id: Option<&str>) -> Result<Vec<Document>> {
    let (sql, args): (String, Vec<&dyn rusqlite::ToSql>) = match &project_id {
        Some(p) => (
            format!(
                "SELECT {DOC_COLS} FROM documents
                  WHERE id IN (SELECT document_id FROM project_documents WHERE project_id = ?1)
                  ORDER BY created_at DESC, id DESC"
            ),
            vec![p],
        ),
        None => (
            format!("SELECT {DOC_COLS} FROM documents ORDER BY created_at DESC, id DESC"),
            vec![],
        ),
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(args.as_slice(), document_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn insert_document(
    conn: &Connection,
    content_hash: &str,
    original_name: &str,
    kind: DocKind,
) -> Result<Document> {
    let (id, now) = (new_id(), now_ms());
    conn.execute(
        "INSERT INTO documents (id, content_hash, original_name, kind, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'queued', ?5, ?5)",
        params![id, content_hash, original_name, kind.as_str(), now],
    )?;
    get_document(conn, &id)
}

pub fn attach(conn: &Connection, project_id: &str, document_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO project_documents (project_id, document_id, added_at) VALUES (?1, ?2, ?3)",
        params![project_id, document_id, now_ms()],
    )?;
    Ok(())
}

pub fn detach(conn: &Connection, project_id: &str, document_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM project_documents WHERE project_id = ?1 AND document_id = ?2",
        params![project_id, document_id],
    )?;
    Ok(())
}

pub fn set_status(conn: &Connection, id: &str, status: DocStatus, error: Option<&str>) -> Result<Document> {
    let n = conn.execute(
        "UPDATE documents SET status = ?1, error = ?2, updated_at = ?3 WHERE id = ?4",
        params![status.as_str(), error, now_ms(), id],
    )?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    get_document(conn, id)
}

/// Returns the content hash and kind so the caller can remove the managed file.
/// Only call this once no other document uses the file (hashes are unique, so none does).
pub fn delete_document(conn: &Connection, id: &str) -> Result<(String, DocKind)> {
    let doc = get_document(conn, id)?;
    conn.execute("DELETE FROM documents WHERE id = ?1", [id])?;
    Ok((doc.content_hash, doc.kind))
}

pub fn update_metadata(conn: &Connection, id: &str, patch: &MetadataPatch) -> Result<Document> {
    get_document(conn, id)?;
    let blank = |s: &str| if s.trim().is_empty() { None } else { Some(s.trim().to_string()) };
    if let Some(t) = &patch.title {
        let t = blank(t).map(|t| clean_name(&t, "Title")).transpose()?;
        conn.execute("UPDATE documents SET title = ?1 WHERE id = ?2", params![t, id])?;
    }
    if let Some(a) = &patch.authors {
        let a: Vec<String> = a.iter().filter_map(|s| blank(s)).collect();
        conn.execute(
            "UPDATE documents SET authors = ?1 WHERE id = ?2",
            params![serde_json::to_string(&a).unwrap_or_else(|_| "[]".into()), id],
        )?;
    }
    if let Some(y) = patch.year {
        if y != 0 && !(1000..=2200).contains(&y) {
            return Err(AppError::Invalid("That year doesn't look right.".into()));
        }
        conn.execute(
            "UPDATE documents SET year = ?1 WHERE id = ?2",
            params![(y != 0).then_some(y), id],
        )?;
    }
    if let Some(d) = &patch.doi {
        conn.execute("UPDATE documents SET doi = ?1 WHERE id = ?2", params![blank(d), id])?;
    }
    conn.execute("UPDATE documents SET updated_at = ?1 WHERE id = ?2", params![now_ms(), id])?;
    get_document(conn, id)
}

/// Documents that were queued or being processed when the app stopped. They go back in the queue.
pub fn requeue_unfinished(conn: &Connection) -> Result<Vec<Document>> {
    conn.execute(
        "UPDATE documents SET status = 'queued', updated_at = ?1 WHERE status IN ('queued', 'processing')",
        [now_ms()],
    )?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {DOC_COLS} FROM documents WHERE status = 'queued' ORDER BY created_at, id"
    ))?;
    let rows = stmt.query_map([], document_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub struct NewPage {
    pub page_no: u32,
    pub width: f32,
    pub height: f32,
    /// "ok", "low" or "none"
    pub quality: &'static str,
}

pub struct NewPassage {
    pub page_start: u32,
    pub page_end: u32,
    pub text: String,
    /// JSON, see the `passages.boxes` column.
    pub boxes: Option<String>,
}

/// Metadata read from the file. Only fills fields the document doesn't have yet,
/// so a re-run never overwrites what the user corrected.
#[derive(Debug, Default, Clone)]
pub struct Extracted {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub doi: Option<String>,
}

/// Replaces everything indexed for a document in one transaction, then sets its final status.
pub fn store_content(
    conn: &mut Connection,
    id: &str,
    pages: &[NewPage],
    passages: &[NewPassage],
    meta: &Extracted,
    status: DocStatus,
    note: Option<&str>,
) -> Result<Document> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM passages WHERE document_id = ?1", [id])?;
    tx.execute("DELETE FROM pages WHERE document_id = ?1", [id])?;
    for p in pages {
        tx.execute(
            "INSERT INTO pages (document_id, page_no, width, height, quality) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, p.page_no, p.width, p.height, p.quality],
        )?;
    }
    for (i, p) in passages.iter().enumerate() {
        tx.execute(
            "INSERT INTO passages (id, document_id, ordinal, page_start, page_end, text, text_folded, boxes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![new_id(), id, i as i64, p.page_start, p.page_end, p.text, fold(&p.text), p.boxes],
        )?;
    }
    let authors = serde_json::to_string(&meta.authors).unwrap_or_else(|_| "[]".into());
    tx.execute(
        "UPDATE documents SET
            page_count = ?1,
            title   = COALESCE(NULLIF(title, ''), ?2),
            authors = CASE WHEN authors = '[]' THEN ?3 ELSE authors END,
            year    = COALESCE(year, ?4),
            doi     = COALESCE(NULLIF(doi, ''), ?5),
            status = ?6, error = ?7, updated_at = ?8
          WHERE id = ?9",
        params![
            (!pages.is_empty()).then_some(pages.len() as u32),
            meta.title,
            authors,
            meta.year,
            meta.doi,
            status.as_str(),
            note,
            now_ms(),
            id
        ],
    )?;
    tx.commit()?;
    get_document(conn, id)
}

/// One FTS5 query from free text. Every term is quoted, so user input can't use FTS syntax.
/// `all_terms` requires every term (library search); otherwise any term matches (retrieval).
/// `prefix_last` lets the last term match as a prefix, for search-as-you-type.
fn match_expr(query: &str, all_terms: bool, prefix_last: bool) -> Option<String> {
    let folded = fold(query);
    let terms: Vec<&str> = folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() {
        return None;
    }
    let last = terms.len() - 1;
    let parts: Vec<String> = terms
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if prefix_last && i == last {
                format!("\"{t}\"*")
            } else {
                format!("\"{t}\"")
            }
        })
        .collect();
    Some(parts.join(if all_terms { " AND " } else { " OR " }))
}

pub struct SearchOptions<'a> {
    /// Only documents attached to this project.
    pub project_id: Option<&'a str>,
    pub all_terms: bool,
    pub prefix_last: bool,
    pub limit: u32,
}

/// Keyword search over passages of documents that are ready, best match first.
pub fn search_passages(conn: &Connection, query: &str, opts: &SearchOptions) -> Result<Vec<SearchHit>> {
    let Some(expr) = match_expr(query, opts.all_terms, opts.prefix_last) else {
        return Ok(vec![]);
    };
    let scope = if opts.project_id.is_some() {
        "AND d.id IN (SELECT document_id FROM project_documents WHERE project_id = ?3)"
    } else {
        ""
    };
    let sql = format!(
        "SELECT p.id, d.id, COALESCE(NULLIF(d.title, ''), d.original_name), p.page_start, p.page_end, p.text
           FROM passages_fts f
           JOIN passages p ON p.rowid = f.rowid
           JOIN documents d ON d.id = p.document_id
          WHERE passages_fts MATCH ?1 AND d.status = 'ready' {scope}
          ORDER BY bm25(passages_fts)
          LIMIT ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let map = |r: &Row| {
        Ok(SearchHit {
            passage_id: r.get(0)?,
            document_id: r.get(1)?,
            document_title: r.get(2)?,
            page_start: r.get(3)?,
            page_end: r.get(4)?,
            text: r.get(5)?,
        })
    };
    let rows = match opts.project_id {
        Some(p) => stmt.query_map(params![expr, opts.limit, p], map)?.collect::<rusqlite::Result<Vec<_>>>()?,
        None => stmt.query_map(params![expr, opts.limit], map)?.collect::<rusqlite::Result<Vec<_>>>()?,
    };
    Ok(rows)
}

pub struct StoredPassage {
    pub id: Id,
    pub document_id: Id,
    pub page_start: u32,
    pub page_end: u32,
    pub text: String,
    pub boxes: Option<String>,
}

pub fn get_passage(conn: &Connection, id: &str) -> Result<StoredPassage> {
    conn.query_row(
        "SELECT id, document_id, page_start, page_end, text, boxes FROM passages WHERE id = ?1",
        [id],
        |r| {
            Ok(StoredPassage {
                id: r.get(0)?,
                document_id: r.get(1)?,
                page_start: r.get(2)?,
                page_end: r.get(3)?,
                text: r.get(4)?,
                boxes: r.get(5)?,
            })
        },
    )
    .optional()?
    .ok_or(AppError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{projects, test_conn};

    fn passage(page: u32, text: &str) -> NewPassage {
        NewPassage { page_start: page, page_end: page, text: text.into(), boxes: None }
    }

    fn ready_doc(conn: &mut Connection, hash: &str, name: &str, texts: &[&str]) -> Document {
        let d = insert_document(conn, hash, name, DocKind::Pdf).unwrap();
        let passages: Vec<_> = texts.iter().enumerate().map(|(i, t)| passage(i as u32 + 1, t)).collect();
        let pages = vec![NewPage { page_no: 1, width: 600.0, height: 800.0, quality: "ok" }];
        store_content(conn, &d.id, &pages, &passages, &Extracted::default(), DocStatus::Ready, None).unwrap()
    }

    fn opts() -> SearchOptions<'static> {
        SearchOptions { project_id: None, all_terms: true, prefix_last: false, limit: 10 }
    }

    #[test]
    fn search_folds_norwegian_and_ignores_fts_syntax() {
        let mut conn = test_conn();
        ready_doc(&mut conn, "h1", "a.pdf", &["Økosystemet ved Ærø er sårbart", "Plankton and iron"]);
        for q in ["okosystemet", "ØKOSYSTEMET", "aero", "Ærø", "sarbart", "sårbart"] {
            assert_eq!(search_passages(&conn, q, &opts()).unwrap().len(), 1, "query {q}");
        }
        // quotes, operators and a trailing star are plain text, not FTS syntax
        assert!(search_passages(&conn, "\"iron\" NOT plankton*", &opts()).is_ok());
        assert_eq!(search_passages(&conn, "   ", &opts()).unwrap().len(), 0);
    }

    #[test]
    fn all_terms_vs_any_term_and_prefix() {
        let mut conn = test_conn();
        ready_doc(&mut conn, "h1", "a.pdf", &["domoic acid in diatoms", "iron limits growth"]);
        assert_eq!(search_passages(&conn, "domoic iron", &opts()).unwrap().len(), 0);
        let any = SearchOptions { all_terms: false, ..opts() };
        assert_eq!(search_passages(&conn, "domoic iron", &any).unwrap().len(), 2);
        let prefix = SearchOptions { prefix_last: true, ..opts() };
        assert_eq!(search_passages(&conn, "diat", &prefix).unwrap().len(), 1);
        assert_eq!(search_passages(&conn, "diat", &opts()).unwrap().len(), 0);
    }

    #[test]
    fn only_ready_documents_are_searchable_and_scope_follows_project() {
        let mut conn = test_conn();
        let a = ready_doc(&mut conn, "h1", "a.pdf", &["shared word alpha"]);
        let b = ready_doc(&mut conn, "h2", "b.pdf", &["shared word beta"]);
        let p = projects::create_project(&conn, "P").unwrap();
        attach(&conn, &p.id, &a.id).unwrap();
        assert_eq!(search_passages(&conn, "shared", &opts()).unwrap().len(), 2);
        let scoped = SearchOptions { project_id: Some(&p.id), ..opts() };
        let hits = search_passages(&conn, "shared", &scoped).unwrap();
        assert_eq!((hits.len(), hits[0].document_id.as_str()), (1, a.id.as_str()));

        set_status(&conn, &b.id, DocStatus::Error, Some("bad")).unwrap();
        assert_eq!(search_passages(&conn, "shared", &opts()).unwrap().len(), 1);
    }

    #[test]
    fn deleting_a_document_or_project_cleans_up() {
        let mut conn = test_conn();
        let a = ready_doc(&mut conn, "h1", "a.pdf", &["findable text"]);
        let p = projects::create_project(&conn, "P").unwrap();
        attach(&conn, &p.id, &a.id).unwrap();
        projects::delete_project(&conn, &p.id).unwrap();
        assert_eq!(list_documents(&conn, None).unwrap().len(), 1, "documents outlive projects");
        assert!(list_documents(&conn, Some(&p.id)).unwrap().is_empty());

        let (hash, kind) = delete_document(&conn, &a.id).unwrap();
        assert_eq!((hash.as_str(), kind), ("h1", DocKind::Pdf));
        assert!(search_passages(&conn, "findable", &opts()).unwrap().is_empty());
        let rows: i64 = conn.query_row("SELECT count(*) FROM passages", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn hash_is_unique_and_reimport_is_detected() {
        let conn = test_conn();
        let d = insert_document(&conn, "h1", "a.pdf", DocKind::Pdf).unwrap();
        assert_eq!(find_by_hash(&conn, "h1").unwrap().unwrap().id, d.id);
        assert!(find_by_hash(&conn, "zzz").unwrap().is_none());
        assert!(insert_document(&conn, "h1", "copy.pdf", DocKind::Pdf).is_err());
    }

    #[test]
    fn extracted_metadata_never_overwrites_user_edits() {
        let mut conn = test_conn();
        let d = insert_document(&conn, "h1", "a.pdf", DocKind::Pdf).unwrap();
        update_metadata(&conn, &d.id, &MetadataPatch { title: Some("My title".into()), ..Default::default() }).unwrap();
        let meta = Extracted {
            title: Some("Extracted title".into()),
            authors: vec!["A. Author".into()],
            year: Some(2004),
            doi: Some("10.1/x".into()),
        };
        let out = store_content(&mut conn, &d.id, &[], &[], &meta, DocStatus::NoText, Some("scan")).unwrap();
        assert_eq!(out.title.as_deref(), Some("My title"));
        assert_eq!((out.authors.as_slice(), out.year, out.doi.as_deref()), (&["A. Author".to_string()][..], Some(2004), Some("10.1/x")));
        assert_eq!((out.status, out.page_count), (DocStatus::NoText, None));
    }

    #[test]
    fn metadata_patch_validates_and_clears() {
        let conn = test_conn();
        let d = insert_document(&conn, "h1", "a.pdf", DocKind::Pdf).unwrap();
        let set = MetadataPatch { title: Some(" T ".into()), authors: Some(vec![" A ".into(), "".into()]), year: Some(1999), doi: Some("10.1/x".into()) };
        let out = update_metadata(&conn, &d.id, &set).unwrap();
        assert_eq!((out.title.as_deref(), out.authors.len(), out.year), (Some("T"), 1, Some(1999)));
        let clear = MetadataPatch { title: Some("".into()), year: Some(0), doi: Some("".into()), ..Default::default() };
        let out = update_metadata(&conn, &d.id, &clear).unwrap();
        assert_eq!((out.title, out.year, out.doi), (None, None, None));
        assert!(update_metadata(&conn, &d.id, &MetadataPatch { year: Some(12), ..Default::default() }).is_err());
    }

    #[test]
    fn unfinished_documents_are_requeued_in_order() {
        let mut conn = test_conn();
        let a = insert_document(&conn, "h1", "a.pdf", DocKind::Pdf).unwrap();
        let b = insert_document(&conn, "h2", "b.pdf", DocKind::Pdf).unwrap();
        let c = ready_doc(&mut conn, "h3", "c.pdf", &["x"]);
        set_status(&conn, &b.id, DocStatus::Processing, None).unwrap();
        let queued = requeue_unfinished(&conn).unwrap();
        let ids: Vec<_> = queued.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, [a.id.as_str(), b.id.as_str()]);
        assert_eq!(get_document(&conn, &c.id).unwrap().status, DocStatus::Ready);
    }

    #[test]
    fn storing_again_replaces_old_passages() {
        let mut conn = test_conn();
        let d = ready_doc(&mut conn, "h1", "a.pdf", &["old words here"]);
        store_content(&mut conn, &d.id, &[], &[passage(1, "new words")], &Extracted::default(), DocStatus::Ready, None).unwrap();
        assert!(search_passages(&conn, "old", &opts()).unwrap().is_empty());
        assert_eq!(search_passages(&conn, "new", &opts()).unwrap().len(), 1);
        assert_eq!(get_document(&conn, &d.id).unwrap().passage_count, 1);
    }
}
