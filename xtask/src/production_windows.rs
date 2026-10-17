//! Native Windows process ownership and PID-scoped window lifecycle.
use super::*;
use std::{ffi::c_void, os::windows::io::AsRawHandle};
type Handle = *mut c_void;
#[repr(C)]
struct Limits {
    user: i64,
    total: i64,
    flags: u32,
    min: usize,
    max: usize,
    active: u32,
    affinity: usize,
    priority: u32,
    scheduling: u32,
}
#[repr(C)]
struct Io {
    read_ops: u64,
    write_ops: u64,
    other_ops: u64,
    read_bytes: u64,
    write_bytes: u64,
    other_bytes: u64,
}
#[repr(C)]
struct Extended {
    limits: Limits,
    io: Io,
    process_memory: usize,
    job_memory: usize,
    peak_process: usize,
    peak_job: usize,
}
#[repr(C)]
struct Accounting {
    user: i64,
    kernel: i64,
    period_user: i64,
    period_kernel: i64,
    faults: u32,
    total: u32,
    active: u32,
    terminated: u32,
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(job: Handle, class: i32, info: *const c_void, len: u32) -> i32;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn QueryInformationJobObject(
        job: Handle,
        class: i32,
        info: *mut c_void,
        len: u32,
        returned: *mut u32,
    ) -> i32;
    fn TerminateJobObject(job: Handle, code: u32) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn EnumWindows(callback: unsafe extern "system" fn(Handle, isize) -> i32, data: isize) -> i32;
    fn IsWindowVisible(window: Handle) -> i32;
    fn GetWindowThreadProcessId(window: Handle, pid: *mut u32) -> u32;
    fn GetWindowTextW(window: Handle, text: *mut u16, len: i32) -> i32;
    fn PostMessageW(window: Handle, message: u32, wparam: usize, lparam: isize) -> i32;
}
pub struct Job(Handle);
impl Job {
    pub fn attach(child: &Child) -> Result<Self> {
        // SAFETY: null name/attributes select an unnamed private job; repr(C) structures match Win64 Win32 ABI.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let job = Self(handle);
            let mut limits: Extended = std::mem::zeroed();
            limits.limits.flags = 0x2000;
            if SetInformationJobObject(
                handle,
                9,
                &limits as *const _ as *const c_void,
                std::mem::size_of::<Extended>() as u32,
            ) == 0
                || AssignProcessToJobObject(handle, child.as_raw_handle()) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(job)
        }
    }
    pub fn empty(&self) -> Result<bool> {
        // SAFETY: job handle remains live; query writes only this initialized ABI-compatible output buffer.
        unsafe {
            let mut info: Accounting = std::mem::zeroed();
            if QueryInformationJobObject(
                self.0,
                1,
                &mut info as *mut _ as *mut c_void,
                std::mem::size_of::<Accounting>() as u32,
                std::ptr::null_mut(),
            ) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(info.active == 0)
        }
    }
    pub fn terminate(&self) {
        // SAFETY: this unnamed job contains only the process attached by this smoke owner and its descendants.
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.terminate();
        // SAFETY: Job exclusively owns this live handle and closes it exactly once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Search {
    pid: u32,
    window: Option<Handle>,
}
unsafe extern "system" fn visit(window: Handle, data: isize) -> i32 {
    // SAFETY: EnumWindows is synchronous; data points to Search for the whole enumeration and Win32 writes bounded stack buffers.
    unsafe {
        let search = &mut *(data as *mut Search);
        let mut pid = 0;
        GetWindowThreadProcessId(window, &mut pid);
        if pid == search.pid && IsWindowVisible(window) != 0 {
            let mut text = [0u16; 256];
            let length = GetWindowTextW(window, text.as_mut_ptr(), text.len() as i32);
            if length > 0 && String::from_utf16_lossy(&text[..length as usize]) == TITLE {
                search.window = Some(window);
            }
        }
    }
    1
}
pub fn window(pid: u32) -> Result<Option<String>> {
    let mut search = Search { pid, window: None };
    // SAFETY: synchronous callback receives a unique mutable stack Search; no callback reference escapes.
    if unsafe { EnumWindows(visit, &mut search as *mut _ as isize) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(search.window.map(|window| (window as usize).to_string()))
}
pub fn close(pid: u32, window: &str) -> Result<()> {
    let value: usize = window.parse()?;
    let window = value as Handle;
    // SAFETY: verify owner PID immediately before posting pointer-free WM_CLOSE to this exact owned handle, never broadcast.
    unsafe {
        let mut owner = 0;
        GetWindowThreadProcessId(window, &mut owner);
        if owner != pid {
            return Err("Window owner changed before close".into());
        }
        if PostMessageW(window, 0x0010, 0, 0) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}
