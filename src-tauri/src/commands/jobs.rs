use tauri::State;

use crate::error::CmdResult;
use crate::services::jobs::LaneSnapshot;
use crate::state::AppState;

/// Every kind of background work and where it stands. The `jobsChanged` event sends the same list
/// when it changes, this seeds it when a page opens.
#[tauri::command]
#[specta::specta]
pub async fn jobs(state: State<'_, AppState>) -> CmdResult<Vec<LaneSnapshot>> {
    Ok(state.jobs.snapshot().await?)
}
