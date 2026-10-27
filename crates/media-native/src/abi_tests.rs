use super::*;
use nvsf_core::media_abi::{MAX_JSON_BYTES, MediaHealth};

unsafe extern "C" fn collect(context: *mut c_void, data: *const u8, len: usize) -> i32 {
    if context.is_null() || data.is_null() || len == 0 || len > MAX_JSON_BYTES {
        return ERROR;
    }
    // SAFETY: each test passes an exclusive Vec owner; native buffers remain live in callback.
    unsafe { &mut *context.cast::<Vec<u8>>() }
        .extend_from_slice(unsafe { std::slice::from_raw_parts(data, len) });
    CONTINUE
}

#[test]
fn health_reports_real_versions_and_matching_native_abi() {
    let mut bytes = Vec::<u8>::new();
    // SAFETY: the callback/context remain live for the synchronous call.
    assert_eq!(
        unsafe {
            nvsf_media_health_v1(
                Some(collect),
                (&mut bytes as *mut Vec<u8>).cast(),
                None,
                std::ptr::null_mut(),
            )
        },
        CONTINUE
    );
    let health: MediaHealth = serde_json::from_slice(&bytes).unwrap();
    health.validate().unwrap();
    assert_ne!(health.versions.avformat, 0);
    assert_ne!(health.versions.avcodec, 0);
    assert_ne!(health.versions.avutil, 0);
    assert_ne!(health.versions.swscale, 0);
}

#[test]
fn invalid_request_pointers_and_lengths_fail_before_native_input_access() {
    for (request, len) in [
        (std::ptr::null(), 0),
        (std::ptr::null(), 1),
        (
            std::ptr::NonNull::<u8>::dangling().as_ptr().cast_const(),
            MAX_JSON_BYTES + 1,
        ),
    ] {
        let mut error = Vec::<u8>::new();
        // SAFETY: invalid pointers are deliberately paired with rejected lengths/null checks;
        // none may be dereferenced. The error callback receives a valid owned context.
        let status = unsafe {
            nvsf_media_probe_v1(
                request,
                len,
                Some(collect),
                std::ptr::null_mut(),
                Some(collect),
                (&mut error as *mut Vec<u8>).cast(),
            )
        };
        assert_eq!(status, ERROR);
        assert!(!error.is_empty() && error.len() <= MAX_ERROR_BYTES);
    }
}

#[test]
fn malformed_json_and_utf8_fail_with_bounded_diagnostics() {
    for request in [
        b"not json".as_slice(),
        b"\xff",
        b"{\"path\":\"\"}",
        b"{\"path\":\"x\",\"extra\":1}",
    ] {
        let mut error = Vec::<u8>::new();
        // SAFETY: the live input and error owner outlive this call; probe must not emit success.
        let status = unsafe {
            nvsf_media_probe_v1(
                request.as_ptr(),
                request.len(),
                Some(collect),
                std::ptr::null_mut(),
                Some(collect),
                (&mut error as *mut Vec<u8>).cast(),
            )
        };
        assert_eq!(status, ERROR);
        assert!(!error.is_empty() && error.len() <= MAX_ERROR_BYTES);
        assert!(std::str::from_utf8(&error).is_ok());
    }
}

#[test]
fn native_panic_is_caught_and_error_text_respects_utf8_byte_bound() {
    let mut error = Vec::<u8>::new();
    for formatted in [false, true] {
        error.clear();
        assert_eq!(
            boundary(
                Some(collect),
                (&mut error as *mut Vec<u8>).cast(),
                || -> Result<i32> {
                    if formatted {
                        std::panic::panic_any(String::from("controlled formatted native panic"));
                    }
                    panic!("controlled native boundary panic")
                }
            ),
            ERROR
        );
        assert_eq!(
            std::str::from_utf8(&error).unwrap(),
            "Native media operation panicked"
        );
    }
    error.clear();
    let diagnostic = "a".repeat(MAX_ERROR_BYTES - 1) + "日";
    send_error(
        Some(collect),
        (&mut error as *mut Vec<u8>).cast(),
        &diagnostic,
    );
    assert_eq!(error.len(), 4095);
    assert!(std::str::from_utf8(&error).is_ok());
}

#[test]
fn response_stop_and_error_are_not_native_success() {
    unsafe extern "C" fn stop(_: *mut c_void, _: *const u8, _: usize) -> i32 {
        STOP
    }
    unsafe extern "C" fn error(_: *mut c_void, _: *const u8, _: usize) -> i32 {
        ERROR
    }
    for (callback, expected) in [(stop as JsonCallback, STOP), (error as JsonCallback, ERROR)] {
        // SAFETY: callback ignores its borrowed response and context, never retaining either.
        assert_eq!(
            unsafe {
                nvsf_media_health_v1(
                    Some(callback),
                    std::ptr::null_mut(),
                    None,
                    std::ptr::null_mut(),
                )
            },
            expected
        );
    }
}
