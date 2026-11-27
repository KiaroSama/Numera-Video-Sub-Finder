use crate::extraction::{Control, SrtOptions};
use crate::{AppError, Paths, Result, fail, storage::write_atomic};
use nvsf_core::{ExtractConfig, Progress, Project};
use serde::{Deserialize, Serialize};
#[path = "worker_idle.rs"]
mod idle;
#[path = "worker_output.rs"]
mod output;
#[path = "worker_process.rs"]
pub(crate) mod process;
#[path = "worker_protocol.rs"]
pub mod protocol;
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
    Failure(protocol::Failure),
}

pub fn supervise(
    config: ExtractConfig,
    control: &Control,
    callback: impl FnMut(Progress),
) -> Result<Project> {
    supervise_inner(config, control, callback, None, None, SrtOptions::default())
}

/// Applies encoding during the worker's first SRT publication, under its output lock.
pub fn supervise_with_srt(
    config: ExtractConfig,
    control: &Control,
    callback: impl FnMut(Progress),
    options: SrtOptions,
) -> Result<Project> {
    supervise_inner(config, control, callback, None, None, options)
}

/// Runs the ordinary isolated worker against the caller's selected saved checkpoint.
pub fn supervise_resume(
    config: ExtractConfig,
    control: &Control,
    callback: impl FnMut(Progress),
    checkpoint: &Path,
) -> Result<Project> {
    let checkpoint = checkpoint.canonicalize()?;
    if !checkpoint.is_file() {
        return Err(fail("Selected checkpoint must be a regular file"));
    }
    supervise_inner(
        config,
        control,
        callback,
        None,
        Some(&checkpoint),
        SrtOptions::default(),
    )
}

/// Same isolated worker supervision; the worker writes provider profiles and evidence
/// into the caller-validated new directory before reporting completion.
pub fn supervise_profiled(
    config: ExtractConfig,
    control: &Control,
    callback: impl FnMut(Progress),
    profile_directory: &Path,
) -> Result<Project> {
    supervise_inner(
        config,
        control,
        callback,
        Some(profile_directory),
        None,
        SrtOptions::default(),
    )
}

fn supervise_inner(
    config: ExtractConfig,
    control: &Control,
    mut callback: impl FnMut(Progress),
    profile_directory: Option<&Path>,
    selected_checkpoint: Option<&Path>,
    options: SrtOptions,
) -> Result<Project> {
    config.validate()?;
    let paths = Paths::new(config.data_root.as_deref().map(Path::new))?;
    let scratch = tempfile::tempdir_in(&paths.cache)?;
    let request = scratch.path().join("request.json");
    let job_id = protocol::job_id(scratch.path())?;
    write_atomic(
        &request,
        &protocol::encode(job_id, &config, protocol::CONTROL_LIMIT)?,
        false,
    )?;
    let mut command = Command::new(process::executable()?);
    command
        .arg("worker")
        .arg("--request")
        .arg(&request)
        .arg("--control")
        .arg(scratch.path());
    if let Some(directory) = profile_directory {
        command.arg("--provider-profile").arg(directory);
    }
    if let Some(checkpoint) = selected_checkpoint {
        command.arg("--resume-project").arg(checkpoint);
    }
    if options.bom {
        command.arg("--srt-bom");
    }
    if options.crlf {
        command.arg("--srt-crlf");
    }
    command
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
    let mut reader = output::Output::new(stdout, job_id)?;
    let deadline = Instant::now() + Duration::from_secs(24 * 3600);
    let mut heartbeat = idle::IdleWatch::new(Instant::now());
    let mut output = None;
    let mut failure = None;
    let mut exited_at = None;
    let monitored = (|| -> Result<()> {
        loop {
            let cancel = control.cancel.load(Ordering::Relaxed);
            let pause = control.pause.load(Ordering::Relaxed);
            let flag = scratch.path().join("pause");
            if pause && !flag.exists() {
                write_atomic(&flag, &protocol::encode(job_id, &"pause", 1024)?, false)?;
            } else if !pause && flag.exists() {
                std::fs::remove_file(&flag)?;
            }
            if cancel || Instant::now() >= deadline || heartbeat.expired(Instant::now(), pause) {
                let _ = write_atomic(
                    &scratch.path().join("cancel"),
                    &protocol::encode(job_id, &"cancel", 1024)?,
                    false,
                );
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
                    heartbeat.progress(Instant::now());
                    callback(progress);
                }
                output::Poll::Event(Event::Completed(project)) => output = Some(*project),
                output::Poll::Event(Event::Failed(message)) => failure = Some(fail(message)),
                output::Poll::Event(Event::Failure(error)) => failure = Some(error.into_error()),
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
    process::guard_parent_pipe(false)
}
