use crate::state::{Host, Job, JobStatus, Result, blocking, lock, path_text};
use nvsf_core::ExtractConfig;
use serde::Serialize;
use std::sync::{Arc, Mutex, atomic::Ordering, mpsc};

#[tauri::command]
pub async fn start_job(
    host: tauri::State<'_, Host>,
    source_token: String,
    output_token: String,
    config: ExtractConfig,
) -> Result<String> {
    let host = host.inner().clone();
    blocking(move || {
        let _operation = lock(&host.0.operation)?;
        host.ensure_idle()?;
        if host.0.unsaved.load(Ordering::Acquire) {
            return Err("Save current review corrections before starting a new extraction".into());
        }
        let (queue_id, config) =
            crate::queue::register(&host, &source_token, &output_token, config)?;
        launch(&host, config, Some(queue_id))
    })
    .await
}

// Caller owns the operation lock; every entry point shares one job owner.
pub fn launch(host: &Host, config: ExtractConfig, queue_id: Option<String>) -> Result<String> {
    if host.0.unsaved.load(Ordering::Acquire) {
        return Err("Save current review corrections before starting/resuming a job".into());
    }
    if host.0.model_busy.load(Ordering::Acquire) {
        return Err("Wait for model installation to finish".into());
    }
    config.validate().map_err(|e| e.to_string())?;
    let mut jobs = lock(&host.0.job)?;
    if let Some(job) = jobs.as_mut() {
        if !job
            .thread
            .as_ref()
            .is_none_or(std::thread::JoinHandle::is_finished)
        {
            return Err("A job is already running; cancel it or wait for completion".into());
        }
        if let Some(thread) = job.thread.take() {
            thread.join().map_err(|_| {
                "Previous worker panicked; review its checkpoint before retrying".to_string()
            })?;
        }
    }
    let id = host.token("job");
    let status = Arc::new(Mutex::new(JobStatus {
        id: id.clone(),
        state: "running".into(),
        ..JobStatus::default()
    }));
    let control = nvsf_app::Control::default();
    let worker_control = control.clone();
    let worker_status = status.clone();
    let host_clone = host.clone();
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    if let Some(id) = &queue_id {
        crate::queue::mark(host, id, "running", None)?;
    }
    let running_queue = queue_id.clone();
    let thread = std::thread::Builder::new().name("subtitle-job".into()).spawn(move || {
        host_clone.0.log.event("INFO", "JOB", "Extraction started with registered source and destination");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            nvsf_app::extract(config, &worker_control, |progress| {
                if let Ok(mut snapshot) = worker_status.lock() { snapshot.progress = Some(progress); }
            })
        }));
        if let Ok(mut snapshot) = worker_status.lock() {
            match result {
                Ok(Ok(project)) => {
                    let history_path = host_clone.0.paths.projects.join(format!("{}.nvsf.json", snapshot.id));
                    let saved = nvsf_app::storage::save_project(&project, &history_path);
                    match host_clone.0.project.lock() {
                        Ok(mut active) => {
                            *active = Some(project);
                            snapshot.project_ready = true;
                            snapshot.state = "completed".into();
                            if let Err(e) = saved { snapshot.error = Some(format!("Extraction completed, but history save failed: {e}")); }
                            host_clone.0.log.event("INFO", "JOB", "Extraction completed; review project available");
                        }
                        Err(_) => { snapshot.state = "failed".into(); snapshot.error = Some("Workspace project state unavailable".into()); }
                    }
                }
                Ok(Err(e)) => {
                    snapshot.state = if worker_control.cancel.load(Ordering::Acquire) { "cancelled" } else { "failed" }.into();
                    snapshot.error = Some(e.to_string());
                    host_clone.0.log.event("ERROR", "JOB", "Extraction stopped; inspect workspace error and preserved checkpoint");
                }
                Err(_) => {
                    snapshot.state = "failed".into();
                    snapshot.error = Some("Native worker panicked; existing checkpoint is preserved. Restart the application before retrying".into());
                    host_clone.0.log.event("ERROR", "JOB", "Worker panic caught");
                }
            }
        }
        if let Some(id) = &running_queue {
            let snapshot = worker_status.lock().ok().map(|s| s.clone());
            if let Some(snapshot) = snapshot {
                if let Err(e) = crate::queue::mark(&host_clone, id, &snapshot.state, snapshot.error) {
                    if let Ok(mut snapshot) = worker_status.lock() { snapshot.error = Some(format!("Job stopped but durable queue update failed: {e}")); }
                    host_clone.0.log.event("ERROR", "QUEUE", "Durable job status could not be saved; previous queue state preserved");
                }
            }
        }
        let _ = done_tx.send(());
    }).map_err(|e| {
        if let Some(id) = &queue_id { let _ = crate::queue::mark(host, id, "failed", Some(e.to_string())); }
        format!("Unable to start extraction worker: {e}")
    })?;
    *jobs = Some(Job {
        id: id.clone(),
        control,
        status,
        done: done_rx,
        thread: Some(thread),
    });
    Ok(id)
}

