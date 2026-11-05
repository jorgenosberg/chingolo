//! The embedding engine: model state, the background worker that gives every ready passage a
//! vector, and query embedding for search. The worker has its own thread and database
//! connection, like the import worker, so neither blocks the other or chat.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use fastembed::TextEmbedding;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use specta::Type;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::model::{self, FILES, ModelState, REPO_URL, REVISION};
use super::{DOC_PREFIX, MODEL_NAME, QUERY_PREFIX};
use crate::error::{AppError, Result};
use crate::persistence::{Db, library as repo, vectors};
use crate::services::ingestion::queue::{DocumentChanged, Emit};

/// Passages per model call. Small, so a search query never waits long for the model.
const BATCH: u32 = 8;

/// Sent whenever the model state, the indexing counts or the pause flag change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingStatus {
    pub model: ModelState,
    /// Passages of ready documents that have a vector, and all such passages.
    pub indexed: u32,
    pub total: u32,
    pub paused: bool,
}

pub type StatusEmit = Arc<dyn Fn(EmbeddingStatus) + Send + Sync>;

/// Turns text into L2-normalized vectors. The real one wraps the ONNX model; tests use a fake.
pub trait Encoder: Send {
    fn encode(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

pub type Loader = Arc<dyn Fn(&Path) -> Result<Box<dyn Encoder>> + Send + Sync>;

struct Gemma(TextEmbedding);

impl Encoder for Gemma {
    fn encode(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let vecs = self
            .0
            .embed(texts, Some(BATCH as usize))
            .map_err(|e| AppError::Internal(format!("The search model failed: {e}")))?;
        Ok(vecs.into_iter().map(normalize).collect())
    }
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

fn gemma_loader() -> Loader {
    Arc::new(|dir| Ok(Box::new(Gemma(super::load_model(dir)?)) as Box<dyn Encoder>))
}

struct Inner {
    dir: PathBuf,
    state: Mutex<ModelState>,
    counts: Mutex<(u32, u32)>,
    paused: AtomicBool,
    encoder: Mutex<Option<Box<dyn Encoder>>>,
    /// Searches waiting for the model. The worker steps aside for them between batches.
    waiting: AtomicUsize,
    wake: mpsc::UnboundedSender<()>,
    emit_status: StatusEmit,
    emit_doc: Emit,
    load: Loader,
}

#[derive(Clone)]
pub struct Embedder(Arc<Inner>);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Embedder {
    /// `app_data` is the app's data folder. `emit_doc` must not be the one that wakes this
    /// engine, or every batch would wake itself.
    pub fn start(db_path: &Path, app_data: &Path, emit_status: StatusEmit, emit_doc: Emit) -> Result<Self> {
        let dir = model::model_dir(app_data);
        let ready = model::is_complete(&dir, FILES);
        Self::start_with(db_path, dir, ready, gemma_loader(), emit_status, emit_doc)
    }

    pub fn start_with(
        db_path: &Path,
        dir: PathBuf,
        ready: bool,
        load: Loader,
        emit_status: StatusEmit,
        emit_doc: Emit,
    ) -> Result<Self> {
        let conn = Db::open_secondary(db_path)?;
        let (wake, rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            dir,
            state: Mutex::new(if ready { ModelState::Ready } else { ModelState::Missing }),
            counts: Mutex::new((0, 0)),
            paused: AtomicBool::new(false),
            encoder: Mutex::new(None),
            waiting: AtomicUsize::new(0),
            wake,
            emit_status,
            emit_doc,
            load,
        });
        let worker = inner.clone();
        std::thread::Builder::new()
            .name("embed".into())
            .spawn(move || worker_loop(conn, rx, worker))
            .map_err(|e| AppError::Internal(format!("couldn't start the embedding worker: {e}")))?;
        let me = Embedder(inner);
        me.wake();
        Ok(me)
    }

    pub fn status(&self) -> EmbeddingStatus {
        let (indexed, total) = *lock(&self.0.counts);
        EmbeddingStatus {
            model: lock(&self.0.state).clone(),
            indexed,
            total,
            paused: self.0.paused.load(Ordering::SeqCst),
        }
    }

    fn publish(&self) {
        (self.0.emit_status)(self.status());
    }

    /// Ask the worker to look for passages without a vector. Cheap, safe to call often.
    pub fn wake(&self) {
        let _ = self.0.wake.send(());
    }

    pub fn set_paused(&self, paused: bool) {
        self.0.paused.store(paused, Ordering::SeqCst);
        self.publish();
        if !paused {
            self.wake();
        }
    }

    fn set_state(&self, state: ModelState) {
        *lock(&self.0.state) = state;
        self.publish();
    }

    /// Makes sure the model is usable: starts the download if files are missing (and resumes a
    /// partial one), or retries loading when they are all there. Returns at once.
    pub fn ensure_model(&self) {
        let complete = model::is_complete(&self.0.dir, FILES);
        let downloading = {
            let mut state = lock(&self.0.state);
            let downloading = matches!(*state, ModelState::Downloading { .. });
            if !downloading {
                *state = if complete {
                    ModelState::Ready
                } else {
                    ModelState::Downloading { done: 0.0, total: model::total_bytes(FILES) as f64 }
                };
            }
            downloading
        };
        if downloading {
            return;
        }
        self.publish();
        if complete {
            self.wake();
            return;
        }
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            let client = model::http_client();
            let cancel = CancellationToken::new();
            let progress_for = me.clone();
            let result = model::download(&client, REPO_URL, FILES, &me.0.dir, &cancel, &mut |done, total| {
                progress_for.set_state(ModelState::Downloading { done: done as f64, total: total as f64 });
            })
            .await;
            match result {
                Ok(()) => {
                    me.set_state(ModelState::Ready);
                    me.wake();
                }
                Err(AppError::Cancelled) => me.set_state(ModelState::Missing),
                Err(e) => me.set_state(ModelState::Failed { message: e.to_string() }),
            }
        });
    }

