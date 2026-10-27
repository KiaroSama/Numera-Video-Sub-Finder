//! Killable hosted-only CoreGraphics/Accessibility observation of the owned production GUI.

use std::{
    ffi::{c_char, c_void},
    ptr,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
type Ref = *const c_void;
const UTF8: u32 = 0x0800_0100;
const MAX_WINDOWS: usize = 64;

#[derive(Clone, Copy)]
struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}
impl Bounds {
    fn valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .into_iter()
            .all(f64::is_finite)
            && self.width > 0.0
            && self.height > 0.0
    }
    fn matches(self, other: Self) -> bool {
        self.valid()
            && other.valid()
            && [
                self.x - other.x,
                self.y - other.y,
                self.width - other.width,
                self.height - other.height,
            ]
            .into_iter()
            .all(|delta| delta.abs() <= 1.0)
    }
}

fn target_pid(pid: u32, own: u32) -> Result<i32> {
    i32::try_from(pid)
        .ok()
        .filter(|id| *id > 1 && pid != own)
        .ok_or_else(|| "Native window target PID is invalid".into())
}
fn bounded_count(count: isize, maximum: usize) -> Result<usize> {
    usize::try_from(count)
        .ok()
        .filter(|count| *count <= maximum)
        .ok_or_else(|| "Native window collection exceeds its bound".into())
}
fn unique_match(count: usize) -> Result<bool> {
    match count {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err("Owned native window identity is ambiguous".into()),
    }
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: Ref);
    fn CFRetain(value: Ref) -> Ref;
    fn CFGetTypeID(value: Ref) -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFArrayGetCount(array: Ref) -> isize;
    fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFDictionaryGetValue(dictionary: Ref, key: Ref) -> Ref;
    fn CFStringGetTypeID() -> usize;
    fn CFStringGetLength(value: Ref) -> isize;
    fn CFStringCreateWithCString(allocator: Ref, bytes: *const c_char, encoding: u32) -> Ref;
    fn CFStringGetCString(value: Ref, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFNumberGetTypeID() -> usize;
    fn CFNumberGetValue(value: Ref, kind: i32, output: *mut c_void) -> u8;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(value: Ref) -> u8;
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(options: u32, relative: u32) -> Ref;
    static kCGWindowOwnerPID: Ref;
    static kCGWindowLayer: Ref;
    static kCGWindowIsOnscreen: Ref;
    static kCGWindowAlpha: Ref;
    static kCGWindowName: Ref;
    static kCGWindowBounds: Ref;
}
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXUIElementCreateApplication(pid: i32) -> Ref;
    fn AXUIElementGetTypeID() -> usize;
    fn AXUIElementGetPid(element: Ref, pid: *mut i32) -> i32;
    fn AXUIElementSetMessagingTimeout(element: Ref, seconds: f32) -> i32;
    fn AXUIElementGetAttributeValueCount(element: Ref, name: Ref, count: *mut isize) -> i32;
    fn AXUIElementCopyAttributeValues(
        element: Ref,
        name: Ref,
        start: isize,
        maximum: isize,
        output: *mut Ref,
    ) -> i32;
    fn AXUIElementCopyAttributeValue(element: Ref, name: Ref, output: *mut Ref) -> i32;
    fn AXUIElementPerformAction(element: Ref, action: Ref) -> i32;
    fn AXValueGetTypeID() -> usize;
    fn AXValueGetType(value: Ref) -> u32;
    fn AXValueGetValue(value: Ref, kind: u32, output: *mut c_void) -> u8;
    fn getpgid(pid: i32) -> i32;
}

