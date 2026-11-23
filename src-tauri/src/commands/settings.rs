use tauri::State;

use crate::domain::{AppSettings, ModelInfo, ModelList, ProviderError, SettingsPatch};
use crate::error::{AppError, CmdResult};
use crate::persistence::settings as repo;
use crate::security;
use crate::state::AppState;

/// Provider ids that have a key in the Keychain, in a fixed order, and the Keychain's error if it
/// couldn't be read. A locked or missing keychain reads as no keys, so the app still opens and
/// Settings can show the problem.
async fn scan_keys(state: &AppState) -> (Vec<String>, Option<String>) {
    let mut out = Vec::new();
    for id in state.providers.ids() {
        match security::get_key(id).await {
            Ok(Some(_)) => out.push(id.to_string()),
            Ok(None) => {}
            Err(e) => {
                log::warn!("couldn't read the {id} key: {e}");
                return (out, Some(e.to_string()));
            }
        }
    }
    (out, None)
}

async fn providers_with_keys(state: &AppState) -> Vec<String> {
    scan_keys(state).await.0
}

/// The provider a new conversation starts with: the one the default model belongs to, else the
/// first one with a key, else Gemini (which asks for a key on the first send).
pub async fn pick_provider(state: &AppState) -> crate::error::Result<String> {
    let saved = state.db.call(|c| repo::get(c, "default_provider")).await?;
    if let Some(p) = saved.filter(|p| state.providers.get(p).is_ok()) {
        return Ok(p);
    }
    Ok(providers_with_keys(state)
        .await
        .into_iter()
        .next()
        .unwrap_or_else(|| "gemini".to_string()))
}

use crate::services::ingestion::details_ai;

const FAVORITES_KEY: &str = "favorite_models";
const MAX_FAVORITES: usize = 100;

/// Favorites are stored as a JSON array of `provider/id`. Anything else reads as no favorites, so a
/// damaged value can't break the pickers.
fn parse_favorites(raw: Option<&str>) -> Vec<String> {
    let list: Vec<String> = raw
        .and_then(|r| serde_json::from_str(r).ok())
        .unwrap_or_default();
    clean_favorites(list)
}

/// Keeps entries shaped `provider/id`, drops repeats, keeps the user's order.
fn clean_favorites(list: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    list.into_iter()
        .map(|m| m.trim().to_string())
        .filter(|m| {
            m.split_once('/')
                .is_some_and(|(p, id)| !p.is_empty() && !id.is_empty())
        })
        .filter(|m| seen.insert(m.clone()))
        .take(MAX_FAVORITES)
        .collect()
}

fn favorites_json(list: Vec<String>) -> String {
    serde_json::to_string(&clean_favorites(list)).expect("a list of strings serialises")
}

