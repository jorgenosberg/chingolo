use tauri::State;

use crate::domain::{AppSettings, ModelInfo, SettingsPatch};
use crate::error::{AppError, CmdResult};
use crate::persistence::settings as repo;
use crate::security;
use crate::state::AppState;

const DEFAULT_PROVIDER: &str = "gemini";

async fn read_settings(state: &AppState) -> crate::error::Result<AppSettings> {
    let has_api_key = security::get_key(DEFAULT_PROVIDER).await?.is_some();
    state
        .db
        .call(move |c| {
            Ok(AppSettings {
                onboarded: repo::get_bool(c, "onboarded")?,
                privacy_acknowledged: repo::get_bool(c, "privacy_acknowledged")?,
                default_model: repo::get(c, "default_model")?,
                has_api_key,
                embedding_notice_dismissed: repo::get_bool(c, "embedding_notice_dismissed")?,
            })
        })
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn get_settings(state: State<'_, AppState>) -> CmdResult<AppSettings> {
    Ok(read_settings(&state).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn update_settings(
    state: State<'_, AppState>,
    patch: SettingsPatch,
) -> CmdResult<AppSettings> {
    state
        .db
        .call(move |c| {
            if let Some(v) = patch.onboarded {
                repo::set_bool(c, "onboarded", v)?;
            }
            if let Some(v) = patch.privacy_acknowledged {
                repo::set_bool(c, "privacy_acknowledged", v)?;
            }
            if let Some(v) = patch.default_model {
                repo::set(c, "default_model", &v)?;
            }
            if let Some(v) = patch.embedding_notice_dismissed {
                repo::set_bool(c, "embedding_notice_dismissed", v)?;
            }
            Ok(())
        })
        .await?;
    Ok(read_settings(&state).await?)
}

/// Checks the key by listing models, and stores it in the Keychain only if that works.
#[tauri::command]
#[specta::specta]
pub async fn save_api_key(
    state: State<'_, AppState>,
    provider: String,
    key: String,
) -> CmdResult<Vec<ModelInfo>> {
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(AppError::Invalid("Paste your API key first.".into()).into());
    }
    let models = state.providers.get(&provider)?.list_models(&key).await?;
    security::set_key(&provider, &key).await?;
    Ok(models)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_api_key(state: State<'_, AppState>, provider: String) -> CmdResult<()> {
    state.providers.get(&provider)?;
    Ok(security::delete_key(&provider).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn list_models(
    state: State<'_, AppState>,
    provider: String,
) -> CmdResult<Vec<ModelInfo>> {
    let p = state.providers.get(&provider)?;
    let key = security::get_key(&provider)
        .await?
        .ok_or_else(|| AppError::NoApiKey(provider.clone()))?;
    Ok(p.list_models(&key).await?)
}
