#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "e2e")]
mod e2e;
mod files;
mod jobs;
mod model_manager;
mod queue;
mod recognition;
mod resume;
#[cfg(test)]
mod resume_tests;
mod state;
mod visual_retry;

use state::Host;
use tauri::Manager;

#[tauri::command]
fn set_unsaved(host: tauri::State<'_, Host>, dirty: bool) {
    host.inner()
        .0
        .unsaved
        .store(dirty, std::sync::atomic::Ordering::Release);
}

#[tauri::command]
fn ui_ready(window: tauri::WebviewWindow, theme: String) -> state::Result<()> {
    let theme = match theme.as_str() {
        "dark" => Some(tauri::Theme::Dark),
        "light" => Some(tauri::Theme::Light),
        "system" => None,
        _ => return Err("Unknown theme; choose dark, light or system".into()),
    };
    window.set_theme(theme).map_err(|e| e.to_string())?;
    window.show().map_err(|e| e.to_string())?;
    Ok(())
}

fn main() {
    let paths = match nvsf_app::Paths::new(None) {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("Unable to initialize writable application data: {e}");
            std::process::exit(1);
        }
    };
    let builder = tauri::Builder::default()
        .setup(move |app| {
            let mut paths = paths.clone();
            if std::env::var_os("NVSF_RUNTIME_ROOT").is_none() {
                let bundled = app.path().resource_dir()?.join("runtimes");
                if bundled.is_dir() {
                    paths.runtimes = bundled;
                    nvsf_app::models::seed_bundled(&paths)?;
                }
            }
            app.manage(Host::new(paths));
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init());
    #[cfg(feature = "e2e")]
    let builder = builder.plugin(tauri_plugin_webdriver::init());
    macro_rules! commands {
        ($($extra:path),* $(,)?) => { tauri::generate_handler![
            ui_ready,
            set_unsaved,
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
            model_manager::install_models,
            model_manager::cancel_model_install,
            model_manager::import_model,
            model_manager::export_model,
            jobs::doctor,
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
    let app = builder
        .build(tauri::generate_context!())
        .expect("Native desktop initialization failed");
    let shutdown = app.state::<Host>().inner().clone();
    app.run(move |app, event| match event {
        tauri::RunEvent::ExitRequested { api, .. } => {
            if !shutdown.shutdown() { api.prevent_exit(); }
        }
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::CloseRequested { api, .. },
            ..
        } if label == "main" => {
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
    });
}
