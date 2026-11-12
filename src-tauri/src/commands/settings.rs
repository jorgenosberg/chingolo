use tauri::State;

use crate::domain::{AppSettings, ModelInfo, ModelList, ProviderError, SettingsPatch};
use crate::error::{AppError, CmdResult};
use crate::persistence::settings as repo;
use crate::security;
use crate::state::AppState;

/// Provider ids that have a key in the Keychain, in a fixed order.
async fn providers_with_keys(state: &AppState) -> crate::error::Result<Vec<String>> {
    let mut out = Vec::new();
    for id in state.providers.ids() {
        if security::get_key(id).await?.is_some() {
            out.push(id.to_string());
        }
    }
    Ok(out)
}

/// The provider a new conversation starts with: the one the default model belongs to, else the
/// first one with a key, else Gemini (which asks for a key on the first send).
pub async fn pick_provider(state: &AppState) -> crate::error::Result<String> {
    let saved = state.db.call(|c| repo::get(c, "default_provider")).await?;
    if let Some(p) = saved.filter(|p| state.providers.get(p).is_ok()) {
        return Ok(p);
    }
    Ok(providers_with_keys(state).await?.into_iter().next().unwrap_or_else(|| "gemini".to_string()))
}

async fn read_settings(state: &AppState) -> crate::error::Result<AppSettings> {
    let api_keys = providers_with_keys(state).await?;
    state
        .db
        .call(move |c| {
            Ok(AppSettings {
                onboarded: repo::get_bool(c, "onboarded")?,
                privacy_acknowledged: repo::get_bool(c, "privacy_acknowledged")?,
                default_model: repo::get(c, "default_model")?,
                default_provider: repo::get(c, "default_provider")?,
                api_keys,
                embedding_notice_dismissed: repo::get_bool(c, "embedding_notice_dismissed")?,
                rerank_enabled: repo::get_bool(c, "rerank_enabled")?,
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
            if let Some(v) = patch.default_provider {
                repo::set(c, "default_provider", &v)?;
            }
            if let Some(v) = patch.embedding_notice_dismissed {
                repo::set_bool(c, "embedding_notice_dismissed", v)?;
            }
            if let Some(v) = patch.rerank_enabled {
                repo::set_bool(c, "rerank_enabled", v)?;
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

/// Models from every provider that has a key, loaded in parallel. A provider that fails is
/// reported in `errors` and the others still show up.
#[tauri::command]
#[specta::specta]
pub async fn list_all_models(state: State<'_, AppState>) -> CmdResult<ModelList> {
    let ids = providers_with_keys(&state).await?;
    let loads = ids.into_iter().map(|id| {
        let providers = state.providers.clone();
        async move {
            let result = async {
                let key = security::get_key(&id).await?.ok_or_else(|| AppError::NoApiKey(id.clone()))?;
                providers.get(&id)?.list_models(&key).await
            }
            .await;
            (id, result)
        }
    });
    let (mut models, mut errors) = (Vec::new(), Vec::new());
    for (provider, result) in futures_util::future::join_all(loads).await {
        match result {
            Ok(list) => models.extend(list),
            Err(e) => {
                log::warn!("couldn't list {provider} models: {e}");
                errors.push(ProviderError { provider, message: e.to_string() });
            }
        }
    }
    Ok(ModelList { models, errors })
}
