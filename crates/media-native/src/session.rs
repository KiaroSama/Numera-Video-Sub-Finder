use super::{boundary, decoder::Decoder, fail, read_request, validate_path};
use nvsf_core::media_abi::*;
use std::{ffi::c_void, panic::catch_unwind, path::Path};

/// # Safety
/// The request is readable for `len` bytes. `session` points to writable pointer storage.
/// Callbacks and their contexts obey the shared synchronous ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvsf_media_session_open(
    request: *const u8,
    len: usize,
    session: *mut *mut c_void,
    error: Option<JsonCallback>,
    error_context: *mut c_void,
) -> i32 {
    boundary(error, error_context, || {
        if session.is_null() {
            return Err(fail("Missing preview session output"));
        }
        // SAFETY: the caller owns readable request bytes and writable output for this call.
        let request: ProbeRequest = unsafe { read_request(request, len) }?;
        validate_path(&request.path)?;
        let decoder = Decoder::open(Path::new(&request.path))?;
        // SAFETY: transfer sole ownership to the caller, which closes it through this ABI.
        unsafe { session.write(Box::into_raw(Box::new(decoder)).cast()) };
        Ok(CONTINUE)
    })
}

/// # Safety
/// `session` is a live handle from `nvsf_media_session_open`, exclusively borrowed on its
/// opening thread. Callbacks and contexts obey the shared synchronous ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvsf_media_session_frame(
    session: *mut c_void,
    at_ms: u64,
    step: i8,
    max_width: u32,
    callback: Option<FrameCallback>,
    context: *mut c_void,
    error: Option<JsonCallback>,
    error_context: *mut c_void,
) -> i32 {
    boundary(error, error_context, || {
        let callback = callback.ok_or_else(|| fail("Missing preview frame callback"))?;
        // SAFETY: the caller guarantees a live, exclusive handle (or null).
        let decoder = unsafe { session.cast::<Decoder>().as_mut() }
            .ok_or_else(|| fail("Missing preview session"))?;
        let frame = decoder.preview(at_ms, step, max_width)?;
        let borrowed = MediaFrameV1 {
            rgb: frame.image.as_raw().as_ptr(),
            len: frame.image.as_raw().len(),
            width: frame.image.width(),
            height: frame.image.height(),
            time_ms: frame.time_ms,
            estimated_duration_ms: frame.estimated_duration_ms,
        };
        // SAFETY: both the descriptor and its RGB pixels live until the callback returns.
        match unsafe { callback(context, &borrowed) } {
            CONTINUE => Ok(CONTINUE),
            _ => Err(fail("Preview consumer rejected the frame")),
        }
    })
}

/// # Safety
/// A non-null handle was returned by `nvsf_media_session_open` and has not been closed.
/// This call consumes it on its opening thread. No other calls may use it concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvsf_media_session_close(session: *mut c_void) {
    if !session.is_null()
        && let Err(payload) = catch_unwind(|| {
            // SAFETY: this call consumes the one owning pointer returned by Box::into_raw.
            drop(unsafe { Box::from_raw(session.cast::<Decoder>()) });
        })
    {
        discard_panic_payload(payload);
    }
}
