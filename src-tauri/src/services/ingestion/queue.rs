//! The background worker. One thread reads files one at a time, so pdfium never runs twice at once,
//! and it uses its own database connection so a long import doesn't block chat.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use specta::Type;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::pdf::PdfReader;
use super::segment::segment;
use super::{Extraction, docx};
use crate::domain::Id;
use crate::domain::library::{DocKind, DocStatus, Document};
use crate::error::{AppError, Result};
use crate::persistence::{Db, library as repo};

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: u32,
    pub total: u32,
}

/// Sent whenever a document is added or its status changes, and while pages are being read.
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct DocumentChanged {
    pub document: Document,
    pub progress: Option<Progress>,
}

pub type Emit = Arc<dyn Fn(DocumentChanged) + Send + Sync>;

struct Inner {
    tx: mpsc::UnboundedSender<Document>,
    tokens: Mutex<HashMap<Id, CancellationToken>>,
    library_dir: PathBuf,
    emit: Emit,
}

#[derive(Clone)]
pub struct Ingestor(Arc<Inner>);

/// Where the managed copy of a file lives: `<library>/<first two hash chars>/<hash>.<ext>`.
pub fn library_path(library_dir: &Path, hash: &str, kind: DocKind) -> PathBuf {
    library_dir.join(&hash[..2]).join(format!("{hash}.{}", kind.as_str()))
}

impl Ingestor {
    pub fn start(db_path: &Path, library_dir: PathBuf, emit: Emit) -> Result<Self> {
        let conn = Db::open_secondary(db_path)?;
        let (tx, rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner { tx, tokens: Mutex::new(HashMap::new()), library_dir, emit });
        let worker = inner.clone();
        std::thread::Builder::new()
            .name("ingest".into())
            .spawn(move || worker_loop(conn, rx, worker))
            .map_err(|e| AppError::Internal(format!("couldn't start the import worker: {e}")))?;
        Ok(Ingestor(inner))
    }

    pub fn library_dir(&self) -> &Path {
        &self.0.library_dir
    }

    pub fn emit(&self, document: Document, progress: Option<Progress>) {
        (self.0.emit)(DocumentChanged { document, progress });
    }

    pub fn enqueue(&self, doc: &Document) {
        if let Ok(mut t) = self.0.tokens.lock() {
            t.insert(doc.id.clone(), CancellationToken::new());
        }
        let _ = self.0.tx.send(doc.clone());
    }

