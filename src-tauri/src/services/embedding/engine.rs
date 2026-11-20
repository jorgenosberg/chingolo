//! The embedding engine: model state, the background worker that gives every ready passage a
//! vector, and query embedding for search. The worker has its own thread and database
//! connection, like the import worker, so neither blocks the other or chat.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use fastembed::TextEmbedding;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use specta::Type;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::cloud::GeminiEncoder;
use super::model::{self, FILES, ModelState, REPO_URL};
use super::{Backend, DOC_PREFIX, QUERY_PREFIX};
use crate::error::{AppError, Result};
use crate::persistence::usage::{self, NewUsage};
use crate::persistence::{Db, library as repo, vectors};
use crate::security;
use crate::services::ingestion::queue::{DocumentChanged, Emit};

/// Sent whenever the model state, the indexing counts or the pause flag change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingStatus {
    pub backend: Backend,
    /// For Gemini, `missing` means the user hasn't agreed to send passages yet.
    pub model: ModelState,
    /// Passages of ready documents that have a vector, and all such passages.
    pub indexed: u32,
    pub total: u32,
    pub paused: bool,
}

pub type StatusEmit = Arc<dyn Fn(EmbeddingStatus) + Send + Sync>;

/// Turns text into L2-normalized vectors. The real ones wrap the ONNX model or the Gemini API;
/// tests use a fake.
pub trait Encoder: Send {
    fn encode(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// What the last `encode` call used, for encoders that are paid by the token.
    fn last_usage(&self) -> Option<EncodeUsage> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeUsage {
    pub tokens: u32,
    /// Guessed from text length, because the response had no count.
    pub estimated: bool,
}

/// Builds the encoder for a backend. The folder holds the local model's files.
pub type Loader = Arc<dyn Fn(Backend, &Path) -> Result<Box<dyn Encoder>> + Send + Sync>;

struct Gemma(TextEmbedding);

impl Encoder for Gemma {
    fn encode(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let vecs = self
            .0
            .embed(texts, Some(Backend::Local.batch() as usize))
            .map_err(|e| AppError::Internal(format!("The search model failed: {e}")))?;
        Ok(vecs.into_iter().map(normalize).collect())
    }
}

pub(super) fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

fn real_loader() -> Loader {
    Arc::new(|backend, dir| match backend {
        Backend::Local => Ok(Box::new(Gemma(super::load_model(dir)?)) as Box<dyn Encoder>),
        Backend::Gemini => Ok(Box::new(GeminiEncoder::new(Arc::new(|| security::get_key_blocking("gemini"))))),
    })
}

/// Where a backend starts: Gemini once the user has agreed, local once its files are there.
fn initial_state(backend: Backend, cloud_accepted: bool, local_ready: bool) -> ModelState {
    let ready = match backend {
        Backend::Gemini => cloud_accepted,
        Backend::Local => local_ready,
    };
    if ready { ModelState::Ready } else { ModelState::Missing }
}

const QUERY_CACHE_SIZE: usize = 64;

struct Inner {
    dir: PathBuf,
    backend: Mutex<Backend>,
    state: Mutex<ModelState>,
    counts: Mutex<(u32, u32)>,
    paused: AtomicBool,
    encoder: Mutex<Option<Box<dyn Encoder>>>,
    /// Searches waiting for the model. The worker steps aside for them between batches.
    waiting: AtomicUsize,
    /// Recent query vectors, so a repeated search costs no call. Oldest first.
    query_cache: Mutex<Vec<(Backend, String, Vec<f32>)>>,
    wake: mpsc::UnboundedSender<()>,
    emit_status: StatusEmit,
    emit_doc: Emit,
    load: Loader,
    /// Writes usage rows. Separate from the worker's connection because searches record too.
    usage_db: Mutex<Connection>,
}

#[derive(Clone)]
pub struct Embedder(Arc<Inner>);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Embedder {
    /// `app_data` is the app's data folder. `emit_doc` must not be the one that wakes this
    /// engine, or every batch would wake itself.
    pub fn start(
        db_path: &Path,
        app_data: &Path,
        backend: Backend,
        cloud_accepted: bool,
        emit_status: StatusEmit,
        emit_doc: Emit,
    ) -> Result<Self> {
        let dir = model::model_dir(app_data);
        let ready = initial_state(backend, cloud_accepted, model::is_complete(&dir, FILES)) == ModelState::Ready;
        Self::start_with(db_path, dir, backend, ready, real_loader(), emit_status, emit_doc)
    }

    pub fn start_with(
        db_path: &Path,
        dir: PathBuf,
        backend: Backend,
        ready: bool,
        load: Loader,
        emit_status: StatusEmit,
        emit_doc: Emit,
    ) -> Result<Self> {
        let conn = Db::open_secondary(db_path)?;
        let (wake, rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            dir,
            backend: Mutex::new(backend),
            state: Mutex::new(if ready { ModelState::Ready } else { ModelState::Missing }),
            counts: Mutex::new((0, 0)),
            paused: AtomicBool::new(false),
            encoder: Mutex::new(None),
            waiting: AtomicUsize::new(0),
            query_cache: Mutex::new(Vec::new()),
            wake,
            emit_status,
            emit_doc,
            load,
            usage_db: Mutex::new(Db::open_secondary(db_path)?),
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

    pub fn backend(&self) -> Backend {
        *lock(&self.0.backend)
    }

    /// Switches the model that makes vectors. Vectors from the old one are dropped by the worker
    /// and everything is embedded again. `cloud_accepted` only matters for Gemini.
    pub fn use_backend(&self, backend: Backend, cloud_accepted: bool) {
        log::info!("embedding backend set to {backend:?} (cloud accepted: {cloud_accepted})");
        *lock(&self.0.backend) = backend;
        *lock(&self.0.encoder) = None;
        lock(&self.0.query_cache).clear();
        *lock(&self.0.counts) = (0, 0);
        let local_ready = model::is_complete(&self.0.dir, FILES);
        *lock(&self.0.state) = initial_state(backend, cloud_accepted, local_ready);
        self.publish();
        self.wake();
    }

    pub fn status(&self) -> EmbeddingStatus {
        let (indexed, total) = *lock(&self.0.counts);
        EmbeddingStatus {
            backend: self.backend(),
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

    /// Makes sure the model is usable. For Gemini that is the user's yes (or a retry after an
    /// error). For the local model it starts the download if files are missing (and resumes a
    /// partial one), or retries loading when they are all there. Returns at once.
    pub fn ensure_model(&self) {
        if self.backend() == Backend::Gemini {
            self.set_state(ModelState::Ready);
            self.wake();
            return;
        }
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
            *slot = Some((self.0.load)(self.backend(), &self.0.dir)?);
        }
        f(slot.as_mut().expect("just loaded").as_mut())
    }

    /// The vector for a search query, or `None` while the model isn't ready (keyword search
    /// still works). Blocks while the model runs, so call it from a blocking thread.
    pub fn embed_query(&self, query: &str) -> Result<Option<Vec<f32>>> {
        if !matches!(*lock(&self.0.state), ModelState::Ready) {
            return Ok(None);
        }
        let backend = self.backend();
        // Case and spacing don't change what a search means, so they share one cached vector.
        let key = query.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        if let Some((_, _, v)) = lock(&self.0.query_cache).iter().find(|(b, k, _)| *b == backend && *k == key) {
            log::debug!("query vector from the cache");
            return Ok(Some(v.clone()));
        }
        self.0.waiting.fetch_add(1, Ordering::SeqCst);
        let text = vec![format!("{QUERY_PREFIX}{query}")];
        let started = Instant::now();
        let waited_for_model = self.0.waiting.load(Ordering::SeqCst);
        let out = self.encode_and_record("embed_query", &text, started);
        self.0.waiting.fetch_sub(1, Ordering::SeqCst);
        log::debug!(
            "query embedded in {} ms ({} searches waiting)",
            started.elapsed().as_millis(),
            waited_for_model
        );
        let vector = out?.into_iter().next();
        if let Some(v) = &vector {
            let mut cache = lock(&self.0.query_cache);
            if cache.len() >= QUERY_CACHE_SIZE {
                cache.remove(0);
            }
            cache.push((backend, key, v.clone()));
        }
        Ok(vector)
    }

    /// Runs the encoder and, for Gemini, records what the call used and how long it took.
    fn encode_and_record(&self, kind: &str, texts: &[String], started: Instant) -> Result<Vec<Vec<f32>>> {
        let backend = self.backend();
        let out = self.with_encoder(|e| {
            let r = e.encode(texts);
            let u = e.last_usage();
            r.map(|v| (v, u))
        });
        if backend == Backend::Gemini {
            let (tokens, estimated) = match &out {
                Ok((_, Some(u))) => (u.tokens, u.estimated),
                _ => (0, false),
            };
            let row = NewUsage {
                provider: "gemini",
                model: backend.identity().0,
                kind,
                input_tokens: tokens,
                output_tokens: 0,
                cached_tokens: 0,
                estimated,
                latency_ms: started.elapsed().as_millis() as u32,
                ok: out.is_ok(),
                conversation_id: None,
            };
            if let Err(e) = usage::record(&lock(&self.0.usage_db), &row) {
                log::warn!("couldn't record embedding usage: {e}");
            }
        }
        out.map(|(v, _)| v)
    }

    fn refresh_counts(&self, conn: &Connection) {
        let (model, revision) = self.backend().identity();
        if let Ok((indexed, total)) = vectors::totals(conn, model, revision) {
            *lock(&self.0.counts) = (indexed, total);
        }
    }

    fn fail(&self, e: AppError) {
        log::error!("embedding stopped: {e}");
        self.set_state(ModelState::Failed { message: e.to_string() });
    }

    /// Embeds pending passages until none are left, the model isn't ready or the user pauses.
    fn drain(&self, conn: &mut Connection) {
        // Vectors from another model or revision are no use: drop them so everything is re-embedded.
        let (model, revision) = self.backend().identity();
        let _ = vectors::purge_stale(conn, model, revision);
        if let Ok((indexed, total)) = vectors::totals(conn, model, revision) {
            if indexed < total && matches!(*lock(&self.0.state), ModelState::Ready) {
                log::info!("embedding {} passages with {model}", total - indexed);
            }
        }
        loop {
            let backend = self.backend();
            let (model, revision) = backend.identity();
            let ready = matches!(*lock(&self.0.state), ModelState::Ready);
            if !ready || self.0.paused.load(Ordering::SeqCst) {
                break;
            }
            let Ok(batch) = vectors::pending_batch(conn, model, revision, backend.batch()) else { break };
            if batch.is_empty() {
                break;
            }
            let texts: Vec<String> = batch.iter().map(|p| format!("{DOC_PREFIX}{}", p.text)).collect();
            let started = Instant::now();
            let vecs = match self.encode_and_record("embed_index", &texts, started) {
                Ok(v) if v.len() == batch.len() => v,
                Ok(_) => return self.fail(AppError::Internal("The search model returned the wrong number of vectors.".into())),
                Err(e) => return self.fail(e),
            };
            // The backend may have been switched while the batch was in flight. Those vectors
            // belong to the old model, so drop them and start over.
            if self.backend() != backend {
                continue;
            }
            let items: Vec<_> = batch.iter().map(|p| p.passage_id.clone()).zip(vecs).collect();
            if let Err(e) = vectors::store_vectors(conn, &items, model, revision) {
                return self.fail(e);
            }
            log::debug!(
                "embedded {} passages with {model} in {} ms",
                batch.len(),
                started.elapsed().as_millis()
            );
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
        let (indexed, total) = *lock(&self.0.counts);
        log::info!("embedding idle: {indexed} of {total} passages indexed");
    }
}

fn worker_loop(mut conn: Connection, mut rx: mpsc::UnboundedReceiver<()>, inner: Arc<Inner>) {
    let me = Embedder(inner);
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
    use crate::services::embedding::{MODEL_NAME, model::REVISION};

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
        rig_on(Backend::Local, ready, fail_after, load_fails)
    }

    fn rig_on(backend: Backend, ready: bool, fail_after: Option<usize>, load_fails: bool) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let _main = Db::open(&path).unwrap();
        let db = Db::open_secondary(&path).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let load: Loader = Arc::new(move |_, _| {
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
            backend,
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
        let pages = vec![NewPage { page_no: 1, width: 1.0, height: 1.0, quality: "ok", ocr: false }];
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
        // Paused while the documents are added, so the worker can't start on the first one early.
        r.embedder.set_paused(true);
        let a = add_doc(&mut r.db, "a", 11);
        let b = add_doc(&mut r.db, "b", 3);
        r.embedder.set_paused(false);
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
    fn a_repeated_query_is_embedded_once() {
        let r = rig(true, None, false);
        let first = r.embedder.embed_query("Coral  reefs").unwrap().unwrap();
        assert_eq!(r.calls.load(Ordering::SeqCst), 1);
        // Same words with other case and spacing hit the cache and make no call.
        assert_eq!(r.embedder.embed_query(" coral reefs ").unwrap().unwrap(), first);
        assert_eq!(r.calls.load(Ordering::SeqCst), 1);
        r.embedder.embed_query("kelp").unwrap();
        assert_eq!(r.calls.load(Ordering::SeqCst), 2);
        // A new backend must not reuse vectors from the old one.
        r.embedder.use_backend(Backend::Gemini, true);
        assert!(lock(&r.embedder.0.query_cache).is_empty());
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

    fn totals_for(r: &Rig, backend: Backend) -> (u32, u32) {
        let (model, revision) = backend.identity();
        vectors::totals(&r.db, model, revision).unwrap()
    }

    #[test]
    fn gemini_sends_nothing_until_the_user_agrees_then_embeds_in_larger_batches() {
        let mut r = rig_on(Backend::Gemini, false, None, false);
        add_doc(&mut r.db, "a", 40);
        r.embedder.wake();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(r.calls.load(Ordering::SeqCst), 0, "no request before consent");
        assert_eq!(r.embedder.embed_query("coral").unwrap(), None);

        r.embedder.ensure_model();
        wait_for("embedding", || totals_for(&r, Backend::Gemini) == (40, 40));
        assert_eq!(r.calls.load(Ordering::SeqCst), 2, "40 passages in batches of 32");
        assert_eq!(r.embedder.status().backend, Backend::Gemini);
    }

    #[test]
    fn switching_backend_drops_the_old_vectors_and_embeds_again() {
        let mut r = rig_on(Backend::Local, true, None, false);
        add_doc(&mut r.db, "a", 5);
        r.embedder.wake();
        wait_for("local embedding", || totals_for(&r, Backend::Local) == (5, 5));

        r.embedder.use_backend(Backend::Gemini, true);
        wait_for("gemini embedding", || totals_for(&r, Backend::Gemini) == (5, 5));
        let (model, revision) = Backend::Local.identity();
        let left: u32 = r
            .db
            .query_row("SELECT COUNT(*) FROM passage_vectors WHERE model = ?1 AND revision = ?2", [model, revision], |row| row.get(0))
            .unwrap();
        assert_eq!(left, 0, "vectors from the other model are gone");
    }

    #[test]
    fn switching_to_gemini_without_consent_keeps_it_waiting() {
        let mut r = rig_on(Backend::Local, true, None, false);
        add_doc(&mut r.db, "a", 3);
        r.embedder.use_backend(Backend::Gemini, false);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(r.embedder.status().model, ModelState::Missing);
        assert_eq!(totals_for(&r, Backend::Gemini).0, 0);
    }

    /// Imports a folder of papers through the real import worker, embeds them with the real
    /// model, and prints the timings. `CHINGOLO_KEEP_DB` keeps the database for
    /// `tools/retrieval-bench app`.
    /// Run: CHINGOLO_TEST_PAPERS=<folder> CHINGOLO_EMBED_MODEL_DIR=<model folder> \
    ///      CHINGOLO_KEEP_DB=<file> cargo test corpus_embed -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn corpus_embed() {
        use crate::services::ingestion::import::import_paths;
        use crate::services::ingestion::queue::Ingestor;
        use std::time::Instant;

        let papers = std::path::PathBuf::from(std::env::var("CHINGOLO_TEST_PAPERS").expect("set CHINGOLO_TEST_PAPERS"));
        let model_dir = std::path::PathBuf::from(std::env::var("CHINGOLO_EMBED_MODEL_DIR").expect("set CHINGOLO_EMBED_MODEL_DIR"));
        let tmp = tempfile::tempdir().unwrap();
        let db_path = match std::env::var("CHINGOLO_KEEP_DB") {
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
            Backend::Local,
            true,
            real_loader(),
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
            .call(move |c| crate::services::retrieval::hybrid_search(c, "why do some marine algae produce toxins", &q, Backend::Local.identity(), &crate::domain::library::DocFilter::All, 5))
            .await
            .unwrap();
        for h in &hits {
            println!("  {:?} {} p{}", h.matched_by, h.document_title.chars().take(40).collect::<String>(), h.page_start);
        }
        db.call(|c| Ok(c.pragma_update(None, "wal_checkpoint", "TRUNCATE")?)).await.ok();
    }
}
