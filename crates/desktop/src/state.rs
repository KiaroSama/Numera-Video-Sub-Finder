use nvsf_app::{Control, Paths, logging::Log};
use nvsf_core::{MediaInfo, Progress, Project};
use serde::Serialize;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
        mpsc::Receiver,
    },
    thread::JoinHandle,
};

pub type Result<T> = std::result::Result<T, String>;

pub fn lock<T>(value: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    value
        .lock()
        .map_err(|_| "Workspace state unavailable; restart the application".into())
}

#[derive(Clone, Serialize)]
pub struct Source {
    pub token: String,
    pub name: String,
    pub fingerprint: String,
    pub media: MediaInfo,
}

#[derive(Clone, Serialize)]
pub struct Destination {
    pub token: String,
    pub name: String,
}

#[derive(Clone, Serialize, Default)]
pub struct JobStatus {
    pub id: String,
    pub state: String,
    pub progress: Option<Progress>,
    pub error: Option<String>,
    pub project_ready: bool,
}

pub struct Job {
    pub id: String,
    pub control: Control,
    pub status: Arc<Mutex<JobStatus>>,
    pub done: Receiver<()>,
    pub thread: Option<JoinHandle<()>>,
}

pub struct Inner {
    pub paths: Paths,
    pub log: Log,
    pub sources: Mutex<HashMap<String, PathBuf>>,
    pub destinations: Mutex<HashMap<String, PathBuf>>,
    pub history: Mutex<HashMap<String, PathBuf>>,
    pub project: Mutex<Option<Project>>,
    pub queue: Mutex<crate::queue::Store>,
    pub job: Mutex<Option<Job>>,
    pub next: AtomicU64,
    pub session: String,
    pub model_busy: std::sync::atomic::AtomicBool,
    pub model_cancel: std::sync::atomic::AtomicBool,
    pub operation: Mutex<()>,
    pub source_busy: std::sync::atomic::AtomicBool,
    pub unsaved: std::sync::atomic::AtomicBool,
    pub closing: std::sync::atomic::AtomicBool,
    pub maintenance: crate::updates::Maintenance,
}

#[derive(Clone)]
pub struct Host(pub Arc<Inner>);

impl Host {
    pub fn new(paths: Paths) -> Self {
        let log = Log::new(&paths, "nvsf-desktop");
        let queue = crate::queue::Store::load(&paths.root);
        Self(Arc::new(Inner {
            paths,
            log,
            sources: Mutex::new(HashMap::new()),
            destinations: Mutex::new(HashMap::new()),
            history: Mutex::new(HashMap::new()),
            project: Mutex::new(None),
            queue: Mutex::new(queue),
            job: Mutex::new(None),
            next: AtomicU64::new(1),
            session: format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ),
            model_busy: std::sync::atomic::AtomicBool::new(false),
            model_cancel: std::sync::atomic::AtomicBool::new(false),
            operation: Mutex::new(()),
            source_busy: std::sync::atomic::AtomicBool::new(false),
            unsaved: std::sync::atomic::AtomicBool::new(false),
            closing: std::sync::atomic::AtomicBool::new(false),
            maintenance: crate::updates::Maintenance::default(),
        }))
    }
    pub fn token(&self, kind: &str) -> String {
        format!(
            "{kind}-{}-{}",
            self.0.session,
            self.0.next.fetch_add(1, Ordering::Relaxed)
        )
    }
    pub fn source(&self, token: &str) -> Result<PathBuf> {
        let path = lock(&self.0.sources)?
            .get(token)
            .cloned()
            .ok_or_else(|| "Select a video with the native picker first".to_string())?;
        if path.is_symlink() {
            return Err("Granted source was replaced with a symlink; select it again".into());
        }
        if !path.is_file() {
            return Err("Granted source is no longer a regular file".into());
        }
        Ok(path)
    }
    pub fn destination(&self, token: &str) -> Result<PathBuf> {
        let path = lock(&self.0.destinations)?
            .get(token)
            .cloned()
            .ok_or_else(|| "Choose an output with the native save picker first".to_string())?;
        if path.is_symlink() {
            return Err(
                "Granted output was replaced with a symlink; choose another destination".into(),
            );
        }
        Ok(path)
    }
    pub fn ensure_idle(&self) -> Result<()> {
        if self.0.closing.load(Ordering::Acquire)
            || self.0.source_busy.load(Ordering::Acquire)
            || self.0.maintenance.installing.load(Ordering::Acquire)
        {
            return Err(
                "Wait for source operations, update installation or application shutdown to finish"
                    .into(),
            );
        }
        let job = lock(&self.0.job)?;
        if let Some(job) = job.as_ref() {
            if !job.thread.as_ref().is_none_or(JoinHandle::is_finished) {
                return Err(
                    "A job is active; pause or cancel it before changing projects or models".into(),
                );
            }
        }
        if self.0.model_busy.load(Ordering::Acquire) {
            return Err("Model installation is active".into());
        }
        Ok(())
    }
    pub fn shutdown(&self) -> bool {
        if self.0.maintenance.installing.load(Ordering::Acquire) {
            self.0.log.event(
                "WARNING",
                "UPDATES",
                "Close deferred until native update installation finishes",
            );
            return false;
        }
        if self.0.model_busy.load(Ordering::Acquire) {
            self.0.model_cancel.store(true, Ordering::Release);
            self.0.log.event(
                "INFO",
                "MODELS",
                "Close requested cancellation; waiting for resource operation before exit",
            );
            return false;
        }
        if let Ok(mut jobs) = self.0.job.lock() {
            if let Some(job) = jobs.as_mut() {
                job.control.cancel.store(true, Ordering::Release);
                job.control.pause.store(false, Ordering::Release);
                let stopped = job.thread.as_ref().is_none_or(JoinHandle::is_finished)
                    || job
                        .done
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .is_ok();
                if stopped {
                    if let Some(thread) = job.thread.take() {
                        if thread.join().is_err() {
                            self.0
                                .log
                                .event("ERROR", "JOB", "Worker panicked during shutdown");
                        }
                    }
                } else {
                    self.0.log.event(
                        "WARNING",
                        "JOB",
                        "Worker cancellation exceeded 5s; close blocked until the owned worker stops",
                    );
                    return false;
                }
            }
        }
        if !self.0.maintenance.stop() {
            self.0.log.event(
                "WARNING",
                "UPDATES",
                "Waiting for bounded metadata check before exit",
            );
            return false;
        }
        self.0
            .log
            .event("INFO", "DESKTOP", "Native workspace shutdown requested");
        true
    }
}

pub async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|_| "Native operation stopped unexpectedly".to_string())?
}

pub fn path_text(path: &std::path::Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "This path is not valid UTF-8; choose a Unicode-compatible path".into())
}

pub fn display_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Selected file".into())
}
