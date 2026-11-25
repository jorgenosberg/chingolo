use tauri::State;

use crate::domain::{SavedFilter, SavedItem, SavedSource, SavedTagInfo};
use crate::error::CmdResult;
use crate::persistence::saved as repo;
use crate::services::usage_counts::{self, Event};
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
        .call(move |c| {
            let item = repo::save_reply(c, &message_id)?;
            usage_counts::count_conn(c, Event::SavedItemCreated, "", "", "reply");
            Ok(item)
        })
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
        .call(move |c| {
            let item = repo::save_selection(c, &message_id, &text, &handles)?;
            usage_counts::count_conn(c, Event::SavedItemCreated, "", "", "selection");
            Ok(item)
        })
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
    chapter_id: Option<String>,
) -> CmdResult<SavedItem> {
    Ok(state
        .db
        .call(move |c| {
            let item = repo::save_passage(
                c,
                &document_id,
                page,
                &text,
                project_id.as_deref(),
                chapter_id.as_deref(),
            )?;
            usage_counts::count_conn(c, Event::SavedItemCreated, "", "", "passage");
            Ok(item)
        })
        .await?)
}

/// Adds a note to the end of the item's thread. Returns the updated item.
#[tauri::command]
#[specta::specta]
pub async fn add_saved_note(
    state: State<'_, AppState>,
    item_id: String,
    body: String,
) -> CmdResult<SavedItem> {
    Ok(state
        .db
        .call(move |c| repo::add_note(c, &item_id, &body))
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn update_saved_note(
    state: State<'_, AppState>,
    note_id: String,
    body: String,
) -> CmdResult<SavedItem> {
    Ok(state
        .db
        .call(move |c| repo::update_note(c, &note_id, &body))
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_saved_note(
    state: State<'_, AppState>,
    note_id: String,
) -> CmdResult<SavedItem> {
    Ok(state
        .db
        .call(move |c| repo::delete_note(c, &note_id))
        .await?)
}

/// Sets the project and folder of every item. The folder must belong to the project.
#[tauri::command]
#[specta::specta]
pub async fn move_saved_items(
    state: State<'_, AppState>,
    ids: Vec<String>,
    project_id: Option<String>,
    chapter_id: Option<String>,
) -> CmdResult<()> {
    Ok(state
        .db
        .call(move |c| repo::move_items(c, &ids, project_id.as_deref(), chapter_id.as_deref()))
        .await?)
}

/// Deletes the items and returns how many were there.
#[tauri::command]
#[specta::specta]
pub async fn delete_saved_items(state: State<'_, AppState>, ids: Vec<String>) -> CmdResult<u32> {
    Ok(state.db.call(move |c| repo::delete_items(c, &ids)).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn list_saved_tags(state: State<'_, AppState>) -> CmdResult<Vec<SavedTagInfo>> {
    Ok(state.db.call(|c| repo::list_tags(c)).await?)
}

/// Adds the named tags (made if new) to every item and takes the `remove_tag_ids` off them.
#[tauri::command]
#[specta::specta]
pub async fn tag_saved_items(
    state: State<'_, AppState>,
    ids: Vec<String>,
    add_names: Vec<String>,
    remove_tag_ids: Vec<String>,
    new_color: Option<String>,
) -> CmdResult<()> {
    Ok(state
        .db
        .call(move |c| repo::tag_items(c, &ids, &add_names, &remove_tag_ids, new_color.as_deref()))
        .await?)
}

/// A name another tag already has merges the two.
#[tauri::command]
#[specta::specta]
pub async fn rename_saved_tag(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> CmdResult<()> {
    Ok(state
        .db
        .call(move |c| repo::rename_tag(c, &id, &name))
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn set_saved_tag_color(
    state: State<'_, AppState>,
    id: String,
    color: String,
) -> CmdResult<()> {
    Ok(state
        .db
        .call(move |c| repo::set_tag_color(c, &id, &color))
        .await?)
}

/// Takes the tag off every item and deletes it. The items stay.
#[tauri::command]
#[specta::specta]
pub async fn delete_saved_tag(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    Ok(state.db.call(move |c| repo::delete_tag(c, &id)).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn set_saved_used(
    state: State<'_, AppState>,
    ids: Vec<String>,
    used: bool,
) -> CmdResult<()> {
    Ok(state
        .db
        .call(move |c| repo::set_used(c, &ids, used))
        .await?)
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
