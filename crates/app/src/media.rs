use crate::{AppError, Paths, Result, fail};
use image::RgbImage;
use libloading::Library;
use nvsf_core::{MediaInfo, media_abi::*};
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
};

pub const NATIVE_LIBRARY_NAME: &str = if cfg!(windows) {
    "nvsf_media_native.dll"
} else if cfg!(target_os = "macos") {
    "libnvsf_media_native.dylib"
} else {
    "libnvsf_media_native.so"
};

pub struct DecodedFrame {
    pub image: RgbImage,
    pub time_ms: u64,
    pub estimated_duration_ms: u64,
}

/// Loads only the selected runtime root, not PATH. The package/managed-pack admission owner
/// must verify its provenance before loading: loading any native library executes its code.
pub struct MediaRuntime {
    probe: ProbeFn,
    decode: DecodeFn,
    health: MediaHealth,
    _library: Library,
}

impl MediaRuntime {
    pub fn load(paths: &Paths) -> Result<Self> {
        let path = paths.runtime(NATIVE_LIBRARY_NAME)?;
        if !path.is_absolute() || path.to_str().is_none() {
            return Err(fail(
                "Native media library needs an absolute UTF-8 runtime path",
            ));
        }
        let library = paths.library(NATIVE_LIBRARY_NAME)?;
        // SAFETY: these versioned C signatures are shared with the native crate. The Library
        // remains owned by this object for every call; health is checked before frame delivery.
        let (probe, decode, health_fn) = unsafe {
            (
                *library
                    .get::<ProbeFn>(b"nvsf_media_probe_v1\0")
                    .map_err(|e| fail(format!("Missing native media probe ABI: {e}")))?,
                *library
                    .get::<DecodeFn>(b"nvsf_media_decode_v1\0")
                    .map_err(|e| fail(format!("Missing native media decode ABI: {e}")))?,
                *library
                    .get::<HealthFn>(b"nvsf_media_health_v1\0")
                    .map_err(|e| fail(format!("Missing native media health ABI: {e}")))?,
            )
        };
        let mut response = Reply::new(MAX_JSON_BYTES);
        let mut error = Reply::new(MAX_ERROR_BYTES);
        // SAFETY: both context pointers remain live and exclusive during this synchronous call.
        let status = unsafe {
            health_fn(
                Some(receive_bytes),
                response.context(),
                Some(receive_bytes),
                error.context(),
            )
        };
        native_status(status, &mut error)?;
        let health: MediaHealth = serde_json::from_slice(&response.finish()?)?;
        health.validate()?;
        Ok(Self {
            probe,
            decode,
            health,
            _library: library,
        })
    }

    pub fn health(&self) -> &MediaHealth {
        &self.health
    }

    pub fn probe(&self, path: &Path) -> Result<MediaInfo> {
        let request = request_bytes(&ProbeRequest {
            path: input_path(path)?,
        })?;
        let mut response = Reply::new(MAX_JSON_BYTES);
        let mut error = Reply::new(MAX_ERROR_BYTES);
        // SAFETY: request/context owners outlive the call, callbacks copy before returning.
        let status = unsafe {
            (self.probe)(
                request.as_ptr(),
                request.len(),
                Some(receive_bytes),
                response.context(),
                Some(receive_bytes),
                error.context(),
            )
        };
        native_status(status, &mut error)?;
        let info: MediaInfo = serde_json::from_slice(&response.finish()?)?;
        let pixels = u64::from(info.width) * u64::from(info.height);
        if info.width == 0
            || info.height == 0
            || pixels > MAX_FRAME_PIXELS
            || info.time_base_num <= 0
            || info.time_base_den <= 0
        {
            return Err(fail("Native media returned invalid probe metadata"));
        }
        Ok(info)
    }

    pub fn decode<F: FnMut(DecodedFrame) -> Result<bool>>(
        &self,
        path: &Path,
        start_ms: u64,
        end_ms: Option<u64>,
        mut consume: F,
    ) -> Result<()> {
        if end_ms.is_some_and(|end| end <= start_ms) {
            return Err(fail("Media decode end must be after start"));
        }
        let request = request_bytes(&DecodeRequest {
            path: input_path(path)?,
            start_ms,
            end_ms,
        })?;
        let mut state = Frames {
            consume: &mut consume,
            error: None,
            stopped: false,
            last_time: None,
            start_ms,
            end_ms,
        };
        let mut error = Reply::new(MAX_ERROR_BYTES);
        // SAFETY: state and request stay alive/exclusive; no callback may escape this call.
        let status = unsafe {
            (self.decode)(
                request.as_ptr(),
                request.len(),
                Some(receive_frame::<F>),
                (&mut state as *mut Frames<'_, F>).cast(),
                Some(receive_bytes),
                error.context(),
            )
        };
        finish_decode(status, state.error, state.stopped, &mut error)
    }
}

impl MediaRuntime {
    pub fn preview(&self, path: &Path, time_ms: u64) -> Result<RgbImage> {
        let mut result = None;
        self.decode(path, time_ms, None, |frame| {
            result = Some(frame.image);
            Ok(false)
        })?;
        result.ok_or_else(|| fail("No frame at the requested presentation time"))
    }
}

fn input_path(path: &Path) -> Result<String> {
    let path = path
        .to_str()
        .ok_or_else(|| fail("Media input path must be UTF-8"))?;
    if path.is_empty() || path.contains('\0') || path.len() > MAX_JSON_BYTES {
        return Err(fail("Invalid or oversized media input path"));
    }
    Ok(path.into())
}

fn request_bytes(request: &impl serde::Serialize) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(request)?;
    validate_json(&bytes)?;
    Ok(bytes)
}

