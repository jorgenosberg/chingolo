//! One list of everything the app does in the background, for the sidebar indicator and the
//! Activity page. The database stays the source of truth where it already is (document status,
//! layout and table counts, lookup errors). The few live facts (the item being read, pause flags)
//! come from the workers. A snapshot combines both, and one task sends it when it changes.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;
use tokio::sync::Notify;

use crate::domain::Id;
use crate::domain::library::DocStatus;
use crate::error::{AppError, Result};
use crate::persistence::{Db, library as repo, settings};
use crate::services::embedding::Backend;
use crate::services::embedding::engine::Embedder;
use crate::services::embedding::model::ModelState;
use crate::services::ingestion::activity::Activity;
use crate::services::ingestion::enrich::Enricher;
use crate::services::ingestion::import;
use crate::services::ingestion::ocr::OcrModel;
use crate::services::ingestion::queue::Ingestor;

/// How many failed documents a lane lists.
const FAILED_LIMIT: u32 = 50;
/// While work runs the snapshot is rebuilt this often, so counts and download bytes move.
const TICK: Duration = Duration::from_secs(1);
/// With nothing running, a change is picked up through `poke`. This is only a safety net.
const IDLE_TICK: Duration = Duration::from_secs(10);
/// Changes within this window are sent as one snapshot.
const DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Lane {
    /// Reading files into passages (with text recognition for scans).
    Read,
    /// Page layout, tables and the second reading of table cells.
    Layout,
    /// Title, authors and year from the file, and Crossref or model lookups.
    Details,
    /// Embedding passages for meaning search.
    Index,
    DownloadOcr,
    DownloadLayout,
    DownloadTables,
    DownloadEmbedding,
}

impl Lane {
    /// Downloads are stopped and started again instead.
    pub fn pausable(self) -> bool {
        matches!(
            self,
            Lane::Read | Lane::Layout | Lane::Details | Lane::Index
        )
    }
}

/// The lanes the user paused, as a JSON array. Survives restarts.
pub const PAUSED_KEY: &str = "paused_lanes";

