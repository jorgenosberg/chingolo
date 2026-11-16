use tauri::State;

use crate::services::embedding::model::ModelState;
use crate::state::AppState;

#[tauri::command]
#[specta::specta]
pub fn ocr_status(state: State<'_, AppState>) -> ModelState {
    state.ocr.status()
}

/// Downloads the text recognition models (about 31 MB), if they aren't there yet. Documents read
/// afterwards use them for pages without text. Returns at once, poll `ocr_status` for progress.
#[tauri::command]
#[specta::specta]
pub fn download_ocr_model(state: State<'_, AppState>) -> ModelState {
    state.ocr.ensure();
    state.ocr.status()
}

#[tauri::command]
#[specta::specta]
pub fn layout_status(state: State<'_, AppState>) -> ModelState {
    state.layout.status()
}

/// Downloads the page layout model (about 130 MB), if it isn't there yet. When it is ready, documents
/// already in the library get their layout in the background. Returns at once, poll `layout_status`.
#[tauri::command]
#[specta::specta]
pub fn download_layout_model(state: State<'_, AppState>) -> ModelState {
    state.layout.ensure();
    let (model, ingest, db) = (state.layout.clone(), state.ingest.clone(), state.db.clone());
    tauri::async_runtime::spawn(async move {
        while matches!(model.status(), ModelState::Downloading { .. }) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        if matches!(model.status(), ModelState::Ready) {
            let _ = ingest.enqueue_layouts(&db).await;
        }
    });
    state.layout.status()
}
