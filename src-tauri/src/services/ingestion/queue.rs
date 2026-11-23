//! The background worker. One thread reads files one at a time, so pdfium never runs twice at once,
//! and it uses its own database connection so a long import doesn't block chat.

use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use specta::Type;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::activity::Activity;
use super::layout::{self, LayoutSlot};
use super::ocr::OcrSlot;
use super::pdf::PdfReader;
use super::segment::segment;
use super::tables::{self, TableSlot};
use super::{Extraction, docx};
use crate::domain::Id;
use crate::domain::library::{DocKind, DocStatus, Document};
use crate::error::{AppError, Result};
use crate::persistence::{Db, library as repo};
use crate::services::jobs::Lane;
use crate::services::usage_counts::{self, Event};

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
    /// Table regions read and in all. Zero until the table model is downloaded.
    pub tables_done: u32,
    pub tables_total: u32,
}

pub type Emit = Arc<dyn Fn(DocumentChanged) + Send + Sync>;

/// Read everything again, or only look at the details (title, authors and so on).
enum Job {
    Read(Document),
    Details(Document),
    /// Page layout for a document that is already searchable. Rebuilds its passages from it.
    Layout(Document),
}

impl Job {
    fn doc(&self) -> &Document {
        let (Job::Read(d) | Job::Details(d) | Job::Layout(d)) = self;
        d
    }

    fn lane(&self) -> Lane {
        match self {
            Job::Read(_) => Lane::Read,
            Job::Details(_) => Lane::Details,
            Job::Layout(_) => Lane::Layout,
        }
    }

