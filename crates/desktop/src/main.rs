#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "e2e")]
mod e2e;
mod files;
mod jobs;
mod language_manager;
mod model_manager;
mod queue;
mod recognition;
mod resume;
#[cfg(test)]
mod resume_tests;
mod runtime_manager;
mod state;
#[cfg(test)]
mod update_tests;
mod updates;
mod visual_retry;

use state::Host;
use tauri::Manager;

#[tauri::command]
fn set_unsaved(host: tauri::State<'_, Host>, dirty: bool) -> state::Result<()> {
    let host = host.inner();
    host.0
        .unsaved
        .store(dirty, std::sync::atomic::Ordering::Release);
    Ok(())
}

#[tauri::command]
fn ui_ready(
    window: tauri::WebviewWindow,
    host: tauri::State<'_, Host>,
    theme: String,
) -> state::Result<()> {
    let host = host.inner();
    host.0
        .log
        .event("INFO", "DESKTOP", "phase=ui-ready-received");
    let theme = match theme.as_str() {
        "dark" => Some(tauri::Theme::Dark),
        "light" => Some(tauri::Theme::Light),
        "system" => None,
        _ => return Err("Unknown theme; choose dark, light or system".into()),
    };
    window.set_theme(theme).map_err(|e| {
        host.0
            .log
            .event("ERROR", "DESKTOP", "phase=theme-apply-failed");
        e.to_string()
    })?;
    window.show().map_err(|e| {
        host.0
            .log
            .event("ERROR", "DESKTOP", "phase=window-show-failed");
        e.to_string()
    })?;
    host.0
        .log
        .event("INFO", "DESKTOP", "phase=window-show-accepted");
    Ok(())
}

fn main() {
    let started = std::time::Instant::now();
    eprintln!(
        "[INFO] [DESKTOP] phase=paths-start pid={}",
        std::process::id()
    );
    let paths = match nvsf_app::Paths::new(None) {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("Unable to initialize writable application data: {e}");
            std::process::exit(1);
        }
    };
    let log = nvsf_app::logging::Log::new(&paths, "nvsf-desktop");
    log.event(
        "INFO",
        "DESKTOP",
        &format!(
            "phase=paths-ready elapsed_ms={} pid={}",
            started.elapsed().as_millis(),
            std::process::id()
        ),
    );
    let setup_log = log.clone();
    let page_log = log.clone();
    let builder = tauri::Builder::default()
        .on_page_load(move |_webview, payload| {
            let phase = match payload.event() {
                tauri::webview::PageLoadEvent::Started => "phase=page-load-started",
                tauri::webview::PageLoadEvent::Finished => "phase=page-load-finished",
            };
            page_log.event("INFO", "DESKTOP", phase);
        })
        .setup(move |app| {
            setup_log.event("INFO", "DESKTOP", "phase=setup-entered");
            let mut paths = paths.clone();
            if std::env::var_os("NVSF_RUNTIME_ROOT").is_none()
                && let Ok(resources) = app.path().resource_dir()
            {
                paths.use_bundled_runtimes(&resources.join("runtimes"))?;
            }
            // Bare portable macOS binaries have no Resources directory. Paths retains
            // sibling/managed resolution; doctor still validates actual runtimes.
            app.manage(Host::new(paths, setup_log.clone()));
            setup_log.event("INFO", "DESKTOP", "phase=host-managed");
            updates::schedule(app.handle());
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build());
    #[cfg(feature = "e2e")]
    let builder = builder.plugin(tauri_plugin_webdriver::init());
    macro_rules! commands {
        ($($extra:path),* $(,)?) => { tauri::generate_handler![
            ui_ready,
            set_unsaved,
            updates::update_status,
            updates::check_updates,
            updates::set_update_checks,
            updates::download_update,
            updates::install_update,
            updates::cancel_update,
            updates::save_update,
            files::pick_source,
            files::choose_output,
            files::preview_frame,
            jobs::start_job,
            queue::queue_list,
            queue::queue_add,
            queue::queue_remove,
            queue::queue_regrant,
            queue::queue_start,
            resume::pick_checkpoint,
            jobs::job_status,
            jobs::control_job,
            files::current_project,
            files::replace_project,
            files::save_project,
            files::export_project,
            files::pick_project,
            files::list_history,
            files::open_history,
            jobs::model_catalog,
            language_manager::language_catalog,
            language_manager::install_language,
            language_manager::import_language,
            language_manager::export_language,
            model_manager::install_models,
            model_manager::cancel_model_install,
            model_manager::import_model,
            model_manager::export_model,
            jobs::doctor,
            runtime_manager::repair_runtimes,
            recognition::suggest_regions,
            recognition::recognize_cue,
            visual_retry::install_visual_retry,
            visual_retry::cancel_visual_retry,
            visual_retry::propose_visual_retry,
            $($extra),*
        ] };
    }
    #[cfg(feature = "e2e")]
    let builder = builder.invoke_handler(commands![e2e::e2e_fixture, e2e::e2e_save_edits]);
    #[cfg(not(feature = "e2e"))]
    let builder = builder.invoke_handler(commands![]);
    log.event("INFO", "DESKTOP", "phase=native-build-started");
    let app = builder
        .build(tauri::generate_context!())
        .unwrap_or_else(|error| {
            log.event("ERROR", "DESKTOP", "phase=native-build-failed");
            panic!("Native desktop initialization failed: {error}");
        });
    log.event("INFO", "DESKTOP", "phase=native-build-returned");
    app.run(move |app, event| {
        // Tauri manages Host in setup during Ready, after Builder::build returns.
        let Some(shutdown) = app.try_state::<Host>().map(|state| state.inner().clone()) else {
            return;
        };
        match event {
        tauri::RunEvent::ExitRequested { api, .. } => {
            shutdown.0.closing.store(true, std::sync::atomic::Ordering::Release);
            if !shutdown.shutdown() {
                shutdown.0.closing.store(false, std::sync::atomic::Ordering::Release);
                api.prevent_exit();
            }
        }
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::CloseRequested { api, .. },
            ..
        } if label == "main" => {
            shutdown.0.log.event("INFO", "DESKTOP", "phase=native-close-received");
            api.prevent_close();
            if shutdown.0.closing.swap(true, std::sync::atomic::Ordering::AcqRel) { return; }
            let host = shutdown.clone();
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
                if host.0.unsaved.load(std::sync::atomic::Ordering::Acquire)
                    && !app.dialog().message("Your review has unsaved changes. Return to save them, or discard and close?")
                        .title("Unsaved review changes").buttons(MessageDialogButtons::OkCancelCustom("Discard and close".into(), "Return to review".into())).blocking_show() {
                    host.0.closing.store(false, std::sync::atomic::Ordering::Release);
                    return;
                }
                if !host.shutdown() {
                    host.0.closing.store(false, std::sync::atomic::Ordering::Release);
                    app.dialog().message("An extraction or model operation is still stopping. The application remains open to preserve checkpoints and resources. Wait for cancellation to finish, then close again.").title("Worker shutdown pending").blocking_show();
                    return;
                }
                host.0.log.event("INFO", "DESKTOP", "phase=shutdown-complete");
                app.exit(0);
            });
        }
        tauri::RunEvent::Exit => {
            if let Some(host) = app.try_state::<Host>() {
                host.inner().0
                    .log
                    .event("INFO", "DESKTOP", "Native event loop exited");
            }
        }
        _ => {}
        }
    });
}
