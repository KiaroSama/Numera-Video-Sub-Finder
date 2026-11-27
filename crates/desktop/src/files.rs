use crate::state::{self, Destination, Host, Result, Source, blocking, display_name, lock};
use nvsf_app::{
    native_request::{self, Operation, Response},
    storage::AtomicOutput,
};
use nvsf_core::Project;
use serde::Serialize;
use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

#[path = "project_staging.rs"]
mod project_staging;
pub(crate) use project_staging::apply_project_edits;

#[derive(Clone, Default)]
pub(crate) struct ProjectFiles(Arc<Mutex<ProjectPaths>>);

#[derive(Default)]
struct ProjectPaths {
    active: Option<(Option<String>, PathBuf)>,
    generation: u64,
    recent: Vec<(PathBuf, String)>,
    recent_grants: std::collections::HashMap<String, String>,
}

fn job_id(host: &Host) -> Result<Option<String>> {
    Ok(lock(&host.0.job)?.as_ref().map(|job| job.id.clone()))
}

#[tauri::command]
pub async fn pick_source(
    app: tauri::AppHandle,
    host: tauri::State<'_, Host>,
    expected_hash: Option<String>,
) -> Result<Option<Source>> {
    let host = host.inner().clone();
    blocking(move || {
        let Some(file) = app
            .dialog()
            .file()
            .set_title("Open subtitle video")
            .add_filter(
                "Video",
                &[
                    "mp4", "mkv", "mov", "avi", "webm", "m4v", "ts", "mpeg", "wmv",
                ],
            )
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = file.into_path().map_err(|e| e.to_string())?;
        grant_source_with(&host, &path, expected_hash.as_deref(), |path| {
            probe_source(&host, path)
        })
        .map(Some)
    })
    .await
}

#[tauri::command]
pub async fn choose_output(
    app: tauri::AppHandle,
    host: tauri::State<'_, Host>,
) -> Result<Option<Destination>> {
    let host = host.inner().clone();
    blocking(move || {
        let Some(file) = app
            .dialog()
            .file()
            .set_title("Choose subtitle output")
            .add_filter("SubRip subtitles", &["srt"])
            .set_file_name("subtitles.srt")
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = file.into_path().map_err(|e| e.to_string())?;
        grant_output(&host, &path).map(Some)
    })
    .await
}

pub(crate) fn probe_source(host: &Host, path: &std::path::Path) -> Result<nvsf_core::MediaInfo> {
    host.0
        .source_cancel
        .store(false, std::sync::atomic::Ordering::Release);
    let (response, _) =
        native_request::supervise(&host.0.paths, path, Operation::Probe, &host.0.source_cancel)
            .map_err(|e| e.to_string())?;
    match response {
        Response::Probe(info) => Ok(info),
        _ => Err("Native probe returned a different response".into()),
    }
}

pub(crate) fn grant_source_with(
    host: &Host,
    path: &std::path::Path,
    expected_hash: Option<&str>,
    probe: impl FnOnce(&std::path::Path) -> Result<nvsf_core::MediaInfo>,
) -> Result<Source> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    if !path.is_file() {
        return Err("Select a regular local video file".into());
    }
    let _operation = lock(&host.0.operation)?;
    host.ensure_idle()?;
    let fingerprint = nvsf_app::storage::hash_file(&path).map_err(|e| e.to_string())?;
    if expected_hash.is_some_and(|expected| fingerprint != expected) {
        return Err("Selected video does not match the project's source fingerprint".into());
    }
    host.0
        .source_cancel
        .store(false, std::sync::atomic::Ordering::Release);
    let media = probe(&path)?;
    let mut preview = lock(&host.0.preview_daemon)?;
    if preview
        .as_ref()
        .is_some_and(|daemon| !daemon.matches(&path))
    {
        preview.take();
    }
    let token = host.token("source");
    let mut grants = lock(&host.0.sources)?;
    if grants.len() >= 1024 {
        return Err("Source limit reached; restart the workspace to release old grants".into());
    }
    grants.insert(token.clone(), path.clone());
    host.0.log.event(
        "INFO",
        "SOURCE",
        "Native source granted and metadata probed",
    );
    Ok(Source {
        token,
        name: display_name(&path),
        fingerprint,
        media,
    })
}

