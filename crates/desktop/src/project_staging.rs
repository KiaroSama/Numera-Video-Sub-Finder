use super::ProjectFiles;
use crate::state::{Host, Result, display_name, lock, path_text};
use nvsf_core::Project;
use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

pub(crate) struct Snapshot {
    project: Option<Project>,
    active: Option<(Option<String>, PathBuf)>,
    dirty: bool,
    extract_only_dirty: bool,
}

impl ProjectFiles {
    pub(crate) fn snapshot(&self, host: &Host) -> Result<Snapshot> {
        let paths = lock(&self.0)?;
        Ok(Snapshot {
            project: lock(&host.0.project)?.clone(),
            active: paths.active.clone(),
            dirty: host.0.unsaved.load(Ordering::Acquire),
            extract_only_dirty: host.0.extract_only_dirty.load(Ordering::Acquire),
        })
    }

    pub(crate) fn restore(&self, host: &Host, snapshot: Snapshot) -> Result<()> {
        let mut paths = lock(&self.0)?;
        let mut active = lock(&host.0.project)?;
        *active = snapshot.project;
        paths.active = snapshot.active;
        paths.generation = paths.generation.wrapping_add(1);
        host.0
            .extract_only_dirty
            .store(snapshot.extract_only_dirty, Ordering::Release);
        host.0.unsaved.store(snapshot.dirty, Ordering::Release);
        Ok(())
    }
}

pub(super) fn for_webview(mut project: Project) -> Project {
    project.config.input = display_name(Path::new(&project.config.input));
    if !project.config.output.is_empty() {
        project.config.output = display_name(Path::new(&project.config.output));
    }
    project.config.data_root = None;
    for cue in project
        .cues
        .iter_mut()
        .chain(&mut project.active_appearances)
    {
        if let Some(path) = &mut cue.crop {
            *path = display_name(Path::new(path));
        }
    }
    project
}

// Caller holds the operation lock, including when the native close confirmation is active.
pub(crate) fn apply_project_edits(
    host: &Host,
    files: &ProjectFiles,
    mut project: Project,
    source_token: Option<&str>,
) -> Result<()> {
    if let Some(token) = source_token
        && project.is_setup()
    {
        if host.review_dirty()
            && lock(&host.0.project)?
                .as_ref()
                .is_some_and(|old| !old.is_setup())
        {
            return Err("Save the current review corrections before replacing the review".into());
        }
        let source = host.source(token)?;
        project.config.input = path_text(&source)?;
        let fingerprint =
            nvsf_app::storage::hash_file(&source).map_err(|error| error.to_string())?;
        if !project.source_hash.is_empty() && project.source_hash != fingerprint {
            return Err("The selected source changed on disk; reopen it before saving".into());
        }
        project.source_hash = fingerprint;
        project.config.output.clear();
        project.config.data_root = None;
        project.config_hash.clear();
        validate(&project)?;
        let mut paths = lock(&files.0)?;
        let mut current = lock(&host.0.project)?;
        if current
            .as_ref()
            .is_none_or(|old| !old.is_setup() || old.source_hash != project.source_hash)
        {
            paths.active = None;
        }
        paths.generation = paths.generation.wrapping_add(1);
        *current = Some(project);
        if !host.review_dirty() {
            host.0.extract_only_dirty.store(true, Ordering::Release);
        }
        host.0.unsaved.store(true, Ordering::Release);
        return Ok(());
    }

    let mut current = lock(&host.0.project)?;
    let old = current
        .as_mut()
        .ok_or_else(|| "Choose a source video or open a project before saving".to_string())?;
    // The WebView may return display names, but cannot change Review's native paths.
    if project.config.input == display_name(Path::new(&old.config.input)) {
        project.config.input.clone_from(&old.config.input);
    }
    if project.config.output == display_name(Path::new(&old.config.output)) {
        project.config.output.clone_from(&old.config.output);
    }
    if project.config.data_root.is_none() {
        project.config.data_root.clone_from(&old.config.data_root);
    }
    validate(&project)?;
    if old.source_hash != project.source_hash
        || old.config_hash != project.config_hash
        || old.model_hash != project.model_hash
        || old.config != project.config
    {
        return Err("Only cue corrections may replace a review project; source, settings and model must match".into());
    }
    // Split cues can share the same evidence. Only restore paths already owned by this project.
    let crops: std::collections::HashMap<_, _> = old
        .cues
        .iter()
        .filter_map(|cue| {
            let path = cue.crop.as_ref()?;
            Some((display_name(Path::new(path)), path))
        })
        .collect();
    for cue in &mut project.cues {
        cue.crop = cue
            .crop
            .as_ref()
            .and_then(|name| crops.get(&display_name(Path::new(name))))
            .map(|path| (*path).clone());
    }
    old.cues = project.cues;
    Ok(())
}

fn validate(project: &Project) -> Result<()> {
    project.validate().map_err(|error| error.to_string())?;
    if serde_json::to_vec(project)
        .map_err(|error| error.to_string())?
        .len()
        > 64 * 1024 * 1024
    {
        return Err("Project exceeds 64MiB workspace bound".into());
    }
    Ok(())
}
