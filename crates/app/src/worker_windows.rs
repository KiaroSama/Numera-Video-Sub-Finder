use crate::{Result, fail};
use std::{ffi::c_void, io::Read, os::windows::io::AsRawHandle, process::Child};

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

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(job: Handle, class: i32, info: *const c_void, len: u32) -> i32;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn TerminateJobObject(job: Handle, code: u32) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
}

pub(super) struct Job(Handle);

impl Job {
    pub(super) fn attach(child: &Child) -> Result<Self> {
        // SAFETY: null attributes create a noninherited private handle. The repr(C)
        // structures match the Win64 ABI on both supported Windows architectures.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let job = Self(handle);
            let mut limits: Extended = std::mem::zeroed();
            limits.limits.flags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
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

    pub(super) fn terminate(&self) -> Result<()> {
        // SAFETY: this live unnamed job owns only the admitted worker and its descendants.
        if unsafe { TerminateJobObject(self.0, 1) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: this is the sole noninherited handle; closing kills all members even
        // when the supervisor itself is terminated without running Rust destructors.
        unsafe { CloseHandle(self.0) };
    }
}

pub(super) fn await_start() -> Result<()> {
    let mut byte = [0u8; 1];
    std::io::stdin().read_exact(&mut byte)?;
    if byte != *b"S" {
        return Err(fail("Worker owner did not authorize native startup"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "worker_windows_tests.rs"]
mod tests;