async fn read_settings(state: &AppState) -> crate::error::Result<AppSettings> {
    let (api_keys, keychain_error) = scan_keys(state).await;
    state
        .db
        .call(move |c| {
            Ok(AppSettings {
                onboarded: repo::get_bool(c, "onboarded")?,
                privacy_acknowledged: repo::get_bool(c, "privacy_acknowledged")?,
                default_model: repo::get(c, "default_model")?,
                default_provider: repo::get(c, "default_provider")?,
                api_keys,
                keychain_error,
                embedding_notice_dismissed: repo::get_bool(c, "embedding_notice_dismissed")?,
                rerank_enabled: repo::get_bool(c, "rerank_enabled")?,
                rerank_model: repo::get(c, "rerank_model")?.filter(|m| !m.is_empty()),
                rerank_provider: repo::get(c, "rerank_provider")?.filter(|m| !m.is_empty()),
                crossref_enabled: repo::get_bool(
                    c,
                    crate::services::ingestion::enrich::CROSSREF_KEY,
                )?,
                title_search_enabled: repo::get_bool(
                    c,
                    crate::services::ingestion::enrich::TITLE_KEY,
                )?,
                metadata_ai_enabled: repo::get_bool(c, details_ai::ENABLED_KEY)?,
                metadata_model: repo::get(c, details_ai::MODEL_KEY)?.filter(|m| !m.is_empty()),
                metadata_provider: repo::get(c, details_ai::PROVIDER_KEY)?
                    .filter(|m| !m.is_empty()),
                favorite_models: parse_favorites(repo::get(c, FAVORITES_KEY)?.as_deref()),
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                last_seen_version: repo::get(c, "last_seen_version")?.filter(|v| !v.is_empty()),
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
    let lookups_on = patch.crossref_enabled == Some(true)
        || patch.title_search_enabled == Some(true)
        || patch.metadata_ai_enabled == Some(true);
    state
        .db
        .call(move |c| {
            if let Some(v) = patch.onboarded {
                repo::set_bool(c, "onboarded", v)?;
            }
            if let Some(v) = patch.privacy_acknowledged {
                repo::set_bool(c, "privacy_acknowledged", v)?;
            }
            if let Some(v) = patch.last_seen_version {
                repo::set(c, "last_seen_version", &v)?;
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
            if let Some(v) = patch.rerank_model {
                repo::set(c, "rerank_model", &v)?;
            }
            if let Some(v) = patch.rerank_provider {
                repo::set(c, "rerank_provider", &v)?;
            }
            if let Some(v) = patch.crossref_enabled {
                repo::set_bool(c, crate::services::ingestion::enrich::CROSSREF_KEY, v)?;
            }
            if let Some(v) = patch.title_search_enabled {
                repo::set_bool(c, crate::services::ingestion::enrich::TITLE_KEY, v)?;
            }
            if let Some(v) = patch.metadata_ai_enabled {
                repo::set_bool(c, details_ai::ENABLED_KEY, v)?;
            }
            if let Some(v) = patch.metadata_model {
                repo::set(c, details_ai::MODEL_KEY, &v)?;
            }
            if let Some(v) = patch.metadata_provider {
                repo::set(c, details_ai::PROVIDER_KEY, &v)?;
            }
            if let Some(v) = patch.favorite_models {
                repo::set(c, FAVORITES_KEY, &favorites_json(v))?;
            }
            Ok(())
        })
        .await?;
    if lookups_on {
        // Documents imported before the lookup was on get it now.
        let _ = state.enricher.resume(&state.db, true).await;
    }
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
    let ids = providers_with_keys(&state).await;
    let loads = ids.into_iter().map(|id| {
        let providers = state.providers.clone();
        async move {
            let result = async {
                let key = security::get_key(&id)
                    .await?
                    .ok_or_else(|| AppError::NoApiKey(id.clone()))?;
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
                errors.push(ProviderError {
                    provider,
                    message: e.to_string(),
                });
            }
        }
    }
    Ok(ModelList { models, errors })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn favorites_round_trip_in_the_users_order() {
        let list = v(&["openai/gpt-x", "gemini/flash", "gemini/pro"]);
        assert_eq!(parse_favorites(Some(&favorites_json(list.clone()))), list);
    }

    #[test]
    fn favorites_drop_repeats_and_malformed_entries() {
        let saved = favorites_json(v(&[
            "gemini/a",
            " gemini/a ",
            "nonsense",
            "/x",
            "openai/",
            "",
            "openai/b",
        ]));
        assert_eq!(parse_favorites(Some(&saved)), v(&["gemini/a", "openai/b"]));
    }

    #[test]
    fn a_damaged_or_missing_value_means_no_favorites() {
        assert!(parse_favorites(None).is_empty());
        assert!(parse_favorites(Some("not json")).is_empty());
        assert!(parse_favorites(Some(r#"{"a":1}"#)).is_empty());
        assert!(parse_favorites(Some("[]")).is_empty());
    }

    #[test]
    fn the_list_is_capped() {
        let many: Vec<String> = (0..300).map(|i| format!("gemini/m{i}")).collect();
        assert_eq!(
            parse_favorites(Some(&favorites_json(many))).len(),
            MAX_FAVORITES
        );
    }
}
