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
use tokio::sync::Notify;

use crate::commands::library::DropHover;
use crate::domain::library::DocStatus;
use crate::persistence::Db;
use crate::services::ai::Providers;
use crate::services::docfile;
use crate::services::embedding::Backend;
use crate::services::embedding::engine::{Embedder, EmbeddingStatus, StatusEmit};
use crate::services::ingestion::activity::Activity;
use crate::services::ingestion::import::{ImportSummary, import_paths};
use crate::services::ingestion::queue::{DocumentChanged, Emit, Ingestor};
use crate::services::jobs::{Jobs, JobsChanged, Sources};
use crate::state::{AppState, Generations};

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .disable_serde_phases()
        .events(collect_events![
            DocumentChanged,
            JobsChanged,
            ImportSummary,
            DropHover,
            EmbeddingStatus
        ])
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
            commands::chat::search_messages,
            commands::chat::create_conversation,
            commands::chat::rename_conversation,
            commands::chat::archive_conversation,
            commands::chat::set_conversation_model,
            commands::chat::set_conversation_scope,
            commands::chat::delete_conversation,
            commands::chat::discard_empty_conversation,
            commands::chat::set_conversation_draft,
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
            commands::settings::list_all_models,
            commands::reporting::connect_reporting,
            commands::reporting::disconnect_reporting,
            commands::reporting::test_reporting_connection,
            commands::reporting::report_frontend_error,
            commands::reporting::preview_bug_report,
            commands::reporting::send_bug_report,
            commands::reporting::count_event,
            commands::library::pick_and_import,
            commands::library::set_import_target,
            commands::library::list_documents,
            commands::library::update_document_metadata,
            commands::library::delete_document,
            commands::library::cancel_import,
            commands::library::retry_import,
            commands::library::refresh_details,
            commands::library::refresh_all_details,
            commands::library::attach_document,
            commands::library::detach_document,
            commands::library::set_project_documents,
            commands::library::delete_documents,
            commands::library::reveal_document_file,
            commands::library::search_library,
            commands::library::get_passage,
            commands::library::get_layout,
            commands::library::get_source,
            commands::saved::list_saved,
            commands::saved::save_reply,
            commands::saved::save_selection,
            commands::saved::save_passage,
            commands::saved::add_saved_note,
            commands::saved::update_saved_note,
            commands::saved::delete_saved_note,
            commands::saved::move_saved,
            commands::saved::delete_saved,
            commands::saved::get_saved_source,
            commands::update::check_for_update,
            commands::update::install_update,
            commands::ocr::ocr_status,
            commands::ocr::download_ocr_model,
            commands::ocr::layout_status,
            commands::ocr::download_layout_model,
            commands::ocr::tables_status,
            commands::ocr::download_tables_model,
            commands::embedding::embedding_status,
            commands::embedding::download_embedding_model,
            commands::embedding::allow_cloud_embedding,
            commands::embedding::set_embedding_backend,
            commands::usage::get_usage,
            commands::usage::get_conversation_cost,
            commands::usage::set_prices_auto_update,
            commands::usage::update_prices_now,
            commands::usage::show_logs,
            commands::jobs::jobs,
            commands::jobs::set_lane_paused,
            commands::jobs::retry_job,
            commands::jobs::cancel_job,
        ])
}

/// Files dropped on the window are imported here, so their paths never reach the webview.
fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    let WindowEvent::DragDrop(drag) = event else {
        return;
    };
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
/// Logs go to the console and to a file in `~/Library/Logs/app.chingolo.desktop/`. They hold
/// timings, counts and errors, never document text, messages or API keys.
fn logging() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    use tauri_plugin_log::{RotationStrategy, Target, TargetKind};
    // Our own code logs at debug level in dev builds. Dependencies stay at info, they are chatty.
    let ours = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    tauri_plugin_log::Builder::new()
        .targets([
            Target::new(TargetKind::Stdout),
            Target::new(TargetKind::LogDir {
                file_name: Some("chingolo".into()),
            }),
        ])
        .level(log::LevelFilter::Info)
        .level_for("chingolo_lib", ours)
        .level_for("chingolo", ours)
        .max_file_size(2_000_000)
        .rotation_strategy(RotationStrategy::KeepSome(5))
        .build()
}

