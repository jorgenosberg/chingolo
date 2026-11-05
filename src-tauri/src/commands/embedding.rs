use tauri::State;

use crate::error::CmdResult;
use crate::persistence::settings;
use crate::services::embedding::engine::EmbeddingStatus;
use crate::state::AppState;

/// The user's yes to the one-time model download. Kept so an interrupted download resumes at startup.
pub const DOWNLOAD_ACCEPTED: &str = "embedding_download_accepted";

#[tauri::command]
#[specta::specta]
pub fn embedding_status(state: State<'_, AppState>) -> EmbeddingStatus {
    state.embedder.status()
}

/// Starts (or resumes) the model download, or retries loading the model if the files are there.
#[tauri::command]
#[specta::specta]
pub async fn download_embedding_model(state: State<'_, AppState>) -> CmdResult<EmbeddingStatus> {
    state.db.call(|c| settings::set_bool(c, DOWNLOAD_ACCEPTED, true)).await?;
    state.embedder.ensure_model();
    Ok(state.embedder.status())
}

#[tauri::command]
#[specta::specta]
pub fn set_embedding_paused(state: State<'_, AppState>, paused: bool) -> EmbeddingStatus {
    state.embedder.set_paused(paused);
    state.embedder.status()
}
