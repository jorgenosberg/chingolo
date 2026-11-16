//! The background worker. One thread reads files one at a time, so pdfium never runs twice at once,
//! and it uses its own database connection so a long import doesn't block chat.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use specta::Type;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::activity::Activity;
use super::layout::{self, LayoutSlot};
use super::ocr::OcrSlot;
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

/// How far page layout has got, for the library's progress notice.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LayoutProgress {
    /// False until the layout model is downloaded.
    pub available: bool,
    pub done: u32,
    pub total: u32,
    pub docs_left: u32,
}

pub type Emit = Arc<dyn Fn(DocumentChanged) + Send + Sync>;

/// Read everything again, or only look at the details (title, authors and so on).
enum Job {
    Read(Document),
    Details(Document),
    /// Page layout for a document that is already searchable. Rebuilds its passages from it.
    Layout(Document),
}

struct Inner {
    tx: mpsc::UnboundedSender<Job>,
    tokens: Mutex<HashMap<Id, CancellationToken>>,
    library_dir: PathBuf,
    emit: Emit,
    /// Where the text recognition models are (or will be). None turns recognition off.
    ocr_dir: Mutex<Option<PathBuf>>,
    /// Same for the layout model.
    layout_dir: Mutex<Option<PathBuf>>,
    /// Called with the document after layout changed its passages (new ones need vectors).
    on_layout: Mutex<Option<Arc<dyn Fn(&Document) + Send + Sync>>>,
    /// Pages the running layout job has done, which the database doesn't know until it ends.
    layout_live: AtomicU32,
    activity: Mutex<Option<Activity>>,
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
        let inner = Arc::new(Inner { tx, tokens: Mutex::new(HashMap::new()), library_dir, emit, ocr_dir: Mutex::new(None), layout_dir: Mutex::new(None), on_layout: Mutex::new(None), layout_live: AtomicU32::new(0), activity: Mutex::new(None) });
        let worker = inner.clone();
        std::thread::Builder::new()
            .name("ingest".into())
            .spawn(move || worker_loop(conn, rx, worker))
            .map_err(|e| AppError::Internal(format!("couldn't start the import worker: {e}")))?;
        Ok(Ingestor(inner))
    }

    /// Pages without a text layer are read with text recognition from the next job on, once
    /// the models in `dir` are downloaded.
    pub fn set_ocr_dir(&self, dir: Option<PathBuf>) {
        if let Ok(mut d) = self.0.ocr_dir.lock() {
            *d = dir;
        }
    }

    /// Page layout runs on ready documents once the model in `dir` is downloaded.
    pub fn set_layout_dir(&self, dir: Option<PathBuf>) {
        if let Ok(mut d) = self.0.layout_dir.lock() {
            *d = dir;
        }
    }

    pub fn set_on_layout(&self, f: Arc<dyn Fn(&Document) + Send + Sync>) {
        if let Ok(mut h) = self.0.on_layout.lock() {
            *h = Some(f);
        }
    }

    /// Layout work: pages done and in all (pages with text of ready PDFs), and documents still waiting.
    pub async fn layout_progress(&self, db: &Db) -> Result<LayoutProgress> {
        let ready = self.0.layout_dir.lock().ok().and_then(|d| d.clone()).is_some_and(|d| layout::is_ready(&d));
        if !ready {
            return Ok(LayoutProgress::default());
        }
        let (done, total, docs_left) = db.call(|c| repo::layout_counts(c)).await?;
        Ok(LayoutProgress { available: true, done: (done + self.0.layout_live.load(Ordering::Relaxed)).min(total), total, docs_left })
    }

    /// Queues layout for every ready PDF that has pages without it. Call at start and after the
    /// layout model finished downloading. Returns how many documents.
    pub async fn enqueue_layouts(&self, db: &Db) -> Result<usize> {
        let ready = self.0.layout_dir.lock().ok().and_then(|d| d.clone()).is_some_and(|d| layout::is_ready(&d));
        if !ready {
            return Ok(0);
        }
        let docs = db.call(|c| repo::documents_without_layout(c)).await?;
        for d in &docs {
            let _ = self.0.tx.send(Job::Layout(d.clone()));
        }
        Ok(docs.len())
    }

    /// Where details jobs are reported while they wait and run.
    pub fn set_activity(&self, activity: Activity) {
        if let Ok(mut a) = self.0.activity.lock() {
            *a = Some(activity);
        }
    }

    fn activity(&self) -> Option<Activity> {
        self.0.activity.lock().ok().and_then(|a| a.clone())
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
        let _ = self.0.tx.send(Job::Read(doc.clone()));
    }

    /// Reads the file properties and first pages again and refreshes the details. Passages,
    /// status and vectors stay as they are.
    pub fn enqueue_details(&self, doc: &Document) {
        if let Some(a) = self.activity() {
            a.start(&doc.id);
        }
        let _ = self.0.tx.send(Job::Details(doc.clone()));
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

fn worker_loop(mut conn: Connection, mut rx: mpsc::UnboundedReceiver<Job>, inner: Arc<Inner>) {
    let mut pdf: Option<PdfReader> = None;
    let mut ocr = OcrSlot::default();
    let mut layout = LayoutSlot::default();
    while let Some(job) = rx.blocking_recv() {
        let (Job::Read(doc) | Job::Details(doc) | Job::Layout(doc)) = &job;
        let doc = doc.clone();
        ocr.set_dir(inner.ocr_dir.lock().ok().and_then(|d| d.clone()));
        layout.set_dir(inner.layout_dir.lock().ok().and_then(|d| d.clone()));
        let token = inner.tokens.lock().ok().and_then(|t| t.get(&doc.id).cloned()).unwrap_or_default();
        let path = library_path(&inner.library_dir, &doc.content_hash, doc.kind);
        let emit = |d: Document, p: Option<Progress>| (inner.emit)(DocumentChanged { document: d, progress: p });
        // A bug in a parser must not take the worker down with it.
        let ran = std::panic::catch_unwind(AssertUnwindSafe(|| match &job {
            Job::Read(_) => run_job(&mut conn, &mut pdf, &mut ocr, &path, &doc, &token, &emit),
            Job::Details(_) => run_details(&conn, &mut pdf, &mut ocr, &path, &doc, &emit),
            Job::Layout(_) => {
                let live = &inner.layout_live;
                let notify = |d: &Document| inner.on_layout.lock().ok().and_then(|h| h.clone()).iter().for_each(|f| f(d));
                let out = run_layout(&mut conn, &mut pdf, &mut ocr, &mut layout, &path, &doc, &token, live, &notify);
                live.store(0, Ordering::Relaxed);
                out
            }
        }));
        if ran.is_err() && matches!(job, Job::Details(_)) {
            log::warn!("reading the details of {} crashed", doc.original_name);
        } else if ran.is_err() && matches!(job, Job::Layout(_)) {
            log::warn!("page layout of {} crashed, its passages are unchanged", doc.original_name);
        } else if ran.is_err() {
            if let Ok(d) = repo::set_status(&conn, &doc.id, DocStatus::Error, Some("Reading this file crashed. It may be damaged.")) {
                emit(d, None);
            }
        }
        if matches!(job, Job::Details(_))
            && let Some(a) = inner.activity.lock().ok().and_then(|a| a.clone())
        {
            a.finish(&doc.id);
        }
        if matches!(job, Job::Read(_)) {
            if let Ok(mut t) = inner.tokens.lock() {
                t.remove(&doc.id);
            }
            // the document is searchable now, so layout can improve it in the background
            if doc.kind == DocKind::Pdf && layout.get().is_some() && repo::get_document(&conn, &doc.id).is_ok_and(|d| d.status == DocStatus::Ready) {
                let _ = inner.tx.send(Job::Layout(doc.clone()));
            }
        }
    }
}

/// Refreshes title, authors, year and the rest from the file. Failures only log: the document
/// keeps the details it has.
pub fn run_details(
    conn: &Connection,
    pdf: &mut Option<PdfReader>,
    ocr: &mut OcrSlot,
    path: &Path,
    doc: &Document,
    emit: &dyn Fn(Document, Option<Progress>),
) {
    let started = std::time::Instant::now();
    let found = match doc.kind {
        DocKind::Pdf => reader(pdf).and_then(|r| r.extract_meta(path, ocr)),
        DocKind::Docx => docx::extract(path, &|| false).map(|ex| ex.meta),
    };
    let fields = found.as_ref().map(|m| m.fields()).unwrap_or_default();
    let result = found.and_then(|m| repo::apply_metadata(conn, &doc.id, &m, "file", true));
    match result {
        Ok(()) => {
            log::info!(
                "details refreshed for {}: found {} ({} edited fields kept), {} ms",
                doc.original_name,
                if fields.is_empty() { "nothing".to_string() } else { fields.join(", ") },
                doc.meta_edited.len(),
                started.elapsed().as_millis()
            );
            if let Ok(d) = repo::get_document(conn, &doc.id) {
                emit(d, None);
            }
        }
        Err(AppError::NotFound) => {}
        Err(e) => log::warn!("couldn't refresh the details of {}: {e}", doc.original_name),
    }
}

/// Runs layout on the pages that have text and rebuilds the passages from it. Problems only log:
/// the passages stay as they are. Runs in the background, so a document is searchable long before.
#[allow(clippy::too_many_arguments)]
fn run_layout(
    conn: &mut Connection,
    pdf: &mut Option<PdfReader>,
    ocr: &mut OcrSlot,
    layout: &mut LayoutSlot,
    path: &Path,
    doc: &Document,
    token: &CancellationToken,
    live: &AtomicU32,
    notify: &dyn Fn(&Document),
) {
    let started = std::time::Instant::now();
    let Some(model) = layout.get() else { return };
    let cancelled = || token.is_cancelled();
    let result = (|| -> Result<Option<Document>> {
        let pages = repo::pages_without_layout(conn, &doc.id)?;
        if pages.is_empty() {
            return Ok(None);
        }
        let r = reader(pdf)?;
        let found = r.detect_layout(path, &pages, model, &cancelled, &mut |done, _| live.store(done, Ordering::Relaxed))?;
        let ex = r.extract_with(path, ocr, &cancelled, &mut |_, _| {})?;
        let (blocks, stats) = layout::apply(ex.blocks, &found);
        let passages = segment(&blocks);
        if passages.is_empty() {
            return Ok(None);
        }
        let regions: Vec<_> = found
            .iter()
            .flat_map(|p| p.regions.iter().map(|g| repo::NewRegion { page_no: p.page, ordinal: g.order, label: g.label.clone(), score: g.score, rect: g.rect }))
            .collect();
        let done: Vec<u32> = found.iter().map(|p| p.page).collect();
        let d = repo::store_layout(conn, &doc.id, &done, &regions, &passages)?;
        log::info!(
            "layout for {}: {} pages, {} lines, {} dropped (headers, footers, numbers), {} unplaced, {} pages kept as they were, {} passages, {} ms",
            doc.original_name, done.len(), stats.lines, stats.dropped, stats.unplaced, stats.pages_kept, passages.len(), started.elapsed().as_millis()
        );
        Ok(Some(d))
    })();
    match result {
        Ok(Some(d)) => notify(&d),
        Ok(None) | Err(AppError::NotFound | AppError::Cancelled) => {}
        Err(e) => log::warn!("page layout of {} failed: {e}", doc.original_name),
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
    ocr: &mut OcrSlot,
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
            r.extract_with(path, ocr, &cancelled, &mut |done, total| {
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
            DocKind::Pdf if ocr.missing() => {
                "No text found. This looks like a scan. Download the text recognition model in Settings, then read it again."
            }
            DocKind::Pdf => "No text found. This is probably a scan without a text layer.",
            DocKind::Docx => "No text found in this document.",
        };
        (DocStatus::NoText, Some(why.to_string()))
    } else {
        (DocStatus::Ready, page_note(&ex, ocr.missing()))
    };
    match repo::store_content(conn, &doc.id, &ex.pages, &passages, &ex.meta, status, note.as_deref()) {
        Ok(d) => {
            log::info!(
                "read {}: {} pages, {} passages, details found: {}",
                doc.original_name,
                ex.pages.len(),
                passages.len(),
                ex.meta.fields().join(", ")
            );
            emit(d, None)
        }
        Err(AppError::NotFound) => {}
        Err(e) => finish(conn, DocStatus::Error, Some(&format!("Couldn't save the text: {e}"))),
    }
}

/// A short note about pages that are missing or unreliable, if any.
fn page_note(ex: &Extraction, ocr_missing: bool) -> Option<String> {
    let total = ex.pages.len();
    let count = |q: &str| ex.pages.iter().filter(|p| p.quality == q).count();
    let (none, low) = (count("none"), count("low"));
    let read = ex.pages.iter().filter(|p| p.ocr).count();
    let mut parts = vec![];
    if read > 0 {
        parts.push(format!("{read} pages were read by text recognition and may have errors."));
    }
    if ex.unreadable_pages > 0 {
        parts.push(format!("{} pages could not be read.", ex.unreadable_pages));
    }
    let none_other = none.saturating_sub(ex.unreadable_pages as usize);
    if none_other > 0 {
        parts.push(format!("{none_other} of {total} pages have no text (blank, images or scans)."));
        if ocr_missing {
            parts.push("Download the text recognition model in Settings and read it again to include scanned pages.".into());
        }
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
        run_job(conn, &mut None, &mut OcrSlot::default(), path, doc, token, &|d, _| seen.borrow_mut().push(d.status));
        seen.into_inner()
    }

    #[test]
    fn a_document_goes_from_processing_to_ready_and_becomes_searchable() {
        let dir = tempfile::tempdir().unwrap();
        let (doc, path, mut conn) = docx_file(dir.path(), "h1", &["Plankton blooms follow iron enrichment in the Gulf of Alaska."]);
        let seen = run(&mut conn, &path, &doc, &CancellationToken::new());
        assert_eq!(seen, [DocStatus::Processing, DocStatus::Ready]);
        let opts = repo::SearchOptions { filter: &crate::domain::library::DocFilter::All, all_terms: true, prefix_last: false, limit: 5 };
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
        let page = |q| NewPage { page_no: 1, width: 1.0, height: 1.0, quality: q, ocr: false };
        let ex = |pages, unreadable| Extraction { pages, blocks: vec![], meta: Extracted::default(), unreadable_pages: unreadable };
        assert_eq!(page_note(&ex(vec![page("ok"), page("ok")], 0), false), None);
        let n = page_note(&ex(vec![page("ok"), page("none"), page("low"), page("none")], 1), false).unwrap();
        let mut read = page("ok");
        read.ocr = true;
        let n2 = page_note(&ex(vec![read, page("none")], 0), true).unwrap();
        assert!(n2.contains("1 pages were read by text recognition") && n2.contains("Download the text recognition model"), "{n2}");
        assert!(n.contains("1 pages could not be read") && n.contains("1 of 4 pages have no text") && n.contains("1 pages have unreliable"), "{n}");
    }

    /// CHINGUI_TEST_PAPERS=<folder> CHINGUI_LAYOUT_DIR=<model folder> cargo test layout_job -- --ignored --nocapture
    #[test]
    #[ignore]
    fn layout_job_rebuilds_passages_and_keeps_unchanged_ones() {
        let papers = std::env::var("CHINGUI_TEST_PAPERS").expect("set CHINGUI_TEST_PAPERS");
        let model = std::env::var("CHINGUI_LAYOUT_DIR").expect("set CHINGUI_LAYOUT_DIR");
        let path = Path::new(&papers).join("nature02437.pdf");
        let mut conn = test_conn();
        let doc = repo::insert_document(&conn, "hnat", "nature02437.pdf", DocKind::Pdf).unwrap();
        let mut pdf = None;
        let mut ocr = OcrSlot::default();
        run_job(&mut conn, &mut pdf, &mut ocr, &path, &doc, &CancellationToken::new(), &|_, _| {});
        let ids = |c: &Connection| -> Vec<String> {
            let mut s = c.prepare("SELECT id FROM passages WHERE document_id = ?1").unwrap();
            s.query_map([&doc.id], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
        };
        let before = ids(&conn);
        let mut slot = LayoutSlot::default();
        slot.set_dir(Some(model.into()));
        let seen = std::cell::Cell::new(0);
        run_layout(&mut conn, &mut pdf, &mut ocr, &mut slot, &path, &doc, &CancellationToken::new(), &AtomicU32::new(0), &|_| seen.set(seen.get() + 1));
        let after = ids(&conn);
        let same = after.iter().filter(|a| before.contains(a)).count();
        println!("passages {} -> {}, {} kept their id", before.len(), after.len(), same);
        assert_eq!(seen.get(), 1);
        assert!(same > 0 && repo::pages_without_layout(&conn, &doc.id).unwrap().is_empty());
        let regions: i64 = conn.query_row("SELECT count(*) FROM page_regions", [], |r| r.get(0)).unwrap();
        assert!(regions > 50);
        // a second run has nothing left to do
        run_layout(&mut conn, &mut pdf, &mut ocr, &mut slot, &path, &doc, &CancellationToken::new(), &AtomicU32::new(0), &|_| seen.set(seen.get() + 1));
        assert_eq!(seen.get(), 1);
    }

    /// Imports a folder into a database file for the retrieval bench (keyword search only, no vectors).
    /// CHINGUI_TEST_PAPERS=<folder> CHINGUI_KEEP_DB=<file> [CHINGUI_LAYOUT_DIR=<model folder>] cargo test corpus_db -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn corpus_db() {
        use super::super::import::import_paths;
        let papers = PathBuf::from(std::env::var("CHINGUI_TEST_PAPERS").expect("set CHINGUI_TEST_PAPERS"));
        let db_path = PathBuf::from(std::env::var("CHINGUI_KEEP_DB").expect("set CHINGUI_KEEP_DB"));
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", db_path.display()));
        }
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&db_path).unwrap();
        let ingest = Ingestor::start(&db_path, tmp.path().join("library"), Arc::new(|_| {})).unwrap();
        let layout = std::env::var("CHINGUI_LAYOUT_DIR").ok().map(PathBuf::from);
        ingest.set_layout_dir(layout.clone());
        let t = std::time::Instant::now();
        import_paths(&db, &ingest, vec![papers], None).await.unwrap();
        loop {
            let docs = db.call(|c| repo::list_documents(c, None)).await.unwrap();
            let busy = docs.iter().any(|d| matches!(d.status, DocStatus::Queued | DocStatus::Processing));
            let waiting = layout.is_some() && !db.call(|c| repo::documents_without_layout(c)).await.unwrap().is_empty();
            if !busy && !waiting {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let n: i64 = db.call(|c| Ok(c.query_row("SELECT count(*) FROM passages", [], |r| r.get(0))?)).await.unwrap();
        println!("{n} passages after {:.0}s", t.elapsed().as_secs_f32());
    }

    #[test]
    fn library_paths_are_sharded_by_hash() {
        let p = library_path(Path::new("/lib"), "abcdef", DocKind::Pdf);
        assert_eq!(p, Path::new("/lib/ab/abcdef.pdf"));
    }
}
