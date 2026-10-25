use crate::{Result, fail};
#[cfg(windows)]
use std::io::Write;
use std::{
    process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

pub(crate) struct Process {
    child: Child,
    _lease: Option<ChildStdin>,
    #[cfg(windows)]
    job: Option<super::windows::Job>,
    reaped: bool,
}

impl Process {
    pub(crate) fn spawn(command: &mut Command) -> Result<Self> {
        command.stdin(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut owned = Self {
            child: command.spawn()?,
            _lease: None,
            #[cfg(windows)]
            job: None,
            reaped: false,
        };
        #[cfg(windows)]
        {
            owned.job = Some(super::windows::Job::attach(&owned.child)?);
        }
        let lease = owned
            .child
            .stdin
            .take()
            .ok_or_else(|| fail("Worker parent-lifetime pipe unavailable"))?;
        #[cfg(windows)]
        let lease = {
            let mut lease = lease;
            lease.write_all(b"S")?;
            lease
        };
        // Keep outside Child: wait() otherwise closes the parent's lifetime writer.
        owned._lease = Some(lease);
        Ok(owned)
    }

    pub(crate) fn id(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn stdout(&mut self) -> Result<ChildStdout> {
        self.child
            .stdout
            .take()
            .ok_or_else(|| fail("Worker progress stream unavailable"))
    }

    pub(crate) fn exited(&mut self) -> Result<bool> {
        #[cfg(windows)]
        return Ok(self.child.try_wait()?.is_some());
        #[cfg(unix)]
        {
            // WNOWAIT retains this exact child PID until group cleanup, preventing PGID reuse.
            #[repr(C, align(16))]
            struct Info([i32; 32]);
            let mut info = Info([0; 32]);
            #[cfg(target_os = "linux")]
            const OPTIONS: i32 = 4 | 1 | 0x01000000;
            #[cfg(target_os = "macos")]
            const OPTIONS: i32 = 4 | 1 | 0x20;
            // SAFETY: supported native64 siginfo_t fits this aligned 128-byte storage;
            // waitid writes synchronously and its first c_int is si_signo on both OSes.
            let status = unsafe { waitid(1, self.id(), info.0.as_mut_ptr().cast(), OPTIONS) };
            if status != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(info.0[0] != 0)
        }
    }

    pub(crate) fn signal_tree(&mut self) -> Result<()> {
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.terminate()?;
        } else {
            self.child.kill()?;
        }
        #[cfg(unix)]
        {
            // SAFETY: the unreaped child still reserves this isolated group's identity.
            if unsafe { kill(-(self.id() as i32), 9) } != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(3) {
                    // ESRCH: group already empty.
                    return Err(error.into());
                }
            }
        }
        Ok(())
    }

    pub(crate) fn finish(&mut self) -> Result<ExitStatus> {
        self.signal_tree()?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.exited()? {
            if Instant::now() >= deadline {
                return Err(fail("Owned worker did not exit within cleanup deadline"));
            }
            std::thread::park_timeout(Duration::from_millis(10));
        }
        let status = self.child.wait()?;
        self.reaped = true;
        self._lease.take();
        #[cfg(windows)]
        self.job.take();
        Ok(status)
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.finish();
        }
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn waitid(kind: u32, id: u32, info: *mut std::ffi::c_void, options: i32) -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
}

pub(crate) fn executable() -> Result<std::path::PathBuf> {
    let current = std::env::current_exe()?;
    let worker = if current.file_stem().is_some_and(|s| s == "nvsf") {
        current
    } else {
        current
            .parent()
            .ok_or_else(|| fail("Missing executable root"))?
            .join(if cfg!(windows) { "nvsf.exe" } else { "nvsf" })
    };
    if !worker.is_file() {
        return Err(fail(
            "Packaged nvsf worker missing; reinstall matching package",
        ));
    }
    Ok(worker)
}
