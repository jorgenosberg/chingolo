//! Documents, pages and passages. The ingestion worker and the library commands both use these.

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params, params_from_iter};

use crate::domain::library::{
    DocFilter, DocKind, DocStatus, Document, MatchKind, MetadataPatch, SearchHit, fold,
};
use crate::domain::{Id, clean_name, new_id, now_ms};
use crate::error::{AppError, Result};

const DOC_COLS: &str = "id, content_hash, original_name, kind, title, authors, year, doi, page_count,
    status, error, created_at, updated_at,
    (SELECT count(*) FROM passages p WHERE p.document_id = documents.id),
    (SELECT json_group_array(project_id) FROM project_documents pd WHERE pd.document_id = documents.id),
    (SELECT count(*) FROM passage_vectors v JOIN passages p ON p.id = v.passage_id WHERE p.document_id = documents.id)";

fn document_from_row(r: &Row) -> rusqlite::Result<Document> {
    let kind: String = r.get(3)?;
    let authors: String = r.get(5)?;
    let status: String = r.get(9)?;
    let project_ids: String = r.get(14)?;
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
        project_ids: serde_json::from_str(&project_ids).unwrap_or_default(),
        embedded_count: r.get(15)?,
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

impl DocFilter {
    pub fn from_project(project_id: Option<&str>) -> Self {
        project_id.map_or(DocFilter::All, |p| DocFilter::Project(p.to_string()))
    }
}

/// The one place that turns a filter into SQL, so search, vector scan, counts and listings can't
/// disagree. `col` is the document id column (`d.id` or `id`), `first` the number of the first
/// `?` it may use. Returns the clause (empty for everything) and its parameters, in order.
pub fn scope_clause(filter: &DocFilter, col: &str, first: usize) -> (String, Vec<String>) {
    match filter {
        DocFilter::All => (String::new(), vec![]),
        DocFilter::Project(p) => (
            format!("AND {col} IN (SELECT document_id FROM project_documents WHERE project_id = ?{first})"),
            vec![p.clone()],
        ),
        DocFilter::Documents(ids) if ids.is_empty() => ("AND 0".to_string(), vec![]),
        DocFilter::Documents(ids) => {
            let marks: Vec<String> = (first..first + ids.len()).map(|n| format!("?{n}")).collect();
            (format!("AND {col} IN ({})", marks.join(", ")), ids.clone())
        }
    }
}

/// All documents, newest first, or only those attached to a project.
pub fn list_documents(conn: &Connection, project_id: Option<&str>) -> Result<Vec<Document>> {
    list_documents_in(conn, &DocFilter::from_project(project_id))
}

/// Documents inside a filter, newest first. Includes documents that aren't ready.
pub fn list_documents_in(conn: &Connection, filter: &DocFilter) -> Result<Vec<Document>> {
    let (clause, args) = scope_clause(filter, "id", 1);
    let sql = format!("SELECT {DOC_COLS} FROM documents WHERE 1 = 1 {clause} ORDER BY created_at DESC, id DESC");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), document_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Which of these ids are documents. Used to validate a scope the user chose.
pub fn existing_ids(conn: &Connection, ids: &[Id]) -> Result<Vec<Id>> {
    let mut stmt = conn.prepare("SELECT 1 FROM documents WHERE id = ?1")?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if stmt.exists([id])? {
            out.push(id.clone());
        }
    }
    Ok(out)
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

#[derive(Debug)]
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
    // Take the write lock up front so the busy timeout applies (a deferred transaction that
    // starts reading and then upgrades can fail at once when another connection wrote meanwhile).
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
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

/// Words too common to help in a question, English and Norwegian (folded). Same list the
/// retrieval benchmark used, plus a few Norwegian ones.
pub(crate) const COMMON: &[&str] = &[
    "the", "and", "for", "are", "was", "were", "what", "how", "when", "why", "which", "who", "does", "did", "has",
    "have", "had", "that", "this", "with", "from", "into", "than", "then", "them", "they", "their", "there",
    "these", "those", "can", "could", "would", "should", "about", "over", "under", "after", "before", "between",
    "during", "each", "other", "some", "such", "only", "also", "not", "but", "out", "any", "all", "its", "our",
    "your", "you", "his", "her", "she", "him", "where", "whose", "been", "being", "more", "most", "very", "much",
    "many", "may", "might", "own", "same", "too", "via", "per", "og", "som", "det", "til", "med", "den", "det",
    "ikke", "har", "var", "kan", "vil", "hva", "hvordan", "hvorfor", "hvilke", "hvilken", "hvem", "fra", "etter",
    "mellom", "om",
];

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
    // In retrieval mode a question's filler words would match nearly everything.
    let terms: Vec<&str> = if all_terms {
        terms
    } else {
        let kept: Vec<&str> = terms.iter().copied().filter(|t| t.chars().count() > 2 && !COMMON.contains(t)).collect();
        if kept.is_empty() { terms } else { kept }
    };
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
    /// Which documents may be searched.
    pub filter: &'a DocFilter,
    pub all_terms: bool,
    pub prefix_last: bool,
    pub limit: u32,
}