struct Owned(Ref);
impl Owned {
    fn new(value: Ref) -> Result<Self> {
        if value.is_null() {
            Err("Native API returned no value".into())
        } else {
            Ok(Self(value))
        }
    }
    fn text(value: &'static std::ffi::CStr) -> Result<Self> {
        // SAFETY: static NUL-terminated UTF-8 string and the default CF allocator.
        Self::new(unsafe { CFStringCreateWithCString(ptr::null(), value.as_ptr(), UTF8) })
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: exactly one Create/Copy/Retain reference belongs to this guard.
        unsafe { CFRelease(self.0) };
    }
}
fn typed(value: Ref, kind: usize) -> Result<Ref> {
    // SAFETY: non-null values are live objects borrowed from an owned CF collection/value.
    if value.is_null() || unsafe { CFGetTypeID(value) } != kind {
        Err("Native API returned an unexpected value type".into())
    } else {
        Ok(value)
    }
}
fn ax_ok(status: i32) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("Native Accessibility API failed with code {status}").into())
    }
}
fn check_time(started: Instant) -> Result<()> {
    if started.elapsed() >= Duration::from_millis(2500) {
        Err("Native window observation exhausted its bounded helper budget".into())
    } else {
        Ok(())
    }
}
fn element(value: Ref, pid: i32, started: Instant) -> Result<()> {
    check_time(started)?;
    // SAFETY: type IDs and timeout/PID outputs use public AX declarations and live CF objects.
    unsafe {
        typed(value, AXUIElementGetTypeID())?;
        ax_ok(AXUIElementSetMessagingTimeout(value, 0.5))?;
        let mut actual = 0;
        ax_ok(AXUIElementGetPid(value, &mut actual))?;
        if actual != pid {
            return Err("Accessibility element is not owned by the target PID".into());
        }
    }
    Ok(())
}
fn attribute(element: Ref, name: &'static std::ffi::CStr, started: Instant) -> Result<Owned> {
    check_time(started)?;
    let name = Owned::text(name)?;
    let mut value = ptr::null();
    // SAFETY: element was validated/timeout-bounded; output is a retained CF value on success.
    ax_ok(unsafe { AXUIElementCopyAttributeValue(element, name.0, &mut value) })?;
    Owned::new(value)
}
fn text(value: Ref) -> Result<String> {
    // SAFETY: verify the concrete CF type before using string APIs; 256 UTF-16 units fit1025 UTF-8 bytes.
    unsafe {
        typed(value, CFStringGetTypeID())?;
        bounded_count(CFStringGetLength(value), 256)?;
        let mut bytes = [0u8; 1025];
        if CFStringGetCString(value, bytes.as_mut_ptr().cast(), bytes.len() as isize, UTF8) == 0 {
            return Err("Native window title is not bounded UTF-8".into());
        }
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .ok_or("Native text is not terminated")?;
        Ok(std::str::from_utf8(&bytes[..end])?.to_owned())
    }
}
fn number(value: Ref) -> Result<f64> {
    let mut number = 0.0f64;
    // SAFETY: checked CFNumber; kCFNumberFloat64Type=6 writes one f64.
    unsafe {
        typed(value, CFNumberGetTypeID())?;
        if CFNumberGetValue(value, 6, (&mut number as *mut f64).cast()) == 0 || !number.is_finite()
        {
            return Err("Native window numeric value is invalid".into());
        }
    }
    Ok(number)
}
fn boolean(value: Ref) -> Result<bool> {
    // SAFETY: checked concrete CFBoolean object.
    unsafe {
        typed(value, CFBooleanGetTypeID())?;
        Ok(CFBooleanGetValue(value) != 0)
    }
}
fn dictionary(value: Ref, key: Ref) -> Result<Ref> {
    // SAFETY: checked live dictionary; returned value stays borrowed while its owner lives.
    unsafe {
        typed(value, CFDictionaryGetTypeID())?;
        Ok(CFDictionaryGetValue(value, key))
    }
}
fn cg_bounds(value: Ref) -> Result<Bounds> {
    let mut values = [0.0; 4];
    for (out, key) in values.iter_mut().zip([c"X", c"Y", c"Width", c"Height"]) {
        let key = Owned::text(key)?;
        *out = number(dictionary(value, key.0)?)?;
    }
    Ok(Bounds {
        x: values[0],
        y: values[1],
        width: values[2],
        height: values[3],
    })
}
fn visible_bounds(pid: i32, started: Instant) -> Result<Vec<Bounds>> {
    // SAFETY: public on-screen-only/exclude-desktop flags1|16, null relative window0; Copy ownership.
    let array = Owned::new(unsafe { CGWindowListCopyWindowInfo(1 | 16, 0) })?;
    let mut bounds = Vec::new();
    // SAFETY: type checked array and each returned dictionary remain alive until the guard drops.
    unsafe {
        typed(array.0, CFArrayGetTypeID())?;
        for index in 0..bounded_count(CFArrayGetCount(array.0), 4096)? {
            check_time(started)?;
            let window = CFArrayGetValueAtIndex(array.0, index as isize);
            if number(dictionary(window, kCGWindowOwnerPID)?)? != f64::from(pid)
                || number(dictionary(window, kCGWindowLayer)?)? != 0.0
            {
                continue;
            }
            if !boolean(dictionary(window, kCGWindowIsOnscreen)?)?
                || number(dictionary(window, kCGWindowAlpha)?)? <= 0.0
            {
                continue;
            }
            let name = dictionary(window, kCGWindowName)?;
            // Quartz names are optional/privacy-redacted; AX supplies the exact title below.
            if !name.is_null() {
                let name = text(name)?;
                if !name.is_empty() && name != crate::production::TITLE {
                    continue;
                }
            }
            let value = cg_bounds(dictionary(window, kCGWindowBounds)?)?;
            if value.valid() {
                bounds.push(value);
            }
            bounded_count(bounds.len() as isize, MAX_WINDOWS)?;
        }
    }
    Ok(bounds)
}
#[repr(C)]
#[derive(Default)]
struct Pair {
    first: f64,
    second: f64,
}
fn ax_pair(value: Ref, kind: u32) -> Result<Pair> {
    let mut pair = Pair::default();
    // SAFETY: CGPoint/CGSize each contain two CGFloat=f64 values on both64-bit desktop targets.
    unsafe {
        typed(value, AXValueGetTypeID())?;
        if AXValueGetType(value) != kind
            || AXValueGetValue(value, kind, (&mut pair as *mut Pair).cast()) == 0
        {
            return Err("Native Accessibility geometry type is invalid".into());
        }
    }
    Ok(pair)
}
fn owned_window(pid: i32, visible: &[Bounds], started: Instant) -> Result<Option<Owned>> {
    // SAFETY: positive exact owned target PID; Create returns one owned AX application reference.
    let app = Owned::new(unsafe { AXUIElementCreateApplication(pid) })?;
    element(app.0, pid, started)?;
    let name = Owned::text(c"AXWindows")?;
    let mut count = 0;
    // SAFETY: validated app/name and initialized count; its messaging timeout is0.5s.
    ax_ok(unsafe { AXUIElementGetAttributeValueCount(app.0, name.0, &mut count) })?;
    let count = bounded_count(count, MAX_WINDOWS)?;
    if count == 0 {
        return Ok(None);
    }
    check_time(started)?;
    let mut windows = ptr::null();
    // SAFETY: cap before copying; Copy result owned, never request the unbounded entire AXWindows value.
    ax_ok(unsafe {
        AXUIElementCopyAttributeValues(app.0, name.0, 0, count as isize, &mut windows)
    })?;
    let windows = Owned::new(windows)?;
    let mut found = None;
    let mut matches = 0;
    // SAFETY: collection and borrowed children remain alive; retain only the unique selected window.
    unsafe {
        typed(windows.0, CFArrayGetTypeID())?;
        for index in 0..bounded_count(CFArrayGetCount(windows.0), MAX_WINDOWS)? {
            let window = CFArrayGetValueAtIndex(windows.0, index as isize);
            element(window, pid, started)?;
            if text(attribute(window, c"AXRole", started)?.0)? != "AXWindow"
                || text(attribute(window, c"AXTitle", started)?.0)? != crate::production::TITLE
            {
                continue;
            }
            if boolean(attribute(window, c"AXMinimized", started)?.0)? {
                continue;
            }
            let position = ax_pair(attribute(window, c"AXPosition", started)?.0, 1)?;
            let size = ax_pair(attribute(window, c"AXSize", started)?.0, 2)?;
            let bounds = Bounds {
                x: position.first,
                y: position.second,
                width: size.first,
                height: size.second,
            };
            let geometry_matches = visible
                .iter()
                .filter(|candidate| candidate.matches(bounds))
                .count();
            if !unique_match(geometry_matches)? {
                continue;
            }
            matches += 1;
            unique_match(matches)?;
            found = Some(Owned::new(CFRetain(window))?);
        }
    }
    Ok(found)
}

