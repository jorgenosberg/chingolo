use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, CmdResult};
use crate::persistence::settings as repo;
use crate::security;
use crate::services::{crashes, reporting};
use crate::state::AppState;

/// Checks the address and token against the server, and saves both only if that works.
#[tauri::command]
#[specta::specta]
pub async fn connect_reporting(
    state: State<'_, AppState>,
    url: String,
    token: String,
) -> CmdResult<()> {
    let url = reporting::normalize_url(&url)?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(AppError::Invalid("Paste the token first.".into()).into());
    }
    reporting::ping(&url, &token).await?;
    security::set_reporting_token(&token).await?;
    state
        .db
        .call(move |c| {
            repo::set(c, reporting::URL_KEY, &url)?;
            repo::set(c, reporting::ERROR_KEY, "")
        })
        .await?;
    Ok(())
}

/// Forgets the server and token and turns both reports off.
#[tauri::command]
#[specta::specta]
pub async fn disconnect_reporting(state: State<'_, AppState>) -> CmdResult<()> {
    security::delete_reporting_token().await?;
    state
        .db
        .call(|c| {
            repo::set(c, reporting::URL_KEY, "")?;
            repo::set(c, reporting::ERROR_KEY, "")?;
            repo::set_bool(c, reporting::USAGE_KEY, false)?;
            repo::set_bool(c, reporting::CRASH_KEY, false)
        })
        .await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn test_reporting_connection(state: State<'_, AppState>) -> CmdResult<()> {
    let t = reporting::target(&state.db)
        .await
        .ok_or_else(|| AppError::Invalid("Not connected to a server.".into()))?;
    reporting::ping(&t.base, &t.token).await?;
    state
        .db
        .call(|c| repo::set(c, reporting::ERROR_KEY, ""))
        .await?;
    Ok(())
}

/// An uncaught error in the webview. Never fails, so the error handler can't loop.
#[tauri::command]
#[specta::specta]
pub async fn report_frontend_error(
    app: AppHandle,
    state: State<'_, AppState>,
    message: String,
    stack: Option<String>,
    location: Option<String>,
) -> CmdResult<()> {
    if let Ok(data) = app.path().app_data_dir() {
        crashes::report_frontend(
            &state.db,
            &crashes::dir(&data),
            &message,
            stack.as_deref(),
            location.as_deref(),
        )
        .await;
    }
    Ok(())
}

fn log_file(app: &AppHandle) -> std::path::PathBuf {
    app.path()
        .app_log_dir()
        .unwrap_or_default()
        .join("chingolo.log")
}

/// The exact JSON a bug report would send, for the preview.
#[tauri::command]
#[specta::specta]
pub async fn preview_bug_report(
    app: AppHandle,
    state: State<'_, AppState>,
    description: String,
    include_log: bool,
) -> CmdResult<String> {
    let body =
        reporting::bug_report_body(&state.db, &log_file(&app), &description, include_log).await?;
    Ok(serde_json::to_string_pretty(&body).map_err(|e| AppError::Internal(e.to_string()))?)
}

/// Works with the other two reports off: the user presses send themselves.
#[tauri::command]
#[specta::specta]
pub async fn send_bug_report(
    app: AppHandle,
    state: State<'_, AppState>,
    description: String,
    include_log: bool,
) -> CmdResult<()> {
    let target = reporting::target(&state.db)
        .await
        .ok_or_else(|| AppError::Invalid("Connect to a reporting server first.".into()))?;
    let body =
        reporting::bug_report_body(&state.db, &log_file(&app), &description, include_log).await?;
    match reporting::post(&state.db, &target, "/v1/reports", &body).await {
        reporting::Outcome::Sent => Ok(()),
        reporting::Outcome::Retry => {
            Err(AppError::Provider("Couldn't reach the server. Try again later.".into()).into())
        }
        reporting::Outcome::BadToken => {
            Err(AppError::Provider("The server didn't accept the saved token.".into()).into())
        }
        reporting::Outcome::Drop => {
            Err(AppError::Provider("The server turned the report down.".into()).into())
        }
    }
}