struct Reply {
    limit: usize,
    bytes: Option<Vec<u8>>,
    error: Option<AppError>,
}
impl Reply {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            bytes: None,
            error: None,
        }
    }
    fn context(&mut self) -> *mut c_void {
        (self as *mut Self).cast()
    }
    fn finish(self) -> Result<Vec<u8>> {
        if let Some(error) = self.error {
            return Err(error);
        }
        self.bytes
            .ok_or_else(|| fail("Native media returned no response"))
    }
}

unsafe extern "C" fn receive_bytes(context: *mut c_void, data: *const u8, len: usize) -> i32 {
    if context.is_null() {
        return ERROR;
    }
    // SAFETY: only synchronous ABI calls receive this pointer to an exclusive live Reply.
    let reply = unsafe { &mut *context.cast::<Reply>() };
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
        if reply.error.is_some()
            || reply.bytes.is_some()
            || data.is_null()
            || len == 0
            || len > reply.limit
        {
            return Err(fail("Invalid, repeated or oversized native media response"));
        }
        // SAFETY: the ABI requires this checked nonnull borrowed buffer to live until return.
        let bytes = unsafe { std::slice::from_raw_parts(data, len) };
        std::str::from_utf8(bytes).map_err(|_| fail("Native media response must be UTF-8"))?;
        reply.bytes = Some(bytes.to_vec());
        Ok(())
    }));
    match result {
        Ok(Ok(())) => CONTINUE,
        Ok(Err(error)) => {
            if reply.error.is_none() {
                reply.error = Some(error);
            }
            ERROR
        }
        Err(payload) => {
            // A panic payload may itself have a panicking destructor; never unwind through C.
            std::mem::forget(payload);
            reply.error = Some(fail("Native media response callback panicked"));
            ERROR
        }
    }
}

fn finish_decode(
    status: i32,
    consumer_error: Option<AppError>,
    stopped: bool,
    error: &mut Reply,
) -> Result<()> {
    if let Some(error) = consumer_error {
        return Err(error);
    }
    if stopped && status == STOP && error.bytes.is_none() && error.error.is_none() {
        return Ok(());
    }
    native_status(status, error)?;
    if stopped {
        return Err(fail("Native media ignored the consumer stop status"));
    }
    Ok(())
}

fn native_status(status: i32, error: &mut Reply) -> Result<()> {
    if let Some(error) = error.error.take() {
        return Err(error);
    }
    if let Some(bytes) = error.bytes.take() {
        let message =
            String::from_utf8(bytes).map_err(|_| fail("Invalid native media error encoding"))?;
        return Err(fail(format!("Native media operation failed: {message}")));
    }
    match status {
        CONTINUE => Ok(()),
        ERROR => Err(fail("Native media operation failed without diagnostic")),
        _ => Err(fail(
            "Native media returned an unexpected completion status",
        )),
    }
}

struct Frames<'a, F> {
    consume: &'a mut F,
    error: Option<AppError>,
    stopped: bool,
    last_time: Option<u64>,
    start_ms: u64,
    end_ms: Option<u64>,
}

unsafe extern "C" fn receive_frame<F: FnMut(DecodedFrame) -> Result<bool>>(
    context: *mut c_void,
    frame: *const MediaFrameV1,
) -> i32 {
    if context.is_null() {
        return ERROR;
    }
    // SAFETY: caller pairs this monomorphized callback with its exclusive Frames<F> owner.
    let state = unsafe { &mut *context.cast::<Frames<'_, F>>() };
    if state.error.is_some() {
        return ERROR;
    }
    if state.stopped {
        return STOP;
    }
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<bool> {
        if frame.is_null() {
            return Err(fail("Native media returned a null frame"));
        }
        // SAFETY: the ABI owns this aligned descriptor until this callback returns.
        let frame = unsafe { &*frame };
        checked_rgb_len(frame.width, frame.height, frame.len)?;
        if frame.rgb.is_null()
            || frame.estimated_duration_ms == 0
            || frame.time_ms < state.start_ms
            || state.end_ms.is_some_and(|end| frame.time_ms >= end)
            || state.last_time.is_some_and(|last| frame.time_ms < last)
        {
            return Err(fail("Native media returned invalid frame data or timing"));
        }
        // SAFETY: checked byte budget/length above; ABI owner lends readable RGB until return.
        let bytes = unsafe { std::slice::from_raw_parts(frame.rgb, frame.len) };
        let image = RgbImage::from_raw(frame.width, frame.height, bytes.to_vec())
            .ok_or_else(|| fail("Invalid native RGB frame"))?;
        state.last_time = Some(frame.time_ms);
        (state.consume)(DecodedFrame {
            image,
            time_ms: frame.time_ms,
            estimated_duration_ms: frame.estimated_duration_ms,
        })
    }));
    match result {
        Ok(Ok(true)) => CONTINUE,
        Ok(Ok(false)) => {
            state.stopped = true;
            STOP
        }
        Ok(Err(error)) => {
            state.error = Some(error);
            ERROR
        }
        Err(payload) => {
            std::mem::forget(payload);
            state.error = Some(fail("Media frame consumer panicked"));
            ERROR
        }
    }
}

