#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod close;
#[cfg(test)]
mod command_tests;
mod drop;
#[cfg(feature = "e2e")]
mod e2e;
mod files;
mod jobs;
mod language_manager;
mod model_manager;
mod preview_transport;
mod queue;
#[cfg(test)]
mod queue_tests;
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
use tauri::{Emitter, Manager};

#[tauri::command]
fn set_unsaved(
    host: tauri::State<'_, Host>,
    dirty: bool,
    review_dirty: Option<bool>,
) -> state::Result<()> {
    use std::sync::atomic::Ordering::Release;
    let host = host.inner();
    host.0
        .extract_only_dirty
        .store(dirty && review_dirty == Some(false), Release);
    host.0.unsaved.store(dirty, Release);
    Ok(())
}

#[tauri::command]
fn restart_app(app: tauri::AppHandle, host: tauri::State<'_, Host>) -> state::Result<()> {
    let host = host.inner();
    if host.0.unsaved.load(std::sync::atomic::Ordering::Acquire) {
        return Err("Save or discard the current corrections before restarting".into());
    }
    host.0
        .log
        .event("INFO", "DESKTOP", "phase=restart-requested");
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = close::request_exit(app.clone(), host.clone(), true).await {
            use tauri_plugin_dialog::DialogExt;
            host.0.log.event("WARNING", "DESKTOP", &error);
            app.dialog()
                .message(error)
                .title("Restart deferred")
                .show(|_| {});
        }
    });
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
    apply_window_theme(&window, host, &theme)?;
    window.show().map_err(|e| {
        host.0
            .log
            .event("ERROR", "DESKTOP", "phase=window-show-failed");
        e.to_string()
    })?;
    host.0
        .log
        .event("INFO", "DESKTOP", "phase=window-show-accepted");
    let app = window.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || updates::schedule(&app));
    Ok(())
}

fn apply_window_theme(
    window: &tauri::WebviewWindow,
    host: &Host,
    theme: &str,
) -> state::Result<()> {
    let theme = match theme {
        "dark" => Some(tauri::Theme::Dark),
        "light" => Some(tauri::Theme::Light),
        "system" => None,
        _ => return Err("Unknown theme; choose dark, light or system".into()),
    };
    window.set_theme(theme).map_err(|error| {
        host.0
            .log
            .event("ERROR", "DESKTOP", "phase=theme-apply-failed");
        error.to_string()
    })?;
    #[cfg(windows)]
    {
        let native = window.clone();
        let log = host.0.log.clone();
        window
            .run_on_main_thread(move || {
                if let Err(error) = refresh_titlebar(&native, theme) {
                    log.event("WARNING", "DESKTOP", &error);
                }
            })
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(windows)]
fn refresh_titlebar(
    window: &tauri::WebviewWindow,
    theme: Option<tauri::Theme>,
) -> state::Result<()> {
    use std::{ffi::c_void, ptr};
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut c_void,
            attribute: u32,
            value: *const c_void,
            len: u32,
        ) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowPos(
            hwnd: *mut c_void,
            after: *mut c_void,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
        fn RedrawWindow(
            hwnd: *mut c_void,
            rect: *const c_void,
            region: *mut c_void,
            flags: u32,
        ) -> i32;
    }
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_FRAMECHANGED: u32 = 0x0020;
    const RDW_INVALIDATE: u32 = 0x0001;
    const RDW_UPDATENOW: u32 = 0x0100;
    const RDW_FRAME: u32 = 0x0400;
    let hwnd = window.hwnd().map_err(|error| error.to_string())?.0;
    let use_dark = i32::from(match theme {
        Some(theme) => theme == tauri::Theme::Dark,
        None => window.theme().map_err(|error| error.to_string())? == tauri::Theme::Dark,
    });
    // SAFETY: this is the live window on its event thread. The BOOL lives for the call;
    // null optional pointers are valid, and no workspace lock is held during repaint.
    let (applied, positioned, redrawn) = unsafe {
        let applied = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            ptr::from_ref(&use_dark).cast(),
            4,
        );
        let positioned = SetWindowPos(
            hwnd,
            ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
        let redrawn = RedrawWindow(
            hwnd,
            ptr::null(),
            ptr::null_mut(),
            RDW_FRAME | RDW_INVALIDATE | RDW_UPDATENOW,
        );
        (applied, positioned, redrawn)
    };
    if applied < 0 || positioned == 0 || redrawn == 0 {
        return Err("Windows could not refresh the native title bar theme".into());
    }
    Ok(())
}

#[tauri::command]
async fn set_window_theme(
    window: tauri::WebviewWindow,
    host: tauri::State<'_, Host>,
    theme: String,
) -> state::Result<()> {
    apply_window_theme(&window, host.inner(), &theme)
}

