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
    (SELECT count(*) FROM passage_vectors v JOIN passages p ON p.id = v.passage_id WHERE p.document_id = documents.id),
    journal, publisher, volume, issue, pages, series, isbn, issn, meta_edited, meta_source,
    (SELECT count(*) FROM pages g WHERE g.document_id = documents.id AND g.quality != 'ok'), enrich_error";

fn document_from_row(r: &Row) -> rusqlite::Result<Document> {
    let kind: String = r.get(3)?;
    let authors: String = r.get(5)?;
    let status: String = r.get(9)?;
    let project_ids: String = r.get(14)?;
    let (edited, source): (String, String) = (r.get(24)?, r.get(25)?);
    Ok(Document {
        journal: r.get(16)?,
        publisher: r.get(17)?,
        volume: r.get(18)?,
        issue: r.get(19)?,
        pages: r.get(20)?,
        series: r.get(21)?,
        isbn: r.get(22)?,
        issn: r.get(23)?,
        meta_edited: serde_json::from_str(&edited).unwrap_or_default(),
        meta_source: serde_json::from_str(&source).unwrap_or_default(),
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
        weak_pages: r.get(26)?,
        enrich_error: r.get(27)?,
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

/// Records that the details lookups finished (`None`) or failed, and why.
pub fn mark_enriched(conn: &Connection, id: &str, failure: Option<&str>) -> Result<()> {
    let done = failure.is_none().then(now_ms);
    conn.execute("UPDATE documents SET enriched_at = ?1, enrich_error = ?2 WHERE id = ?3", params![done, failure, id])?;
    Ok(())
}

/// Ready documents whose lookups never finished (the app closed, or one failed), or all of them.
pub fn unenriched(conn: &Connection, all: bool) -> Result<Vec<Id>> {
    let mut stmt = conn.prepare("SELECT id FROM documents WHERE status = 'ready' AND (?1 OR enriched_at IS NULL) ORDER BY created_at, id")?;
    Ok(stmt.query_map([all], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
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

/// The text of the passages that start on the first `pages` pages, in order. Used to check that a
/// lookup result belongs to this document.
pub fn head_text(conn: &Connection, id: &str, pages: u32) -> Result<String> {
    let mut stmt = conn.prepare("SELECT text FROM passages WHERE document_id = ?1 AND page_start <= ?2 ORDER BY ordinal")?;
    let rows = stmt.query_map(params![id, pages], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?.join(" "))
}

/// Passages that start on pages 5 to 10 or on the last two pages: where a report's imprint (ISBN,
/// publisher, edition) sits when the title pages don't show it.
pub fn imprint_passages(conn: &Connection, id: &str, page_count: Option<u32>) -> Result<Vec<String>> {
    let tail = page_count.unwrap_or(0).saturating_sub(1).max(11);
    let mut stmt = conn.prepare("SELECT text FROM passages WHERE document_id = ?1 AND ((page_start BETWEEN 5 AND 10) OR page_start >= ?2) ORDER BY ordinal")?;
    let rows = stmt.query_map(params![id, tail], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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
    let doc = get_document(conn, id)?;
    let blank = |s: &str| if s.trim().is_empty() { None } else { Some(s.trim().to_string()) };
    let mut edited = doc.meta_edited;
    let mut source = doc.meta_source;
    let mut mark = |field: &str| {
        if !edited.iter().any(|f| f == field) {
            edited.push(field.to_string());
        }
        source.remove(field);
    };
    if let Some(y) = patch.year
        && y != 0
        && !(1000..=2200).contains(&y)
    {
        return Err(AppError::Invalid("That year doesn't look right.".into()));
    }
    let title = patch.title.as_deref().map(|t| blank(t).map(|t| clean_name(&t, "Title")).transpose()).transpose()?;
    if let Some(t) = title {
        conn.execute("UPDATE documents SET title = ?1 WHERE id = ?2", params![t, id])?;
        mark("title");
    }
    if let Some(a) = &patch.authors {
        let a: Vec<String> = a.iter().filter_map(|s| blank(s)).collect();
        conn.execute(
            "UPDATE documents SET authors = ?1 WHERE id = ?2",
            params![serde_json::to_string(&a).unwrap_or_else(|_| "[]".into()), id],
        )?;
        mark("authors");
    }
    if let Some(y) = patch.year {
        conn.execute("UPDATE documents SET year = ?1 WHERE id = ?2", params![(y != 0).then_some(y), id])?;
        mark("year");
    }
    let text_fields = [
        ("doi", &patch.doi),
        ("journal", &patch.journal),
        ("publisher", &patch.publisher),
        ("volume", &patch.volume),
        ("issue", &patch.issue),
        ("pages", &patch.pages),
        ("series", &patch.series),
        ("isbn", &patch.isbn),
        ("issn", &patch.issn),
    ];
    for (col, value) in text_fields {
        if let Some(v) = value {
            // The column names above are constants, never user input.
            conn.execute(&format!("UPDATE documents SET {col} = ?1 WHERE id = ?2"), params![blank(v), id])?;
            mark(col);
        }
    }
    conn.execute(
        "UPDATE documents SET meta_edited = ?1, meta_source = ?2, updated_at = ?3 WHERE id = ?4",
        params![to_json(&edited), to_json(&source), now_ms(), id],
    )?;
    get_document(conn, id)
}

fn to_json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_default()
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
    /// The text came from text recognition, not from the file.
    pub ocr: bool,
}

pub struct NewPassage {
    pub page_start: u32,
    pub page_end: u32,
    pub text: String,
    /// JSON, see the `passages.boxes` column.
    pub boxes: Option<String>,
}

/// Metadata detected from the file, Crossref or a model. `apply_metadata` stores it
/// and never touches a field the user edited.
#[derive(Debug, Default, Clone)]
pub struct Extracted {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub doi: Option<String>,
    pub journal: Option<String>,
    pub publisher: Option<String>,
    pub volume: Option<String>,
    pub issue: Option<String>,
    pub pages: Option<String>,
    pub series: Option<String>,
    pub isbn: Option<String>,
    pub issn: Option<String>,
}

impl Extracted {
    /// Names of the fields that have a value, for logs (never the values).
    pub fn fields(&self) -> Vec<&'static str> {
        [
            ("title", self.title.is_some()),
            ("authors", !self.authors.is_empty()),
            ("year", self.year.is_some()),
            ("doi", self.doi.is_some()),
            ("journal", self.journal.is_some()),
            ("publisher", self.publisher.is_some()),
            ("volume", self.volume.is_some()),
            ("issue", self.issue.is_some()),
            ("pages", self.pages.is_some()),
            ("series", self.series.is_some()),
            ("isbn", self.isbn.is_some()),
            ("issn", self.issn.is_some()),
        ]
        .into_iter()
        .filter_map(|(n, has)| has.then_some(n))
        .collect()
    }
}

/// Writes detected metadata. Fields in `meta_edited` are skipped. With `replace`, a field the
/// detection found nothing for is cleared (a fresh read of the file is the whole truth). Without
/// it, only found values are written, so Crossref or a model can add to what the file gave.
/// `source` ("file", "crossref", "doi", "datacite", "nb", "openlibrary" or "ai") is recorded for every field
/// written. A fresh read never changes what an online record gave (the publisher's or library's
/// own data) and never clears what a model found.
pub fn apply_metadata(conn: &Connection, id: &str, m: &Extracted, source: &str, replace: bool) -> Result<()> {
    let doc = get_document(conn, id)?;
    let mut sources = doc.meta_source;
    let text = |v: &Option<String>| v.as_ref().map(|s| rusqlite::types::Value::Text(s.clone()));
    let fields: [(&str, Option<rusqlite::types::Value>); 12] = [
        ("title", text(&m.title)),
        ("authors", (!m.authors.is_empty()).then(|| rusqlite::types::Value::Text(to_json(&m.authors)))),
        ("year", m.year.map(|y| rusqlite::types::Value::Integer(y.into()))),
        ("doi", text(&m.doi)),
        ("journal", text(&m.journal)),
        ("publisher", text(&m.publisher)),
        ("volume", text(&m.volume)),
        ("issue", text(&m.issue)),
        ("pages", text(&m.pages)),
        ("series", text(&m.series)),
        ("isbn", text(&m.isbn)),
        ("issn", text(&m.issn)),
    ];
    for (col, value) in fields {
        let from = sources.get(col).map(String::as_str);
        let kept = replace && (matches!(from, Some("crossref" | "doi" | "datacite" | "nb" | "openlibrary")) || (from == Some("ai") && value.is_none()));
        if doc.meta_edited.iter().any(|f| f == col) || (value.is_none() && !replace) || kept {
            continue;
        }
        let value = match (value, col) {
            (None, "authors") => Some(rusqlite::types::Value::Text("[]".into())),
            (v, _) => v,
        };
        match &value {
            Some(rusqlite::types::Value::Text(t)) if col == "authors" && t == "[]" => sources.remove(col),
            Some(_) => sources.insert(col.to_string(), source.to_string()),
            None => sources.remove(col),
        };
        // The column names above are constants, never user input.
        conn.execute(&format!("UPDATE documents SET {col} = ?1 WHERE id = ?2"), params![value, id])?;
    }
    conn.execute("UPDATE documents SET meta_source = ?1, updated_at = ?2 WHERE id = ?3", params![to_json(&sources), now_ms(), id])?;
    Ok(())
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
    tx.execute("DELETE FROM page_regions WHERE document_id = ?1", [id])?;
    for p in pages {
        tx.execute(
            "INSERT INTO pages (document_id, page_no, width, height, quality, ocr) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, p.page_no, p.width, p.height, p.quality, p.ocr],
        )?;
    }
    for (i, p) in passages.iter().enumerate() {
        tx.execute(
            "INSERT INTO passages (id, document_id, ordinal, page_start, page_end, text, text_folded, boxes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![new_id(), id, i as i64, p.page_start, p.page_end, p.text, fold(&p.text), p.boxes],
        )?;
    }
    tx.execute(
        "UPDATE documents SET page_count = ?1, status = ?2, error = ?3, updated_at = ?4, enriched_at = NULL, enrich_error = NULL WHERE id = ?5",
        params![(!pages.is_empty()).then_some(pages.len() as u32), status.as_str(), note, now_ms(), id],
    )?;
    apply_metadata(&tx, id, meta, "file", true)?;
    tx.commit()?;
    get_document(conn, id)
}

/// A region found by the layout model, in PDF user space.
pub struct NewRegion {
    pub page_no: u32,
    pub ordinal: u32,
    pub label: String,
    pub score: f32,
    pub rect: [f32; 4],
}

/// Stores the layout of a document and swaps its passages for ones built from it. A passage whose
/// text is unchanged keeps its id (so its vector and any citation stay valid) and only gets its
/// place and boxes updated. Passages with new text are inserted, the rest are removed.
/// `layout_pages` are the pages layout ran on.
pub fn store_layout(
    conn: &mut Connection,
    id: &str,
    layout_pages: &[u32],
    regions: &[NewRegion],
    passages: &[NewPassage],
) -> Result<Document> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    get_document(&tx, id)?;
    tx.execute("DELETE FROM page_regions WHERE document_id = ?1", [id])?;
    for r in regions {
        tx.execute(
            "INSERT INTO page_regions (document_id, page_no, ordinal, label, score, left, bottom, right, top)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, r.page_no, r.ordinal, r.label, r.score, r.rect[0], r.rect[1], r.rect[2], r.rect[3]],
        )?;
    }
    let at = now_ms();
    for p in layout_pages {
        tx.execute("UPDATE pages SET layout_at = ?1 WHERE document_id = ?2 AND page_no = ?3", params![at, id, p])?;
    }

    let mut old: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    {
        let mut stmt = tx.prepare("SELECT id, text FROM passages WHERE document_id = ?1 ORDER BY ordinal DESC")?;
        for row in stmt.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (pid, text) = row?;
            old.entry(text).or_default().push(pid);
        }
    }
    // clear the unique (document, ordinal) slots before renumbering
    tx.execute("UPDATE passages SET ordinal = ordinal + 1000000 WHERE document_id = ?1", [id])?;
    let mut kept = std::collections::HashSet::new();
    for (i, p) in passages.iter().enumerate() {
        match old.get_mut(&p.text).and_then(Vec::pop) {
            Some(pid) => {
                tx.execute(
                    "UPDATE passages SET ordinal = ?1, page_start = ?2, page_end = ?3, boxes = ?4 WHERE id = ?5",
                    params![-(i as i64) - 1, p.page_start, p.page_end, p.boxes, pid],
                )?;
                kept.insert(pid);
            }
            None => {
                tx.execute(
                    "INSERT INTO passages (id, document_id, ordinal, page_start, page_end, text, text_folded, boxes)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![new_id(), id, -(i as i64) - 1, p.page_start, p.page_end, p.text, fold(&p.text), p.boxes],
                )?;
            }
        }
    }
    for pid in old.into_values().flatten() {
        tx.execute("DELETE FROM passages WHERE id = ?1", [pid])?;
    }
    // final ordinals: the negative placeholders become 0, 1, 2...
    tx.execute("UPDATE passages SET ordinal = -ordinal - 1 WHERE document_id = ?1 AND ordinal < 0", [id])?;
    tx.execute("UPDATE documents SET updated_at = ?1 WHERE id = ?2", params![at, id])?;
    tx.commit()?;
    get_document(conn, id)
}

pub fn list_regions(conn: &Connection, id: &str) -> Result<Vec<crate::domain::library::PageRegion>> {
    let mut stmt = conn.prepare(
        "SELECT page_no, ordinal, label, score, left, bottom, right, top FROM page_regions WHERE document_id = ?1 ORDER BY page_no, ordinal",
    )?;
    let rows = stmt.query_map([id], |r| {
        Ok(crate::domain::library::PageRegion {
            page: r.get(0)?,
            ordinal: r.get(1)?,
            label: r.get(2)?,
            score: r.get(3)?,
            rect: [r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?],
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// PDF documents that are ready and have pages without layout yet.
pub fn documents_without_layout(conn: &Connection) -> Result<Vec<Document>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {DOC_COLS} FROM documents WHERE status = 'ready' AND kind = 'pdf'
           AND EXISTS (SELECT 1 FROM pages p WHERE p.document_id = documents.id AND p.layout_at IS NULL)
         ORDER BY created_at, id"
    ))?;
    let rows = stmt.query_map([], document_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// (pages with layout, all pages, documents still without layout) over ready PDFs.
pub fn layout_counts(conn: &Connection) -> Result<(u32, u32, u32)> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(p.layout_at IS NOT NULL), 0), COUNT(*), COUNT(DISTINCT CASE WHEN p.layout_at IS NULL THEN p.document_id END)
           FROM pages p JOIN documents d ON d.id = p.document_id
          WHERE d.kind = 'pdf' AND d.status = 'ready'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?)
}

/// Pages of a document without layout yet, text or not (figure pages have none).
pub fn pages_without_layout(conn: &Connection, id: &str) -> Result<Vec<u32>> {
    let mut stmt = conn.prepare("SELECT page_no FROM pages WHERE document_id = ?1 AND layout_at IS NULL ORDER BY page_no")?;
    let rows = stmt.query_map([id], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
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
    Ok(search_passages_scored(conn, query, opts)?.into_iter().map(|(h, _)| h).collect())
}

/// Like `search_passages`, with each hit's BM25 score (higher is better).
pub fn search_passages_scored(conn: &Connection, query: &str, opts: &SearchOptions) -> Result<Vec<(SearchHit, f32)>> {
    let Some(expr) = match_expr(query, opts.all_terms, opts.prefix_last) else {
        return Ok(vec![]);
    };
    let (scope, scope_args) = scope_clause(opts.filter, "d.id", 3);
    let sql = format!(
        "SELECT p.id, d.id, COALESCE(NULLIF(d.title, ''), d.original_name), p.page_start, p.page_end, p.text, -bm25(passages_fts)
           FROM passages_fts f
           JOIN passages p ON p.rowid = f.rowid
           JOIN documents d ON d.id = p.document_id
          WHERE passages_fts MATCH ?1 AND d.status = 'ready' {scope}
          ORDER BY bm25(passages_fts)
          LIMIT ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let map = |r: &Row| {
        let hit = SearchHit {
            passage_id: r.get(0)?,
            document_id: r.get(1)?,
            document_title: r.get(2)?,
            page_start: r.get(3)?,
            page_end: r.get(4)?,
            text: r.get(5)?,
            matched_by: MatchKind::Keyword,
        };
        Ok((hit, r.get::<_, f64>(6)? as f32))
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
        let pages = vec![NewPage { page_no: 1, width: 600.0, height: 800.0, quality: "ok", ocr: false }];
        store_content(conn, &d.id, &pages, &passages, &Extracted::default(), DocStatus::Ready, None).unwrap()
    }

    fn opts() -> SearchOptions<'static> {
        SearchOptions { filter: &crate::domain::library::DocFilter::All, all_terms: true, prefix_last: false, limit: 10 }
    }

    #[test]
    fn layout_keeps_ids_of_unchanged_passages_and_swaps_the_rest() {
        let mut conn = test_conn();
        let d = ready_doc(&mut conn, "h1", "a.pdf", &["Plankton blooms follow iron", "Running head 2025", "Diatoms dominate"]);
        let ids = |c: &Connection| -> Vec<(String, String)> {
            let mut s = c.prepare("SELECT id, text FROM passages WHERE document_id = ?1 ORDER BY ordinal").unwrap();
            s.query_map([&d.id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(|r| r.unwrap()).collect()
        };
        let before = ids(&conn);
        let new = [passage(1, "Diatoms dominate"), passage(1, "Plankton blooms follow iron"), passage(1, "A table of numbers")];
        let region = NewRegion { page_no: 1, ordinal: 0, label: "text".into(), score: 0.9, rect: [0.0, 0.0, 1.0, 1.0] };
        store_layout(&mut conn, &d.id, &[1], &[region], &new).unwrap();
        let after = ids(&conn);
        assert_eq!(after.iter().map(|p| p.1.as_str()).collect::<Vec<_>>(), ["Diatoms dominate", "Plankton blooms follow iron", "A table of numbers"]);
        assert_eq!(after[0].0, before[2].0);
        assert_eq!(after[1].0, before[0].0);
        assert!(before.iter().all(|b| b.1 != "Running head 2025" || !after.iter().any(|a| a.0 == b.0)));
        let n: i64 = conn.query_row("SELECT count(*) FROM passages_fts WHERE passages_fts MATCH 'running'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "the dropped passage left the search index");
        assert_eq!(pages_without_layout(&conn, &d.id).unwrap(), Vec::<u32>::new());
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
            journal: Some("Nature".into()),
            ..Default::default()
        };
        let out = store_content(&mut conn, &d.id, &[], &[], &meta, DocStatus::NoText, Some("scan")).unwrap();
        assert_eq!(out.title.as_deref(), Some("My title"));
        assert_eq!((out.authors.as_slice(), out.year, out.doi.as_deref()), (&["A. Author".to_string()][..], Some(2004), Some("10.1/x")));
        assert_eq!((out.status, out.page_count), (DocStatus::NoText, None));
        assert_eq!(out.meta_edited, ["title"]);
        assert_eq!(out.meta_source.get("journal").map(String::as_str), Some("file"));
        assert!(!out.meta_source.contains_key("title"));
    }

    #[test]
    fn a_fresh_read_replaces_stale_details_but_merging_only_adds() {
        let conn = test_conn();
        let d = insert_document(&conn, "h1", "a.pdf", DocKind::Pdf).unwrap();
        let first = Extracted { title: Some("Old".into()), authors: vec!["X".into()], year: Some(1990), ..Default::default() };
        apply_metadata(&conn, &d.id, &first, "file", true).unwrap();
        // a read that finds a new title and nothing else clears the rest
        let again = Extracted { title: Some("New".into()), ..Default::default() };
        apply_metadata(&conn, &d.id, &again, "file", true).unwrap();
        let out = get_document(&conn, &d.id).unwrap();
        assert_eq!((out.title.as_deref(), out.authors.len(), out.year), (Some("New"), 0, None));
        assert!(!out.meta_source.contains_key("year") && !out.meta_source.contains_key("authors"));
        // a lookup that only knows the year and journal adds them and keeps the title
        let more = Extracted { year: Some(2004), journal: Some("Nature".into()), ..Default::default() };
        apply_metadata(&conn, &d.id, &more, "crossref", false).unwrap();
        let out = get_document(&conn, &d.id).unwrap();
        assert_eq!((out.title.as_deref(), out.year, out.journal.as_deref()), (Some("New"), Some(2004), Some("Nature")));
        assert_eq!(out.meta_source.get("year").map(String::as_str), Some("crossref"));
        assert_eq!(out.meta_source.get("title").map(String::as_str), Some("file"));
    }

    #[test]
    fn a_fresh_read_keeps_what_crossref_gave_and_what_a_model_found() {
        let conn = test_conn();
        let d = insert_document(&conn, "h1", "a.pdf", DocKind::Pdf).unwrap();
        apply_metadata(&conn, &d.id, &Extracted { title: Some("File".into()), year: Some(1990), ..Default::default() }, "file", true).unwrap();
        apply_metadata(&conn, &d.id, &Extracted { title: Some("Crossref".into()), journal: Some("Nature".into()), ..Default::default() }, "crossref", false).unwrap();
        apply_metadata(&conn, &d.id, &Extracted { series: Some("S".into()), ..Default::default() }, "ai", false).unwrap();
        let read = Extracted { title: Some("Other".into()), year: Some(1991), ..Default::default() };
        apply_metadata(&conn, &d.id, &read, "file", true).unwrap();
        let out = get_document(&conn, &d.id).unwrap();
        assert_eq!((out.title.as_deref(), out.journal.as_deref(), out.series.as_deref(), out.year), (Some("Crossref"), Some("Nature"), Some("S"), Some(1991)));
    }

    #[test]
    fn clearing_a_field_counts_as_an_edit_and_survives_a_refresh() {
        let conn = test_conn();
        let d = insert_document(&conn, "h1", "a.pdf", DocKind::Pdf).unwrap();
        apply_metadata(&conn, &d.id, &Extracted { year: Some(1990), isbn: Some("978".into()), ..Default::default() }, "file", true).unwrap();
        let cleared = update_metadata(&conn, &d.id, &MetadataPatch { year: Some(0), ..Default::default() }).unwrap();
        assert_eq!((cleared.year, cleared.meta_edited.as_slice()), (None, &["year".to_string()][..]));
        apply_metadata(&conn, &d.id, &Extracted { year: Some(2001), isbn: Some("979".into()), ..Default::default() }, "file", true).unwrap();
        let out = get_document(&conn, &d.id).unwrap();
        assert_eq!((out.year, out.isbn.as_deref()), (None, Some("979")));
    }

    #[test]
    fn metadata_patch_validates_and_clears() {
        let conn = test_conn();
        let d = insert_document(&conn, "h1", "a.pdf", DocKind::Pdf).unwrap();
        let set = MetadataPatch { title: Some(" T ".into()), authors: Some(vec![" A ".into(), "".into()]), year: Some(1999), doi: Some("10.1/x".into()), ..Default::default() };
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
