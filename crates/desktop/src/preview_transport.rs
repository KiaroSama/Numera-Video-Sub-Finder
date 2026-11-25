use crate::state::{Host, Result};
use nvsf_app::native_request::{self, Operation, Response};

#[tauri::command]
pub async fn preview_transport(
    host: tauri::State<'_, Host>,
    token: String,
    time_ms: u64,
    step: i8,
) -> Result<tauri::ipc::Response> {
    crate::files::with_source_preview(host.inner(), token, move |host, path| {
        let (response, png) = native_request::supervise(
            &host.0.paths,
            path,
            Operation::TransportFrame {
                at_ms: time_ms,
                step,
            },
            &host.0.source_cancel,
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
        // The PNG remains binary; metadata does not expand an 8MiB image into JSON integers.
        let mut packet = Vec::with_capacity(24 + png.len());
        packet.extend_from_slice(b"NVSFPRV1");
        packet.extend_from_slice(&time_ms.to_le_bytes());
        packet.extend_from_slice(&estimated_duration_ms.to_le_bytes());
        packet.extend_from_slice(&png);
        Ok(tauri::ipc::Response::new(packet))
    })
    .await
}
