use super::{Event, protocol};
use crate::{Result, fail};
use std::{io::Read, process::ChildStdout};

const LIMIT: usize = protocol::EVENT_LIMIT;

#[derive(Default)]
struct Messages {
    bytes: Vec<u8>,
    scanned: usize,
}

impl Messages {
    fn append(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > LIMIT.saturating_sub(self.bytes.len()) {
            return Err(fail("Worker message exceeds64MiB"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    #[cfg(test)]
    fn empty(&self) -> bool {
        self.bytes.is_empty()
    }

    fn next(&mut self, eof: bool, job_id: &str) -> Result<Option<Event>> {
        let end = self.bytes[self.scanned..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|offset| self.scanned + offset + 1);
        let count = match end {
            Some(count) => count,
            None if eof && !self.bytes.is_empty() => self.bytes.len(),
            None => {
                self.scanned = self.bytes.len();
                return Ok(None);
            }
        };
        let event = protocol::decode(&self.bytes[..count], job_id, LIMIT)?;
        self.bytes.drain(..count);
        self.scanned = 0;
        Ok(Some(event))
    }
}

pub(super) enum Poll {
    Event(Event),
    Progress,
    Pending,
    Ended,
}

/// The supervisor is the sole pipe reader; no reader thread needs joining.
pub(super) struct Output {
    pipe: ChildStdout,
    messages: Messages,
    job_id: String,
    eof: bool,
}

impl Output {
    pub(super) fn new(pipe: ChildStdout, job_id: &str) -> Result<Self> {
        #[cfg(unix)]
        nonblocking(&pipe)?;
        Ok(Self {
            pipe,
            messages: Messages::default(),
            job_id: job_id.into(),
            eof: false,
        })
    }

    pub(super) fn poll(&mut self) -> Result<Poll> {
        if let Some(event) = self.messages.next(self.eof, &self.job_id)? {
            return Ok(Poll::Event(event));
        }
        if self.eof {
            return Ok(Poll::Ended);
        }
        #[cfg(windows)]
        let available = match available(&self.pipe)? {
            Some(0) => return Ok(Poll::Pending),
            Some(count) => count.min(8192),
            None => {
                self.eof = true;
                return self.poll();
            }
        };
        #[cfg(unix)]
        let available: usize = 8192;
        let available = available.min(LIMIT.saturating_sub(self.messages.bytes.len()).max(1));
        let mut bytes = [0u8; 8192];
        match self.pipe.read(&mut bytes[..available]) {
            Ok(0) => {
                self.eof = true;
                self.poll()
            }
            Ok(count) => {
                self.messages.append(&bytes[..count])?;
                Ok(Poll::Progress)
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                Ok(Poll::Pending)
            }
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(unix)]
fn nonblocking(pipe: &ChildStdout) -> Result<()> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn fcntl(fd: i32, command: i32, ...) -> i32;
    }
    #[cfg(target_os = "macos")]
    const NONBLOCK: i32 = 0x4;
    #[cfg(target_os = "linux")]
    const NONBLOCK: i32 = 0x800;
    // SAFETY: owned pipe descriptor; F_GETFL takes no additional argument, F_SETFL
    // takes an int. Preserve all flags and change only this reader's status.
    let flags = unsafe { fcntl(pipe.as_raw_fd(), 3) };
    if flags < 0 || unsafe { fcntl(pipe.as_raw_fd(), 4, flags | NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(windows)]
fn available(pipe: &ChildStdout) -> Result<Option<usize>> {
    use std::{ffi::c_void, os::windows::io::AsRawHandle};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn PeekNamedPipe(
            pipe: *mut c_void,
            buffer: *mut c_void,
            size: u32,
            read: *mut u32,
            available: *mut u32,
            left: *mut u32,
        ) -> i32;
    }
    let mut count = 0;
    // SAFETY: Rust's supported Windows ChildStdout uses an overlapped named-pipe
    // reader. This owner performs no concurrent I/O; read at most observed bytes.
    if unsafe {
        PeekNamedPipe(
            pipe.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut count,
            std::ptr::null_mut(),
        )
    } == 0
    {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::BrokenPipe {
            return Ok(None);
        }
        return Err(error.into());
    }
    Ok(Some(count as usize))
}

#[cfg(test)]
#[path = "worker_output_tests.rs"]
mod tests;
