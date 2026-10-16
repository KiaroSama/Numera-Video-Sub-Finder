use crate::{Paths, Result, fail};
use image::RgbImage;
use libloading::Library;
use nvsf_core::Recognition;
use std::ffi::{CStr, CString, c_char, c_int, c_void};

type Handle = *mut c_void;
#[cfg(not(windows))]
type Init = unsafe extern "C" fn(Handle, *const c_char, *const c_char, c_int) -> c_int;
#[cfg(windows)]
type Init = unsafe extern "C" fn(
    Handle,
    *const c_char,
    c_int,
    *const c_char,
    c_int,
    *mut *mut c_char,
    c_int,
    *mut *mut c_char,
    *mut *mut c_char,
    usize,
    c_int,
) -> c_int;
struct Api {
    create: unsafe extern "C" fn() -> Handle,
    delete: unsafe extern "C" fn(Handle),
    init: Init,
    set_image: unsafe extern "C" fn(Handle, *const u8, c_int, c_int, c_int, c_int),
    set_mode: unsafe extern "C" fn(Handle, c_int),
    recognize: unsafe extern "C" fn(Handle, Handle) -> c_int,
    text: unsafe extern "C" fn(Handle) -> *mut c_char,
    free_text: unsafe extern "C" fn(*const c_char),
    confidence: unsafe extern "C" fn(Handle) -> c_int,
    monitor_create: unsafe extern "C" fn() -> Handle,
    monitor_delete: unsafe extern "C" fn(Handle),
    monitor_deadline: unsafe extern "C" fn(Handle, c_int),
}

pub struct Tesseract {
    handle: Handle,
    api: Api,
    _library: Library,
}
impl Tesseract {
    pub fn new(paths: &Paths, language: &str) -> Result<Self> {
        let library = paths.library(crate::runtimes::TESSERACT_LIBRARY)?;
        unsafe fn symbol<T: Copy>(lib: &Library, name: &[u8]) -> Result<T> {
            // SAFETY: caller supplies signatures verified against Tesseract5.5.3 capi.h.
            Ok(*unsafe { lib.get::<T>(name) }.map_err(|e| fail(e.to_string()))?)
        }
        // SAFETY: exact ABI signatures are defined above; library outlives every use.
        let api = unsafe {
            Api {
                create: symbol(&library, b"TessBaseAPICreate\0")?,
                delete: symbol(&library, b"TessBaseAPIDelete\0")?,
                #[cfg(not(windows))]
                init: symbol(&library, b"TessBaseAPIInit2\0")?,
                #[cfg(windows)]
                init: symbol(&library, b"TessBaseAPIInit5\0")?,
                set_image: symbol(&library, b"TessBaseAPISetImage\0")?,
                set_mode: symbol(&library, b"TessBaseAPISetPageSegMode\0")?,
                recognize: symbol(&library, b"TessBaseAPIRecognize\0")?,
                text: symbol(&library, b"TessBaseAPIGetUTF8Text\0")?,
                free_text: symbol(&library, b"TessDeleteText\0")?,
                confidence: symbol(&library, b"TessBaseAPIMeanTextConf\0")?,
                monitor_create: symbol(&library, b"TessMonitorCreate\0")?,
                monitor_delete: symbol(&library, b"TessMonitorDelete\0")?,
                monitor_deadline: symbol(&library, b"TessMonitorSetDeadlineMSecs\0")?,
            }
        };
        let language = nvsf_core::languages::find(language)
            .ok_or_else(|| fail("Unsupported Tesseract language"))?;
        let pack = crate::models::verify_tessdata(paths, &language.code)?;
        #[cfg(windows)]
        let data = {
            use sha2::{Digest, Sha256};
            use std::io::Read;
            if language.bytes == 0 || language.bytes > 32 * 1024 * 1024 {
                return Err(fail("Tesseract model exceeds its memory limit"));
            }
            let mut bytes = Vec::with_capacity(language.bytes as usize);
            std::fs::File::open(pack)?
                .take(language.bytes + 1)
                .read_to_end(&mut bytes)?;
            let hash: String = Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            if bytes.len() as u64 != language.bytes || hash != language.sha256 {
                return Err(fail("Tesseract model changed during initialization"));
            }
            bytes
        };
        #[cfg(not(windows))]
        let data = CString::new(
            pack.parent()
                .and_then(|p| p.to_str())
                .ok_or_else(|| fail("Tesseract data path must be UTF-8"))?,
        )
        .map_err(|_| fail("Invalid Tesseract path"))?;
        let lang = CString::new(language.code.as_str()).map_err(|_| fail("Invalid language"))?;
        // SAFETY: allocate native owner; all string pointers remain alive through initialization.
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err(fail("Tesseract allocation failed"));
        }
        let result = Self {
            handle,
            api,
            _library: library,
        };
        // Official tessdata_best resources contain only the LSTM engine (OEM_LSTM_ONLY=1).
        #[cfg(not(windows))]
        // SAFETY: nonnull owned handle, initialized C strings remain alive through Init2.
        let status = unsafe { (result.api.init)(handle, data.as_ptr(), lang.as_ptr(), 1) };
        #[cfg(windows)]
        let status = {
            let size = c_int::try_from(data.len())
                .map_err(|_| fail("Tesseract model length exceeds its native limit"))?;
            // SAFETY: verified bytes remain alive through Init5's synchronous copy; no native path I/O.
            unsafe {
                (result.api.init)(
                    handle,
                    data.as_ptr().cast(),
                    size,
                    lang.as_ptr(),
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                    0,
                )
            }
        };
        if status != 0 {
            return Err(fail("Tesseract initialization failed"));
        }
        Ok(result)
    }

    pub fn recognize(&mut self, image: &RgbImage, vertical: bool) -> Result<Recognition> {
        if image.width() == 0
            || image.height() == 0
            || image.width() > 16384
            || image.height() > 16384
        {
            return Err(fail("Invalid Tesseract image"));
        }
        // SAFETY: RGB bytes remain alive throughout recognition, checked dimensions/stride fit i32.
        unsafe {
            (self.api.set_mode)(self.handle, if vertical { 5 } else { 7 });
            (self.api.set_image)(
                self.handle,
                image.as_raw().as_ptr(),
                image.width() as c_int,
                image.height() as c_int,
                3,
                image.width() as c_int * 3,
            );
            let monitor = (self.api.monitor_create)();
            if monitor.is_null() {
                return Err(fail("Tesseract monitor allocation failed"));
            }
            (self.api.monitor_deadline)(monitor, 5000);
            let status = (self.api.recognize)(self.handle, monitor);
            (self.api.monitor_delete)(monitor);
            if status != 0 {
                return Err(fail("Tesseract recognition failed or timed out"));
            }
            let text = (self.api.text)(self.handle);
            if text.is_null() {
                return Err(fail("Tesseract returned no text"));
            }
            let decoded = CStr::from_ptr(text).to_str().map(str::to_owned);
            (self.api.free_text)(text);
            let text = decoded
                .map_err(|_| fail("Tesseract returned invalid UTF-8"))?
                .trim()
                .to_string();
            let confidence = (self.api.confidence)(self.handle).clamp(0, 100) as f32 / 100.0;
            Ok(Recognition {
                text,
                confidence: Some(confidence),
                crop: None,
            })
        }
    }
}
impl Drop for Tesseract {
    fn drop(&mut self) {
        // SAFETY: this owner releases the handle once while its library is still loaded.
        unsafe { (self.api.delete)(self.handle) };
    }
}
