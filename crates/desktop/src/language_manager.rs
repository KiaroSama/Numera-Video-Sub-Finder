use crate::{
    model_manager::begin,
    state::{Host, Result, blocking, display_name},
};
use std::sync::atomic::Ordering;
use tauri_plugin_dialog::DialogExt;

fn code(code: &str) -> Result<String> {
    nvsf_core::languages::find(code)
        .map(|l| l.code.clone())
        .ok_or_else(|| "Choose an official language code".into())
}

#[tauri::command]
pub async fn language_catalog(
    host: tauri::State<'_, Host>,
) -> Result<Vec<nvsf_app::languages::LanguageStatus>> {
    let host = host.inner().clone();
    blocking(move || Ok(nvsf_app::languages::catalog(&host.0.paths))).await
}

#[tauri::command]
pub async fn install_language(
    host: tauri::State<'_, Host>,
    language: String,
    consent: bool,
) -> Result<()> {
    if !consent {
        return Err("Language installation requires explicit approval".into());
    }
    let language = code(&language)?;
    let host = host.inner().clone();
    let operation = begin(&host)?;
    blocking(move || {
        let _operation = operation;
        host.0.log.event(
            "INFO",
            "LANGUAGES",
            "Explicit selected language provisioning requested",
        );
        nvsf_app::languages::install(&host.0.paths, &language, &host.0.model_cancel)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn import_language(
    app: tauri::AppHandle,
    host: tauri::State<'_, Host>,
    language: String,
) -> Result<Option<String>> {
    let language = code(&language)?;
    let host = host.inner().clone();
    let operation = begin(&host)?;
    blocking(move || {
        let _operation = operation;
        let Some(file) = app.dialog().file().set_title("Import verified language pack").add_filter("Tesseract language", &["traineddata"]).blocking_pick_file() else { return Ok(None); };
        let source = file.into_path().map_err(|e| e.to_string())?;
        if host.0.paths.models.join("tessdata").join(format!("{language}.traineddata")).exists() {
            use tauri_plugin_dialog::MessageDialogButtons;
            if !app.dialog().message("Verify and replace this installed language pack? Invalid files preserve the current pack.").title("Replace language pack").buttons(MessageDialogButtons::OkCancelCustom("Verify and replace".into(), "Cancel".into())).blocking_show() { return Ok(None); }
        }
        nvsf_app::languages::import(&host.0.paths, &language, &source, &host.0.model_cancel).map_err(|e| e.to_string())?;
        Ok(Some(language))
    }).await
}

#[tauri::command]
pub async fn export_language(
    app: tauri::AppHandle,
    host: tauri::State<'_, Host>,
    language: String,
) -> Result<Option<String>> {
    let language = code(&language)?;
    let host = host.inner().clone();
    let operation = begin(&host)?;
    blocking(move || {
        let _operation = operation;
        let Some(file) = app
            .dialog()
            .file()
            .set_title("Export verified language pack")
            .add_filter("Tesseract language", &["traineddata"])
            .set_file_name(format!("{language}.traineddata"))
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let destination = file.into_path().map_err(|e| e.to_string())?;
        if host.0.model_cancel.load(Ordering::Acquire) {
            return Err("Language export cancelled".into());
        }
        nvsf_app::languages::export(&host.0.paths, &language, &destination)
            .map_err(|e| e.to_string())?;
        Ok(Some(display_name(&destination)))
    })
    .await
}
