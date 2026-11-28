use super::{ProjectFiles, job_id, project_filename, project_staging, save_prepared_project};
use crate::state::{Host, Result, lock, path_text};
use nvsf_app::storage::AtomicOutput;
use nvsf_core::Project;
use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

pub(super) struct SaveRequest {
    pub project: Option<Project>,
    pub source_token: Option<String>,
    pub as_new: bool,
}

struct PendingSave {
    project: Project,
    original: Vec<u8>,
    job: Option<String>,
    generation: u64,
    active_path: Option<PathBuf>,
    replaces_review: bool,
}

fn ensure_admitted(host: &Host, closing: bool) -> Result<()> {
    if !closing {
        return host.ensure_idle();
    }
    if lock(&host.0.job)?.as_ref().is_some_and(|job| {
        job.thread
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
    }) {
        return Err("Wait for the current extraction to finish before saving and exiting".into());
    }
    Ok(())
}

impl PendingSave {
    fn prepare(
        host: &Host,
        files: &ProjectFiles,
        request: SaveRequest,
        closing: bool,
    ) -> Result<Self> {
        let _operation = lock(&host.0.operation)?;
        ensure_admitted(host, closing)?;
        let previous = lock(&host.0.project)?.clone();
        let original = project_staging::identity(&previous)?;
        let setup = request.source_token.is_some()
            && request.project.as_ref().is_some_and(Project::is_setup);
        let project = match request.project {
            Some(project) => project_staging::prepare_project(
                host,
                previous.as_ref(),
                project,
                request.source_token.as_deref(),
            )?,
            None if request.source_token.is_some() => {
                return Err("A source grant requires a project to save".into());
            }
            None => previous.clone().ok_or_else(|| {
                "No active project to save yet. Choose a video or open a project first.".to_string()
            })?,
        };
        let job = job_id(host)?;
        let paths = lock(&files.0)?;
        let retain_path =
            !setup || project_staging::retains_setup_path(previous.as_ref(), &project);
        let active_path = paths
            .active
            .as_ref()
            .filter(|(id, _)| retain_path && id == &job)
            .map(|(_, path)| path.clone());
        Ok(Self {
            project,
            original,
            job,
            generation: paths.generation,
            active_path,
            replaces_review: setup && previous.as_ref().is_some_and(|old| !old.is_setup()),
        })
    }

    fn filename(&self) -> String {
        self.active_path
            .as_ref()
            .and_then(|path| path.file_name().and_then(|name| name.to_str()))
            .map(project_filename)
            .unwrap_or_else(|| {
                project_filename(
                    Path::new(&self.project.config.input)
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or("subtitles"),
                )
            })
    }

    fn commit(
        &self,
        host: &Host,
        files: &ProjectFiles,
        verify_close: Option<&dyn Fn() -> Result<()>>,
        destination: (&Path, AtomicOutput),
        bytes: &[u8],
        publish: impl FnOnce(AtomicOutput, &[u8]) -> Result<()>,
    ) -> Result<()> {
        let (path, output) = destination;
        if let Some(verify) = verify_close {
            verify()?;
        }
        let _operation = lock(&host.0.operation)?;
        ensure_admitted(host, verify_close.is_some())?;
        let mut paths = lock(&files.0)?;
        let mut active = lock(&host.0.project)?;
        if job_id(host)? != self.job
            || paths.generation != self.generation
            || project_staging::identity(&active)? != self.original
            || (self.replaces_review && host.review_dirty())
        {
            return Err(
                "The workspace changed while choosing a project destination; save again".into(),
            );
        }
        publish(output, bytes)?;
        *active = Some(self.project.clone());
        let fingerprint = nvsf_app::providers::profiles::hash_bytes(bytes);
        paths.recent.retain(|(recent, _)| recent != path);
        paths.recent.insert(0, (path.to_path_buf(), fingerprint));
        paths.recent.truncate(200);
        paths.generation = paths.generation.wrapping_add(1);
        paths.active = Some((self.job.clone(), path.to_path_buf()));
        host.0.unsaved.store(false, Ordering::Release);
        host.0.extract_only_dirty.store(false, Ordering::Release);
        Ok(())
    }
}

// The picker and overwrite prompt run without workspace locks. Publication is the only mutation.
pub(super) fn save(
    host: &Host,
    files: &ProjectFiles,
    request: SaveRequest,
    verify_close: Option<&dyn Fn() -> Result<()>>,
    pick: impl FnOnce(&str) -> Result<Option<PathBuf>>,
    confirm: impl FnOnce() -> Result<bool>,
    publish: impl FnOnce(AtomicOutput, &[u8]) -> Result<()>,
) -> Result<Option<String>> {
    let as_new = request.as_new;
    let pending = PendingSave::prepare(host, files, request, verify_close.is_some())?;
    let path = if let Some(path) = pending.active_path.as_ref().filter(|_| !as_new) {
        path.clone()
    } else if let Some(path) = pick(&pending.filename())? {
        path
    } else {
        return Ok(None);
    };
    path_text(&path)?;
    if !path.is_absolute() {
        return Err("Choose an absolute destination for the review project".into());
    }
    let prepared = AtomicOutput::for_confirmation(&path).map_err(|error| error.to_string())?;
    save_prepared_project(
        &pending.project,
        &path,
        prepared,
        confirm,
        |output, bytes| pending.commit(host, files, verify_close, (&path, output), bytes, publish),
    )
}

#[cfg(test)]
#[path = "project_save_tests.rs"]
mod tests;
