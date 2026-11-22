use tauri::State;

use crate::domain::{SavedFilter, SavedItem, SavedSource};
use crate::error::CmdResult;
use crate::persistence::saved as repo;
use crate::state::AppState;

#[tauri::command]
#[specta::specta]
pub async fn list_saved(
    state: State<'_, AppState>,
    filter: SavedFilter,
) -> CmdResult<Vec<SavedItem>> {
    Ok(state.db.call(move |c| repo::list(c, &filter)).await?)
}

/// Saving a reply that is already saved returns the existing item.
#[tauri::command]
#[specta::specta]
pub async fn save_reply(state: State<'_, AppState>, message_id: String) -> CmdResult<SavedItem> {
    Ok(state
        .db
        .call(move |c| repo::save_reply(c, &message_id))
        .await?)
}

/// `handles` are the `[S<n>]` numbers of the citation chips inside the selection.
#[tauri::command]
#[specta::specta]
pub async fn save_selection(
    state: State<'_, AppState>,
    message_id: String,
    text: String,
    handles: Vec<u32>,
) -> CmdResult<SavedItem> {
    Ok(state
        .db
        .call(move |c| repo::save_selection(c, &message_id, &text, &handles))
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn save_passage(
    state: State<'_, AppState>,
    document_id: String,
    page: u32,
    text: String,
    project_id: Option<String>,
) -> CmdResult<SavedItem> {
    Ok(state
        .db
        .call(move |c| repo::save_passage(c, &document_id, page, &text, project_id.as_deref()))
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn update_saved_note(
    state: State<'_, AppState>,
    id: String,
    note: String,
) -> CmdResult<SavedItem> {
    Ok(state
        .db
        .call(move |c| repo::update_note(c, &id, &note))
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn move_saved(
    state: State<'_, AppState>,
    id: String,
    project_id: Option<String>,
) -> CmdResult<SavedItem> {
    Ok(state
        .db
        .call(move |c| repo::set_project(c, &id, project_id.as_deref()))
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_saved(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    Ok(state.db.call(move |c| repo::delete(c, &id)).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn get_saved_source(
    state: State<'_, AppState>,
    item_id: String,
    handle: u32,
) -> CmdResult<Option<SavedSource>> {
    Ok(state
        .db
        .call(move |c| repo::get_source(c, &item_id, handle))
        .await?)
}