/// Keyword search over passages of documents that are ready, best match first.
pub fn search_passages(conn: &Connection, query: &str, opts: &SearchOptions) -> Result<Vec<SearchHit>> {
    let Some(expr) = match_expr(query, opts.all_terms, opts.prefix_last) else {
        return Ok(vec![]);
    };
    let (scope, scope_args) = scope_clause(opts.filter, "d.id", 3);
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
            matched_by: MatchKind::Keyword,
        })
    };
    let mut args: Vec<&dyn rusqlite::ToSql> = vec![&expr, &opts.limit];
    args.extend(scope_args.iter().map(|a| a as &dyn rusqlite::ToSql));
    let rows = stmt.query_map(params_from_iter(args), map)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Search hits for these passage ids, in the order given. Ids that no longer exist are skipped.
pub fn hits_for_ids(conn: &Connection, ids: &[Id], matched_by: MatchKind) -> Result<Vec<SearchHit>> {
    let mut stmt = conn.prepare(
        "SELECT p.id, d.id, COALESCE(NULLIF(d.title, ''), d.original_name), p.page_start, p.page_end, p.text
           FROM passages p JOIN documents d ON d.id = p.document_id
          WHERE p.id = ?1",
    )?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let hit = stmt
            .query_row([id], |r| {
                Ok(SearchHit {
                    passage_id: r.get(0)?,
                    document_id: r.get(1)?,
                    document_title: r.get(2)?,
                    page_start: r.get(3)?,
                    page_end: r.get(4)?,
                    text: r.get(5)?,
                    matched_by,
                })
            })
            .optional()?;
        out.extend(hit);
    }
    Ok(out)
}

/// How many documents a question could draw on: ready ones inside the filter.
pub fn ready_document_count(conn: &Connection, filter: &DocFilter) -> Result<u32> {
    let (clause, args) = scope_clause(filter, "d.id", 1);
    Ok(conn.query_row(
        &format!("SELECT COUNT(*) FROM documents d WHERE d.status = 'ready' {clause}"),
        params_from_iter(args.iter()),
        |r| r.get(0),
    )?)
}

fn hit_from_row(r: &Row, matched_by: MatchKind) -> rusqlite::Result<SearchHit> {
    Ok(SearchHit {
        passage_id: r.get(0)?,
        document_id: r.get(1)?,
        document_title: r.get(2)?,
        page_start: r.get(3)?,
        page_end: r.get(4)?,
        text: r.get(5)?,
        matched_by,
    })
}

/// The passage and up to `before` and `after` neighbours in reading order. Empty if the passage
/// no longer exists (the file was read again).
pub fn passages_around(conn: &Connection, passage_id: &str, before: u32, after: u32) -> Result<Vec<SearchHit>> {
    let mut stmt = conn.prepare(
        "SELECT p.id, d.id, COALESCE(NULLIF(d.title, ''), d.original_name), p.page_start, p.page_end, p.text
           FROM passages p JOIN documents d ON d.id = p.document_id
          WHERE p.document_id = (SELECT document_id FROM passages WHERE id = ?1)
            AND p.ordinal BETWEEN (SELECT ordinal FROM passages WHERE id = ?1) - ?2
                              AND (SELECT ordinal FROM passages WHERE id = ?1) + ?3
          ORDER BY p.ordinal",
    )?;
    let rows = stmt.query_map(params![passage_id, before, after], |r| hit_from_row(r, MatchKind::Context))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Passages that touch this page, in reading order.
pub fn passages_on_page(conn: &Connection, document_id: &str, page: u32, limit: u32) -> Result<Vec<SearchHit>> {
    let mut stmt = conn.prepare(
        "SELECT p.id, d.id, COALESCE(NULLIF(d.title, ''), d.original_name), p.page_start, p.page_end, p.text
           FROM passages p JOIN documents d ON d.id = p.document_id
          WHERE p.document_id = ?1 AND p.page_start <= ?2 AND p.page_end >= ?2
          ORDER BY p.ordinal LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![document_id, page, limit], |r| hit_from_row(r, MatchKind::Context))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
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
        SearchOptions { filter: &crate::domain::library::DocFilter::All, all_terms: true, prefix_last: false, limit: 10 }
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
        let pf = DocFilter::Project(p.id.clone());
        let scoped = SearchOptions { filter: &pf, ..opts() };
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
        assert_eq!(get_document(&conn, &a.id).unwrap().project_ids, vec![p.id.clone()]);
        projects::delete_project(&conn, &p.id).unwrap();
        assert!(get_document(&conn, &a.id).unwrap().project_ids.is_empty());
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
