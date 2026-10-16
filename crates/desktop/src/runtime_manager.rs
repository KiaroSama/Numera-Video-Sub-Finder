use crate::{
    model_manager::begin,
    state::{Host, Result, blocking, lock},
};
use std::sync::atomic::Ordering;

#[tauri::command]
pub async fn repair_runtimes(host: tauri::State<'_, Host>, consent: bool) -> Result<String> {
    if !consent {
        return Err("Explicit app-local dependency repair approval required".into());
    }
    let host = host.inner().clone();
    let operation = begin(&host)?;
    blocking(move || {
        let _operation = operation;
        {
        let _guard = lock(&host.0.operation)?;
        nvsf_core::maintenance::authorize_install(
            consent,
            host.0.source_busy.load(Ordering::Acquire) || host.0.closing.load(Ordering::Acquire),
            host.0.unsaved.load(Ordering::Acquire),
            true,
        ).map_err(|e| e.to_string())?;
        }
        let diagnostics = nvsf_app::runtimes::diagnose(&host.0.paths);
        if diagnostics.iter().all(|row| row.state == "verified") {
            return Ok("Existing native APIs and versions verified; no dependency payload downloaded.".into());
        }
        host.0.log.event("INFO", "RUNTIMES", "Explicit app-local native dependency repair requested");
        let result = (|| {
            let (manifest, signature, base) = nvsf_app::runtimes::fetch().map_err(|e| e.to_string())?;
            nvsf_app::runtimes::repair(&host.0.paths, &manifest, &signature, &base, consent, &host.0.model_cancel)
                .map_err(|e| e.to_string())?;
            Ok("Signed native dependencies installed in application data. Close and reopen the application before extracting; system software and saved projects were not changed.".into())
        })();
        host.0.log.event(if result.is_ok() { "INFO" } else { "WARNING" }, "RUNTIMES", "Dependency repair stopped; verified selection and restart determine activation");
        result
    }).await
}
