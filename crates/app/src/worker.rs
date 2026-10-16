use crate::extraction::Control;
use crate::{AppError, Paths, Result, fail, storage::write_atomic};
use nvsf_core::{ExtractConfig, Progress, Project};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read},
    path::Path,
    process::{Command, Stdio},
    sync::{atomic::Ordering, mpsc},
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
    let mut owned = OwnedChild(command.spawn()?);
    // Keep the writer outside Child: wait() closes a Child-owned stdin before waiting.
    let _parent_lease = owned
        .0
        .stdin
        .take()
        .ok_or_else(|| fail("Worker parent-lifetime pipe unavailable"))?;
    let child = &mut owned.0;
    let pid = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| fail("Worker progress stream unavailable"))?;
    let (tx, rx) = mpsc::sync_channel(8);
    let reader = std::thread::spawn(move || {
        let mut stream = BufReader::new(stdout);
        loop {
            let mut bytes = Vec::new();
            let result = stream
                .by_ref()
                .take(64 * 1024 * 1024 + 1)
                .read_until(b'\n', &mut bytes);
            match result {
                Ok(0) => break,
                Ok(_) if bytes.len() <= 64 * 1024 * 1024 => {
                    if tx
                        .send(serde_json::from_slice::<Event>(&bytes).map_err(|e| e.to_string()))
                        .is_err()
                    {
                        break;
                    }
                }
                _ => {
                    let _ = tx.send(Err("Worker message exceeds limit or pipe failed".into()));
                    break;
                }
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(24 * 3600);
    let mut heartbeat = Instant::now();
    let mut output = None;
    let mut failure = None;
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
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Ok(Event::Progress(progress))) => {
                    heartbeat = Instant::now();
                    callback(progress);
                }
                Ok(Ok(Event::Completed(project))) => {
                    output = Some(*project);
                }
                Ok(Ok(Event::Failed(message))) => {
                    failure = Some(fail(message));
                }
                Ok(Err(e)) => {
                    failure = Some(fail(e));
                    terminate(child, pid);
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if child.try_wait()?.is_some() && output.is_some() {
                break;
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
    let status = child.wait();
    drop(rx);
    reader
        .join()
        .map_err(|_| fail("Worker progress reader panicked"))?;
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
