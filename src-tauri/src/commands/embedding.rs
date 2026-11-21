use rusqlite::Connection;
use tauri::State;

use crate::error::{CmdResult, Result};
use crate::persistence::settings;
use crate::services::embedding::Backend;
use crate::services::embedding::engine::EmbeddingStatus;
use crate::state::AppState;

/// The user's yes to the one-time local model download. Kept so an interrupted download resumes at startup.
pub const DOWNLOAD_ACCEPTED: &str = "embedding_download_accepted";
/// The user's yes to sending passages of their papers to Gemini to be embedded.
pub const CLOUD_ACCEPTED: &str = "embedding_cloud_accepted";
const BACKEND: &str = "embedding_backend";

/// The saved choice. Without one, Gemini, except for anyone who already accepted the local
/// download before Gemini embeddings existed.
pub fn saved_backend(conn: &Connection) -> Result<Backend> {
    if let Some(b) = settings::get(conn, BACKEND)?
        .as_deref()
        .and_then(Backend::from_setting)
    {
        return Ok(b);
    }
    Ok(if settings::get_bool(conn, DOWNLOAD_ACCEPTED)? {
        Backend::Local
    } else {
        Backend::Gemini
    })
}

#[tauri::command]
#[specta::specta]
pub fn embedding_status(state: State<'_, AppState>) -> EmbeddingStatus {
    state.embedder.status()
}

/// Starts (or resumes) the local model download, or retries loading the model if the files are
/// there. Does nothing while Gemini is the backend.
#[tauri::command]
#[specta::specta]
pub async fn download_embedding_model(state: State<'_, AppState>) -> CmdResult<EmbeddingStatus> {
    state
        .db
        .call(|c| settings::set_bool(c, DOWNLOAD_ACCEPTED, true))
        .await?;
    if state.embedder.backend() == Backend::Local {
        state.embedder.ensure_model();
    }
    Ok(state.embedder.status())
}

/// The user's yes to sending passages to Gemini. Also the retry after an error.
#[tauri::command]
#[specta::specta]
pub async fn allow_cloud_embedding(state: State<'_, AppState>) -> CmdResult<EmbeddingStatus> {
    state
        .db
        .call(|c| settings::set_bool(c, CLOUD_ACCEPTED, true))
        .await?;
    if state.embedder.backend() == Backend::Gemini {
        state.embedder.ensure_model();
    }
    Ok(state.embedder.status())
}

/// Switches the model behind meaning search. The library is indexed again with the new one.
#[tauri::command]
#[specta::specta]
pub async fn set_embedding_backend(
    state: State<'_, AppState>,
    backend: Backend,
) -> CmdResult<EmbeddingStatus> {
    let (cloud, local) = state
        .db
        .call(move |c| {
            settings::set(c, BACKEND, backend.as_setting())?;
            Ok((
                settings::get_bool(c, CLOUD_ACCEPTED)?,
                settings::get_bool(c, DOWNLOAD_ACCEPTED)?,
            ))
        })
        .await?;
    state.embedder.use_backend(backend, cloud);
    if backend == Backend::Local && local {
        state.embedder.ensure_model();
    }
    Ok(state.embedder.status())
}

#[tauri::command]
#[specta::specta]
pub fn set_embedding_paused(state: State<'_, AppState>, paused: bool) -> EmbeddingStatus {
    state.embedder.set_paused(paused);
    state.embedder.status()
}
