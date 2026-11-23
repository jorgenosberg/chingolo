use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::domain::library::{
    DocFilter, DocStatus, Document, MetadataPatch, PageBoxes, PageRegion, PassageView, SearchHit,
    SourceView,
};
use crate::error::{AppError, CmdResult};
use crate::persistence::library::{self as repo, SearchOptions};
use crate::services::ingestion::import::{self, ImportSummary};
use crate::services::retrieval;
use crate::state::AppState;

/// True while files are dragged over the window.
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct DropHover {
    pub active: bool,
}

/// Opens the system file or folder dialog and imports what the user picks. Paths never pass
/// through the webview.
#[tauri::command]
#[specta::specta]
pub async fn pick_and_import(
    app: AppHandle,
    state: State<'_, AppState>,
    folder: bool,
    project_id: Option<String>,
) -> CmdResult<ImportSummary> {
    let dialog = app.dialog().file();
    let picked: Vec<PathBuf> = if folder {
        dialog
            .blocking_pick_folder()
            .into_iter()
            .filter_map(|p| p.into_path().ok())
            .collect()
    } else {
        dialog
            .add_filter("Papers (PDF, Word)", &["pdf", "docx"])
            .blocking_pick_files()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| p.into_path().ok())
            .collect()
    };
    if picked.is_empty() {
        return Ok(ImportSummary::default());
    }
    Ok(import::import_paths(&state.db, &state.ingest, picked, project_id).await?)
}