#[cfg(test)]
mod callback_tests {
    use super::*;

    // Exercise the actual C ABI callbacks with borrowed storage, not a native-library mock.
    fn deliver<F: FnMut(DecodedFrame) -> Result<bool>>(
        state: &mut Frames<'_, F>,
        frame: &MediaFrameV1,
    ) -> i32 {
        // SAFETY: both descriptor and its RGB buffer live through this synchronous callback.
        unsafe { receive_frame::<F>((state as *mut Frames<'_, F>).cast(), frame) }
    }
    fn descriptor(rgb: &[u8]) -> MediaFrameV1 {
        MediaFrameV1 {
            rgb: rgb.as_ptr(),
            len: rgb.len(),
            width: 2,
            height: 1,
            time_ms: 12,
            estimated_duration_ms: 4,
        }
    }
    fn frames<F>(consume: &mut F) -> Frames<'_, F> {
        Frames {
            consume,
            error: None,
            stopped: false,
            last_time: None,
            start_ms: 0,
            end_ms: None,
        }
    }

    #[test]
    fn frame_is_owned_before_return_and_consumer_stop_is_distinct() {
        let mut rgb = [1, 2, 3, 4, 5, 6];
        let mut saved = None;
        let mut consume = |frame: DecodedFrame| {
            saved = Some(frame);
            Ok(false)
        };
        let mut state = frames(&mut consume);
        assert_eq!(deliver(&mut state, &descriptor(&rgb)), STOP);
        assert!(
            finish_decode(
                STOP,
                state.error,
                state.stopped,
                &mut Reply::new(MAX_ERROR_BYTES)
            )
            .is_ok()
        );
        rgb.fill(0);
        let saved = saved.unwrap();
        assert_eq!(saved.image.as_raw(), &[1, 2, 3, 4, 5, 6]);
        assert_eq!(saved.time_ms, 12);
        assert_eq!(saved.estimated_duration_ms, 4);
        assert!(finish_decode(STOP, None, false, &mut Reply::new(MAX_ERROR_BYTES)).is_err());
        assert!(finish_decode(CONTINUE, None, true, &mut Reply::new(MAX_ERROR_BYTES)).is_err());
    }

    #[test]
    fn original_consumer_error_survives_native_error_and_panic_never_unwinds() {
        let rgb = [0; 6];
        let mut consume = |_: DecodedFrame| Err(AppError::Cancelled);
        let mut state = frames(&mut consume);
        assert_eq!(deliver(&mut state, &descriptor(&rgb)), ERROR);
        let mut native_error = Reply::new(MAX_ERROR_BYTES);
        native_error.bytes = Some(b"consumer rejected a frame".to_vec());
        assert!(matches!(
            finish_decode(ERROR, state.error, false, &mut native_error),
            Err(AppError::Cancelled)
        ));
        let mut consume = |_: DecodedFrame| -> Result<bool> { panic!("controlled callback panic") };
        let mut state = frames(&mut consume);
        assert_eq!(deliver(&mut state, &descriptor(&rgb)), ERROR);
        assert!(
            state
                .error
                .unwrap()
                .to_string()
                .contains("consumer panicked")
        );
    }

    #[test]
    fn invalid_borrowed_buffers_are_rejected_without_delivery_or_reads() {
        let mut calls = 0;
        let mut consume = |_: DecodedFrame| {
            calls += 1;
            Ok(true)
        };
        let mut state = frames(&mut consume);
        let frame = MediaFrameV1 {
            rgb: std::ptr::NonNull::<u8>::dangling().as_ptr(),
            len: 7,
            width: 2,
            height: 1,
            time_ms: 0,
            estimated_duration_ms: 4,
        };
        assert_eq!(deliver(&mut state, &frame), ERROR);
        assert!(state.error.is_some());
        assert_eq!(calls, 0);
        let mut reply = Reply::new(MAX_JSON_BYTES);
        // SAFETY: oversized length is rejected before the deliberately unreadable pointer is used.
        assert_eq!(
            unsafe {
                receive_bytes(
                    reply.context(),
                    std::ptr::NonNull::<u8>::dangling().as_ptr(),
                    MAX_JSON_BYTES + 1,
                )
            },
            ERROR
        );
        assert!(reply.finish().is_err());
    }
}
