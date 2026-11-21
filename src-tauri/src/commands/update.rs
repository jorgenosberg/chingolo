use serde::Serialize;
use specta::Type;
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

use crate::error::{AppError, CmdResult};

#[derive(Serialize, Type)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: Option<String>,
}

fn failed(e: tauri_plugin_updater::Error) -> AppError {
    AppError::Internal(format!("update check failed: {e}"))
}

/// Asks the release page whether a newer version exists. Nothing is downloaded.
#[tauri::command]
#[specta::specta]
pub async fn check_for_update(app: AppHandle) -> CmdResult<Option<UpdateInfo>> {
    let update = app
        .updater()
        .map_err(failed)?
        .check()
        .await
        .map_err(failed)?;
    Ok(update.map(|u| {
        log::info!(
            "update available: {} (running {})",
            u.version,
            u.current_version
        );
        UpdateInfo {
            version: u.version,
            notes: u.body,
        }
    }))
}

/// Downloads the newer version, checks its signature, installs it and restarts the app.
#[tauri::command]
#[specta::specta]
pub async fn install_update(app: AppHandle) -> CmdResult<()> {
    let Some(update) = app
        .updater()
        .map_err(failed)?
        .check()
        .await
        .map_err(failed)?
    else {
        return Ok(());
    };
    log::info!("installing update {}", update.version);
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(failed)?;
    app.restart()
}
