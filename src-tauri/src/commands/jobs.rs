use tauri::State;

use crate::error::CmdResult;
use crate::services::jobs::{Lane, LaneSnapshot};
use crate::state::AppState;

/// Every kind of background work and where it stands. The `jobsChanged` event sends the same list
/// when it changes, this seeds it when a page opens.
#[tauri::command]
#[specta::specta]
pub async fn jobs(state: State<'_, AppState>) -> CmdResult<Vec<LaneSnapshot>> {
    Ok(state.jobs.snapshot().await?)
}

/// Pauses or resumes reading, layout, details or indexing, and remembers it across restarts. A job
/// already running finishes first.
#[tauri::command]
#[specta::specta]
pub async fn set_lane_paused(
    state: State<'_, AppState>,
    lane: Lane,
    paused: bool,
) -> CmdResult<()> {
    Ok(state.jobs.set_paused(lane, paused).await?)
}

/// Runs a failed item again. The document lanes need `document_id`.
#[tauri::command]
#[specta::specta]
pub async fn retry_job(
    state: State<'_, AppState>,
    lane: Lane,
    document_id: Option<String>,
) -> CmdResult<()> {
    Ok(state.jobs.retry(lane, document_id).await?)
}

/// Stops a queued or running document, or a model download. For layout it means skipping that
/// document until a retry. True if something was stopped.
#[tauri::command]
#[specta::specta]
pub async fn cancel_job(
    state: State<'_, AppState>,
    lane: Lane,
    document_id: Option<String>,
) -> CmdResult<bool> {
    Ok(state.jobs.cancel(lane, document_id).await?)
}
