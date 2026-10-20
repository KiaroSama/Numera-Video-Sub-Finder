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
        import_language_from(&host, &language, &source, || {
            use tauri_plugin_dialog::MessageDialogButtons;
            app.dialog().message("Verify and replace this installed language pack? Invalid files preserve the current pack.").title("Replace language pack").buttons(MessageDialogButtons::OkCancelCustom("Verify and replace".into(), "Cancel".into())).blocking_show()
        })
    }).await
}

pub(crate) fn import_language_from(
    host: &Host,
    language: &str,
    source: &std::path::Path,
    confirm: impl FnOnce() -> bool,
) -> Result<Option<String>> {
    if host
        .0
        .paths
        .models
        .join("tessdata")
        .join(format!("{language}.traineddata"))
        .exists()
        && !confirm()
    {
        return Ok(None);
    }
    nvsf_app::languages::import(&host.0.paths, language, source, &host.0.model_cancel)
        .map_err(|e| e.to_string())?;
    Ok(Some(language.into()))
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
        export_language_to(&host, &language, &destination).map(Some)
    })
    .await
}

pub(crate) fn export_language_to(
    host: &Host,
    language: &str,
    destination: &std::path::Path,
) -> Result<String> {
    if host.0.model_cancel.load(Ordering::Acquire) {
        return Err("Language export cancelled".into());
    }
    nvsf_app::languages::export(&host.0.paths, language, destination).map_err(|e| e.to_string())?;
    Ok(display_name(destination))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_file_actions_preserve_packs_on_denial_cancel_and_invalid_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let paths = nvsf_app::Paths::new(Some(directory.path())).unwrap();
        let log = nvsf_app::logging::Log::new(&paths, "nvsf-desktop");
        let host = Host::new(paths, log);
        let root = host.0.paths.models.join("tessdata");
        std::fs::create_dir_all(&root).unwrap();
        let installed = root.join("rus.traineddata");
        let source = directory.path().join("candidate 日本.traineddata");
        let destination = directory.path().join("export.traineddata");
        std::fs::write(&installed, b"preserved pack").unwrap();
        std::fs::write(&source, b"invalid candidate").unwrap();
        assert!(
            import_language_from(&host, "rus", &source, || false)
                .unwrap()
                .is_none()
        );
        host.0.model_cancel.store(true, Ordering::Release);
        assert!(
            import_language_from(&host, "rus", &source, || true)
                .unwrap_err()
                .contains("Job cancelled")
        );
        assert_eq!(
            export_language_to(&host, "rus", &destination).unwrap_err(),
            "Language export cancelled"
        );
        host.0.model_cancel.store(false, Ordering::Release);
        assert!(
            import_language_from(&host, "rus", &source, || true)
                .unwrap_err()
                .contains("size or checksum mismatch")
        );
        assert!(export_language_to(&host, "rus", &destination).is_err());
        assert_eq!(std::fs::read(&installed).unwrap(), b"preserved pack");
        assert_eq!(std::fs::read(&source).unwrap(), b"invalid candidate");
        assert!(!destination.exists());
    }
}