    /// Runs `f` with the encoder, loading the model on first use.
    fn with_encoder<T>(&self, f: impl FnOnce(&mut dyn Encoder) -> Result<T>) -> Result<T> {
        let mut slot = lock(&self.0.encoder);
        if slot.is_none() {
            *slot = Some((self.0.load)(&self.0.dir)?);
        }
        f(slot.as_mut().expect("just loaded").as_mut())
    }

    /// The vector for a search query, or `None` while the model isn't ready (keyword search
    /// still works). Blocks while the model runs, so call it from a blocking thread.
    pub fn embed_query(&self, query: &str) -> Result<Option<Vec<f32>>> {
        if !matches!(*lock(&self.0.state), ModelState::Ready) {
            return Ok(None);
        }
        self.0.waiting.fetch_add(1, Ordering::SeqCst);
        let text = vec![format!("{QUERY_PREFIX}{query}")];
        let out = self.with_encoder(|e| e.encode(&text));
        self.0.waiting.fetch_sub(1, Ordering::SeqCst);
        Ok(out?.into_iter().next())
    }

    fn refresh_counts(&self, conn: &Connection) {
        if let Ok((indexed, total)) = vectors::totals(conn, MODEL_NAME, REVISION) {
            *lock(&self.0.counts) = (indexed, total);
        }
    }

    fn fail(&self, e: AppError) {
        self.set_state(ModelState::Failed { message: e.to_string() });
    }

