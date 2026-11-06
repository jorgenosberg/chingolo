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