pub fn run() {
    let builder = specta_builder();

    #[cfg(debug_assertions)]
    builder
        .export(
            specta_typescript::Typescript::default(),
            "../src/lib/bindings.ts",
        )
        .expect("failed to export TypeScript bindings");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(logging())
        .register_asynchronous_uri_scheme_protocol(docfile::SCHEME, |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                responder.respond(docfile::handle(&app, request).await);
            });
        })
        .on_window_event(on_window_event)
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            crate::services::crashes::install_hook(crate::services::crashes::dir(&dir));
            log::info!("Chingolo {} starting", env!("CARGO_PKG_VERSION"));
            let db_path = dir.join("chingolo.sqlite");
            let db = Db::open(&db_path)?;
            crate::services::pricing::load_saved(&dir);
            {
                use crate::services::usage_counts as counts;
                let counting = db.clone();
                tauri::async_runtime::spawn(async move {
                    counts::count(&counting, counts::Event::AppOpened, "", "", "").await;
                    counts::keep_flushing(counting).await;
                });
            }
            tauri::async_runtime::spawn(crate::services::crashes::send_pending(
                db.clone(),
                crate::services::crashes::dir(&dir),
            ));
            tauri::async_runtime::spawn(crate::services::pricing::keep_fresh(
                db.clone(),
                dir.clone(),
            ));
            // Woken by anything that changes background work, so the job list refreshes.
            let poke = Arc::new(Notify::new());
            let handle = app.handle().clone();
            let emit_poke = poke.clone();
            let emit: Emit = Arc::new(move |e| {
                let _ = e.emit(&handle);
                emit_poke.notify_one();
            });
            let status_handle = app.handle().clone();
            let status_poke = poke.clone();
            let status_emit: StatusEmit = Arc::new(move |s| {
                let _ = s.emit(&status_handle);
                status_poke.notify_one();
            });
            let (backend, cloud_accepted) = {
                let conn = Db::open_secondary(&db_path)?;
                (
                    commands::embedding::saved_backend(&conn)?,
                    crate::persistence::settings::get_bool(
                        &conn,
                        commands::embedding::CLOUD_ACCEPTED,
                    )?,
                )
            };
            let embedder = Embedder::start(
                &db_path,
                &dir,
                backend,
                cloud_accepted,
                status_emit,
                emit.clone(),
            )?;
            // A document that becomes ready wakes the embedding worker. The worker's own
            // document events skip this, so it doesn't wake itself.
            // It also queues the document for a details lookup, which has its own emit.
            let providers = Providers::new();
            let activity_poke = poke.clone();
            let activity = Activity::new(move || activity_poke.notify_one());
            let enricher = crate::services::ingestion::enrich::Enricher::start(
                db.clone(),
                emit.clone(),
                providers.clone(),
                activity.clone(),
            );
            let after_layout = (embedder.clone(), emit.clone());
            let (wake, raw_emit, queue) = (embedder.clone(), emit, enricher.clone());
            let ingest_emit: Emit = Arc::new(move |e| {
                let ready = e.document.status == DocStatus::Ready;
                let id = e.document.id.clone();
                raw_emit(e);
                if ready {
                    wake.wake();
                    queue.queue(&id);
                }
            });
            let ingest = Ingestor::start(&db_path, dir.join("library"), ingest_emit)?;
            ingest.set_activity(activity.clone());
            ingest.set_poke(poke.clone());
            let ocr = crate::services::ingestion::ocr::OcrModel::new(&dir);
            ingest.set_ocr_dir(Some(ocr.dir().to_path_buf()));
            let layout = crate::services::ingestion::layout::model(&dir);
            ingest.set_layout_dir(Some(layout.dir().to_path_buf()));
            let tables = crate::services::ingestion::tables::model(&dir);
            ingest.set_tables_dir(Some(tables.dir().to_path_buf()));
            // Work that was waiting for a model starts when its download finishes.
            let scans = (db.clone(), ingest.clone());
            ocr.set_on_ready(Arc::new(move || {
                let (db, ingest) = scans.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = ingest.enqueue_scans(&db).await;
                });
            }));
            let layouts = (db.clone(), ingest.clone());
            let on_layout_model: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                let (db, ingest) = layouts.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = ingest.enqueue_layouts(&db).await;
                });
            });
            layout.set_on_ready(on_layout_model.clone());
            tables.set_on_ready(on_layout_model);
            ingest.set_on_layout(Arc::new(move |d| {
                after_layout.1(crate::services::ingestion::queue::DocumentChanged {
                    document: d.clone(),
                    progress: None,
                });
                after_layout.0.wake();
            }));
            // Documents that were still being read when the app last stopped.
            let (resume_db, resume_ingest) = (db.clone(), ingest.clone());
            tauri::async_runtime::spawn(async move {
                let _ = resume_ingest.resume(&resume_db).await;
                let _ = resume_ingest.enqueue_layouts(&resume_db).await;
            });
            // If the user agreed to the local model download earlier, finish it.
            if backend == Backend::Local {
                let (model_db, model_embedder) = (db.clone(), embedder.clone());
                tauri::async_runtime::spawn(async move {
                    let accepted = model_db
                        .call(|c| {
                            crate::persistence::settings::get_bool(
                                c,
                                commands::embedding::DOWNLOAD_ACCEPTED,
                            )
                        })
                        .await
                        .unwrap_or(false);
                    if accepted {
                        model_embedder.ensure_model();
                    }
                });
            }
            // Lookups that were cut short when the app last stopped.
            let (resume_db, resume) = (db.clone(), enricher.clone());
            tauri::async_runtime::spawn(async move {
                let _ = resume.resume(&resume_db, false).await;
            });
            let jobs = Jobs::new(
                Sources {
                    db: db.clone(),
                    ingest: ingest.clone(),
                    embedder: embedder.clone(),
                    ocr: ocr.clone(),
                    layout: layout.clone(),
                    tables: tables.clone(),
                    activity,
                    enricher: enricher.clone(),
                },
                poke,
            );
            let paused_jobs = jobs.clone();
            tauri::async_runtime::spawn(async move { paused_jobs.load_paused().await });
            let jobs_handle = app.handle().clone();
            jobs.run(move |e| {
                let _ = e.emit(&jobs_handle);
            });
            app.manage(AppState {
                db,
                providers,
                generations: Generations::default(),
                ingest,
                embedder,
                ocr,
                layout,
                tables,
                enricher,
                jobs,
                import_target: Arc::new(Mutex::new(None)),
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Chingolo");
}

#[cfg(test)]
mod tests {
    /// Keeps `src/lib/bindings.ts` in sync when running `cargo test`.
    #[test]
    fn export_bindings() {
        super::specta_builder()
            .export(
                specta_typescript::Typescript::default(),
                "../src/lib/bindings.ts",
            )
            .expect("failed to export TypeScript bindings");
    }
}
