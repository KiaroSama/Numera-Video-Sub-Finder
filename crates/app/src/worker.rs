use crate::extraction::Control;
use crate::{AppError, Paths, Result, fail, storage::write_atomic};
use nvsf_core::{ExtractConfig, Progress, Project};
use serde::{Deserialize, Serialize};
#[path = "worker_output.rs"]
mod output;
#[cfg(windows)]
#[path = "worker_windows.rs"]
mod windows;

#[cfg(unix)]
use std::io::Read;
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
    let mut command = Command::new(worker);
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
    let log = crate::logging::Log::new(&paths, "nvsf-supervisor");
    let mut owned = OwnedChild(command.spawn()?);
    log.event("INFO", "WORKER", &format!("Spawned pid={}", owned.0.id()));
    #[cfg(windows)]
    let job = windows::Job::attach(&owned.0)?;
    // Keep the writer outside Child: wait() closes a Child-owned stdin before waiting.
    let _parent_lease = owned
        .0
        .stdin
        .take()
        .ok_or_else(|| fail("Worker parent-lifetime pipe unavailable"))?;
    #[cfg(windows)]
    let _parent_lease = {
        use std::io::Write;
        let mut lease = _parent_lease;
        lease.write_all(b"S")?;
        lease
    };
    let child = &mut owned.0;
    let pid = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| fail("Worker progress stream unavailable"))?;
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
                    if child.try_wait()?.is_some() {
                        break;
                    }
                    std::thread::park_timeout(Duration::from_millis(20));
                }
                if child.try_wait()?.is_none() {
                    terminate(child, pid);
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
                output::Poll::Ended => break,
            }
            if child.try_wait()?.is_some() {
                let exited = exited_at.get_or_insert_with(Instant::now);
                if output_progress {
                    *exited = Instant::now();
                }
                if exited.elapsed() >= Duration::from_secs(2) {
                    return Err(fail("Worker output remained open after process exit"));
                }
            }
        }
        if child.try_wait()?.is_none() {
            terminate(child, pid);
        }
        Ok(())
    })();
    if monitored.is_err() && !matches!(child.try_wait(), Ok(Some(_))) {
        terminate(child, pid);
    }
    #[cfg(windows)]
    let job_cleanup = {
        let result = job.terminate();
        drop(job);
        result
    };
    let status = child.wait();
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
    #[cfg(windows)]
    job_cleanup?;
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
        std::thread::Builder::new()
            .name("worker-parent-lease".into())
            .spawn(move || {
                let mut byte = [0u8; 1];
                loop {
                    match std::io::stdin().read(&mut byte) {
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        _ => break,
                    }
                }
                // SAFETY: this live process is the verified leader of its own worker group.
                // The parent holds the only writer and never sends input; EOF means owner loss.
                unsafe { kill(-(pid as i32), 9) };
                std::process::exit(70);
            })?;
    }
    Ok(())
}

#[cfg(unix)]
unsafe extern "C" {
    fn getpgrp() -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
}

struct OwnedChild(std::process::Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let pid = self.0.id();
            terminate(&mut self.0, pid);
        }
    }
}

fn terminate(child: &mut std::process::Child, pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}
