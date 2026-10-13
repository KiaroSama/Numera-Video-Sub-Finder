use crate::state::{Host, Result, blocking, display_name, lock};
use std::sync::atomic::Ordering;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

pub(crate) struct Operation(Host);
impl Drop for Operation {
    fn drop(&mut self) {
        self.0.0.model_busy.store(false, Ordering::Release);
    }
}

pub(crate) fn begin(host: &Host) -> Result<Operation> {
    let _operation = lock(&host.0.operation)?;
    host.ensure_idle()?;
    host.0.model_cancel.store(false, Ordering::Release);
    if host.0.model_busy.swap(true, Ordering::AcqRel) {
        return Err("A model operation is already active".into());
    }
    Ok(Operation(host.clone()))
}

fn model_id(id: &str) -> Result<()> {
    if id.len() > 64
        || !nvsf_app::models::specs()
            .map_err(|e| e.to_string())?
            .iter()
            .any(|m| m.id == id)
    {
        return Err("Select an admitted model identifier from the catalog".into());
    }
    Ok(())
}

#[tauri::command]
pub fn cancel_model_install(host: tauri::State<'_, Host>) -> Result<()> {
    let host = host.inner();
    if !host.0.model_busy.load(Ordering::Acquire) {
        return Err("No model operation is active".into());
    }
    host.0.model_cancel.store(true, Ordering::Release);
    host.0
        .log
        .event("INFO", "MODELS", "User requested resource cancellation");
    Ok(())
}

#[tauri::command]
pub async fn install_models(host: tauri::State<'_, Host>, selected: Option<String>) -> Result<()> {
    if let Some(id) = &selected {
        model_id(id)?;
    }
    let host = host.inner().clone();
    let operation = begin(&host)?;
    blocking(move || {
        let _operation = operation;
        host.0.log.event(
            "INFO",
            "MODELS",
            "Explicit admitted model provisioning requested",
        );
        let result =
            nvsf_app::models::install(&host.0.paths, selected.as_deref(), &host.0.model_cancel)
                .map_err(|e| e.to_string());
        host.0.log.event(
            if result.is_ok() { "INFO" } else { "WARNING" },
            "MODELS",
            "Model provisioning stopped; catalog verification is authoritative",
        );
        result
    })
    .await
}

#[tauri::command]
pub async fn import_model(
    app: tauri::AppHandle,
    host: tauri::State<'_, Host>,
    id: String,
) -> Result<Option<String>> {
    model_id(&id)?;
    let host = host.inner().clone();
    let operation = begin(&host)?;
    blocking(move ||{
        let _operation=operation;
        let Some(file)=app.dialog().file().set_title("Import checksummed ONNX model").add_filter("ONNX model",&["onnx"]).blocking_pick_file()else{return Ok(None);};
        let source=file.into_path().map_err(|e|e.to_string())?.canonicalize().map_err(|e|e.to_string())?;
        if !source.is_file(){return Err("Select a regular ONNX model file".into());}
        if host.0.paths.models.join(format!("{id}.onnx")).exists()
            && !app.dialog().message("Replace this installed model with the selected checksummed artifact? Existing jobs are stopped and invalid imports preserve the current model.")
                .title("Replace installed model").buttons(MessageDialogButtons::OkCancelCustom("Verify and replace".into(),"Cancel".into())).blocking_show(){return Ok(None);}
        if host.0.model_cancel.load(Ordering::Acquire){return Err("Model import cancelled before activation".into());}
        nvsf_app::models::import_model(&host.0.paths,&id,&source).map_err(|e|e.to_string())?;
        host.0.log.event("INFO","MODELS","Explicit model artifact imported after integrity checks");
        Ok(Some(id))
    }).await
}

#[tauri::command]
pub async fn export_model(
    app: tauri::AppHandle,
    host: tauri::State<'_, Host>,
    id: String,
) -> Result<Option<String>> {
    model_id(&id)?;
    let host = host.inner().clone();
    let operation = begin(&host)?;
    blocking(move || {
        let _operation = operation;
        let Some(file) = app
            .dialog()
            .file()
            .set_title("Export verified ONNX model")
            .add_filter("ONNX model", &["onnx"])
            .set_file_name(format!("{id}.onnx"))
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let destination = file.into_path().map_err(|e| e.to_string())?;
        if destination.is_symlink() {
            return Err("Choose a regular model export destination".into());
        }
        if host.0.model_cancel.load(Ordering::Acquire) {
            return Err("Model export cancelled".into());
        }
        nvsf_app::models::export_model(&host.0.paths, &id, &destination)
            .map_err(|e| e.to_string())?;
        host.0.log.event(
            "INFO",
            "MODELS",
            "Verified artifact exported without overwriting existing files",
        );
        Ok(Some(display_name(&destination)))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_identity_is_not_a_filesystem_path() {
        for id in [
            "../model",
            "C:\\model.onnx",
            "https://example.com/model",
            "",
            "fake-success",
        ] {
            assert!(model_id(id).is_err());
        }
        for spec in nvsf_app::models::specs().unwrap() {
            model_id(&spec.id).unwrap();
        }
    }
}
