mod commands;
mod domain;
mod error;
mod persistence;
mod security;
mod services;
mod state;

use std::sync::{Arc, Mutex};

use tauri::{DragDropEvent, Manager, WindowEvent};
use tauri_specta::{Builder, Event, collect_commands, collect_events};

use crate::persistence::Db;
use crate::commands::library::DropHover;
use crate::services::ai::Providers;
use crate::services::ingestion::import::{ImportSummary, import_paths};
use crate::services::ingestion::queue::{DocumentChanged, Emit, Ingestor};
use crate::state::{AppState, Generations};

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .disable_serde_phases()
        .events(collect_events![DocumentChanged, ImportSummary, DropHover])
        .commands(collect_commands![
        commands::projects::list_projects,
        commands::projects::create_project,
        commands::projects::rename_project,
        commands::projects::delete_project,
        commands::projects::list_chapters,
        commands::projects::create_chapter,
        commands::projects::rename_chapter,
        commands::projects::delete_chapter,
        commands::chat::list_conversations,
        commands::chat::create_conversation,
        commands::chat::rename_conversation,
        commands::chat::archive_conversation,
        commands::chat::set_conversation_model,
        commands::chat::delete_conversation,
        commands::chat::list_messages,
        commands::chat::send_message,
        commands::chat::cancel_generation,
        commands::chat::fork_conversation,
        commands::chat::summarize_for_handoff,
        commands::settings::get_settings,
        commands::settings::update_settings,
        commands::settings::save_api_key,
        commands::settings::delete_api_key,
        commands::settings::list_models,
        commands::library::pick_and_import,
        commands::library::set_import_target,
        commands::library::list_documents,
        commands::library::update_document_metadata,
        commands::library::delete_document,
        commands::library::cancel_import,
        commands::library::retry_import,
        commands::library::attach_document,
        commands::library::detach_document,
        commands::library::search_library,
    ])
}

#[cfg(test)]
mod tests {
    /// Keeps `src/lib/bindings.ts` in sync when running `cargo test`.
    #[test]
    fn export_bindings() {
        super::specta_builder()
            .export(specta_typescript::Typescript::default(), "../src/lib/bindings.ts")
            .expect("failed to export TypeScript bindings");
    }
}

/// Files dropped on the window are imported here, so their paths never reach the webview.
fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    let WindowEvent::DragDrop(drag) = event else { return };
    match drag {
        DragDropEvent::Enter { .. } => {
            let _ = DropHover { active: true }.emit(window);
        }
        DragDropEvent::Leave => {
            let _ = DropHover { active: false }.emit(window);
        }
        DragDropEvent::Drop { paths, .. } => {
            let _ = DropHover { active: false }.emit(window);
            let handle = window.app_handle().clone();
            let state = handle.state::<AppState>();
            let (db, ingest) = (state.db.clone(), state.ingest.clone());
            let target = state.import_target.lock().ok().and_then(|t| t.clone());
            let paths = paths.clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(summary) = import_paths(&db, &ingest, paths, target).await {
                    let _ = summary.emit(&handle);
                }
            });
        }
        _ => {}
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = specta_builder();

    #[cfg(debug_assertions)]
    builder
        .export(specta_typescript::Typescript::default(), "../src/lib/bindings.ts")
        .expect("failed to export TypeScript bindings");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(on_window_event)
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db_path = dir.join("chingui.sqlite");
            let db = Db::open(&db_path)?;
            let handle = app.handle().clone();
            let emit: Emit = Arc::new(move |e| {
                let _ = e.emit(&handle);
            });
            let ingest = Ingestor::start(&db_path, dir.join("library"), emit)?;
            // Documents that were still being read when the app last stopped.
            let (resume_db, resume_ingest) = (db.clone(), ingest.clone());
            tauri::async_runtime::spawn(async move {
                let _ = resume_ingest.resume(&resume_db).await;
            });
            app.manage(AppState {
                db,
                providers: Providers::new(),
                generations: Generations::default(),
                ingest,
                import_target: Arc::new(Mutex::new(None)),
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Chingui");
}
