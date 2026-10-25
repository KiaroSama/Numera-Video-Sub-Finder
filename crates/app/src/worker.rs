use crate::extraction::Control;
use crate::{AppError, Paths, Result, fail, storage::write_atomic};
use nvsf_core::{ExtractConfig, Progress, Project};
use serde::{Deserialize, Serialize};
#[path = "worker_output.rs"]
mod output;
#[path = "worker_process.rs"]
pub(crate) mod process;
#[cfg(windows)]
#[path = "worker_windows.rs"]
mod windows;

use std::{
    path::Path,
    process::{Command, Stdio},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Event {
    Progress(Progress),
    Completed(Box<Project>),
    Failed(String),
}

pub fn supervise(
    config: ExtractConfig,
    control: &Control,
    mut callback: impl FnMut(Progress),
) -> Result<Project> {
    config.validate()?;
    let paths = Paths::new(config.data_root.as_deref().map(Path::new))?;
    let scratch = tempfile::tempdir_in(&paths.cache)?;
    let request = scratch.path().join("request.json");
    write_atomic(&request, &serde_json::to_vec(&config)?, false)?;
    let mut command = Command::new(process::executable()?);
    command
        .arg("worker")
        .arg("--request")
        .arg(&request)
        .arg("--control")
        .arg(scratch.path())
        .env("TEMP", scratch.path())
        .env("TMP", scratch.path())
        .env("TMPDIR", scratch.path())
        .env("OMP_NUM_THREADS", config.threads.to_string())
        .env("NVSF_RUNTIME_ROOT", &paths.runtimes)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let log = crate::logging::Log::new(&paths, "nvsf-supervisor");
    let mut child = process::Process::spawn(&mut command)?;
    let pid = child.id();
    log.event("INFO", "WORKER", &format!("Spawned pid={pid}"));
    let stdout = child.stdout()?;
    let mut reader = output::Output::new(stdout)?;
    let deadline = Instant::now() + Duration::from_secs(24 * 3600);
    let mut heartbeat = Instant::now();
    let mut output = None;
    let mut failure = None;
    let mut exited_at = None;
    let monitored = (|| -> Result<()> {
        loop {
            let cancel = control.cancel.load(Ordering::Relaxed);
            let pause = control.pause.load(Ordering::Relaxed);
            let flag = scratch.path().join("pause");
            if pause && !flag.exists() {
                write_atomic(&flag, b"pause", false)?;
            } else if !pause && flag.exists() {
                std::fs::remove_file(&flag)?;
            }
            if cancel
                || Instant::now() >= deadline
                || (!pause && heartbeat.elapsed() > Duration::from_secs(300))
            {
                let _ = write_atomic(&scratch.path().join("cancel"), b"cancel", false);
                let grace = Instant::now() + Duration::from_secs(2);
                while Instant::now() < grace {
                    if child.exited()? {
                        break;
                    }
                    std::thread::park_timeout(Duration::from_millis(20));
                }
                failure = Some(if cancel {
                    AppError::Cancelled
                } else {
                    fail("Worker exceeded wall/idle deadline")
                });
                break;
            }
            let mut output_progress = true;
            match reader.poll()? {
                output::Poll::Event(Event::Progress(progress)) => {
                    heartbeat = Instant::now();
                    callback(progress);
                }
                output::Poll::Event(Event::Completed(project)) => output = Some(*project),
                output::Poll::Event(Event::Failed(message)) => failure = Some(fail(message)),
                output::Poll::Progress => {}
                output::Poll::Pending => {
                    output_progress = false;
                    std::thread::park_timeout(Duration::from_millis(10));
                }
                output::Poll::Ended => {
                    let grace = Instant::now() + Duration::from_secs(2);
                    while !child.exited()? && Instant::now() < grace {
                        std::thread::park_timeout(Duration::from_millis(10));
                    }
                    break;
                }
            }
            if child.exited()? {
                if exited_at.is_none() {
                    child.signal_tree()?;
                }
                let exited = exited_at.get_or_insert_with(Instant::now);
                if output_progress {
                    *exited = Instant::now();
                }
                if exited.elapsed() >= Duration::from_secs(2) {
                    return Err(fail("Worker output remained open after process exit"));
                }
            }
        }
        Ok(())
    })();
    let status = child.finish();
    drop(reader);
    log.event(
        if monitored.is_ok()
            && failure.is_none()
            && status.as_ref().is_ok_and(|status| status.success())
        {
            "INFO"
        } else {
            "ERROR"
        },
        "WORKER",
        &format!(
            "Stopped pid={pid} monitor_ok={} exit={:?}",
            monitored.is_ok(),
            status.as_ref().ok().and_then(|status| status.code())
        ),
    );
    monitored?;
    let status = status?;
    if let Some(error) = failure {
        return Err(error);
    }
    if !status.success() {
        return Err(fail(format!("Worker exited with {status}")));
    }
    output.ok_or_else(|| fail("Worker ended without a complete project"))
}

/// Arm before native work. EOF survives a forcibly killed supervisor, unlike Rust Drop.
pub fn guard_parent_lifetime() -> Result<()> {
    #[cfg(windows)]
    windows::await_start()?;
    #[cfg(unix)]
    {
        let pid = std::process::id();
        // SAFETY: getpgrp has no arguments and returns this process's group.
        if pid <= 1 || unsafe { getpgrp() } != pid as i32 {
            return Err(fail("Worker requires its own isolated process group"));
        }
        // SAFETY: the child below calls only async-signal-safe libc operations, never Rust
        // allocation, locks, stdio, logging, destructors or native frameworks after fork.
        let guardian = unsafe { fork() };
        if guardian < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if guardian == 0 {
            unsafe {
                close(1);
                close(2);
                let mut byte = 0u8;
                loop {
                    if read(0, (&mut byte as *mut u8).cast(), 1) < 0 && *errno() == 4 {
                        continue;
                    }
                    break;
                }
                // This separate group member survives root exit; owner EOF kills it too.
                kill(-(pid as i32), 9);
                _exit(70);
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
unsafe extern "C" {
    fn getpgrp() -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
    fn fork() -> i32;
    fn close(fd: i32) -> i32;
    fn read(fd: i32, buffer: *mut std::ffi::c_void, size: usize) -> isize;
    fn _exit(code: i32) -> !;
    #[cfg(target_os = "linux")]
    #[link_name = "__errno_location"]
    fn errno() -> *mut i32;
    #[cfg(target_os = "macos")]
    #[link_name = "__error"]
    fn errno() -> *mut i32;
}
