use super::*;
use std::ptr::{self, NonNull};

pub(crate) struct MediaSession<'a> {
    handle: NonNull<c_void>,
    frame: libloading::Symbol<'a, SessionFrameFn>,
    close: libloading::Symbol<'a, SessionCloseFn>,
}

impl<'a> MediaSession<'a> {
    pub(crate) fn open(runtime: &'a MediaRuntime, path: &Path) -> Result<Self> {
        let request = request_bytes(&ProbeRequest {
            path: input_path(path)?,
        })?;
        let library = &runtime._library;
        // SAFETY: these C signatures are shared with the native crate. Symbols borrow the library.
        let (open, frame, close) = unsafe {
            (
                library.get::<SessionOpenFn>(b"nvsf_media_session_open\0"),
                library.get::<SessionFrameFn>(b"nvsf_media_session_frame\0"),
                library.get::<SessionCloseFn>(b"nvsf_media_session_close\0"),
            )
        };
        let missing = |error| {
            fail(format!(
                "Native preview session ABI is unavailable: {error}"
            ))
        };
        let (open, frame, close) = (
            open.map_err(missing)?,
            frame.map_err(missing)?,
            close.map_err(missing)?,
        );
        let mut handle = ptr::null_mut();
        let mut error = Reply::new(MAX_ERROR_BYTES);
        // SAFETY: the request, output pointer and error context live through this call.
        let status = unsafe {
            open(
                request.as_ptr(),
                request.len(),
                &mut handle,
                Some(receive_bytes),
                error.context(),
            )
        };
        native_status(status, &mut error)?;
        let handle =
            NonNull::new(handle).ok_or_else(|| fail("Native preview returned no session"))?;
        Ok(Self {
            handle,
            frame,
            close,
        })
    }

    pub(crate) fn frame(&mut self, time_ms: u64, step: i8, max_width: u32) -> Result<DecodedFrame> {
        let mut selected = None;
        let mut error = Reply::new(MAX_ERROR_BYTES);
        let mut consume = |frame| {
            if selected.replace(frame).is_some() {
                return Err(fail("Native preview returned multiple frames"));
            }
            Ok(true)
        };
        let state = &mut Frames {
            consume: &mut consume,
            error: None,
            stopped: false,
            last_time: None,
            start_ms: 0,
            end_ms: None,
        };
        call_frame(state, &mut error, |callback, context, error_context| {
            // SAFETY: the handle is exclusive; callback contexts remain live through the call.
            unsafe {
                (self.frame)(
                    self.handle.as_ptr(),
                    time_ms,
                    step,
                    max_width,
                    callback,
                    context,
                    Some(receive_bytes),
                    error_context,
                )
            }
        })?;
        selected.ok_or_else(|| fail("Native preview returned no frame"))
    }
}

fn call_frame<F: FnMut(DecodedFrame) -> Result<bool>>(
    state: &mut Frames<'_, F>,
    error: &mut Reply,
    call: impl FnOnce(Option<FrameCallback>, *mut c_void, *mut c_void) -> i32,
) -> Result<()> {
    let status = call(
        Some(receive_frame::<F>),
        ptr::from_mut(state).cast(),
        error.context(),
    );
    if let Some(error) = state.error.take() {
        return Err(error);
    }
    native_status(status, error)
}

impl Drop for MediaSession<'_> {
    fn drop(&mut self) {
        // SAFETY: this is the sole owning handle, and the symbol still borrows its library.
        unsafe { (self.close)(self.handle.as_ptr()) };
    }
}
