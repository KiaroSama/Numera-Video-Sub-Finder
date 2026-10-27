mod color;
mod decoder;
mod geometry;
mod timeline;

#[cfg(test)]
mod color_tests;
#[cfg(test)]
mod geometry_tests;
#[cfg(test)]
mod timeline_tests;

use nvsf_core::media_abi::*;
use serde::de::DeserializeOwned;
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
};

#[derive(Debug, thiserror::Error)]
enum NativeError {
    #[error(transparent)]
    Core(#[from] nvsf_core::Error),
    #[error("Media decoding failed: {0}")]
    Media(#[from] ffmpeg_next::Error),
    #[error("Invalid media request or metadata: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Message(String),
}
type Result<T> = std::result::Result<T, NativeError>;
fn fail(message: impl Into<String>) -> NativeError {
    NativeError::Message(message.into())
}

/// # Safety
/// Request is readable for `len` bytes; callbacks/context stay live for this synchronous call.
/// Callbacks must honor the shared ABI's borrowed-buffer and no-unwind contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvsf_media_probe_v1(
    request: *const u8,
    len: usize,
    response: Option<JsonCallback>,
    context: *mut c_void,
    error: Option<JsonCallback>,
    error_context: *mut c_void,
) -> i32 {
    boundary(error, error_context, || {
        // SAFETY: request is borrowed from the caller for this call; null/size checked first.
        let request: ProbeRequest = unsafe { read_request(request, len) }?;
        validate_path(&request.path)?;
        let callback = response.ok_or_else(|| fail("Missing media probe callback"))?;
        let info = decoder::probe(Path::new(&request.path))?;
        send_json(&info, callback, context)
    })
}

/// # Safety
/// Request is readable for `len` bytes. All callbacks are synchronous, serial, borrowed-only,
/// and must not unwind. The runtime retains all native decoder/image ownership.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvsf_media_decode_v1(
    request: *const u8,
    len: usize,
    response: Option<FrameCallback>,
    context: *mut c_void,
    error: Option<JsonCallback>,
    error_context: *mut c_void,
) -> i32 {
    boundary(error, error_context, || {
        // SAFETY: read_request validates null/byte bounds before accessing the borrowed input.
        let request: DecodeRequest = unsafe { read_request(request, len) }?;
        validate_path(&request.path)?;
        if request.end_ms.is_some_and(|end| end <= request.start_ms) {
            return Err(fail("Media decode end must be after start"));
        }
        let callback = response.ok_or_else(|| fail("Missing media frame callback"))?;
        let mut stopped = false;
        decoder::decode(
            Path::new(&request.path),
            request.start_ms,
            request.end_ms,
            |frame| {
                checked_rgb_len(
                    frame.image.width(),
                    frame.image.height(),
                    frame.image.as_raw().len(),
                )?;
                let borrowed = MediaFrameV1 {
                    rgb: frame.image.as_raw().as_ptr(),
                    len: frame.image.as_raw().len(),
                    width: frame.image.width(),
                    height: frame.image.height(),
                    time_ms: frame.time_ms,
                    estimated_duration_ms: frame.estimated_duration_ms,
                };
                // SAFETY: descriptor/RGB owners remain live until the synchronous callback returns.
                match unsafe { callback(context, &borrowed) } {
                    CONTINUE => Ok(true),
                    STOP => {
                        stopped = true;
                        Ok(false)
                    }
                    ERROR => Err(fail("Media consumer rejected a frame")),
                    _ => Err(fail("Invalid media consumer callback status")),
                }
            },
        )?;
        Ok(if stopped { STOP } else { CONTINUE })
    })
}

