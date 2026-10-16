//! Versioned synchronous C ABI. Buffers are borrowed only until their callback returns.
//! The caller owns request/context memory; the runtime owns response/frame memory. Callbacks
//! run serially on the calling thread, never after return, and must not unwind or retain pointers.
//! Probe/health emit exactly one UTF-8 JSON response (at most 1 MiB); the optional error
//! callback instead borrows plain UTF-8 text (at most 4096 bytes). Status 0 means complete/
//! continue, 1 means consumer stop, -1 means failure. No callback may unwind across C.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::ffi::c_void;

pub const ABI_VERSION: u32 = 1;
pub const MAX_JSON_BYTES: usize = 1_048_576;
pub const MAX_ERROR_BYTES: usize = 4096;
pub const MAX_FRAME_PIXELS: u64 = 33_177_600;
pub const CONTINUE: i32 = 0;
pub const STOP: i32 = 1;
pub const ERROR: i32 = -1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MediaFrameV1 {
    pub rgb: *const u8,
    pub len: usize,
    pub width: u32,
    pub height: u32,
    pub time_ms: u64,
    pub estimated_duration_ms: u64,
}

pub type JsonCallback = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> i32;
pub type FrameCallback = unsafe extern "C" fn(*mut c_void, *const MediaFrameV1) -> i32;
pub type ProbeFn = unsafe extern "C" fn(
    *const u8,
    usize,
    Option<JsonCallback>,
    *mut c_void,
    Option<JsonCallback>,
    *mut c_void,
) -> i32;
pub type DecodeFn = unsafe extern "C" fn(
    *const u8,
    usize,
    Option<FrameCallback>,
    *mut c_void,
    Option<JsonCallback>,
    *mut c_void,
) -> i32;
pub type HealthFn = unsafe extern "C" fn(
    Option<JsonCallback>,
    *mut c_void,
    Option<JsonCallback>,
    *mut c_void,
) -> i32;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeRequest {
    pub path: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecodeRequest {
    pub path: String,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaVersions {
    pub avcodec: u32,
    pub avformat: u32,
    pub avutil: u32,
    pub swscale: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaHealth {
    pub abi_version: u32,
    pub frame_size: u32,
    pub pointer_width: u32,
    pub architecture: String,
    pub plugin_version: String,
    pub source_sha: Option<String>,
    pub versions: MediaVersions,
    pub compiled_majors: MediaVersions,
}

impl MediaHealth {
    pub fn validate(&self) -> Result<()> {
        if self.abi_version != ABI_VERSION
            || self.frame_size as usize != std::mem::size_of::<MediaFrameV1>()
            || self.pointer_width != usize::BITS
            || self.architecture != std::env::consts::ARCH
            || self.plugin_version != env!("CARGO_PKG_VERSION")
            || self.source_sha.as_deref() != crate::BUILD_SOURCE_SHA
        {
            return Err(Error::Invalid(
                "Incompatible native media ABI, target or version".into(),
            ));
        }
        let actual = &self.versions;
        let compiled = &self.compiled_majors;
        for (version, major) in [
            (actual.avcodec, compiled.avcodec),
            (actual.avformat, compiled.avformat),
            (actual.avutil, compiled.avutil),
            (actual.swscale, compiled.swscale),
        ] {
            if major == 0 || major > 255 || version >> 16 != major {
                return Err(Error::Invalid(
                    "Incompatible native FFmpeg library version".into(),
                ));
            }
        }
        Ok(())
    }
}

pub fn validate_json(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || bytes.len() > MAX_JSON_BYTES {
        return Err(Error::Invalid(
            "Native media JSON exceeds byte bounds".into(),
        ));
    }
    std::str::from_utf8(bytes)
        .map_err(|_| Error::Invalid("Native media JSON must be UTF-8".into()))?;
    Ok(())
}

pub fn checked_rgb_len(width: u32, height: u32, len: usize) -> Result<usize> {
    let pixels = u64::from(width) * u64::from(height);
    let expected = pixels.checked_mul(3).and_then(|n| usize::try_from(n).ok());
    if width == 0 || height == 0 || pixels > MAX_FRAME_PIXELS || expected != Some(len) {
        return Err(Error::Invalid(
            "Invalid native RGB frame dimensions or byte length".into(),
        ));
    }
    Ok(len)
}