/// Where dropped files go. The window remembers it, because a drop has no other way to say.
#[tauri::command]
#[specta::specta]
pub fn set_import_target(state: State<'_, AppState>, project_id: Option<String>) -> CmdResult<()> {
    *state
        .import_target
        .lock()
        .map_err(|_| AppError::Internal("lock poisoned".into()))? = project_id;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn list_documents(
    state: State<'_, AppState>,
    project_id: Option<String>,
) -> CmdResult<Vec<Document>> {
    Ok(state
        .db
        .call(move |c| repo::list_documents(c, project_id.as_deref()))
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn update_document_metadata(
    state: State<'_, AppState>,
    id: String,
    patch: MetadataPatch,
) -> CmdResult<Document> {
    Ok(state
        .db
        .call(move |c| repo::update_metadata(c, &id, &patch))
        .await?)
}

/// Removes the document, its index entries and the stored copy. The original file is untouched.
#[tauri::command]
#[specta::specta]
pub async fn delete_document(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    import::delete_document(&state.db, &state.ingest, &id).await?;
    // The indexing totals changed.
    state.embedder.wake();
    Ok(())
}

/// True if the document was queued or being read.
#[tauri::command]
#[specta::specta]
pub fn cancel_import(state: State<'_, AppState>, id: String) -> bool {
    state.ingest.cancel(&id)
}

/// Reads a failed or cancelled document again.
#[tauri::command]
#[specta::specta]
pub async fn retry_import(state: State<'_, AppState>, id: String) -> CmdResult<Document> {
    Ok(import::retry_import(&state.db, &state.ingest, &id).await?)
}

/// Looks at the file properties and first pages again and refreshes the details. Fields you
/// edited are kept. Nothing is indexed again.
#[tauri::command]
#[specta::specta]
pub async fn refresh_details(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let doc = state.db.call(move |c| repo::get_document(c, &id)).await?;
    log::info!("refreshing the details of {}", doc.original_name);
    state.ingest.enqueue_details(&doc);
    Ok(())
}

/// The same for every document that is ready or has no text. Returns how many were queued.
#[tauri::command]
#[specta::specta]
pub async fn refresh_all_details(state: State<'_, AppState>) -> CmdResult<u32> {
    let docs = state.db.call(|c| repo::list_documents(c, None)).await?;
    let docs: Vec<_> = docs
        .into_iter()
        .filter(|d| matches!(d.status, DocStatus::Ready | DocStatus::NoText))
        .collect();
    for d in &docs {
        state.ingest.enqueue_details(d);
    }
    log::info!("refreshing the details of {} documents", docs.len());
    Ok(docs.len() as u32)
}

#[tauri::command]
#[specta::specta]
pub async fn attach_document(
    state: State<'_, AppState>,
    project_id: String,
    document_id: String,
) -> CmdResult<()> {
    Ok(state
        .db
        .call(move |c| repo::attach(c, &project_id, &document_id))
        .await?)
}

/// Takes the document out of a project. It stays in the library.
#[tauri::command]
#[specta::specta]
pub async fn detach_document(
    state: State<'_, AppState>,
    project_id: String,
    document_id: String,
) -> CmdResult<()> {
    Ok(state
        .db
        .call(move |c| repo::detach(c, &project_id, &document_id))
        .await?)
}

/// Search over the text of documents that are ready. With `meaning` set, keywords and meaning are
/// merged once the search model is ready (this can cost one embedding call, which is cached for
/// repeated queries). Without it, or for a very short query, it is keyword search only.
#[tauri::command]
#[specta::specta]
pub async fn search_library(
    state: State<'_, AppState>,
    query: String,
    project_id: Option<String>,
    limit: Option<u32>,
    meaning: bool,
) -> CmdResult<Vec<SearchHit>> {
    let limit = limit.unwrap_or(30).clamp(1, 100);
    let trimmed = query.trim().to_string();
    let started = std::time::Instant::now();
    let identity = state.embedder.backend().identity();
    // Meaning search can cost an API call, so the caller asks for it only once typing has paused.
    let vector = if meaning && trimmed.chars().count() >= 3 {
        let embedder = state.embedder.clone();
        let text = trimmed.clone();
        match tauri::async_runtime::spawn_blocking(move || embedder.embed_query(&text)).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                log::warn!("query embedding failed, using keyword search: {e}");
                None
            }
            Err(e) => {
                log::warn!("query embedding task failed, using keyword search: {e}");
                None
            }
        }
    } else {
        None
    };
    let embed_ms = started.elapsed().as_millis();
    let by_meaning = vector.is_some();
    let filter = DocFilter::from_project(project_id.as_deref());
    let hits = state
        .db
        .call(move |c| match vector {
            Some(v) => retrieval::hybrid_search(c, &trimmed, &v, identity, &filter, limit),
            None => {
                let opts = SearchOptions {
                    filter: &filter,
                    all_terms: true,
                    prefix_last: true,
                    limit,
                };
                repo::search_passages(c, &query, &opts)
            }
        })
        .await?;
    log::info!(
        "library search ({}): {} hits, embedding {} ms, total {} ms",
        if by_meaning {
            "keyword + meaning"
        } else {
            "keyword only"
        },
        hits.len(),
        embed_ms,
        started.elapsed().as_millis()
    );
    Ok(hits)
}

/// The saved copy of a cited passage, for when its document was deleted or read again.
#[tauri::command]
#[specta::specta]
pub async fn get_source(
    state: State<'_, AppState>,
    conversation_id: String,
    handle: u32,
) -> CmdResult<SourceView> {
    let s = state
        .db
        .call(move |c| crate::persistence::sources::get(c, &conversation_id, handle))
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(SourceView {
        handle: s.handle,
        title: s.title,
        page: s.page,
        text: s.text,
    })
}

/// The layout regions stored for a document, for the developer view. Empty before layout has run.
#[tauri::command]
#[specta::specta]
pub async fn get_layout(
    state: State<'_, AppState>,
    document_id: String,
) -> CmdResult<Vec<PageRegion>> {
    Ok(state
        .db
        .call(move |c| repo::list_regions(c, &document_id))
        .await?)
}

/// A passage with its line boxes, for opening it in the reader.
#[tauri::command]
#[specta::specta]
pub async fn get_passage(state: State<'_, AppState>, passage_id: String) -> CmdResult<PassageView> {
    let p = state
        .db
        .call(move |c| repo::get_passage(c, &passage_id))
        .await?;
    let boxes = p
        .boxes
        .and_then(|b| serde_json::from_str::<Vec<PageBoxes>>(&b).ok())
        .unwrap_or_default();
    Ok(PassageView {
        id: p.id,
        document_id: p.document_id,
        page_start: p.page_start,
        page_end: p.page_end,
        text: p.text,
        boxes,
    })
}
