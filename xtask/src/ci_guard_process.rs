use std::{
    io,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(super) struct Owned {
    pub(super) child: Child,
    boundary: platform::Boundary,
    cleaned: bool,
}

impl Owned {
    pub(super) fn spawn(command: Command) -> io::Result<Self> {
        let (child, boundary) = platform::spawn(command)?;
        Ok(Self {
            child,
            boundary,
            cleaned: false,
        })
    }

    pub(super) fn active(&self) -> io::Result<Vec<u32>> {
        self.boundary.active()
    }

    pub(super) fn cleanup(&mut self, budget: Duration) -> io::Result<()> {
        let started = Instant::now();
        if !self.active()?.is_empty() {
            self.boundary.terminate()?;
        }
        loop {
            if self.child.try_wait()?.is_some() && self.active()?.is_empty() {
                self.cleaned = true;
                return Ok(());
            }
            if started.elapsed() >= budget {
                return Err(io::Error::other(
                    "Owned process/group cleanup could not be verified.",
                ));
            }
            thread::sleep(super::POLL);
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = self.boundary.terminate();
            let _ = self.child.kill();
            let _ = self.child.try_wait();
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::process::CommandExt;

    const ESRCH: i32 = 3;
    const SIGKILL: i32 = 9;
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }

    fn signal(id: u32, signal: i32, group: bool) -> io::Result<bool> {
        let pid = i32::try_from(id).map_err(io::Error::other)?;
        if pid <= 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid owned PID.",
            ));
        }
        // The negative PID addresses only the process group created by process_group(0).
        if unsafe { kill(if group { -pid } else { pid }, signal) } == 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ESRCH) {
            Ok(false)
        } else {
            Err(error)
        }
    }

    pub(super) struct Boundary(u32);

    impl Boundary {
        pub(super) fn active(&self) -> io::Result<Vec<u32>> {
            Ok(if signal(self.0, 0, true)? {
                vec![self.0]
            } else {
                Vec::new()
            })
        }

        pub(super) fn terminate(&self) -> io::Result<()> {
            signal(self.0, SIGKILL, true).map(|_| ())
        }
    }

    pub(super) fn spawn(mut command: Command) -> io::Result<(Child, Boundary)> {
        let child = command
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let boundary = Boundary(child.id());
        Ok((child, boundary))
    }

    pub(in crate::ci_guard) fn absent_group_contract() -> super::super::Result<()> {
        if !signal(i32::MAX as u32, 0, true)? && signal(0, 0, true).is_err() {
            return Ok(());
        }
        Err("Unix group signalling did not distinguish ESRCH from an invalid group ID.".into())
    }

    pub(in crate::ci_guard) fn pid_alive(pid: u32) -> io::Result<bool> {
        signal(pid, 0, false)
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{
        ffi::{OsString, c_void},
        io::{Read, Write},
        mem::size_of,
        os::windows::{
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
            process::CommandExt,
        },
        ptr::null_mut,
    };

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 258;
    const ERROR_INVALID_PARAMETER: i32 = 87;
    const JOB_OBJECT_BASIC_PROCESS_ID_LIST: i32 = 3;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    const MAX_PROCESSES: usize = 4096;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        per_process_time: i64,
        per_job_time: i64,
        flags: u32,
        min_working_set: usize,
        max_working_set: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io_counters: [u64; 6],
        memory_limits: [usize; 4],
    }

    #[repr(C)]
    struct ProcessIds {
        assigned: u32,
        listed: u32,
        ids: [usize; MAX_PROCESSES],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: i32,
            info: *const c_void,
            size: u32,
        ) -> i32;
        fn QueryInformationJobObject(
            job: *mut c_void,
            class: i32,
            info: *mut c_void,
            size: u32,
            returned: *mut u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, child: *mut c_void) -> i32;
        fn TerminateJobObject(job: *mut c_void, code: u32) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn IsProcessInJob(child: *mut c_void, job: *mut c_void, result: *mut i32) -> i32;
        fn WaitForSingleObject(handle: *mut c_void, timeout_ms: u32) -> u32;
    }

    fn checked(code: i32) -> io::Result<()> {
        if code == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub(super) struct Boundary(OwnedHandle);

    impl Boundary {
        fn new() -> io::Result<Self> {
            // A successful call transfers a new kernel handle, closed by OwnedHandle.
            let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(unsafe { OwnedHandle::from_raw_handle(raw) });
            let limits = ExtendedLimits {
                basic: BasicLimits {
                    flags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    ..BasicLimits::default()
                },
                ..ExtendedLimits::default()
            };
            // The aligned C structure matches JOBOBJECT_EXTENDED_LIMIT_INFORMATION.
            checked(unsafe {
                SetInformationJobObject(
                    raw,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    (&raw const limits).cast(),
                    size_of::<ExtendedLimits>() as u32,
                )
            })?;
            Ok(job)
        }

        pub(super) fn active(&self) -> io::Result<Vec<u32>> {
            let mut list = ProcessIds {
                assigned: 0,
                listed: 0,
                ids: [0; MAX_PROCESSES],
            };
            let job = self.0.as_raw_handle();
            // The fixed C buffer is large enough for MAX_PROCESSES; inventory overflow fails closed.
            checked(unsafe {
                QueryInformationJobObject(
                    job,
                    JOB_OBJECT_BASIC_PROCESS_ID_LIST,
                    (&raw mut list).cast(),
                    size_of::<ProcessIds>() as u32,
                    null_mut(),
                )
            })?;
            if list.listed < list.assigned || list.listed as usize > list.ids.len() {
                return Err(io::Error::other(
                    "Owned job process inventory exceeded its bound.",
                ));
            }
            let mut live = Vec::new();
            for id in &list.ids[..list.listed as usize] {
                let pid = u32::try_from(*id).map_err(io::Error::other)?;
                let Some(child) = ProcessHandle::open(pid)? else {
                    continue;
                };
                let mut inside = 0;
                // Job membership rejects a PID reused between the inventory query and OpenProcess.
                checked(unsafe { IsProcessInJob(child.0.as_raw_handle(), job, &mut inside) })?;
                if inside != 0 && !child.wait(Duration::ZERO)? {
                    live.push(pid);
                }
            }
            Ok(live)
        }

        pub(super) fn terminate(&self) -> io::Result<()> {
            // This handle belongs to the unnamed job created for this invocation.
            checked(unsafe { TerminateJobObject(self.0.as_raw_handle(), 124) })
        }
    }

    pub(in crate::ci_guard) struct ProcessHandle(OwnedHandle);

    impl ProcessHandle {
        fn open(pid: u32) -> io::Result<Option<Self>> {
            // A successful call returns a new process handle; a missing PID is an expected race.
            let handle =
                unsafe { OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if handle.is_null() {
                let error = io::Error::last_os_error();
                return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER) {
                    Ok(None)
                } else {
                    Err(error)
                };
            }
            Ok(Some(Self(unsafe { OwnedHandle::from_raw_handle(handle) })))
        }

        pub(in crate::ci_guard) fn wait(&self, timeout: Duration) -> io::Result<bool> {
            let ms = u32::try_from(timeout.as_millis()).map_err(io::Error::other)?;
            // The process handle has SYNCHRONIZE access and remains live for this wait.
            match unsafe { WaitForSingleObject(self.0.as_raw_handle(), ms) } {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT => Ok(false),
                _ => Err(io::Error::last_os_error()),
            }
        }
    }

    pub(in crate::ci_guard) fn retained_handle(pid: u32) -> io::Result<ProcessHandle> {
        ProcessHandle::open(pid)?
            .ok_or_else(|| io::Error::other("Retained-handle child already disappeared."))
    }

    pub(in crate::ci_guard) fn pid_alive(pid: u32) -> io::Result<bool> {
        ProcessHandle::open(pid)?.map_or(Ok(false), |handle| {
            handle.wait(Duration::ZERO).map(|exited| !exited)
        })
    }

    pub(super) fn spawn(command: Command) -> io::Result<(Child, Boundary)> {
        let job = Boundary::new()?;
        let mut launcher = Command::new(std::env::current_exe()?);
        launcher
            .arg("__ci-guard-owned")
            .arg(command.get_program())
            .args(command.get_args())
            .creation_flags(CREATE_NO_WINDOW);
        if let Some(cwd) = command.get_current_dir() {
            launcher.current_dir(cwd);
        }
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                launcher.env(key, value);
            } else {
                launcher.env_remove(key);
            }
        }
        let mut child = launcher
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        // The launcher cannot start its payload until the parent releases its stdin gate.
        let assigned = checked(unsafe {
            AssignProcessToJobObject(job.0.as_raw_handle(), child.as_raw_handle())
        });
        let result = assigned.and_then(|()| {
            child
                .stdin
                .as_mut()
                .ok_or_else(|| io::Error::other("Missing Windows ownership gate."))
                .and_then(|gate| gate.write_all(b"G"))
        });
        if let Err(error) = result {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok((child, job))
    }

    pub(in crate::ci_guard) fn launch(
        mut args: impl Iterator<Item = OsString>,
    ) -> super::super::Result<i32> {
        use std::os::windows::io::{AsHandle, OwnedHandle};

        super::super::hosted("Command execution is restricted to GitHub-hosted Actions runners.")?;
        let executable = args.next().ok_or("Missing owned executable.")?;
        let stdin = io::stdin();
        // File reads only the gate byte; StdinLock would buffer observer protocol bytes.
        let raw_stdin: OwnedHandle = stdin.as_handle().try_clone_to_owned()?;
        let mut gate = std::fs::File::from(raw_stdin);
        let mut token = [0];
        gate.read_exact(&mut token)?;
        if token != [b'G'] {
            return Err("Invalid Windows ownership gate.".into());
        }
        let status = Command::new(executable)
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .status()?;
        Ok(status.code().unwrap_or(1))
    }
}

#[cfg(unix)]
pub(super) use platform::absent_group_contract;
pub(super) use platform::pid_alive;
#[cfg(windows)]
pub(super) use platform::{launch, retained_handle};
