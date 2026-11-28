use crate::state::{Host, Result, lock};
use nvsf_app::{
    native_request::{self, Operation, Response},
    preview_daemon::{Packet, PreviewDaemon},
};
use std::{path::Path, sync::atomic::Ordering};

#[tauri::command]
pub async fn preview_transport(
    host: tauri::State<'_, Host>,
    token: String,
    time_ms: u64,
    step: i8,
    fast: Option<bool>,
) -> Result<tauri::ipc::Response> {
    crate::files::with_source_preview(host.inner(), token, move |host, path| {
        let packet = frame(host, path, time_ms, step, fast.unwrap_or(false))?;
        Ok(tauri::ipc::Response::new(packet.into_ipc()))
    })
    .await
}

fn frame(host: &Host, path: &Path, time_ms: u64, step: i8, fast: bool) -> Result<Packet> {
    if !matches!(step, -1..=2) {
        return Err("Preview step must be -1, 0, 1 or 2".into());
    }
    if time_ms > 359_999_999_999 {
        return Err("Invalid preview timestamp".into());
    }
    let mut pool = lock(&host.0.preview_daemon)?;
    let cancel = &host.0.source_cancel;
    // Lossless requests have the existing isolated fallback; motion can respawn once.
    for _ in 0..if fast { 2 } else { 1 } {
        if cancel.load(Ordering::Acquire) {
            return Err("Preview was cancelled".into());
        }
        let result = (|| -> nvsf_app::Result<Packet> {
            if pool.as_ref().is_none_or(|daemon| !daemon.matches(path)) {
                pool.take();
                *pool = Some(PreviewDaemon::spawn(&host.0.paths, path)?);
            }
            pool.as_mut()
                .ok_or_else(|| nvsf_app::fail("Preview daemon unavailable"))?
                .request(time_ms, step, fast, cancel)
        })();
        match result {
            Ok(packet) => return Ok(packet),
            Err(error) => {
                pool.take();
                if cancel.load(Ordering::Acquire) || matches!(error, nvsf_app::AppError::Cancelled)
                {
                    return Err("Preview was cancelled".into());
                }
                host.0.log.event(
                    "WARNING",
                    "PREVIEW",
                    "Preview worker failed; retiring the session",
                );
            }
        }
    }
    drop(pool);
    if fast {
        return Err("Playback preview worker stopped; retry from the current position".into());
    }
    let (response, png) = native_request::supervise(
        &host.0.paths,
        path,
        Operation::TransportFrame {
            at_ms: time_ms,
            step: if step == 2 { 0 } else { step },
        },
        cancel,
    )
    .map_err(|error| error.to_string())?;
    let Response::TransportFrame {
        time_ms,
        estimated_duration_ms,
        ..
    } = response
    else {
        return Err("Native source preview returned a different response".into());
    };
    Ok(Packet::new(time_ms, estimated_duration_ms, false, png))
}
