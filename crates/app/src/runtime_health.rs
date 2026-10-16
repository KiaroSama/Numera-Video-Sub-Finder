use crate::{Paths, Result, fail};
use std::ffi::{CStr, c_char, c_void};

pub(crate) fn tesseract(paths: &Paths) -> Result<String> {
    let library = paths.library(crate::runtimes::TESSERACT_LIBRARY)?;
    // SAFETY: TessVersion has this exact signature in the pinned 5.5.3 C API;
    // its borrowed static string is read while the library owner remains alive.
    let version = unsafe {
        let get = library
            .get::<unsafe extern "C" fn() -> *const c_char>(b"TessVersion\0")
            .map_err(|e| fail(e.to_string()))?;
        version_text(get())?
    };
    if version != "5.5.3" {
        return Err(fail("Tesseract runtime must match packaged version 5.5.3"));
    }
    Ok(version)
}

#[repr(C)]
struct OrtApiBase {
    get_api: unsafe extern "system" fn(u32) -> *const c_void,
    get_version: unsafe extern "system" fn() -> *const c_char,
}

pub(crate) fn onnx(paths: &Paths) -> Result<String> {
    let library = paths.library(crate::runtimes::ONNX_LIBRARY)?;
    // SAFETY: API31's official C header specifies these two function pointers.
    // No inference session/provider is created, and all borrows end before unload.
    let version = unsafe {
        let get = library
            .get::<unsafe extern "system" fn() -> *const OrtApiBase>(b"OrtGetApiBase\0")
            .map_err(|e| fail(e.to_string()))?;
        let base = get()
            .as_ref()
            .ok_or_else(|| fail("ONNX Runtime API base is null"))?;
        if (base.get_api)(31).is_null() {
            return Err(fail("ONNX Runtime does not provide required API31"));
        }
        version_text((base.get_version)())?
    };
    if version != "1.31.0" {
        return Err(fail("ONNX Runtime must match packaged version 1.31.0"));
    }
    Ok(version)
}

unsafe fn version_text(text: *const c_char) -> Result<String> {
    if text.is_null() {
        return Err(fail("Native runtime returned no version"));
    }
    // SAFETY: the verified native API promises a borrowed NUL-terminated version string.
    let text = unsafe { CStr::from_ptr(text) }
        .to_str()
        .map_err(|_| fail("Native runtime version is not UTF-8"))?;
    if text.is_empty() || text.len() > 128 {
        return Err(fail("Native runtime version exceeds limits"));
    }
    Ok(text.into())
}