/// # Safety
/// Callbacks/context remain live for this synchronous call and honor the shared ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvsf_media_health_v1(
    response: Option<JsonCallback>,
    context: *mut c_void,
    error: Option<JsonCallback>,
    error_context: *mut c_void,
) -> i32 {
    boundary(error, error_context, || {
        let callback = response.ok_or_else(|| fail("Missing media health callback"))?;
        // SAFETY: FFmpeg's version functions have no arguments and return actual loaded versions.
        let versions = unsafe {
            MediaVersions {
                avcodec: ffmpeg_next::ffi::avcodec_version(),
                avformat: ffmpeg_next::ffi::avformat_version(),
                avutil: ffmpeg_next::ffi::avutil_version(),
                swscale: ffmpeg_next::ffi::swscale_version(),
            }
        };
        let health = MediaHealth {
            abi_version: ABI_VERSION,
            frame_size: std::mem::size_of::<MediaFrameV1>() as u32,
            pointer_width: usize::BITS,
            architecture: std::env::consts::ARCH.into(),
            plugin_version: env!("CARGO_PKG_VERSION").into(),
            source_sha: nvsf_core::BUILD_SOURCE_SHA.map(str::to_owned),
            versions,
            compiled_majors: MediaVersions {
                avcodec: ffmpeg_next::ffi::LIBAVCODEC_VERSION_MAJOR as u32,
                avformat: ffmpeg_next::ffi::LIBAVFORMAT_VERSION_MAJOR as u32,
                avutil: ffmpeg_next::ffi::LIBAVUTIL_VERSION_MAJOR as u32,
                swscale: ffmpeg_next::ffi::LIBSWSCALE_VERSION_MAJOR as u32,
            },
        };
        send_json(&health, callback, context)
    })
}

unsafe fn read_request<T: DeserializeOwned>(request: *const u8, len: usize) -> Result<T> {
    if request.is_null() || len == 0 || len > MAX_JSON_BYTES {
        return Err(fail("Invalid media request pointer or byte length"));
    }
    // SAFETY: caller guarantees readable bytes; null and bounded length are checked above.
    let bytes = unsafe { std::slice::from_raw_parts(request, len) };
    validate_json(bytes)?;
    Ok(serde_json::from_slice(bytes)?)
}

fn validate_path(path: &str) -> Result<()> {
    if path.is_empty() || path.contains('\0') {
        return Err(fail("Invalid media input path"));
    }
    Ok(())
}

fn send_json(
    value: &impl serde::Serialize,
    callback: JsonCallback,
    context: *mut c_void,
) -> Result<i32> {
    let bytes = serde_json::to_vec(value)?;
    validate_json(&bytes)?;
    // SAFETY: the entrypoint caller provides a valid callback; these borrowed bytes live to return.
    match unsafe { callback(context, bytes.as_ptr(), bytes.len()) } {
        CONTINUE => Ok(CONTINUE),
        STOP => Ok(STOP),
        ERROR => Err(fail("Media response consumer rejected metadata")),
        _ => Err(fail("Invalid media response callback status")),
    }
}

fn boundary(
    error: Option<JsonCallback>,
    context: *mut c_void,
    action: impl FnOnce() -> Result<i32>,
) -> i32 {
    match catch_unwind(AssertUnwindSafe(|| match action() {
        Ok(status) => status,
        Err(failure) => {
            send_error(error, context, &failure.to_string());
            ERROR
        }
    })) {
        Ok(status) => status,
        Err(payload) => {
            // A hostile panic payload's Drop can panic too; it must never unwind through C.
            discard_panic_payload(payload);
            send_error(error, context, "Native media operation panicked");
            ERROR
        }
    }
}

fn send_error(callback: Option<JsonCallback>, context: *mut c_void, message: &str) {
    if let Some(callback) = callback {
        let mut end = message.len().min(MAX_ERROR_BYTES);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        let bytes = &message.as_bytes()[..end];
        // SAFETY: the ABI requires a valid no-unwind callback. Diagnostic bytes live to return.
        unsafe { callback(context, bytes.as_ptr(), bytes.len()) };
    }
}

#[cfg(test)]
mod abi_tests;
