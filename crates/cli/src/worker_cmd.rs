use crate::provider_profile;
use nvsf_app::{Control, Result, worker};
use nvsf_core::ExtractConfig;
use std::{io::Write, path::Path, sync::atomic::Ordering};

pub(crate) fn run_worker(
    request: &Path,
    control_dir: &Path,
    profile_directory: Option<&Path>,
    resume_project: Option<&Path>,
) -> Result<()> {
    nvsf_app::worker::guard_parent_lifetime()?;
    use worker::protocol;
    let job_id = protocol::job_id(control_dir)?;
    let config: ExtractConfig = protocol::read(request, job_id, protocol::CONTROL_LIMIT)?;
    config.validate()?;
    if let Some(directory) = profile_directory {
        provider_profile::validate_options(directory, &config.device, &config.language)?;
        provider_profile::validate_new_directory(directory)?;
    }
    let control = Control::default();
    control.pause.store(
        protocol::flag(control_dir, job_id, "pause")?,
        Ordering::Relaxed,
    );
    control.cancel.store(
        protocol::flag(control_dir, job_id, "cancel")?,
        Ordering::Relaxed,
    );
    let flags = control.clone();
    let dir = control_dir.to_path_buf();
    let watcher_job = job_id.to_owned();
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done = stopped.clone();
    let watcher = std::thread::spawn(move || -> Result<()> {
        while !done.load(Ordering::Relaxed) {
            let update = (|| -> Result<()> {
                if protocol::flag(&dir, &watcher_job, "cancel")? {
                    flags.cancel.store(true, Ordering::Relaxed);
                }
                flags.pause.store(
                    protocol::flag(&dir, &watcher_job, "pause")?,
                    Ordering::Relaxed,
                );
                Ok(())
            })();
            if let Err(error) = update {
                flags.cancel.store(true, Ordering::Relaxed);
                return Err(error);
            }
            std::thread::park_timeout(std::time::Duration::from_millis(50));
        }
        Ok(())
    });
    let mut emission_error = None;
    let progress = |p| {
        if emission_error.is_none()
            && let Err(error) = emit(job_id, &worker::Event::Progress(p))
        {
            emission_error = Some(error);
            control.cancel.store(true, Ordering::Relaxed);
        }
    };
    let output = if let Some(directory) = profile_directory {
        let requested_device = config.device.clone();
        let strict = config.gpu_required || config.accelerator_required;
        nvsf_app::extraction::run_profiled(config, &control, progress, directory).and_then(
            |(project, files)| {
                provider_profile::write_evidence(directory, &requested_device, strict, &files)?;
                Ok(project)
            },
        )
    } else if let Some(checkpoint) = resume_project {
        nvsf_app::extraction::run_resuming(config, &control, progress, checkpoint)
    } else {
        nvsf_app::extraction::run(config, &control, progress)
    };
    stopped.store(true, Ordering::Relaxed);
    watcher.thread().unpark();
    watcher
        .join()
        .map_err(|_| nvsf_app::fail("Worker control watcher failed"))??;
    if let Some(error) = emission_error {
        return Err(error);
    }
    match output {
        Ok(p) => {
            emit(job_id, &worker::Event::Completed(Box::new(p)))?;
            Ok(())
        }
        Err(e) => {
            emit(
                job_id,
                &worker::Event::Failure(protocol::Failure::from_error(&e)),
            )?;
            Err(e)
        }
    }
}

fn emit(job_id: &str, event: &worker::Event) -> Result<()> {
    let bytes = worker::protocol::encode(job_id, event, worker::protocol::EVENT_LIMIT - 1)?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&bytes)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}