fn paused_lanes(conn: &rusqlite::Connection) -> Result<Vec<Lane>> {
    Ok(settings::get(conn, PAUSED_KEY)?
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum LaneState {
    Idle,
    Waiting,
    Running,
    Paused,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Unit {
    Pages,
    Passages,
    Tables,
    Bytes,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Count {
    pub done: u32,
    pub total: u32,
    pub unit: Unit,
}

/// What a lane is working on right now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub document_id: Option<Id>,
    pub title: String,
    pub progress: Option<Count>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FailedItem {
    pub document_id: Option<Id>,
    pub title: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LaneSnapshot {
    pub lane: Lane,
    pub state: LaneState,
    /// The user paused this lane. A job already running still finishes, so a lane can be
    /// running and paused at once.
    pub paused: bool,
    /// Overall progress, when the lane can say (pages, passages or bytes).
    pub count: Option<Count>,
    /// Table regions read and in all, for the layout lane.
    pub tables: Option<Count>,
    pub current: Option<Item>,
    /// Documents or items queued behind the current one.
    pub waiting: u32,
    pub failed: Vec<FailedItem>,
}

impl LaneSnapshot {
    fn idle(lane: Lane) -> Self {
        LaneSnapshot {
            lane,
            state: LaneState::Idle,
            paused: false,
            count: None,
            tables: None,
            current: None,
            waiting: 0,
            failed: Vec::new(),
        }
    }

    /// True while something is queued or running, which is when counts move.
    pub fn busy(&self) -> bool {
        matches!(self.state, LaneState::Waiting | LaneState::Running)
    }
}

/// Sent when any lane changes. Always holds every lane, in the order of `Lane`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct JobsChanged {
    pub lanes: Vec<LaneSnapshot>,
}

/// What the snapshot reads from.
#[derive(Clone)]
pub struct Sources {
    pub db: Db,
    pub ingest: Ingestor,
    pub embedder: Embedder,
    pub ocr: OcrModel,
    pub layout: OcrModel,
    pub tables: OcrModel,
    pub activity: Activity,
    pub enricher: Enricher,
}

#[derive(Clone)]
pub struct Jobs {
    src: Sources,
    notify: Arc<Notify>,
}

impl Jobs {
    pub fn new(src: Sources, notify: Arc<Notify>) -> Self {
        Jobs { src, notify }
    }

    /// Tells the sender that something changed. Cheap, safe to call from any thread.
    pub fn poke(&self) {
        self.notify.notify_one();
    }

    pub async fn snapshot(&self) -> Result<Vec<LaneSnapshot>> {
        snapshot(&self.src).await
    }

    /// Applies the pauses saved by an earlier run. Call once at start.
    pub async fn load_paused(&self) {
        let saved = self
            .src
            .db
            .call(|c| paused_lanes(c))
            .await
            .unwrap_or_default();
        for lane in saved {
            self.apply_pause(lane, true);
        }
        self.poke();
    }

    /// Pauses or resumes a lane, and remembers it. A job already running finishes first.
    pub async fn set_paused(&self, lane: Lane, paused: bool) -> Result<()> {
        if !lane.pausable() {
            return Err(AppError::Invalid(
                "This kind of work can't be paused.".into(),
            ));
        }
        self.src
            .db
            .call(move |c| {
                let mut lanes = paused_lanes(c)?;
                lanes.retain(|l| *l != lane);
                if paused {
                    lanes.push(lane);
                }
                let json =
                    serde_json::to_string(&lanes).map_err(|e| AppError::Internal(e.to_string()))?;
                settings::set(c, PAUSED_KEY, &json)
            })
            .await?;
        self.apply_pause(lane, paused);
        self.poke();
        Ok(())
    }

    fn apply_pause(&self, lane: Lane, paused: bool) {
        let s = &self.src;
        match lane {
            Lane::Read | Lane::Layout => s.ingest.set_paused(lane, paused),
            Lane::Details => {
                s.ingest.set_paused(lane, paused);
                s.enricher.set_paused(paused);
            }
            Lane::Index => s.embedder.set_paused(paused),
            _ => {}
        }
    }

    /// Runs a failed item again. `document_id` is needed for the document lanes.
    pub async fn retry(&self, lane: Lane, document_id: Option<String>) -> Result<()> {
        let s = &self.src;
        let need = || document_id.clone().ok_or(AppError::NotFound);
        match lane {
            Lane::Read => {
                import::retry_import(&s.db, &s.ingest, &need()?).await?;
            }
            Lane::Layout => {
                let id = need()?;
                let doc =
                    s.db.call(move |c| {
                        repo::reset_layout_failure(c, &id)?;
                        repo::get_document(c, &id)
                    })
                    .await?;
                s.ingest.enqueue_layout(&doc);
            }
            Lane::Details => s.enricher.queue(&need()?),
            Lane::Index => s.embedder.ensure_model(),
            _ => {}
        }
        self.poke();
        Ok(())
    }

    /// Stops a queued or running document. Layout is skipped for that document until a retry.
    pub async fn cancel(&self, lane: Lane, document_id: &str) -> Result<bool> {
        let s = &self.src;
        let stopped = match lane {
            Lane::Read => s.ingest.cancel(document_id),
            Lane::Layout => {
                let id = document_id.to_string();
                s.db.call(move |c| repo::skip_layout(c, &id)).await?;
                s.ingest.cancel_lane(Lane::Layout, document_id);
                true
            }
            _ => false,
        };
        self.poke();
        Ok(stopped)
    }

    /// Sends a snapshot whenever it differs from the last one, at most every `DEBOUNCE`.
    pub fn run(&self, emit: impl Fn(JobsChanged) + Send + Sync + 'static) {
        let jobs = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut last: Option<Vec<LaneSnapshot>> = None;
            let mut busy = false;
            loop {
                match jobs.snapshot().await {
                    Ok(lanes) => {
                        busy = lanes.iter().any(LaneSnapshot::busy);
                        if last.as_ref() != Some(&lanes) {
                            emit(JobsChanged {
                                lanes: lanes.clone(),
                            });
                            last = Some(lanes);
                        }
                    }
                    Err(e) => log::warn!("couldn't build the job list: {e}"),
                }
                let wait = if busy { TICK } else { IDLE_TICK };
                let _ = tokio::time::timeout(wait, jobs.notify.notified()).await;
                tokio::time::sleep(DEBOUNCE).await;
            }
        });
    }
}

/// What the database knows, read in one call.
struct Stored {
    queued: u32,
    processing: u32,
    failed_reads: Vec<(Id, String, String)>,
    failed_lookups: Vec<(Id, String, String)>,
    failed_layouts: Vec<(Id, String, String)>,
}

async fn stored(db: &Db) -> Result<Stored> {
    db.call(|c| {
        Ok(Stored {
            queued: repo::count_with_status(c, DocStatus::Queued)?,
            processing: repo::count_with_status(c, DocStatus::Processing)?,
            failed_reads: repo::failed_reads(c, FAILED_LIMIT)?,
            failed_lookups: repo::failed_lookups(c, FAILED_LIMIT)?,
            failed_layouts: repo::failed_layouts(c, FAILED_LIMIT)?,
        })
    })
    .await
}

fn failed(rows: Vec<(Id, String, String)>) -> Vec<FailedItem> {
    rows.into_iter()
        .map(|(id, title, error)| FailedItem {
            document_id: Some(id),
            title,
            error,
        })
        .collect()
}

/// Model files are well under 4 GB, so a `u32` holds their byte counts.
fn bytes(done: f64, total: f64) -> Count {
    Count {
        done: done as u32,
        total: total as u32,
        unit: Unit::Bytes,
    }
}

/// A download lane from a model's state. Only a download in progress or a failed one shows.
fn download(lane: Lane, state: ModelState) -> LaneSnapshot {
    let mut out = LaneSnapshot::idle(lane);
    match state {
        ModelState::Downloading { done, total } => {
            out.state = LaneState::Running;
            out.count = Some(bytes(done, total));
        }
        ModelState::Failed { message } => {
            out.state = LaneState::Failed;
            out.failed = vec![FailedItem {
                document_id: None,
                title: String::new(),
                error: message,
            }];
        }
        ModelState::Missing | ModelState::Ready => {}
    }
    out
}

pub async fn snapshot(src: &Sources) -> Result<Vec<LaneSnapshot>> {
    let stored = stored(&src.db).await?;
    let running = src.ingest.running();
    let layout = src.ingest.layout_progress(&src.db).await?;
    let embedding = src.embedder.status();

    // Reading
    let mut read = LaneSnapshot::idle(Lane::Read);
    let reading = running.as_ref().filter(|r| r.lane == Lane::Read);
    read.current = reading.map(|r| Item {
        document_id: Some(r.id.clone()),
        title: r.title.clone(),
        progress: r.progress.as_ref().map(|p| Count {
            done: p.done,
            total: p.total,
            unit: Unit::Pages,
        }),
    });
    read.waiting = stored.queued;
    read.failed = failed(stored.failed_reads);
    read.paused = src.ingest.is_paused(Lane::Read);
    read.state = if reading.is_some() || stored.processing > 0 {
        LaneState::Running
    } else if stored.queued > 0 {
        if read.paused {
            LaneState::Paused
        } else {
            LaneState::Waiting
        }
    } else if !read.failed.is_empty() {
        LaneState::Failed
    } else {
        LaneState::Idle
    };

    // Layout and tables
    let mut lay = LaneSnapshot::idle(Lane::Layout);
    lay.paused = src.ingest.is_paused(Lane::Layout);
    if layout.available {
        let working = running.as_ref().filter(|r| r.lane == Lane::Layout);
        lay.count = Some(Count {
            done: layout.done,
            total: layout.total,
            unit: Unit::Pages,
        });
        if layout.tables_total > 0 {
            lay.tables = Some(Count {
                done: layout.tables_done,
                total: layout.tables_total,
                unit: Unit::Tables,
            });
        }
        lay.waiting = layout.docs_left;
        lay.current = working.map(|r| Item {
            document_id: Some(r.id.clone()),
            title: r.title.clone(),
            progress: None,
        });
        let unfinished = layout.docs_left > 0 || layout.tables_done < layout.tables_total;
        lay.state = if working.is_some() {
            LaneState::Running
        } else if unfinished && lay.paused {
            LaneState::Paused
        } else if unfinished {
            LaneState::Waiting
        } else {
            LaneState::Idle
        };
    }
    lay.failed = failed(stored.failed_layouts);
    if lay.state == LaneState::Idle && !lay.failed.is_empty() {
        lay.state = LaneState::Failed;
    }

    // Details
    let mut details = LaneSnapshot::idle(Lane::Details);
    let ids = src.activity.ids();
    details.waiting = u32::try_from(ids.len()).unwrap_or(u32::MAX);
    details.failed = failed(stored.failed_lookups);
    details.paused = src.ingest.is_paused(Lane::Details);
    let details_running = running.as_ref().is_some_and(|r| r.lane == Lane::Details);
    details.state = if details_running {
        LaneState::Running
    } else if !ids.is_empty() && details.paused {
        LaneState::Paused
    } else if !ids.is_empty() {
        LaneState::Running
    } else if !details.failed.is_empty() {
        LaneState::Failed
    } else {
        LaneState::Idle
    };

    // Indexing. The consent and backend choices are settings and stay on `EmbeddingStatus`.
    let mut index = LaneSnapshot::idle(Lane::Index);
    index.paused = embedding.paused;
    match &embedding.model {
        ModelState::Ready => {
            if embedding.total > 0 {
                index.count = Some(Count {
                    done: embedding.indexed,
                    total: embedding.total,
                    unit: Unit::Passages,
                });
            }
            let behind = embedding.indexed < embedding.total;
            index.waiting = embedding.total.saturating_sub(embedding.indexed);
            index.state = match (behind, embedding.paused) {
                (true, true) => LaneState::Paused,
                (true, false) => LaneState::Running,
                (false, _) => LaneState::Idle,
            };
        }
        ModelState::Failed { message } => {
            index.state = LaneState::Failed;
            index.failed = vec![FailedItem {
                document_id: None,
                title: String::new(),
                error: message.clone(),
            }];
        }
        ModelState::Missing | ModelState::Downloading { .. } => {}
    }

    // The local search model is the only embedding download.
    let embedding_download = match (&embedding.model, embedding.backend) {
        (m @ ModelState::Downloading { .. }, Backend::Local) => m.clone(),
        _ => ModelState::Ready,
    };

    Ok(vec![
        read,
        lay,
        details,
        index,
        download(Lane::DownloadOcr, src.ocr.status()),
        download(Lane::DownloadLayout, src.layout.status()),
        download(Lane::DownloadTables, src.tables.status()),
        download(Lane::DownloadEmbedding, embedding_download),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_shows_only_while_it_runs_or_after_it_failed() {
        let running = download(
            Lane::DownloadLayout,
            ModelState::Downloading {
                done: 10.0,
                total: 40.0,
            },
        );
        assert_eq!(running.state, LaneState::Running);
        assert_eq!(running.count.as_ref().map(|c| c.total), Some(40));

        let failed = download(
            Lane::DownloadOcr,
            ModelState::Failed {
                message: "offline".into(),
            },
        );
        assert_eq!(failed.state, LaneState::Failed);
        assert_eq!(failed.failed[0].error, "offline");

        for s in [ModelState::Missing, ModelState::Ready] {
            assert_eq!(download(Lane::DownloadTables, s).state, LaneState::Idle);
        }
    }

    use crate::error::AppError;
    use crate::services::embedding::engine::Loader;
    use crate::services::ingestion::import::import_paths;
    use crate::services::ingestion::{activity::Activity, layout, tables};
    use std::sync::Mutex;

    /// Real ingest worker, embedder and database, with a fake encoder.
    fn rig(dir: &std::path::Path) -> (Db, Ingestor, Jobs) {
        let db_path = dir.join("t.sqlite");
        let db = Db::open(&db_path).unwrap();
        let poke = Arc::new(Notify::new());
        let ingest = Ingestor::start(&db_path, dir.join("library"), Arc::new(|_| {})).unwrap();
        ingest.set_poke(poke.clone());
        let loader: Loader = Arc::new(|_, _| Err(AppError::Internal("no model in tests".into())));
        let embedder = Embedder::start_with(
            &db_path,
            dir.join("embedding"),
            Backend::Local,
            false,
            loader,
            Arc::new(|_| {}),
            Arc::new(|_| {}),
        )
        .unwrap();
        let activity = Activity::new(|_| {});
        let enricher = Enricher::start(
            db.clone(),
            Arc::new(|_| {}),
            crate::services::ai::Providers::new(),
            activity.clone(),
        );
        let jobs = Jobs::new(
            Sources {
                db: db.clone(),
                ingest: ingest.clone(),
                embedder,
                ocr: OcrModel::new(dir),
                layout: layout::model(dir),
                tables: tables::model(dir),
                activity,
                enricher,
            },
            poke,
        );
        (db, ingest, jobs)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_read_reaches_the_pushed_job_list() {
        let dir = tempfile::tempdir().unwrap();
        let (db, ingest, jobs) = rig(dir.path());
        let seen = Arc::new(Mutex::new(Vec::<JobsChanged>::new()));
        let sink = seen.clone();
        jobs.run(move |e| sink.lock().unwrap().push(e));

        let file = dir.path().join("broken.pdf");
        std::fs::write(&file, b"%PDF-1.4 not really").unwrap();
        import_paths(&db, &ingest, vec![file], None).await.unwrap();

        let failed = async {
            loop {
                let found = seen.lock().unwrap().iter().find_map(|e| {
                    let read = &e.lanes[0];
                    (read.state == LaneState::Failed).then(|| read.clone())
                });
                if let Some(read) = found {
                    return read;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        let read = tokio::time::timeout(Duration::from_secs(10), failed)
            .await
            .expect("a failed read was never pushed");
        assert_eq!(read.lane, Lane::Read);
        assert_eq!(read.failed.len(), 1);
        assert_eq!(read.failed[0].title, "broken.pdf");
        assert_eq!(read.waiting, 0);

        let first = seen.lock().unwrap()[0].clone();
        assert_eq!(first.lanes.len(), 8);
        let snap = jobs.snapshot().await.unwrap();
        for lane in &snap[1..] {
            assert_eq!(lane.state, LaneState::Idle, "{:?}", lane.lane);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_pause_is_remembered_and_applied_again_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let (db, ingest, jobs) = rig(dir.path());
        jobs.set_paused(Lane::Layout, true).await.unwrap();
        jobs.set_paused(Lane::Index, true).await.unwrap();
        jobs.set_paused(Lane::Layout, true).await.unwrap();
        assert!(ingest.is_paused(Lane::Layout));
        assert!(!ingest.is_paused(Lane::Read));
        let snap = jobs.snapshot().await.unwrap();
        assert!(snap[1].paused && snap[3].paused);
        assert!(!snap[0].paused && !snap[2].paused);
        assert!(
            jobs.set_paused(Lane::DownloadOcr, true).await.is_err(),
            "downloads are stopped, not paused"
        );

        let saved = db.call(|c| paused_lanes(c)).await.unwrap();
        assert_eq!(saved, [Lane::Index, Lane::Layout]);

        // a second run on the same database
        drop((ingest, jobs));
        let (_db, ingest, jobs) = rig(dir.path());
        assert!(!ingest.is_paused(Lane::Layout));
        jobs.load_paused().await;
        assert!(ingest.is_paused(Lane::Layout));
        assert!(jobs.snapshot().await.unwrap()[3].paused);

        jobs.set_paused(Lane::Layout, false).await.unwrap();
        assert!(!ingest.is_paused(Lane::Layout));
        let saved = db.call(|c| paused_lanes(c)).await.unwrap();
        assert_eq!(saved, [Lane::Index]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_skipped_layout_waits_for_a_retry_and_a_retry_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        let (db, _ingest, jobs) = rig(dir.path());
        // paused, so the retry below is only queued and the worker leaves it alone
        jobs.set_paused(Lane::Layout, true).await.unwrap();
        let doc = db
            .call(|c| {
                let d = repo::insert_document(
                    c,
                    "h1",
                    "paper.pdf",
                    crate::domain::library::DocKind::Pdf,
                )?;
                repo::set_status(c, &d.id, DocStatus::Ready, None)
            })
            .await
            .unwrap();

        assert!(jobs.cancel(Lane::Layout, &doc.id).await.unwrap());
        let failed = db.call(|c| repo::failed_layouts(c, 10)).await.unwrap();
        assert_eq!(failed[0].0, doc.id);
        assert_eq!(failed[0].2, "Skipped");

        jobs.retry(Lane::Layout, Some(doc.id.clone()))
            .await
            .unwrap();
        let failed = db.call(|c| repo::failed_layouts(c, 10)).await.unwrap();
        assert!(failed.is_empty());
        assert!(jobs.retry(Lane::Layout, None).await.is_err());
    }
}
