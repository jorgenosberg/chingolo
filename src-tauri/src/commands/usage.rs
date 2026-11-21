use tauri::{AppHandle, Manager, State};

use crate::domain::usage::{UsagePeriod, UsageReport, UsageRow};
use crate::error::{AppError, CmdResult};
use crate::persistence::{settings, usage};
use crate::services::pricing;
use crate::state::AppState;

/// What the paid API calls used and roughly cost, by model and purpose.
#[tauri::command]
#[specta::specta]
pub async fn get_usage(state: State<'_, AppState>, period: UsagePeriod) -> CmdResult<UsageReport> {
    let (rows, auto) = state
        .db
        .call(move |c| Ok((usage::summary(c, period)?, pricing::auto_update_enabled(c)?)))
        .await?;
    let prices = pricing::current();
    Ok(UsageReport {
        rows,
        prices_source: prices.source.clone(),
        prices_fetched: prices.fetched_at.clone(),
        prices_auto_update: auto,
    })
}

/// What the calls made for one conversation used and roughly cost. Calls from before this was
/// recorded, and search queries, aren't included.
#[tauri::command]
#[specta::specta]
pub async fn get_conversation_cost(
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<Vec<UsageRow>> {
    Ok(state
        .db
        .call(move |c| usage::conversation_summary(c, &id))
        .await?)
}

/// Turns the weekly price download on or off. Turning it on checks right away.
#[tauri::command]
#[specta::specta]
pub async fn set_prices_auto_update(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> CmdResult<()> {
    state
        .db
        .call(move |c| settings::set_bool(c, pricing::AUTO_KEY, enabled))
        .await?;
    if enabled {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let db = state.db.clone();
        tauri::async_runtime::spawn(async move { pricing::update_if_due(&db, &dir).await });
    }
    Ok(())
}

/// Fetches the price list now, whatever the schedule or the switch says.
#[tauri::command]
#[specta::specta]
pub async fn update_prices_now(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(pricing::update_now(&state.db, &dir).await?)
}

/// Opens the folder with the log files in Finder. The path is the app's own, never the webview's.
#[tauri::command]
#[specta::specta]
pub fn show_logs(app: AppHandle) -> CmdResult<()> {
    let dir = app
        .path()
        .app_log_dir()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    std::fs::create_dir_all(&dir).map_err(|e| AppError::Internal(e.to_string()))?;
    std::process::Command::new("open")
        .arg(&dir)
        .spawn()
        .map_err(|e| AppError::Internal(format!("Couldn't open the log folder: {e}")))?;
    Ok(())
}
