use tauri::State;
use tauri::ipc::Channel;

use crate::domain::library::Scope;
use crate::domain::{Conversation, ConversationFilter, Message};
use crate::error::CmdResult;
use crate::commands::settings::pick_provider;
use crate::persistence::chat as repo;
use crate::persistence::settings;
use crate::services::chat::{self, ChatEvent, SendResult};
use crate::state::AppState;

#[tauri::command]
#[specta::specta]
pub async fn list_conversations(
    state: State<'_, AppState>,
    filter: ConversationFilter,
) -> CmdResult<Vec<Conversation>> {
    Ok(state
        .db
        .call(move |c| repo::list_conversations(c, &filter))
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn create_conversation(
    state: State<'_, AppState>,
    project_id: Option<String>,
    chapter_id: Option<String>,
) -> CmdResult<Conversation> {
    let provider = pick_provider(&state).await?;
    Ok(state
        .db
        .call(move |c| {
            let model = settings::get(c, "default_model")?;
            repo::create_conversation(
                c,
                project_id.as_deref(),
                chapter_id.as_deref(),
                &provider,
                model.as_deref(),
            )
        })
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn rename_conversation(
    state: State<'_, AppState>,
    id: String,
    title: String,
) -> CmdResult<Conversation> {
    Ok(state
        .db
        .call(move |c| repo::rename_conversation(c, &id, &title))
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn archive_conversation(
    state: State<'_, AppState>,
    id: String,
    archived: bool,
) -> CmdResult<Conversation> {
    Ok(state
        .db
        .call(move |c| repo::set_archived(c, &id, archived))
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn set_conversation_model(
    state: State<'_, AppState>,
    id: String,
    provider: String,
    model_id: String,
) -> CmdResult<Conversation> {
    state.providers.get(&provider)?;
    Ok(state
        .db
        .call(move |c| repo::set_model(c, &id, &provider, &model_id))
        .await?)
}

/// Chooses what a conversation may read. `None` goes back to the default.
#[tauri::command]
#[specta::specta]
pub async fn set_conversation_scope(
    state: State<'_, AppState>,
    id: String,
    scope: Option<Scope>,
) -> CmdResult<Conversation> {
    Ok(state.db.call(move |c| repo::change_scope(c, &id, scope)).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_conversation(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    Ok(state
        .db
        .call(move |c| repo::delete_conversation(c, &id))
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn list_messages(
    state: State<'_, AppState>,
    conversation_id: String,
) -> CmdResult<Vec<Message>> {
    Ok(state
        .db
        .call(move |c| repo::list_messages(c, &conversation_id))
        .await?)
}

/// Saves the user's message, then streams the reply over `on_event`.
#[tauri::command]
#[specta::specta]
pub async fn send_message(
    state: State<'_, AppState>,
    conversation_id: String,
    content: String,
    on_event: Channel<ChatEvent>,
) -> CmdResult<SendResult> {
    Ok(chat::send(&state, conversation_id, content, on_event).await?)
}

#[tauri::command]
#[specta::specta]
pub fn cancel_generation(state: State<'_, AppState>, message_id: String) -> bool {
    state.generations.cancel(&message_id)
}

/// Copies a conversation (up to `from_message_id`, or all of it) into a new one.
#[tauri::command]
#[specta::specta]
pub async fn fork_conversation(
    state: State<'_, AppState>,
    id: String,
    from_message_id: Option<String>,
) -> CmdResult<Conversation> {
    Ok(state
        .db
        .call(move |c| repo::fork_conversation(c, &id, from_message_id.as_deref()))
        .await?)
}

/// Returns a handoff note written by the conversation's model. Nothing is saved.
#[tauri::command]
#[specta::specta]
pub async fn summarize_for_handoff(
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<String> {
    Ok(chat::handoff_summary(&state, id).await?)
}
