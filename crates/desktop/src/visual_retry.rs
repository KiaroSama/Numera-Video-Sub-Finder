use crate::{
    model_manager,
    state::{Host, Result, blocking, lock},
};
use nvsf_core::Rect;
use std::sync::atomic::Ordering;

fn require_opt_in(enabled: bool) -> Result<()> {
    if enabled {
        Ok(())
    } else {
        Err("Local visual retry requires explicit opt-in; ordinary OCR remains unchanged".into())
    }
}

#[tauri::command]
pub async fn install_visual_retry(host: tauri::State<'_, Host>, enabled: bool) -> Result<()> {
    require_opt_in(enabled)?;
    let host = host.inner().clone();
    let operation = model_manager::begin(&host)?;
    blocking(move || {
        let _operation = operation;
        host.0.log.event(
            "INFO",
            "VISUAL",
            "User explicitly requested separate visual retry model/projector provisioning",
        );
        nvsf_app::assistant::install(&host.0.paths, &host.0.model_cancel).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub fn cancel_visual_retry(host: tauri::State<'_, Host>) -> Result<()> {
    let host = host.inner();
    if !host.0.model_busy.load(Ordering::Acquire) {
        return Err("No optional resource operation is active".into());
    }
    host.0.model_cancel.store(true, Ordering::Release);
    host.0.log.event(
        "INFO",
        "VISUAL",
        "User requested optional operation cancellation",
    );
    Ok(())
}

#[tauri::command]
pub async fn propose_visual_retry(
    host: tauri::State<'_, Host>,
    token: String,
    cue_id: u64,
    at_ms: u64,
    region: Rect,
    enabled: bool,
) -> Result<nvsf_app::assistant::Proposal> {
    require_opt_in(enabled)?;
    region.validate().map_err(|e| e.to_string())?;
    let host = host.inner().clone();
    let operation = model_manager::begin(&host)?;
    blocking(move || {
        let _operation = operation;
        let source = host.source(&token)?;
        let hash = {
            let project = lock(&host.0.project)?;
            let project = project
                .as_ref()
                .ok_or_else(|| "Open a review project first".to_string())?;
            if cue_id == 0
                || at_ms > 359_999_999_999
                || project
                    .media
                    .duration_ms
                    .is_some_and(|duration| at_ms >= duration)
            {
                return Err("Invalid cue/source preview timestamp".into());
            }
            project.source_hash.clone()
        };
        if nvsf_app::storage::hash_file(&source).map_err(|e| e.to_string())? != hash {
            return Err("Linked video does not match the project's source fingerprint".into());
        }
        if host.0.model_cancel.load(Ordering::Acquire) {
            return Err("Visual retry cancelled before inference".into());
        }
        host.0.log.event(
            "INFO",
            "VISUAL",
            "Explicit original-crop proposal requested; current cue remains unchanged",
        );
        nvsf_app::assistant::propose(
            &host.0.paths,
            &source,
            at_ms,
            region,
            true,
            &host.0.model_cancel,
        )
        .map_err(|e| e.to_string())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visual_operations_fail_closed_without_explicit_opt_in() {
        assert!(require_opt_in(false).is_err());
        require_opt_in(true).unwrap();
    }
}