    /// Embeds pending passages until none are left, the model isn't ready or the user pauses.
    fn drain(&self, conn: &mut Connection) {
        loop {
            let ready = matches!(*lock(&self.0.state), ModelState::Ready);
            if !ready || self.0.paused.load(Ordering::SeqCst) {
                break;
            }
            let Ok(batch) = vectors::pending_batch(conn, MODEL_NAME, REVISION, BATCH) else { break };
            if batch.is_empty() {
                break;
            }
            let texts: Vec<String> = batch.iter().map(|p| format!("{DOC_PREFIX}{}", p.text)).collect();
            let vecs = match self.with_encoder(|e| e.encode(&texts)) {
                Ok(v) if v.len() == batch.len() => v,
                Ok(_) => return self.fail(AppError::Internal("The search model returned the wrong number of vectors.".into())),
                Err(e) => return self.fail(e),
            };
            let items: Vec<_> = batch.iter().map(|p| p.passage_id.clone()).zip(vecs).collect();
            if let Err(e) = vectors::store_vectors(conn, &items, MODEL_NAME, REVISION) {
                return self.fail(e);
            }
            self.refresh_counts(conn);
            self.publish();
            let mut seen: Vec<&str> = Vec::new();
            for p in &batch {
                if seen.contains(&p.document_id.as_str()) {
                    continue;
                }
                seen.push(&p.document_id);
                if let Ok(document) = repo::get_document(conn, &p.document_id) {
                    (self.0.emit_doc)(DocumentChanged { document, progress: None });
                }
            }
            // Let a waiting search have the model before the next batch.
            while self.0.waiting.load(Ordering::SeqCst) > 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        self.refresh_counts(conn);
        self.publish();
    }
}

fn worker_loop(mut conn: Connection, mut rx: mpsc::UnboundedReceiver<()>, inner: Arc<Inner>) {
    let me = Embedder(inner);
    // Vectors from another model or revision are no use: drop them so everything is re-embedded.
    let _ = vectors::purge_stale(&conn, MODEL_NAME, REVISION);
    while rx.blocking_recv().is_some() {
        while rx.try_recv().is_ok() {}
        // A bug in the model code must not take the worker down with it.
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| me.drain(&mut conn)));
        if ran.is_err() {
            me.fail(AppError::Internal("The search model crashed.".into()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::library::{DocKind, DocStatus};
    use crate::persistence::library::{Extracted, NewPage, NewPassage};

    /// A vector made from the text, so equal texts get equal vectors.
    struct Fake {
        calls: Arc<AtomicUsize>,
        fail_after: Option<usize>,
    }

    impl Encoder for Fake {
        fn encode(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_after.is_some_and(|k| n >= k) {
                return Err(AppError::Internal("boom".into()));
            }
            Ok(texts
                .iter()
                .map(|t| {
                    let a = t.len() as f32;
                    normalize(vec![a, 1.0, t.bytes().map(|b| b as f32).sum::<f32>() % 7.0 + 1.0])
                })
                .collect())
        }
    }

    struct Rig {
        _dir: tempfile::TempDir,
        db: Connection,
        embedder: Embedder,
        statuses: Arc<Mutex<Vec<EmbeddingStatus>>>,
        docs: Arc<Mutex<Vec<DocumentChanged>>>,
        calls: Arc<AtomicUsize>,
    }

    fn rig(ready: bool, fail_after: Option<usize>, load_fails: bool) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let _main = Db::open(&path).unwrap();
        let db = Db::open_secondary(&path).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let load: Loader = Arc::new(move |_| {
            if load_fails {
                return Err(AppError::Internal("no model".into()));
            }
            Ok(Box::new(Fake { calls: c.clone(), fail_after }) as Box<dyn Encoder>)
        });
        let statuses = Arc::new(Mutex::new(Vec::new()));
        let docs = Arc::new(Mutex::new(Vec::new()));
        let (s, d) = (statuses.clone(), docs.clone());
        let embedder = Embedder::start_with(
            &path,
            dir.path().join("model"),
            ready,
            load,
            Arc::new(move |e| lock(&s).push(e)),
            Arc::new(move |e| lock(&d).push(e)),
        )
        .unwrap();
        Rig { _dir: dir, db, embedder, statuses, docs, calls }
    }

    fn add_doc(conn: &mut Connection, hash: &str, n: usize) -> crate::domain::Id {
        let d = repo::insert_document(conn, hash, &format!("{hash}.pdf"), DocKind::Pdf).unwrap();
        let passages: Vec<_> = (0..n)
            .map(|i| NewPassage { page_start: 1, page_end: 1, text: format!("{hash} passage number {i}"), boxes: None })
            .collect();
        let pages = vec![NewPage { page_no: 1, width: 1.0, height: 1.0, quality: "ok" }];
        repo::store_content(conn, &d.id, &pages, &passages, &Extracted::default(), DocStatus::Ready, None).unwrap();
        d.id
    }

    fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        for _ in 0..500 {
            if ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    fn totals(r: &Rig) -> (u32, u32) {
        vectors::totals(&r.db, MODEL_NAME, REVISION).unwrap()
    }

    #[test]
    fn embeds_every_ready_passage_and_reports_progress_and_documents() {
        let mut r = rig(true, None, false);
        let a = add_doc(&mut r.db, "a", 11);
        let b = add_doc(&mut r.db, "b", 3);
        r.embedder.wake();
        wait_for("embedding", || totals(&r) == (14, 14));
        wait_for("final status", || r.embedder.status().indexed == 14);

        let last = r.embedder.status();
        assert_eq!((last.indexed, last.total, last.paused), (14, 14, false));
        assert_eq!(r.calls.load(Ordering::SeqCst), 2, "11 + 3 passages in batches of 8, in document order");
        let progress: Vec<u32> = lock(&r.statuses).iter().map(|s| s.indexed).collect();
        assert!(progress.windows(2).all(|w| w[0] <= w[1]) && progress.contains(&8));
        let docs = lock(&r.docs);
        assert!(docs.iter().any(|d| d.document.id == a && d.document.embedded_count > 0));
        assert!(docs.iter().any(|d| d.document.id == b && d.document.embedded_count == 3));
    }

    #[test]
    fn pausing_stops_the_worker_and_resuming_finishes() {
        let mut r = rig(true, None, false);
        r.embedder.set_paused(true);
        add_doc(&mut r.db, "a", 5);
        r.embedder.wake();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(totals(&r).0, 0);
        assert!(r.embedder.status().paused);

        r.embedder.set_paused(false);
        wait_for("embedding after resume", || totals(&r) == (5, 5));
    }

    #[test]
    fn nothing_happens_until_the_model_is_ready_and_keyword_data_is_untouched() {
        let mut r = rig(false, None, false);
        add_doc(&mut r.db, "a", 3);
        r.embedder.wake();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(totals(&r).0, 0);
        assert_eq!(r.calls.load(Ordering::SeqCst), 0);
        assert_eq!(r.embedder.status().model, ModelState::Missing);
        assert_eq!(r.embedder.embed_query("anything").unwrap(), None);
    }

    #[test]
    fn a_model_that_cannot_load_becomes_a_failed_state() {
        let mut r = rig(true, None, true);
        add_doc(&mut r.db, "a", 2);
        r.embedder.wake();
        wait_for("failure", || matches!(r.embedder.status().model, ModelState::Failed { .. }));
        let ModelState::Failed { message } = r.embedder.status().model else { unreachable!() };
        assert!(message.contains("no model"), "{message}");
        assert_eq!(totals(&r).0, 0);
    }

    #[test]
    fn an_error_mid_way_keeps_what_was_stored_and_fails() {
        let mut r = rig(true, Some(1), false);
        add_doc(&mut r.db, "a", 12);
        r.embedder.wake();
        wait_for("failure", || matches!(r.embedder.status().model, ModelState::Failed { .. }));
        assert_eq!(totals(&r).0, 8, "the first batch was kept");
    }

    #[test]
    fn queries_get_a_vector_with_the_query_prefix_when_ready() {
        let r = rig(true, None, false);
        let v = r.embedder.embed_query("coral").unwrap().unwrap();
        assert_eq!(v.len(), 3);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
        // the fake derives its first component from the text length, so the prefix is included
        let expect = ("coral".len() + QUERY_PREFIX.len()) as f32;
        let direct = normalize(vec![expect, 1.0, format!("{QUERY_PREFIX}coral").bytes().map(|b| b as f32).sum::<f32>() % 7.0 + 1.0]);
        assert_eq!(v, direct);
    }

    #[test]
    fn deleting_documents_changes_the_totals_on_the_next_wake() {
        let mut r = rig(true, None, false);
        let a = add_doc(&mut r.db, "a", 3);
        r.embedder.wake();
        wait_for("embedding", || totals(&r) == (3, 3));
        repo::delete_document(&r.db, &a).unwrap();
        r.embedder.wake();
        wait_for("totals", || {
            let s = r.embedder.status();
            (s.indexed, s.total) == (0, 0)
        });
    }

    /// Imports a folder of papers through the real import worker, embeds them with the real
    /// model, and prints the timings. `CHINGUI_KEEP_DB` keeps the database for
    /// `tools/retrieval-bench app`.
    /// Run: CHINGUI_TEST_PAPERS=<folder> CHINGUI_EMBED_MODEL_DIR=<model folder> \
    ///      CHINGUI_KEEP_DB=<file> cargo test corpus_embed -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn corpus_embed() {
        use crate::services::ingestion::import::import_paths;
        use crate::services::ingestion::queue::Ingestor;
        use std::time::Instant;

        let papers = std::path::PathBuf::from(std::env::var("CHINGUI_TEST_PAPERS").expect("set CHINGUI_TEST_PAPERS"));
        let model_dir = std::path::PathBuf::from(std::env::var("CHINGUI_EMBED_MODEL_DIR").expect("set CHINGUI_EMBED_MODEL_DIR"));
        let tmp = tempfile::tempdir().unwrap();
        let db_path = match std::env::var("CHINGUI_KEEP_DB") {
            Ok(p) => {
                let p = std::path::PathBuf::from(p);
                for ext in ["", "-wal", "-shm"] {
                    let _ = std::fs::remove_file(format!("{}{ext}", p.display()));
                }
                p
            }
            Err(_) => tmp.path().join("t.sqlite"),
        };
        let db = Db::open(&db_path).unwrap();
        let docs_changed = Arc::new(AtomicUsize::new(0));
        let counter = docs_changed.clone();
        let embedder = Embedder::start_with(
            &db_path,
            model_dir,
            true,
            gemma_loader(),
            Arc::new(|_| {}),
            Arc::new(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .unwrap();
        let wake = embedder.clone();
        let ingest = Ingestor::start(
            &db_path,
            tmp.path().join("library"),
            Arc::new(move |e| {
                if e.document.status == DocStatus::Ready {
                    wake.wake();
                }
            }),
        )
        .unwrap();

        let t = Instant::now();
        import_paths(&db, &ingest, vec![papers], None).await.unwrap();
        loop {
            let docs = db.call(|c| repo::list_documents(c, None)).await.unwrap();
            if docs.iter().all(|d| !matches!(d.status, DocStatus::Queued | DocStatus::Processing)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        println!("all documents read after {:.1}s, keyword search can be used from here", t.elapsed().as_secs_f32());
        embedder.wake();
        let started = Instant::now();
        let mut last = 0;
        loop {
            let st = embedder.status();
            assert!(!matches!(st.model, ModelState::Failed { .. }), "{:?}", st.model);
            if st.indexed != last {
                last = st.indexed;
                if last % 200 < 8 {
                    println!("  indexed {} of {} ({:.0}s)", st.indexed, st.total, started.elapsed().as_secs_f32());
                }
            }
            if st.total > 0 && st.indexed == st.total {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let secs = started.elapsed().as_secs_f32();
        let st = embedder.status();
        println!("embedded {} passages in {secs:.0}s ({:.1}/s)", st.total, st.total as f32 / secs);

        // keyword search worked before and after; a quick meaning query for the record
        let q = embedder.embed_query("why do some marine algae produce toxins").unwrap().unwrap();
        let hits = db
            .call(move |c| crate::services::retrieval::hybrid_search(c, "why do some marine algae produce toxins", &q, None, 5))
            .await
            .unwrap();
        for h in &hits {
            println!("  {:?} {} p{}", h.matched_by, h.document_title.chars().take(40).collect::<String>(), h.page_start);
        }
        db.call(|c| Ok(c.pragma_update(None, "wal_checkpoint", "TRUNCATE")?)).await.ok();
    }
}