    /// Lower runs first, so a new import never waits behind a backlog of layout work.
    fn priority(&self) -> u8 {
        match self {
            Job::Read(_) => 0,
            Job::Details(_) => 1,
            Job::Layout(_) => 2,
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Called with a document after layout changed its passages.
type LayoutHook = Arc<dyn Fn(&Document) + Send + Sync>;

struct Inner {
    /// Waiting jobs. The worker takes the best one that isn't in a paused lane.
    queue: Mutex<VecDeque<Job>>,
    wake: Condvar,
    paused: Mutex<HashSet<Lane>>,
    tokens: Mutex<HashMap<(Lane, Id), CancellationToken>>,
    library_dir: PathBuf,
    emit: Emit,
    /// Where the text recognition models are (or will be). None turns recognition off.
    ocr_dir: Mutex<Option<PathBuf>>,
    /// Same for the layout model.
    layout_dir: Mutex<Option<PathBuf>>,
    tables_dir: Mutex<Option<PathBuf>>,
    /// Called with the document after layout changed its passages (new ones need vectors).
    on_layout: Mutex<Option<LayoutHook>>,
    /// Pages the running layout job has done, which the database doesn't know until it ends.
    layout_live: AtomicU32,
    /// Same for tables read by the running job.
    tables_live: AtomicU32,
    activity: Mutex<Option<Activity>>,
    /// The job the worker is on, for the job list.
    running: Mutex<Option<Running>>,
    /// Woken when the queue or the running job changes, so the job list refreshes.
    poke: Mutex<Option<Arc<Notify>>>,
}

/// The job on the worker right now. `progress` is pages read so far, for reads only.
#[derive(Debug, Clone)]
pub struct Running {
    pub lane: Lane,
    pub id: Id,
    pub title: String,
    pub progress: Option<Progress>,
}

impl Inner {
    fn new(library_dir: PathBuf, emit: Emit) -> Self {
        Inner {
            queue: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
            paused: Mutex::new(HashSet::new()),
            tokens: Mutex::new(HashMap::new()),
            library_dir,
            emit,
            ocr_dir: Mutex::new(None),
            layout_dir: Mutex::new(None),
            tables_dir: Mutex::new(None),
            on_layout: Mutex::new(None),
            layout_live: AtomicU32::new(0),
            tables_live: AtomicU32::new(0),
            activity: Mutex::new(None),
            running: Mutex::new(None),
            poke: Mutex::new(None),
        }
    }
}

#[derive(Clone)]
pub struct Ingestor(Arc<Inner>);

/// Where the managed copy of a file lives: `<library>/<first two hash chars>/<hash>.<ext>`.
pub fn library_path(library_dir: &Path, hash: &str, kind: DocKind) -> PathBuf {
    library_dir
        .join(&hash[..2])
        .join(format!("{hash}.{}", kind.as_str()))
}

impl Ingestor {
    pub fn start(db_path: &Path, library_dir: PathBuf, emit: Emit) -> Result<Self> {
        let conn = Db::open_secondary(db_path)?;
        let inner = Arc::new(Inner::new(library_dir, emit));
        let worker = inner.clone();
        std::thread::Builder::new()
            .name("ingest".into())
            .spawn(move || worker_loop(conn, worker))
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

    pub fn set_tables_dir(&self, dir: Option<PathBuf>) {
        if let Ok(mut d) = self.0.tables_dir.lock() {
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
        let ready = self
            .0
            .layout_dir
            .lock()
            .ok()
            .and_then(|d| d.clone())
            .is_some_and(|d| layout::is_ready(&d));
        if !ready {
            return Ok(LayoutProgress::default());
        }
        let (done, total, docs_left) = db.call(|c| repo::layout_counts(c)).await?;
        let tables_ready = self
            .0
            .tables_dir
            .lock()
            .ok()
            .and_then(|d| d.clone())
            .is_some_and(|d| tables::is_ready(&d));
        let (tables_done, tables_total) = if tables_ready {
            db.call(|c| repo::table_counts(c)).await?
        } else {
            (0, 0)
        };
        Ok(LayoutProgress {
            available: true,
            done: (done + self.0.layout_live.load(Ordering::Relaxed)).min(total),
            total,
            docs_left,
            tables_done: (tables_done + self.0.tables_live.load(Ordering::Relaxed))
                .min(tables_total),
            tables_total,
        })
    }

    /// Queues layout for one document, for a retry after earlier failures.
    pub fn enqueue_layout(&self, doc: &Document) {
        queue_layout(&self.0, doc);
        self.poke();
    }

    /// Queues layout for every ready PDF that has pages without it. Call at start and after the
    /// layout model finished downloading. Returns how many documents.
    pub async fn enqueue_layouts(&self, db: &Db) -> Result<usize> {
        let ready = self
            .0
            .layout_dir
            .lock()
            .ok()
            .and_then(|d| d.clone())
            .is_some_and(|d| layout::is_ready(&d));
        if !ready {
            return Ok(0);
        }
        let mut docs = db.call(|c| repo::documents_without_layout(c)).await?;
        if self
            .0
            .tables_dir
            .lock()
            .ok()
            .and_then(|d| d.clone())
            .is_some_and(|d| tables::is_ready(&d))
        {
            for d in db.call(|c| repo::documents_without_tables(c)).await? {
                if !docs.iter().any(|x| x.id == d.id) {
                    docs.push(d);
                }
            }
        }
        for d in &docs {
            queue_layout(&self.0, d);
        }
        self.poke();
        Ok(docs.len())
    }

    /// The job list wakes on this when the queue or the running job changes.
    pub fn set_poke(&self, poke: Arc<Notify>) {
        if let Ok(mut p) = self.0.poke.lock() {
            *p = Some(poke);
        }
    }

    fn poke(&self) {
        poke(&self.0);
    }

    /// Stops the worker from starting jobs of this lane. A job already running finishes.
    pub fn set_paused(&self, lane: Lane, paused: bool) {
        {
            let mut set = lock(&self.0.paused);
            if paused {
                set.insert(lane);
            } else {
                set.remove(&lane);
            }
        }
        // Taking the queue lock first means the worker is either waiting or will see the change.
        drop(lock(&self.0.queue));
        self.0.wake.notify_all();
        self.poke();
    }

    pub fn is_paused(&self, lane: Lane) -> bool {
        lock(&self.0.paused).contains(&lane)
    }

    /// The job the worker is on, if any.
    pub fn running(&self) -> Option<Running> {
        self.0.running.lock().ok().and_then(|r| r.clone())
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
        register_token(&self.0, Lane::Read, &doc.id);
        push(&self.0, Job::Read(doc.clone()));
        self.poke();
    }

    /// Reads the file properties and first pages again and refreshes the details. Passages,
    /// status and vectors stay as they are.
    pub fn enqueue_details(&self, doc: &Document) {
        if let Some(a) = self.activity() {
            a.start(&doc.id);
        }
        push(&self.0, Job::Details(doc.clone()));
        self.poke();
    }

    /// True if the document was queued or running.
    pub fn cancel(&self, id: &str) -> bool {
        self.cancel_lane(Lane::Read, id)
    }

    /// Stops a queued or running job of one lane. True if there was one.
    pub fn cancel_lane(&self, lane: Lane, id: &str) -> bool {
        match lock(&self.0.tokens).get(&(lane, id.to_string())).cloned() {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Reads scans that came up empty again, for when the OCR model has just become ready.
    pub async fn enqueue_scans(&self, db: &Db) -> Result<usize> {
        let docs = db
            .call(|c| {
                let mut out = Vec::new();
                for d in repo::no_text_pdfs(c)? {
                    out.push(repo::set_status(c, &d.id, DocStatus::Queued, None)?);
                }
                Ok(out)
            })
            .await?;
        for d in &docs {
            self.enqueue(d);
            self.emit(d.clone(), None);
        }
        Ok(docs.len())
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

/// A fresh cancel token for a queued job, replacing an older one for the same document.
fn register_token(inner: &Inner, lane: Lane, id: &str) {
    lock(&inner.tokens).insert((lane, id.to_string()), CancellationToken::new());
}

fn push(inner: &Inner, job: Job) {
    lock(&inner.queue).push_back(job);
    inner.wake.notify_all();
}

/// Queues layout for a document unless it is already queued or running.
fn queue_layout(inner: &Inner, doc: &Document) {
    let running = lock(&inner.running)
        .as_ref()
        .is_some_and(|r| r.lane == Lane::Layout && r.id == doc.id);
    let mut queue = lock(&inner.queue);
    let queued = queue
        .iter()
        .any(|j| j.lane() == Lane::Layout && j.doc().id == doc.id);
    if running || queued {
        return;
    }
    register_token(inner, Lane::Layout, &doc.id);
    queue.push_back(Job::Layout(doc.clone()));
    inner.wake.notify_all();
}

/// Blocks until a job is waiting in a lane that isn't paused, and takes the best one: reads before
/// details before layout, oldest first within a kind.
fn next_job(inner: &Inner) -> Job {
    let mut queue = lock(&inner.queue);
    loop {
        let paused = lock(&inner.paused).clone();
        let best = queue
            .iter()
            .enumerate()
            .filter(|(_, j)| !paused.contains(&j.lane()))
            .min_by_key(|(i, j)| (j.priority(), *i))
            .map(|(i, _)| i);
        if let Some(job) = best.and_then(|i| queue.remove(i)) {
            return job;
        }
        queue = inner.wake.wait(queue).unwrap_or_else(|e| e.into_inner());
    }
}

fn poke(inner: &Inner) {
    if let Some(p) = inner.poke.lock().ok().and_then(|p| p.clone()) {
        p.notify_one();
    }
}

fn set_running(inner: &Inner, now: Option<Running>) {
    if let Ok(mut r) = inner.running.lock() {
        *r = now;
    }
    poke(inner);
}

fn worker_loop(mut conn: Connection, inner: Arc<Inner>) {
    let mut pdf: Option<PdfReader> = None;
    let mut ocr = OcrSlot::default();
    let mut layout = LayoutSlot::default();
    let mut tables = TableSlot::default();
    loop {
        let job = next_job(&inner);
        let doc = job.doc().clone();
        ocr.set_dir(inner.ocr_dir.lock().ok().and_then(|d| d.clone()));
        layout.set_dir(inner.layout_dir.lock().ok().and_then(|d| d.clone()));
        tables.set_dir(inner.tables_dir.lock().ok().and_then(|d| d.clone()));
        let lane = job.lane();
        let token = lock(&inner.tokens)
            .get(&(lane, doc.id.clone()))
            .cloned()
            .unwrap_or_default();
        let path = library_path(&inner.library_dir, &doc.content_hash, doc.kind);
        set_running(
            &inner,
            Some(Running {
                lane,
                id: doc.id.clone(),
                title: doc
                    .title
                    .clone()
                    .unwrap_or_else(|| doc.original_name.clone()),
                progress: None,
            }),
        );
        let emit = |d: Document, p: Option<Progress>| {
            if let Some(p) = &p
                && let Ok(mut r) = inner.running.lock()
                && let Some(r) = r.as_mut()
            {
                r.progress = Some(p.clone());
            }
            (inner.emit)(DocumentChanged {
                document: d,
                progress: p,
            })
        };
        // A bug in a parser must not take the worker down with it.
        let ran = std::panic::catch_unwind(AssertUnwindSafe(|| match &job {
            Job::Read(_) => run_job(&mut conn, &mut pdf, &mut ocr, &path, &doc, &token, &emit),
            Job::Details(_) => run_details(&conn, &mut pdf, &mut ocr, &path, &doc, &emit),
            Job::Layout(_) => {
                let live = (&inner.layout_live, &inner.tables_live);
                let notify = |d: &Document| {
                    inner
                        .on_layout
                        .lock()
                        .ok()
                        .and_then(|h| h.clone())
                        .iter()
                        .for_each(|f| f(d))
                };
                run_layout(
                    &mut conn,
                    &mut pdf,
                    &mut ocr,
                    &mut layout,
                    &mut tables,
                    &path,
                    &doc,
                    &token,
                    live,
                    &notify,
                );
                live.0.store(0, Ordering::Relaxed);
                live.1.store(0, Ordering::Relaxed);
            }
        }));
        set_running(&inner, None);
        if ran.is_err() && matches!(job, Job::Details(_)) {
            log::warn!("reading the details of document {} crashed", doc.id);
        } else if ran.is_err() && matches!(job, Job::Layout(_)) {
            log::warn!(
                "page layout of {} crashed, its passages are unchanged",
                doc.id
            );
            let _ = repo::set_layout_error(&conn, &doc.id, Some("Page layout crashed"));
            usage_counts::count_conn(&conn, Event::ImportFailed, "", "", "layout");
        } else if ran.is_err()
            && let Ok(d) = repo::set_status(
                &conn,
                &doc.id,
                DocStatus::Error,
                Some("Reading this file crashed. It may be damaged."),
            )
        {
            usage_counts::count_conn(&conn, Event::ImportFailed, "", "", "read");
            emit(d, None);
        }
        if matches!(job, Job::Details(_))
            && let Some(a) = inner.activity.lock().ok().and_then(|a| a.clone())
        {
            a.finish(&doc.id);
        }
        lock(&inner.tokens).remove(&(lane, doc.id.clone()));
        if matches!(job, Job::Read(_)) {
            // the document is searchable now, so layout can improve it in the background
            if doc.kind == DocKind::Pdf
                && layout.get().is_some()
                && repo::get_document(&conn, &doc.id).is_ok_and(|d| d.status == DocStatus::Ready)
            {
                queue_layout(&inner, &doc);
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
                doc.id,
                if fields.is_empty() {
                    "nothing".to_string()
                } else {
                    fields.join(", ")
                },
                doc.meta_edited.len(),
                started.elapsed().as_millis()
            );
            if let Ok(d) = repo::get_document(conn, &doc.id) {
                emit(d, None);
            }
        }
        Err(AppError::NotFound) => {}
        Err(e) => {
            log::warn!("couldn't refresh the details of {}: {e}", doc.id);
            usage_counts::count_conn(conn, Event::ImportFailed, "", "", "details");
        }
    }
}

/// Runs layout on the pages that have text, reads the tables in table regions, and rebuilds the
/// passages from both. Problems only log: the passages stay as they are. Runs in the background,
/// so a document is searchable long before.
#[allow(clippy::too_many_arguments)]
fn run_layout(
    conn: &mut Connection,
    pdf: &mut Option<PdfReader>,
    ocr: &mut OcrSlot,
    layout: &mut LayoutSlot,
    tables: &mut TableSlot,
    path: &Path,
    doc: &Document,
    token: &CancellationToken,
    live: (&AtomicU32, &AtomicU32),
    notify: &dyn Fn(&Document),
) {
    let started = std::time::Instant::now();
    let Some(model) = layout.get() else { return };
    let table_model = tables.get();
    let cancelled = || token.is_cancelled();
    let result = (|| -> Result<Option<Document>> {
        let pages = repo::pages_without_layout(conn, &doc.id)?;
        let r = reader(pdf)?;
        let new = if pages.is_empty() {
            vec![]
        } else {
            r.detect_layout(path, &pages, model, &cancelled, &mut |done, _| {
                live.0.store(done, Ordering::Relaxed)
            })?
        };
        // pages done by an earlier run keep their regions, so the rebuilt passages don't lose them
        let mut found = repo::list_regions(conn, &doc.id)?.into_iter().fold(
            Vec::<layout::PageRegions>::new(),
            |mut acc, g| {
                if acc.last().is_none_or(|p| p.page != g.page) {
                    acc.push(layout::PageRegions {
                        page: g.page,
                        regions: vec![],
                    });
                }
                if let Some(p) = acc.last_mut() {
                    p.regions.push(layout::Region {
                        label: g.label,
                        rect: g.rect,
                        score: g.score,
                        order: g.ordinal,
                    });
                }
                acc
            },
        );
        let done: Vec<u32> = new.iter().map(|p| p.page).collect();
        found.extend(new);
        found.sort_by_key(|p| p.page);

        // tables not read yet, in the regions just found or found by an earlier run
        let mut read = repo::list_tables(conn, &doc.id)?;
        let todo: Vec<(u32, u32, [f32; 4])> = match table_model {
            Some(_) => found
                .iter()
                .flat_map(|p| {
                    p.regions
                        .iter()
                        .filter(|g| g.label == "table")
                        .map(move |g| (p.page, g.order, g.rect))
                })
                .filter(|(page, ordinal, _)| {
                    !read
                        .iter()
                        .any(|t| t.page_no == *page && t.ordinal == *ordinal)
                })
                .collect(),
            None => vec![],
        };
        if done.is_empty() && todo.is_empty() {
            return Ok(None);
        }
        let mut fresh = Vec::new();
        if let (Some(model), false) = (table_model, todo.is_empty()) {
            let regions: Vec<(u32, [f32; 4])> = todo.iter().map(|(p, _, r)| (*p, *r)).collect();
            for (t, (page, ordinal, rect)) in r
                .read_tables(path, &regions, model, &cancelled, &mut |done, _| {
                    live.1.store(done, Ordering::Relaxed)
                })?
                .into_iter()
                .zip(&todo)
            {
                let (status, note, footer, cells) = match t.filled {
                    Ok(f) if f.sound() && !tables::has_duplicate_positions(&f.cells) => {
                        ("ok", None, Some(f.notes).filter(|n| !n.is_empty()), f.cells)
                    }
                    Ok(f) => {
                        let why = format!(
                            "{} of {} characters in cells, {} beside, numbers {}",
                            f.assigned,
                            f.glyphs,
                            f.noted,
                            if f.numbers_ok { "match" } else { "differ" }
                        );
                        ("unsure", Some(why), None, f.cells)
                    }
                    Err(e) => ("failed", Some(e), None, vec![]),
                };
                fresh.push(repo::TableRow {
                    page_no: *page,
                    ordinal: *ordinal,
                    rect: *rect,
                    status: status.into(),
                    note,
                    footer,
                    cells,
                });
            }
            repo::store_tables(conn, &doc.id, &fresh)?;
            read.extend(fresh.iter().cloned());
        }

        let ex = r.extract_with(path, ocr, &cancelled, &mut |_, _| {})?;
        let (blocks, stats) = layout::apply(ex.blocks, &found);
        let grids: Vec<_> = read
            .iter()
            .filter(|t| t.status == "ok")
            .map(|t| {
                (
                    t.page_no,
                    t.rect,
                    tables::passages(t.page_no, t.rect, &t.cells, t.footer.as_deref()),
                )
            })
            .collect();
        let passages = layout::weave(&blocks, &found, &grids);
        if passages.is_empty() {
            return Ok(None);
        }
        let regions: Vec<_> = found
            .iter()
            .flat_map(|p| {
                p.regions.iter().map(|g| repo::NewRegion {
                    page_no: p.page,
                    ordinal: g.order,
                    label: g.label.clone(),
                    score: g.score,
                    rect: g.rect,
                })
            })
            .collect();
        let d = repo::store_layout(conn, &doc.id, &done, &regions, &passages)?;
        let count = |s: &str| fresh.iter().filter(|t| t.status == s).count();
        log::info!(
            "layout for {}: {} pages, {} lines, {} dropped (headers, footers, numbers), {} unplaced, {} pages kept as they were, {} tables read ({} ok, {} unsure, {} failed), {} passages, {} ms",
            doc.id,
            done.len(),
            stats.lines,
            stats.dropped,
            stats.unplaced,
            stats.pages_kept,
            fresh.len(),
            count("ok"),
            count("unsure"),
            count("failed"),
            passages.len(),
            started.elapsed().as_millis()
        );
        Ok(Some(d))
    })();
    match result {
        Ok(found) => {
            let _ = repo::set_layout_error(conn, &doc.id, None);
            if let Some(d) = found {
                notify(&d);
            }
        }
        Err(AppError::NotFound | AppError::Cancelled) => {}
        Err(e) => {
            log::warn!("page layout of {} failed: {e}", doc.id);
            let _ =
                repo::set_layout_error(conn, &doc.id, Some(&format!("Page layout failed: {e}")));
            usage_counts::count_conn(conn, Event::ImportFailed, "", "", "layout");
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
    ocr: &mut OcrSlot,
    path: &Path,
    doc: &Document,
    token: &CancellationToken,
    emit: &dyn Fn(Document, Option<Progress>),
) {
    let finish = |conn: &Connection, status: DocStatus, note: Option<&str>| {
        // The document may have been deleted while it was being read.
        if let Ok(d) = repo::set_status(conn, &doc.id, status, note) {
            if status == DocStatus::Error {
                usage_counts::count_conn(conn, Event::ImportFailed, "", "", "read");
            }
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
    match repo::store_content(
        conn,
        &doc.id,
        &ex.pages,
        &passages,
        &ex.meta,
        status,
        note.as_deref(),
    ) {
        Ok(d) => {
            log::info!(
                "read document {}: {} pages, {} passages, details found: {}",
                doc.id,
                ex.pages.len(),
                passages.len(),
                ex.meta.fields().join(", ")
            );
            match d.status {
                DocStatus::Ready => {
                    usage_counts::count_conn(conn, Event::DocumentImported, "", "", "")
                }
                DocStatus::NoText => {
                    usage_counts::count_conn(conn, Event::ImportFailed, "", "", "no_text")
                }
                _ => {}
            }
            emit(d, None)
        }
        Err(AppError::NotFound) => {}
        Err(e) => finish(
            conn,
            DocStatus::Error,
            Some(&format!("Couldn't save the text: {e}")),
        ),
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
        parts.push(format!(
            "{read} pages were read by text recognition and may have errors."
        ));
    }
    if ex.unreadable_pages > 0 {
        parts.push(format!("{} pages could not be read.", ex.unreadable_pages));
    }
    let none_other = none.saturating_sub(ex.unreadable_pages as usize);
    if none_other > 0 {
        parts.push(format!(
            "{none_other} of {total} pages have no text (blank, images or scans)."
        ));
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

    fn doc(conn: &Connection, hash: &str) -> Document {
        repo::insert_document(conn, hash, &format!("{hash}.pdf"), DocKind::Pdf).unwrap()
    }

    fn inner() -> Inner {
        Inner::new(PathBuf::new(), Arc::new(|_| {}))
    }

    fn order(inner: &Inner, n: usize) -> Vec<(Lane, String)> {
        (0..n)
            .map(|_| {
                let j = next_job(inner);
                (j.lane(), j.doc().original_name.clone())
            })
            .collect()
    }

    #[test]
    fn reads_go_before_details_before_layout_and_each_kind_keeps_its_order() {
        let conn = test_conn();
        let (a, b, c, d, e) = (
            doc(&conn, "a"),
            doc(&conn, "b"),
            doc(&conn, "c"),
            doc(&conn, "d"),
            doc(&conn, "e"),
        );
        let q = inner();
        queue_layout(&q, &a);
        push(&q, Job::Details(b));
        push(&q, Job::Read(c));
        queue_layout(&q, &d);
        push(&q, Job::Read(e));
        let got = order(&q, 5);
        let names: Vec<&str> = got.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(names, ["c.pdf", "e.pdf", "b.pdf", "a.pdf", "d.pdf"]);
    }

    #[test]
    fn a_paused_lane_is_skipped_and_its_jobs_wait() {
        let conn = test_conn();
        let (a, b) = (doc(&conn, "a"), doc(&conn, "b"));
        let q = inner();
        push(&q, Job::Read(a));
        queue_layout(&q, &b);
        lock(&q.paused).insert(Lane::Read);
        let first = next_job(&q);
        assert_eq!(first.lane(), Lane::Layout);
        assert_eq!(lock(&q.queue).len(), 1, "the read stays queued");
    }

    #[test]
    fn the_worker_wakes_when_a_paused_lane_is_resumed() {
        let conn = test_conn();
        let q = Arc::new(inner());
        lock(&q.paused).insert(Lane::Read);
        push(&q, Job::Read(doc(&conn, "a")));
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = q.clone();
        std::thread::spawn(move || {
            let _ = tx.send(next_job(&worker).lane());
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "a paused lane must not start"
        );
        // what `Ingestor::set_paused` does
        lock(&q.paused).remove(&Lane::Read);
        drop(lock(&q.queue));
        q.wake.notify_all();
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),
            Lane::Read
        );
    }

    #[test]
    fn layout_is_not_queued_twice_for_one_document() {
        let conn = test_conn();
        let a = doc(&conn, "a");
        let q = inner();
        queue_layout(&q, &a);
        queue_layout(&q, &a);
        assert_eq!(lock(&q.queue).len(), 1);
        let job = next_job(&q);
        // while it runs, a second request is also dropped
        *lock(&q.running) = Some(Running {
            lane: Lane::Layout,
            id: job.doc().id.clone(),
            title: String::new(),
            progress: None,
        });
        queue_layout(&q, &a);
        assert!(lock(&q.queue).is_empty());
    }

    #[test]
    fn cancel_tokens_are_kept_per_lane_and_document() {
        let conn = test_conn();
        let a = doc(&conn, "a");
        let q = inner();
        register_token(&q, Lane::Read, &a.id);
        register_token(&q, Lane::Layout, &a.id);
        let read = lock(&q.tokens)[&(Lane::Read, a.id.clone())].clone();
        let layout = lock(&q.tokens)[&(Lane::Layout, a.id.clone())].clone();
        read.cancel();
        assert!(read.is_cancelled());
        assert!(!layout.is_cancelled());
    }

    fn docx_file(dir: &Path, hash: &str, paragraphs: &[&str]) -> (Document, PathBuf, Connection) {
        let conn = test_conn();
        let doc = repo::insert_document(&conn, hash, "t.docx", DocKind::Docx).unwrap();
        let path = dir.join("t.docx");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        let body: String = paragraphs
            .iter()
            .map(|p| format!("<w:p><w:r><w:t>{p}</w:t></w:r></w:p>"))
            .collect();
        write!(
            zip,
            r#"<w:document xmlns:w="x"><w:body>{body}</w:body></w:document>"#
        )
        .unwrap();
        zip.finish().unwrap();
        (doc, path, conn)
    }

    fn run(
        conn: &mut Connection,
        path: &Path,
        doc: &Document,
        token: &CancellationToken,
    ) -> Vec<DocStatus> {
        let seen = std::cell::RefCell::new(vec![]);
        run_job(
            conn,
            &mut None,
            &mut OcrSlot::default(),
            path,
            doc,
            token,
            &|d, _| seen.borrow_mut().push(d.status),
        );
        seen.into_inner()
    }

    #[test]
    fn a_document_goes_from_processing_to_ready_and_becomes_searchable() {
        let dir = tempfile::tempdir().unwrap();
        let (doc, path, mut conn) = docx_file(
            dir.path(),
            "h1",
            &["Plankton blooms follow iron enrichment in the Gulf of Alaska."],
        );
        let seen = run(&mut conn, &path, &doc, &CancellationToken::new());
        assert_eq!(seen, [DocStatus::Processing, DocStatus::Ready]);
        let opts = repo::SearchOptions {
            filter: &crate::domain::library::DocFilter::All,
            all_terms: true,
            prefix_last: false,
            limit: 5,
        };
        assert_eq!(
            repo::search_passages(&conn, "plankton iron", &opts)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(repo::get_document(&conn, &doc.id).unwrap().passage_count, 1);
    }

    #[test]
    fn empty_and_broken_files_end_in_a_clear_status() {
        let dir = tempfile::tempdir().unwrap();
        let (doc, path, mut conn) = docx_file(dir.path(), "h1", &[]);
        assert_eq!(
            run(&mut conn, &path, &doc, &CancellationToken::new()),
            [DocStatus::Processing, DocStatus::NoText]
        );
        assert!(
            repo::get_document(&conn, &doc.id)
                .unwrap()
                .error
                .unwrap()
                .contains("No text")
        );

        std::fs::write(&path, b"garbage").unwrap();
        let seen = run(&mut conn, &path, &doc, &CancellationToken::new());
        assert_eq!(seen.last(), Some(&DocStatus::Error));
        assert!(
            repo::get_document(&conn, &doc.id)
                .unwrap()
                .error
                .unwrap()
                .contains("Word document")
        );

        let missing = dir.path().join("gone.docx");
        let seen = run(&mut conn, &missing, &doc, &CancellationToken::new());
        assert_eq!(seen.last(), Some(&DocStatus::Error));
    }

    #[test]
    fn a_cancelled_job_is_not_read_and_keeps_no_content() {
        let dir = tempfile::tempdir().unwrap();
        let (doc, path, mut conn) = docx_file(
            dir.path(),
            "h1",
            &["Some text that should never be indexed."],
        );
        let token = CancellationToken::new();
        token.cancel();
        assert_eq!(run(&mut conn, &path, &doc, &token), [DocStatus::Cancelled]);
        assert_eq!(repo::get_document(&conn, &doc.id).unwrap().passage_count, 0);
    }

    #[test]
    fn a_document_deleted_mid_job_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let (doc, path, mut conn) = docx_file(
            dir.path(),
            "h1",
            &["Text for a document that is about to vanish."],
        );
        repo::delete_document(&conn, &doc.id).unwrap();
        assert!(run(&mut conn, &path, &doc, &CancellationToken::new()).is_empty());
    }

    #[test]
    fn page_notes_describe_missing_text() {
        use crate::persistence::library::{Extracted, NewPage};
        let page = |q| NewPage {
            page_no: 1,
            width: 1.0,
            height: 1.0,
            quality: q,
            ocr: false,
        };
        let ex = |pages, unreadable| Extraction {
            pages,
            blocks: vec![],
            meta: Extracted::default(),
            unreadable_pages: unreadable,
        };
        assert_eq!(page_note(&ex(vec![page("ok"), page("ok")], 0), false), None);
        let n = page_note(
            &ex(vec![page("ok"), page("none"), page("low"), page("none")], 1),
            false,
        )
        .unwrap();
        let mut read = page("ok");
        read.ocr = true;
        let n2 = page_note(&ex(vec![read, page("none")], 0), true).unwrap();
        assert!(
            n2.contains("1 pages were read by text recognition")
                && n2.contains("Download the text recognition model"),
            "{n2}"
        );
        assert!(
            n.contains("1 pages could not be read")
                && n.contains("1 of 4 pages have no text")
                && n.contains("1 pages have unreliable"),
            "{n}"
        );
    }

    /// CHINGOLO_TEST_PAPERS=<folder> CHINGOLO_LAYOUT_DIR=<model folder> cargo test layout_job -- --ignored --nocapture
    #[test]
    #[ignore]
    fn layout_job_rebuilds_passages_and_keeps_unchanged_ones() {
        let papers = std::env::var("CHINGOLO_TEST_PAPERS").expect("set CHINGOLO_TEST_PAPERS");
        let model = std::env::var("CHINGOLO_LAYOUT_DIR").expect("set CHINGOLO_LAYOUT_DIR");
        let path = Path::new(&papers).join("nature02437.pdf");
        let mut conn = test_conn();
        let doc = repo::insert_document(&conn, "hnat", "nature02437.pdf", DocKind::Pdf).unwrap();
        let mut pdf = None;
        let mut ocr = OcrSlot::default();
        run_job(
            &mut conn,
            &mut pdf,
            &mut ocr,
            &path,
            &doc,
            &CancellationToken::new(),
            &|_, _| {},
        );
        let ids = |c: &Connection| -> Vec<String> {
            let mut s = c
                .prepare("SELECT id FROM passages WHERE document_id = ?1")
                .unwrap();
            s.query_map([&doc.id], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        let before = ids(&conn);
        let mut slot = LayoutSlot::default();
        slot.set_dir(Some(model.into()));
        let seen = std::cell::Cell::new(0);
        run_layout(
            &mut conn,
            &mut pdf,
            &mut ocr,
            &mut slot,
            &mut TableSlot::default(),
            &path,
            &doc,
            &CancellationToken::new(),
            (&AtomicU32::new(0), &AtomicU32::new(0)),
            &|_| seen.set(seen.get() + 1),
        );
        let after = ids(&conn);
        let same = after.iter().filter(|a| before.contains(a)).count();
        println!(
            "passages {} -> {}, {} kept their id",
            before.len(),
            after.len(),
            same
        );
        assert_eq!(seen.get(), 1);
        assert!(
            same > 0
                && repo::pages_without_layout(&conn, &doc.id)
                    .unwrap()
                    .is_empty()
        );
        let regions: i64 = conn
            .query_row("SELECT count(*) FROM page_regions", [], |r| r.get(0))
            .unwrap();
        assert!(regions > 50);
        // a second run has nothing left to do
        run_layout(
            &mut conn,
            &mut pdf,
            &mut ocr,
            &mut slot,
            &mut TableSlot::default(),
            &path,
            &doc,
            &CancellationToken::new(),
            (&AtomicU32::new(0), &AtomicU32::new(0)),
            &|_| seen.set(seen.get() + 1),
        );
        assert_eq!(seen.get(), 1);
    }

    /// Layout and tables end to end: grids are stored, table passages carry them, a rerun does nothing.
    /// CHINGOLO_TEST_PAPERS=<folder> CHINGOLO_LAYOUT_DIR=<model> CHINGOLO_TABLES_DIR=<model> cargo test tables_job -- --ignored --nocapture
    #[test]
    #[ignore]
    fn tables_job_stores_grids_and_builds_table_passages() {
        let papers = std::env::var("CHINGOLO_TEST_PAPERS").expect("set CHINGOLO_TEST_PAPERS");
        let model = std::env::var("CHINGOLO_LAYOUT_DIR").expect("set CHINGOLO_LAYOUT_DIR");
        let tables_dir = std::env::var("CHINGOLO_TABLES_DIR").expect("set CHINGOLO_TABLES_DIR");
        let path = Path::new(&papers).join("nature02437.pdf");
        let mut conn = test_conn();
        let doc = repo::insert_document(&conn, "htab", "nature02437.pdf", DocKind::Pdf).unwrap();
        let mut pdf = None;
        let mut ocr = OcrSlot::default();
        run_job(
            &mut conn,
            &mut pdf,
            &mut ocr,
            &path,
            &doc,
            &CancellationToken::new(),
            &|_, _| {},
        );
        let mut layout = LayoutSlot::default();
        layout.set_dir(Some(model.into()));
        let mut tables = TableSlot::default();
        tables.set_dir(Some(tables_dir.into()));
        let seen = std::cell::Cell::new(0);
        let go = |conn: &mut Connection,
                  pdf: &mut Option<PdfReader>,
                  ocr: &mut OcrSlot,
                  layout: &mut LayoutSlot,
                  tables: &mut TableSlot| {
            run_layout(
                conn,
                pdf,
                ocr,
                layout,
                tables,
                &path,
                &doc,
                &CancellationToken::new(),
                (&AtomicU32::new(0), &AtomicU32::new(0)),
                &|_| seen.set(seen.get() + 1),
            );
        };
        go(&mut conn, &mut pdf, &mut ocr, &mut layout, &mut tables);
        let stored = repo::list_tables(&conn, &doc.id).unwrap();
        let table_regions: i64 = conn
            .query_row(
                "SELECT count(*) FROM page_regions WHERE label = 'table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        println!(
            "{table_regions} table regions, table model ready: {}",
            tables.get().is_some()
        );
        for t in &stored {
            println!(
                "page {} table {}: {} ({} cells) {:?}",
                t.page_no,
                t.ordinal,
                t.status,
                t.cells.len(),
                t.note
            );
        }
        assert!(stored.iter().any(|t| t.status == "ok"), "no table was read");
        let grids: i64 = conn
            .query_row(
                "SELECT count(*) FROM passages WHERE text LIKE '| %'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        println!("{grids} table passages");
        let unconfirmed: Vec<String> = conn
            .prepare(
                "SELECT text FROM passages WHERE text LIKE '%Not confirmed by a second reading%'",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        println!("{} passages list unconfirmed cells", unconfirmed.len());
        if let Some(t) = unconfirmed.first() {
            println!("{t}");
        }
        assert!(grids > 0);
        assert_eq!(seen.get(), 1);
        go(&mut conn, &mut pdf, &mut ocr, &mut layout, &mut tables);
        assert_eq!(seen.get(), 1, "a second run should find nothing to do");
        assert!(repo::documents_without_tables(&conn).unwrap().is_empty());
    }

    /// Imports a folder into a database file for the retrieval bench (keyword search only, no vectors).
    /// CHINGOLO_TEST_PAPERS=<folder> CHINGOLO_KEEP_DB=<file> [CHINGOLO_LAYOUT_DIR=<model folder>] cargo test corpus_db -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn corpus_db() {
        use super::super::import::import_paths;
        let papers =
            PathBuf::from(std::env::var("CHINGOLO_TEST_PAPERS").expect("set CHINGOLO_TEST_PAPERS"));
        let db_path =
            PathBuf::from(std::env::var("CHINGOLO_KEEP_DB").expect("set CHINGOLO_KEEP_DB"));
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", db_path.display()));
        }
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&db_path).unwrap();
        let ingest =
            Ingestor::start(&db_path, tmp.path().join("library"), Arc::new(|_| {})).unwrap();
        let layout = std::env::var("CHINGOLO_LAYOUT_DIR").ok().map(PathBuf::from);
        ingest.set_layout_dir(layout.clone());
        let t = std::time::Instant::now();
        import_paths(&db, &ingest, vec![papers], None)
            .await
            .unwrap();
        loop {
            let docs = db.call(|c| repo::list_documents(c, None)).await.unwrap();
            let busy = docs
                .iter()
                .any(|d| matches!(d.status, DocStatus::Queued | DocStatus::Processing));
            let waiting = layout.is_some()
                && !db
                    .call(|c| repo::documents_without_layout(c))
                    .await
                    .unwrap()
                    .is_empty();
            if !busy && !waiting {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let n: i64 = db
            .call(|c| Ok(c.query_row("SELECT count(*) FROM passages", [], |r| r.get(0))?))
            .await
            .unwrap();
        println!("{n} passages after {:.0}s", t.elapsed().as_secs_f32());
    }

    #[test]
    fn library_paths_are_sharded_by_hash() {
        let p = library_path(Path::new("/lib"), "abcdef", DocKind::Pdf);
        assert_eq!(p, Path::new("/lib/ab/abcdef.pdf"));
    }
}
