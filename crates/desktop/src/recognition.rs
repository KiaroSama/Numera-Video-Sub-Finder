use crate::state::{Host, Result, blocking, lock};
use nvsf_app::native_request::{self, Operation, Response};
use nvsf_core::{Recognition, Rect};

#[tauri::command]
pub async fn suggest_regions(
    host: tauri::State<'_, Host>,
    token: String,
    language: String,
) -> Result<Vec<Rect>> {
    if nvsf_core::languages::find(&language).is_none() {
        return Err("Choose a language from the official catalog".into());
    }
    let host = host.inner().clone();
    blocking(move || {
        let _operation = lock(&host.0.operation)?;
        host.ensure_idle()?;
        let source = host.source(&token)?;
        host.0
            .source_cancel
            .store(false, std::sync::atomic::Ordering::Release);
        let (response, _) = native_request::supervise(
            &host.0.paths,
            &source,
            Operation::Suggest { language },
            &host.0.source_cancel,
        )
        .map_err(|e| e.to_string())?;
        match response {
            Response::Suggest(regions) => Ok(regions),
            _ => Err("Native suggestion returned a different response".into()),
        }
    })
    .await
}

#[tauri::command]
pub async fn recognize_cue(
    host: tauri::State<'_, Host>,
    token: String,
    cue_id: u64,
    language: String,
    region: Rect,
    at_ms: u64,
) -> Result<Recognition> {
    if nvsf_core::languages::find(&language).is_none() {
        return Err("Choose a language from the official catalog".into());
    }
    region.validate().map_err(|e| e.to_string())?;
    let host = host.inner().clone();
    blocking(move || {
        let _operation = lock(&host.0.operation)?;
        host.ensure_idle()?;
        let source = host.source(&token)?;
        let hash = {
            let project = lock(&host.0.project)?;
            let project = project
                .as_ref()
                .ok_or_else(|| "No review project is open".to_string())?;
            if cue_id == 0 || at_ms > 359_999_999_999 {
                return Err("Invalid cue or preview timestamp".into());
            }
            if project
                .media
                .duration_ms
                .is_some_and(|duration| at_ms >= duration)
            {
                return Err("Cue preview time is outside the source duration".into());
            }
            project.source_hash.clone()
        };
        if nvsf_app::storage::hash_file(&source).map_err(|e| e.to_string())? != hash {
            return Err("Linked source does not match the review project's fingerprint".into());
        }
        host.0
            .source_cancel
            .store(false, std::sync::atomic::Ordering::Release);
        let (response, _) = native_request::supervise(
            &host.0.paths,
            &source,
            Operation::Recognize {
                at_ms,
                region,
                language,
            },
            &host.0.source_cancel,
        )
        .map_err(|e| e.to_string())?;
        match response {
            Response::Recognize(text) => Ok(text),
            _ => Err("Native recognition returned a different response".into()),
        }
    })
    .await
}