pub(crate) fn grant_output(host: &Host, path: &std::path::Path) -> Result<Destination> {
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_none_or(|e| !e.eq_ignore_ascii_case("srt"))
    {
        return Err("Choose a destination ending in .srt".into());
    }
    state::path_text(path)?;
    if path.is_symlink() {
        return Err("Output is a symlink; choose a regular destination".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "Output directory does not exist".to_string())?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let name = path
        .file_name()
        .ok_or_else(|| "Output needs a filename".to_string())?;
    let path = parent.join(name);
    let token = host.token("output");
    let mut grants = lock(&host.0.destinations)?;
    if grants.len() >= 256 {
        return Err("Output grant limit reached; restart the workspace".into());
    }
    grants.insert(token.clone(), path.clone());
    Ok(Destination {
        token,
        name: display_name(&path),
    })
}

#[tauri::command]
pub async fn preview_frame(
    host: tauri::State<'_, Host>,
    token: String,
    time_ms: u64,
) -> Result<tauri::ipc::Response> {
    with_source_preview(host.inner(), token, move |host, path| {
        let bytes = decode_preview(host, path, time_ms)?;
        Ok(tauri::ipc::Response::new(bytes))
    })
    .await
}

#[tauri::command]
pub async fn preview_initial_frame(host: tauri::State<'_, Host>, token: String) -> Result<u64> {
    with_source_preview(host.inner(), token, |host, path| {
        std::thread::scope(|scope| {
            let (done, wait) = std::sync::mpsc::channel::<()>();
            let cancel = &host.0.source_cancel;
            let Ok(_timer) = std::thread::Builder::new().spawn_scoped(scope, move || {
                if wait.recv_timeout(Duration::from_secs(3))
                    == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                {
                    cancel.store(true, Ordering::Release);
                }
            }) else {
                return Ok(0);
            };
            let result = probe_initial_frame(host, path);
            drop(done);
            result
        })
    })
    .await
}

fn probe_initial_frame(host: &Host, path: &Path) -> Result<u64> {
    let duration = lock(&host.0.project)?
        .as_ref()
        .filter(|project| Path::new(&project.config.input) == path)
        .and_then(|project| project.media.duration_ms);
    let duration = match duration {
        Some(_) => duration,
        None => match native_request::supervise(
            &host.0.paths,
            path,
            Operation::Probe,
            &host.0.source_cancel,
        ) {
            Ok((Response::Probe(media), _)) => media.duration_ms,
            _ => return Ok(0),
        },
    };
    Ok(nvsf_app::media::MediaRuntime::initial_preview_timestamp(
        duration,
        |time| {
            let (response, bytes) = native_request::supervise(
                &host.0.paths,
                path,
                Operation::Preview { at_ms: time },
                &host.0.source_cancel,
            )
            .ok()?;
            let Response::Preview { width, height, .. } = response else {
                return None;
            };
            let mut reader =
                image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png);
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(width);
            limits.max_image_height = Some(height);
            reader.limits(limits);
            let frame = reader.decode().ok()?.into_rgb8();
            (frame.dimensions() == (width, height) && !host.0.source_cancel.load(Ordering::Acquire))
                .then_some(frame)
        },
    ))
}

fn decode_preview(host: &Host, path: &Path, at_ms: u64) -> Result<Vec<u8>> {
    let (_, bytes) = native_request::supervise(
        &host.0.paths,
        path,
        Operation::Preview { at_ms },
        &host.0.source_cancel,
    )
    .map_err(|e| e.to_string())?;
    Ok(bytes)
}

pub(crate) async fn with_source_preview<T: Send + 'static>(
    host: &Host,
    token: String,
    preview: impl FnOnce(&Host, &Path) -> Result<T> + Send + 'static,
) -> Result<T> {
    struct Reset(Host);
    impl Drop for Reset {
        fn drop(&mut self) {
            self.0.0.source_busy.store(false, Ordering::Release);
        }
    }
    let busy = {
        let _operation = lock(&host.0.operation)?;
        if host.0.model_busy.load(Ordering::Acquire) || host.0.closing.load(Ordering::Acquire) {
            return Err("Wait for resource operations or shutdown before previewing".into());
        }
        if host.0.source_busy.swap(true, Ordering::AcqRel) {
            return Err("A source frame is already being decoded".into());
        }
        host.0.source_cancel.store(false, Ordering::Release);
        Reset(host.clone())
    };
    blocking(move || {
        let host = &busy.0;
        let path = host.source(&token)?;
        preview(host, &path)
    })
    .await
}