    /// True if the document was queued or running.
    pub fn cancel(&self, id: &str) -> bool {
        match self.0.tokens.lock().ok().and_then(|t| t.get(id).cloned()) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Puts documents that were unfinished when the app stopped back in the queue.
    pub async fn resume(&self, db: &Db) -> Result<usize> {
        let docs = db.call(|c| repo::requeue_unfinished(c)).await?;
        for d in &docs {
            self.enqueue(d);
            self.emit(d.clone(), None);
        }
        Ok(docs.len())
    }
}

fn worker_loop(mut conn: Connection, mut rx: mpsc::UnboundedReceiver<Document>, inner: Arc<Inner>) {
    let mut pdf: Option<PdfReader> = None;
    while let Some(doc) = rx.blocking_recv() {
        let token = inner.tokens.lock().ok().and_then(|t| t.get(&doc.id).cloned()).unwrap_or_default();
        let path = library_path(&inner.library_dir, &doc.content_hash, doc.kind);
        let emit = |d: Document, p: Option<Progress>| (inner.emit)(DocumentChanged { document: d, progress: p });
        // A bug in a parser must not take the worker down with it.
        let ran = std::panic::catch_unwind(AssertUnwindSafe(|| run_job(&mut conn, &mut pdf, &path, &doc, &token, &emit)));
        if ran.is_err() {
            if let Ok(d) = repo::set_status(&conn, &doc.id, DocStatus::Error, Some("Reading this file crashed. It may be damaged.")) {
                emit(d, None);
            }
        }
        if let Ok(mut t) = inner.tokens.lock() {
            t.remove(&doc.id);
        }
    }
}

fn reader(slot: &mut Option<PdfReader>) -> Result<&PdfReader> {
    if slot.is_none() {
        *slot = Some(PdfReader::new()?);
    }
    Ok(slot.as_ref().expect("just set"))
}

/// Reads, splits and stores one document, then reports its final status. Never fails: problems
/// become the document's status.
pub fn run_job(
    conn: &mut Connection,
    pdf: &mut Option<PdfReader>,
    path: &Path,
    doc: &Document,
    token: &CancellationToken,
    emit: &dyn Fn(Document, Option<Progress>),
) {
    let finish = |conn: &Connection, status: DocStatus, note: Option<&str>| {
        // The document may have been deleted while it was being read.
        if let Ok(d) = repo::set_status(conn, &doc.id, status, note) {
            emit(d, None);
        }
    };
    if token.is_cancelled() {
        return finish(conn, DocStatus::Cancelled, None);
    }
    match repo::set_status(conn, &doc.id, DocStatus::Processing, None) {
        Ok(d) => emit(d, None),
        Err(_) => return,
    }

    let cancelled = || token.is_cancelled();
    let mut last_sent = 0;
    let extraction: Result<Extraction> = match doc.kind {
        DocKind::Pdf => reader(pdf).and_then(|r| {
            r.extract(path, &cancelled, &mut |done, total| {
                // about every 5 percent, and at the end
                if done == total || done >= last_sent + (total / 20).max(1) {
                    last_sent = done;
                    let mut d = doc.clone();
                    d.status = DocStatus::Processing;
                    emit(d, Some(Progress { done, total }));
                }
            })
        }),
        DocKind::Docx => docx::extract(path, &cancelled),
    };

    let ex = match extraction {
        Ok(ex) => ex,
        Err(AppError::Cancelled) => return finish(conn, DocStatus::Cancelled, None),
        Err(e) => return finish(conn, DocStatus::Error, Some(&e.to_string())),
    };
    let passages = segment(&ex.blocks);
    let (status, note) = if passages.is_empty() {
        let why = match doc.kind {
            DocKind::Pdf => "No text found. This is probably a scan without a text layer.",
            DocKind::Docx => "No text found in this document.",
        };
        (DocStatus::NoText, Some(why.to_string()))
    } else {
        (DocStatus::Ready, page_note(&ex))
    };
    match repo::store_content(conn, &doc.id, &ex.pages, &passages, &ex.meta, status, note.as_deref()) {
        Ok(d) => emit(d, None),
        Err(AppError::NotFound) => {}
        Err(e) => finish(conn, DocStatus::Error, Some(&format!("Couldn't save the text: {e}"))),
    }
}

/// A short note about pages that are missing or unreliable, if any.
fn page_note(ex: &Extraction) -> Option<String> {
    let total = ex.pages.len();
    let count = |q: &str| ex.pages.iter().filter(|p| p.quality == q).count();
    let (none, low) = (count("none"), count("low"));
    let mut parts = vec![];
    if ex.unreadable_pages > 0 {
        parts.push(format!("{} pages could not be read.", ex.unreadable_pages));
    }
    let none_other = none.saturating_sub(ex.unreadable_pages as usize);
    if none_other > 0 {
        parts.push(format!("{none_other} of {total} pages have no text (blank, images or scans)."));
    }
    if low > 0 {
        parts.push(format!("{low} pages have unreliable text."));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::persistence::test_conn;

    fn docx_file(dir: &Path, hash: &str, paragraphs: &[&str]) -> (Document, PathBuf, Connection) {
        let conn = test_conn();
        let doc = repo::insert_document(&conn, hash, "t.docx", DocKind::Docx).unwrap();
        let path = dir.join("t.docx");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
        let body: String = paragraphs.iter().map(|p| format!("<w:p><w:r><w:t>{p}</w:t></w:r></w:p>")).collect();
        write!(zip, r#"<w:document xmlns:w="x"><w:body>{body}</w:body></w:document>"#).unwrap();
        zip.finish().unwrap();
        (doc, path, conn)
    }

    fn run(conn: &mut Connection, path: &Path, doc: &Document, token: &CancellationToken) -> Vec<DocStatus> {
        let seen = std::cell::RefCell::new(vec![]);
        run_job(conn, &mut None, path, doc, token, &|d, _| seen.borrow_mut().push(d.status));
        seen.into_inner()
    }

    #[test]
    fn a_document_goes_from_processing_to_ready_and_becomes_searchable() {
        let dir = tempfile::tempdir().unwrap();
        let (doc, path, mut conn) = docx_file(dir.path(), "h1", &["Plankton blooms follow iron enrichment in the Gulf of Alaska."]);
        let seen = run(&mut conn, &path, &doc, &CancellationToken::new());
        assert_eq!(seen, [DocStatus::Processing, DocStatus::Ready]);
        let opts = repo::SearchOptions { project_id: None, all_terms: true, prefix_last: false, limit: 5 };
        assert_eq!(repo::search_passages(&conn, "plankton iron", &opts).unwrap().len(), 1);
        assert_eq!(repo::get_document(&conn, &doc.id).unwrap().passage_count, 1);
    }

    #[test]
    fn empty_and_broken_files_end_in_a_clear_status() {
        let dir = tempfile::tempdir().unwrap();
        let (doc, path, mut conn) = docx_file(dir.path(), "h1", &[]);
        assert_eq!(run(&mut conn, &path, &doc, &CancellationToken::new()), [DocStatus::Processing, DocStatus::NoText]);
        assert!(repo::get_document(&conn, &doc.id).unwrap().error.unwrap().contains("No text"));

        std::fs::write(&path, b"garbage").unwrap();
        let seen = run(&mut conn, &path, &doc, &CancellationToken::new());
        assert_eq!(seen.last(), Some(&DocStatus::Error));
        assert!(repo::get_document(&conn, &doc.id).unwrap().error.unwrap().contains("Word document"));

        let missing = dir.path().join("gone.docx");
        let seen = run(&mut conn, &missing, &doc, &CancellationToken::new());
        assert_eq!(seen.last(), Some(&DocStatus::Error));
    }

    #[test]
    fn a_cancelled_job_is_not_read_and_keeps_no_content() {
        let dir = tempfile::tempdir().unwrap();
        let (doc, path, mut conn) = docx_file(dir.path(), "h1", &["Some text that should never be indexed."]);
        let token = CancellationToken::new();
        token.cancel();
        assert_eq!(run(&mut conn, &path, &doc, &token), [DocStatus::Cancelled]);
        assert_eq!(repo::get_document(&conn, &doc.id).unwrap().passage_count, 0);
    }

    #[test]
    fn a_document_deleted_mid_job_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let (doc, path, mut conn) = docx_file(dir.path(), "h1", &["Text for a document that is about to vanish."]);
        repo::delete_document(&conn, &doc.id).unwrap();
        assert!(run(&mut conn, &path, &doc, &CancellationToken::new()).is_empty());
    }

    #[test]
    fn page_notes_describe_missing_text() {
        use crate::persistence::library::{Extracted, NewPage};
        let page = |q| NewPage { page_no: 1, width: 1.0, height: 1.0, quality: q };
        let ex = |pages, unreadable| Extraction { pages, blocks: vec![], meta: Extracted::default(), unreadable_pages: unreadable };
        assert_eq!(page_note(&ex(vec![page("ok"), page("ok")], 0)), None);
        let n = page_note(&ex(vec![page("ok"), page("none"), page("low"), page("none")], 1)).unwrap();
        assert!(n.contains("1 pages could not be read") && n.contains("1 of 4 pages have no text") && n.contains("1 pages have unreliable"), "{n}");
    }

    #[test]
    fn library_paths_are_sharded_by_hash() {
        let p = library_path(Path::new("/lib"), "abcdef", DocKind::Pdf);
        assert_eq!(p, Path::new("/lib/ab/abcdef.pdf"));
    }
}
