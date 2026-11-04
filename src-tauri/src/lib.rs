mod commands;
mod domain;
mod error;
mod persistence;
mod security;
mod services;
mod state;

use tauri::Manager;
use tauri_specta::{Builder, collect_commands};

use crate::persistence::Db;
use crate::services::ai::Providers;
use crate::state::{AppState, Generations};

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .disable_serde_phases()
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
        commands::settings::get_settings,
        commands::settings::update_settings,
        commands::settings::save_api_key,
        commands::settings::delete_api_key,
        commands::settings::list_models,
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = specta_builder();

    #[cfg(debug_assertions)]
    builder
        .export(specta_typescript::Typescript::default(), "../src/lib/bindings.ts")
        .expect("failed to export TypeScript bindings");

    tauri::Builder::default()
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db = Db::open(&dir.join("chingui.sqlite"))?;
            app.manage(AppState {
                db,
                providers: Providers::new(),
                generations: Generations::default(),
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Chingui");
}
