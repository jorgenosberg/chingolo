use tauri::State;

use crate::domain::{Chapter, Project};
use crate::error::CmdResult;
use crate::persistence::projects as repo;
use crate::state::AppState;

#[tauri::command]
#[specta::specta]
pub async fn list_projects(state: State<'_, AppState>) -> CmdResult<Vec<Project>> {
    Ok(state.db.call(|c| repo::list_projects(c)).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn create_project(state: State<'_, AppState>, name: String) -> CmdResult<Project> {
    Ok(state
        .db
        .call(move |c| repo::create_project(c, &name))
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn rename_project(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> CmdResult<Project> {
    Ok(state
        .db
        .call(move |c| repo::rename_project(c, &id, &name))
        .await?)
}

/// Also deletes the project's chapters and conversations.
#[tauri::command]
#[specta::specta]
pub async fn delete_project(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    Ok(state.db.call(move |c| repo::delete_project(c, &id)).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn list_chapters(
    state: State<'_, AppState>,
    project_id: String,
) -> CmdResult<Vec<Chapter>> {
    Ok(state
        .db
        .call(move |c| repo::list_chapters(c, &project_id))
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn create_chapter(
    state: State<'_, AppState>,
    project_id: String,
    title: String,
) -> CmdResult<Chapter> {
    Ok(state
        .db
        .call(move |c| repo::create_chapter(c, &project_id, &title))
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn rename_chapter(
    state: State<'_, AppState>,
    id: String,
    title: String,
) -> CmdResult<Chapter> {
    Ok(state
        .db
        .call(move |c| repo::rename_chapter(c, &id, &title))
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_chapter(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    Ok(state.db.call(move |c| repo::delete_chapter(c, &id)).await?)
}