#[tauri::command]
pub fn job_status(host: tauri::State<'_, Host>) -> Result<JobStatus> {
    let host = host.inner();
    let jobs = lock(&host.0.job)?;
    match jobs.as_ref() {
        Some(job) => Ok(lock(&job.status)?.clone()),
        None => Ok(JobStatus {
            state: "idle".into(),
            ..JobStatus::default()
        }),
    }
}

#[tauri::command]
pub fn control_job(host: tauri::State<'_, Host>, id: String, action: String) -> Result<()> {
    let host = host.inner();
    let jobs = lock(&host.0.job)?;
    let job = jobs
        .as_ref()
        .filter(|job| job.id == id)
        .ok_or_else(|| "Job token is not active".to_string())?;
    if job
        .thread
        .as_ref()
        .is_none_or(std::thread::JoinHandle::is_finished)
    {
        return Err("Job has already stopped".into());
    }
    let mut status = lock(&job.status)?;
    match action.as_str() {
        "pause" => {
            job.control.pause.store(true, Ordering::Release);
            status.state = "paused".into();
        }
        "resume" => {
            job.control.pause.store(false, Ordering::Release);
            status.state = "running".into();
        }
        "cancel" => {
            job.control.cancel.store(true, Ordering::Release);
            job.control.pause.store(false, Ordering::Release);
            status.state = "cancelling".into();
        }
        _ => return Err("Choose pause, resume or cancel".into()),
    }
    host.0
        .log
        .event("INFO", "JOB", &format!("Control action={action}"));
    Ok(())
}

#[tauri::command]
pub async fn model_catalog(
    host: tauri::State<'_, Host>,
) -> Result<Vec<nvsf_app::models::ModelStatus>> {
    let paths = host.inner().0.paths.clone();
    blocking(move || nvsf_app::models::catalog(&paths).map_err(|e| e.to_string())).await
}

#[derive(Serialize)]
pub struct Doctor {
    pub data_root: String,
    pub models_root: String,
    pub runtimes_present: bool,
    pub log_available: bool,
    pub platform: String,
    pub models: Vec<nvsf_app::models::ModelStatus>,
    pub message: String,
}

#[tauri::command]
pub async fn doctor(host: tauri::State<'_, Host>) -> Result<Doctor> {
    let host = host.inner().clone();
    blocking(move || Ok(Doctor {
        data_root: path_text(&host.0.paths.root)?, models_root: path_text(&host.0.paths.models)?,
        runtimes_present: host.0.paths.runtimes.is_dir(), log_available: host.0.log.path.is_some(),
        platform: format!("{} / {}", std::env::consts::OS, std::env::consts::ARCH),
        models: nvsf_app::models::catalog(&host.0.paths).map_err(|e| e.to_string())?,
        message: "Resource checks only. Provider execution and hardware qualification are reported by actual extraction, not inferred from directory presence.".into(),
    })).await
}