#[tauri::command]
pub fn current_project(host: tauri::State<'_, Host>) -> Result<Option<Project>> {
    Ok(lock(&host.inner().0.project)?
        .clone()
        .map(project_staging::for_webview))
}

#[tauri::command]
pub async fn replace_project(
    host: tauri::State<'_, Host>,
    files: tauri::State<'_, ProjectFiles>,
    project: Project,
    source_token: Option<String>,
) -> Result<()> {
    let host = host.inner().clone();
    let files = files.inner().clone();
    blocking(move || {
        let _operation = lock(&host.0.operation)?;
        host.ensure_idle()?;
        apply_project_edits(&host, &files, project, source_token.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn bind_project_source(host: tauri::State<'_, Host>) -> Result<Option<Source>> {
    let host = host.inner().clone();
    blocking(move || {
        let Some((path, fingerprint)) = lock(&host.0.project)?.as_ref().map(|project| {
            (
                PathBuf::from(&project.config.input),
                project.source_hash.clone(),
            )
        }) else {
            return Ok(None);
        };
        if !path.is_absolute() || !path.is_file() {
            return Ok(None);
        }
        grant_source_with(&host, &path, Some(&fingerprint), |path| {
            probe_source(&host, path)
        })
        .map(Some)
    })
    .await
}

fn confirm_replace(app: &tauri::AppHandle) -> bool {
    app.dialog()
        .message("This destination already exists. Replace the file?")
        .title("Confirm overwrite")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Replace".into(),
            "Cancel".into(),
        ))
        .blocking_show()
}

#[tauri::command]
pub async fn save_project(
    app: tauri::AppHandle,
    host: tauri::State<'_, Host>,
) -> Result<Option<String>> {
    save_project_command(app, host.inner().clone(), false).await
}

#[tauri::command]
pub async fn save_project_as(
    app: tauri::AppHandle,
    host: tauri::State<'_, Host>,
) -> Result<Option<String>> {
    save_project_command(app, host.inner().clone(), true).await
}

async fn save_project_command(
    app: tauri::AppHandle,
    host: Host,
    save_as: bool,
) -> Result<Option<String>> {
    blocking(move || save_active_project(&app, &host, save_as, None)).await
}

pub(crate) fn save_active_project(
    app: &tauri::AppHandle,
    host: &Host,
    save_as: bool,
    verify_close: Option<&dyn Fn() -> Result<()>>,
) -> Result<Option<String>> {
    let files = app.state::<ProjectFiles>();
    let (project, job, generation, active_path) = {
        let _operation = lock(&host.0.operation)?;
        if verify_close.is_none() {
            host.ensure_idle()?;
        } else if lock(&host.0.job)?.as_ref().is_some_and(|job| {
            job.thread
                .as_ref()
                .is_some_and(|worker| !worker.is_finished())
        }) {
            return Err(
                "Wait for the current extraction to finish before saving and exiting".into(),
            );
        }
        let project = lock(&host.0.project)?.clone().ok_or_else(|| {
            "No active project to save yet. Choose a video or open a project first.".to_string()
        })?;
        let job = job_id(host)?;
        let paths = lock(&files.0)?;
        let active_path = paths
            .active
            .as_ref()
            .filter(|(id, _)| id == &job)
            .map(|(_, path)| path.clone());
        (project, job, paths.generation, active_path)
    };
    let active_path = active_path.as_ref();
    let path = if let Some(path) = active_path.filter(|_| !save_as) {
        path.clone()
    } else {
        let name = active_path
            .and_then(|path| path.file_name().and_then(|name| name.to_str()))
            .map(project_filename)
            .unwrap_or_else(|| {
                project_filename(
                    Path::new(&project.config.input)
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or("subtitles"),
                )
            });
        let Some(file) = app
            .dialog()
            .file()
            .set_title("Save Numera project as")
            .add_filter("Numera project", &["nvsf", "nvsf.json", "json"])
            .set_file_name(name)
            .blocking_save_file()
        else {
            return Ok(None);
        };
        file.into_path().map_err(|e| e.to_string())?
    };
    state::path_text(&path)?;
    if !path.is_absolute() {
        return Err("Choose an absolute destination for the review project".into());
    }
    let prepared = AtomicOutput::for_confirmation(&path).map_err(|e| e.to_string())?;
    save_prepared_project(
        &project,
        &path,
        prepared,
        || Ok(confirm_replace(app)),
        |output, bytes| {
            if let Some(verify) = verify_close {
                verify()?;
            }
            let _operation = lock(&host.0.operation)?;
            if verify_close.is_none() {
                host.ensure_idle()?;
            }
            let mut paths = lock(&files.0)?;
            let active = lock(&host.0.project)?;
            if job_id(host)? != job
                || paths.generation != generation
                || active.as_ref().is_none_or(|current| {
                    current.cues != project.cues || current.config != project.config
                })
            {
                return Err(
                    "The workspace changed while choosing a project destination; save again".into(),
                );
            }
            write_project(output, bytes)?;
            let fingerprint = nvsf_app::providers::profiles::hash_bytes(bytes);
            paths.recent.retain(|(recent, _)| recent != &path);
            paths.recent.insert(0, (path.clone(), fingerprint));
            paths.recent.truncate(200);
            paths.generation = paths.generation.wrapping_add(1);
            paths.active = Some((job, path.clone()));
            host.0.unsaved.store(false, Ordering::Release);
            host.0.extract_only_dirty.store(false, Ordering::Release);
            Ok(())
        },
    )
}

pub(crate) fn project_filename(name: &str) -> String {
    let name = name
        .strip_suffix(".nvsf.json")
        .or_else(|| name.strip_suffix(".nvsf"))
        .or_else(|| name.strip_suffix(".json"))
        .unwrap_or(name);
    format!("{name}.nvsf")
}

#[tauri::command]
pub async fn export_project(
    app: tauri::AppHandle,
    host: tauri::State<'_, Host>,
    format: String,
    bom: bool,
    crlf: bool,
) -> Result<Option<String>> {
    host.ensure_idle()?;
    if !["srt", "vtt", "ass", "json"].contains(&format.as_str()) {
        return Err("Unsupported export format".into());
    }
    let project = lock(&host.inner().0.project)?
        .clone()
        .ok_or_else(|| "No project is open".to_string())?;
    blocking(move || {
        let Some(file) = app
            .dialog()
            .file()
            .set_title("Export corrected subtitles")
            .add_filter("Subtitle export", &[format.as_str()])
            .set_file_name(format!("subtitles.{format}"))
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = file.into_path().map_err(|e| e.to_string())?;
        export_project_to(&project, &path, &format, bom, crlf, || {
            Ok(confirm_replace(&app))
        })
    })
    .await
}

#[cfg(test)]
pub(crate) fn save_project_to(
    project: &Project,
    path: &Path,
    confirm: impl FnOnce() -> Result<bool>,
) -> Result<Option<String>> {
    let prepared = AtomicOutput::for_confirmation(path).map_err(|e| e.to_string())?;
    save_prepared_project(project, path, prepared, confirm, write_project)
}

pub(crate) fn write_project(output: AtomicOutput, bytes: &[u8]) -> Result<()> {
    output.write(bytes).map_err(|e| e.to_string())
}

pub(crate) fn save_prepared_project(
    project: &Project,
    path: &Path,
    prepared: (AtomicOutput, bool),
    confirm: impl FnOnce() -> Result<bool>,
    publish: impl FnOnce(AtomicOutput, &[u8]) -> Result<()>,
) -> Result<Option<String>> {
    let (output, overwrite) = prepared;
    output
        .protect_project_source(project)
        .map_err(|e| e.to_string())?;
    if overwrite && !confirm()? {
        return Ok(None);
    }
    publish(
        output,
        &serde_json::to_vec_pretty(project).map_err(|e| e.to_string())?,
    )?;
    Ok(Some(display_name(path)))
}

pub(crate) fn export_project_to(
    project: &Project,
    path: &std::path::Path,
    format: &str,
    bom: bool,
    crlf: bool,
    confirm: impl FnOnce() -> Result<bool>,
) -> Result<Option<String>> {
    let (prepared, overwrite) =
        nvsf_app::export::PreparedExport::for_confirmation(project, path, format, bom, crlf)
            .map_err(|e| e.to_string())?;
    if overwrite && !confirm()? {
        return Ok(None);
    }
    prepared.publish().map_err(|e| e.to_string())?;
    Ok(Some(display_name(path)))
}

#[tauri::command]
pub async fn pick_project(
    app: tauri::AppHandle,
    host: tauri::State<'_, Host>,
) -> Result<Option<Project>> {
    host.ensure_idle()?;
    let host = host.inner().clone();
    blocking(move || {
        let Some(file) = app
            .dialog()
            .file()
            .set_title("Open Numera project")
            .add_filter("Numera project", &["nvsf", "nvsf.json", "json"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = file.into_path().map_err(|e| e.to_string())?;
        let project = nvsf_app::storage::load_project(&path).map_err(|e| e.to_string())?;
        let _operation = lock(&host.0.operation)?;
        host.ensure_idle()?;
        bind_project(&host, &app.state::<ProjectFiles>(), path, &project)?;
        Ok(Some(project_staging::for_webview(project)))
    })
    .await
}

fn bind_project(host: &Host, files: &ProjectFiles, path: PathBuf, project: &Project) -> Result<()> {
    let job = job_id(host)?;
    let mut paths = lock(&files.0)?;
    let mut active = lock(&host.0.project)?;
    paths.generation = paths.generation.wrapping_add(1);
    paths.active = Some((job, path));
    *active = Some(project.clone());
    Ok(())
}

#[derive(Serialize)]
pub struct HistoryEntry {
    pub token: String,
    pub name: String,
    pub modified: Option<u64>,
}

#[tauri::command]
pub async fn list_history(
    host: tauri::State<'_, Host>,
    files: tauri::State<'_, ProjectFiles>,
) -> Result<Vec<HistoryEntry>> {
    let host = host.inner().clone();
    let files = files.inner().clone();
    blocking(move || {
        let root = host
            .0
            .paths
            .projects
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&root)
            .map_err(|e| e.to_string())?
            .take(2000)
        {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if !entry.file_type().map_err(|e| e.to_string())?.is_file()
                || !nvsf_app::storage::is_project_file(&path)
            {
                continue;
            }
            let canonical = path.canonicalize().map_err(|e| e.to_string())?;
            if !canonical.starts_with(&root) {
                continue;
            }
            entries.push((canonical, modified_at(&path), None));
        }
        let mut paths = lock(&files.0)?;
        for (path, fingerprint) in &paths.recent {
            if nvsf_app::storage::is_project_file(path)
                && std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
                && !entries.iter().any(|(entry, _, _)| entry == path)
            {
                entries.push((path.clone(), modified_at(path), Some(fingerprint.clone())));
            }
        }
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        let mut grants = lock(&host.0.history)?;
        grants.clear();
        paths.recent_grants.clear();
        Ok(entries
            .into_iter()
            .take(200)
            .map(|(path, modified, fingerprint)| {
                let token = host.token("history");
                grants.insert(token.clone(), path.clone());
                if let Some(fingerprint) = fingerprint {
                    paths.recent_grants.insert(token.clone(), fingerprint);
                }
                HistoryEntry {
                    token,
                    name: display_name(&path),
                    modified,
                }
            })
            .collect())
    })
    .await
}

fn modified_at(path: &Path) -> Option<u64> {
    path.metadata()
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|time| time.as_secs())
}

#[tauri::command]
pub async fn open_history(
    host: tauri::State<'_, Host>,
    files: tauri::State<'_, ProjectFiles>,
    token: String,
) -> Result<Project> {
    host.ensure_idle()?;
    let host = host.inner().clone();
    let files = files.inner().clone();
    let (path, fingerprint) = {
        let paths = lock(&files.0)?;
        let path = lock(&host.0.history)?
            .get(&token)
            .cloned()
            .ok_or_else(|| "Refresh history before opening this project".to_string())?;
        (path, paths.recent_grants.get(&token).cloned())
    };
    blocking(move || {
        let project = if let Some(expected) = fingerprint {
            open_recent(&path, &expected)?
        } else {
            nvsf_app::storage::load_project(&path).map_err(|e| e.to_string())?
        };
        let _operation = lock(&host.0.operation)?;
        host.ensure_idle()?;
        bind_project(&host, &files, path, &project)?;
        Ok(project_staging::for_webview(project))
    })
    .await
}

fn open_recent(path: &Path, expected: &str) -> Result<Project> {
    use std::io::Read;
    let changed = || "This saved project changed on disk. Reopen it with Open project.".to_string();
    if !std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file()) {
        return Err(changed());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK prevents a substituted FIFO from waiting for a writer at open.
        options.custom_flags(0x800);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err(changed());
    }
    const MAX_PROJECT_BYTES: u64 = 64 * 1024 * 1024;
    let mut bytes = Vec::new();
    file.take(MAX_PROJECT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_PROJECT_BYTES
        || nvsf_app::providers::profiles::hash_bytes(&bytes) != expected
    {
        return Err(changed());
    }
    let project: Project = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    project.validate().map_err(|e| e.to_string())?;
    Ok(project)
}
