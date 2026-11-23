use tauri::State;

use crate::services::embedding::model::ModelState;
use crate::state::AppState;

#[tauri::command]
#[specta::specta]
pub fn ocr_status(state: State<'_, AppState>) -> ModelState {
    state.ocr.status()
}

/// Downloads the text recognition models (about 31 MB), if they aren't there yet. Documents read
/// afterwards use them for pages without text, and scans already in the library are read again once
/// it is ready. Returns at once. Progress is on the job list.
#[tauri::command]
#[specta::specta]
pub fn download_ocr_model(state: State<'_, AppState>) -> ModelState {
    state.ocr.ensure();
    state.jobs.poke();
    state.ocr.status()
}

#[tauri::command]
#[specta::specta]
pub fn layout_status(state: State<'_, AppState>) -> ModelState {
    state.layout.status()
}

/// Downloads the page layout model (about 130 MB), if it isn't there yet. When it is ready, documents
/// already in the library get their layout in the background. Returns at once. Progress is on the job list.
#[tauri::command]
#[specta::specta]
pub fn download_layout_model(state: State<'_, AppState>) -> ModelState {
    state.layout.ensure();
    state.jobs.poke();
    state.layout.status()
}

#[tauri::command]
#[specta::specta]
pub fn tables_status(state: State<'_, AppState>) -> ModelState {
    state.tables.status()
}

/// Downloads the table structure model (about 8 MB), if it isn't there yet. When it is ready, tables in
/// documents that already have layout are read in the background. Progress is on the job list.
#[tauri::command]
#[specta::specta]
pub fn download_tables_model(state: State<'_, AppState>) -> ModelState {
    state.tables.ensure();
    state.jobs.poke();
    state.tables.status()
}

/// How far page layout has got over the library. Poll while `done < total`.
#[tauri::command]
#[specta::specta]
pub async fn layout_progress(
    state: State<'_, AppState>,
) -> crate::error::CmdResult<crate::services::ingestion::queue::LayoutProgress> {
    Ok(state.ingest.layout_progress(&state.db).await?)
}