#[tauri::command]
fn set_window_fullscreen(window: tauri::WebviewWindow, fullscreen: bool) -> state::Result<bool> {
    window
        .set_fullscreen(fullscreen)
        .map_err(|error| error.to_string())?;
    Ok(window.is_fullscreen().unwrap_or(fullscreen))
}

const EMBEDDED_LOGO_SVG: &[u8] = include_bytes!("../../../assets/logo.svg");

fn main() {
    std::hint::black_box(EMBEDDED_LOGO_SVG);
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
            // Portable packages retain sibling/managed runtime resolution;
            // doctor still validates the selected runtime files.
            if !app.manage(Host::new(paths, setup_log.clone())) {
                return Err("Native Host state was already managed".into());
            }
            #[cfg(feature = "e2e")]
            eprintln!(
                "[unix_ms={}] [INFO] [DESKTOP] phase=host-managed pid={}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
                std::process::id()
            );
            setup_log.event("INFO", "DESKTOP", "phase=host-managed");
            if let Some(window) = app.get_webview_window("main") {
                window.set_title(&format!(
                    "Numera Video Sub Finder (v{})",
                    env!("CARGO_PKG_VERSION")
                ))?;
            }
            Ok(())
        })
        .manage(close::CloseSession::default())
        .manage(files::ProjectFiles::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build());
    #[cfg(feature = "e2e")]
    let builder = builder.plugin(tauri_plugin_webdriver::init());
    macro_rules! commands {
        ($($extra:path),* $(,)?) => { tauri::generate_handler![
            ui_ready,
            set_window_theme,
            set_window_fullscreen,
            files::bind_project_source,
            set_unsaved,
            restart_app,
            close::confirm_close,
            updates::update_status,
            updates::check_updates,
            updates::set_update_checks,
            updates::download_update,
            updates::install_update,
            updates::cancel_update,
            updates::save_update,
            files::pick_source,
            drop::accept_dropped_paths,
            files::choose_output,
            files::preview_frame,
            preview_transport::preview_transport,
            files::preview_initial_frame,
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
            files::save_project_as,
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
            recognition::cue_evidence,
            visual_retry::install_visual_retry,
            visual_retry::cancel_visual_retry,
            visual_retry::propose_visual_retry,
            $($extra),*
        ] };
    }
    #[cfg(feature = "e2e")]
    let builder = builder.invoke_handler(commands![
        e2e::e2e_fixture,
        e2e::e2e_drop_fixture,
        e2e::e2e_save_edits,
        e2e::e2e_focus,
        e2e::e2e_close_probe,
        e2e::e2e_request_close,
    ]);
    #[cfg(not(feature = "e2e"))]
    let builder = builder.invoke_handler(commands![]);
    log.event("INFO", "DESKTOP", "phase=native-build-started");
    let context = tauri::generate_context!();
    log.event("INFO", "DESKTOP", "phase=native-context-ready");
    let app = match builder.build(context) {
        Ok(app) => app,
        Err(error) => {
            log.event("ERROR", "DESKTOP", "phase=native-build-failed");
            eprintln!("Native desktop initialization failed: {error}");
            std::process::exit(1);
        }
    };
    log.event("INFO", "DESKTOP", "phase=native-build-returned");
    app.run(move |app, event| {
        // Tauri manages Host in setup during Ready, after Builder::build returns.
        let Some(shutdown) = app.try_state::<Host>().map(|state| state.inner().clone()) else {
            return;
        };
        match event {
            tauri::RunEvent::ExitRequested { api, .. } => {
                if !shutdown
                    .0
                    .exit_approved
                    .load(std::sync::atomic::Ordering::Acquire)
                {
                    api.prevent_exit();
                    close::request(app, &shutdown);
                }
            }
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } if label == "main" => {
                shutdown
                    .0
                    .log
                    .event("INFO", "DESKTOP", "phase=native-close-received");
                api.prevent_close();
                close::request(app, &shutdown);
            }
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::DragDrop(drop_event),
                ..
            } if label == "main" => match drop_event {
                tauri::DragDropEvent::Enter { .. } | tauri::DragDropEvent::Over { .. } => {
                    let _ = app.emit("nvsf://drag-state", true);
                }
                tauri::DragDropEvent::Leave => {
                    let _ = app.emit("nvsf://drag-state", false);
                }
                tauri::DragDropEvent::Drop { paths, .. } => {
                    let _ = app.emit("nvsf://drag-state", false);
                    if let Err(error) = drop::publish_native_drop(app, &shutdown, paths) {
                        shutdown.0.log.event("ERROR", "DROP", &error);
                    }
                }
                _ => {}
            },
            tauri::RunEvent::Exit => {
                if let Some(host) = app.try_state::<Host>() {
                    host.inner()
                        .0
                        .log
                        .event("INFO", "DESKTOP", "Native event loop exited");
                }
            }
            _ => {}
        }
    });
}