pub fn run(pid: u32, close: bool) -> Result<()> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
    {
        return Err("Native macOS production observation requires GitHub-hosted Actions".into());
    }
    let pid = target_pid(pid, std::process::id())?;
    // SAFETY: read-only process-group query; parent retains the owned GUI process identity.
    let group = unsafe { getpgid(pid) };
    if group == -1 {
        let error = std::io::Error::last_os_error();
        if !close && error.raw_os_error() == Some(3) {
            // Still independently query WindowServer after exit, rather than inferring no window.
            if !visible_bounds(pid, Instant::now())?.is_empty() {
                return Err("Owned native window survived target process exit".into());
            }
            return Ok(());
        }
        return Err(error.into());
    }
    if group != pid {
        return Err("Native window target lacks its owned process group".into());
    }
    // SAFETY: read-only trust query; never prompts, requests permission, or changes TCC state.
    if unsafe { AXIsProcessTrusted() } == 0 {
        return Err(
            "Native macOS production observation requires existing Accessibility authorization"
                .into(),
        );
    }
    let started = Instant::now();
    let visible = visible_bounds(pid, started)?;
    let window = if visible.is_empty() {
        None
    } else {
        owned_window(pid, &visible, started)?
    };
    if close {
        let window = window.ok_or("Owned visible native window is unavailable for close")?;
        let button = attribute(window.0, c"AXCloseButton", started)?;
        element(button.0, pid, started)?;
        if text(attribute(button.0, c"AXRole", started)?.0)? != "AXButton"
            || text(attribute(button.0, c"AXSubrole", started)?.0)? != "AXCloseButton"
        {
            return Err("Owned native window close control is invalid".into());
        }
        let action = Owned::text(c"AXPress")?;
        check_time(started)?;
        // SAFETY: exact-PID button from the unique visible/title-matched window, timeout0.5s.
        ax_ok(unsafe { AXUIElementPerformAction(button.0, action.0) })?;
    } else if window.is_some() {
        println!("{}", crate::production::TITLE);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_identity_rejects_unowned_or_unrepresentable_pids() {
        assert_eq!(target_pid(42, 7).unwrap(), 42);
        for pid in [0, 1, 7, i32::MAX as u32 + 1] {
            assert!(target_pid(pid, 7).is_err());
        }
    }

    #[test]
    fn visible_geometry_matches_only_finite_positive_same_window_bounds() {
        let bounds = Bounds {
            x: -800.0,
            y: 30.0,
            width: 720.0,
            height: 640.0,
        };
        assert!(bounds.matches(bounds));
        assert!(bounds.matches(Bounds {
            x: -799.5,
            ..bounds
        }));
        assert!(!bounds.matches(Bounds {
            x: -790.0,
            ..bounds
        }));
        for invalid in [
            Bounds {
                x: f64::NAN,
                ..bounds
            },
            Bounds {
                width: 0.0,
                ..bounds
            },
            Bounds {
                height: f64::INFINITY,
                ..bounds
            },
            Bounds {
                width: -1.0,
                ..bounds
            },
        ] {
            assert!(!invalid.valid());
            assert!(!bounds.matches(invalid));
        }
    }

    #[test]
    fn cf_values_preserve_utf8_and_reject_incorrect_native_types() {
        let value = Owned::text(c"Numera — 日本語").unwrap();
        assert_eq!(text(value.0).unwrap(), "Numera — 日本語");
        assert!(number(value.0).is_err());
        assert!(boolean(value.0).is_err());
        assert!(text(ptr::null()).is_err());
    }

    #[test]
    fn window_cardinality_is_bounded_and_ambiguity_never_passes() {
        assert_eq!(bounded_count(0, 64).unwrap(), 0);
        assert_eq!(bounded_count(64, 64).unwrap(), 64);
        assert!(bounded_count(-1, 64).is_err());
        assert!(bounded_count(65, 64).is_err());
        assert!(!unique_match(0).unwrap());
        assert!(unique_match(1).unwrap());
        assert!(unique_match(2).is_err());
    }
}
