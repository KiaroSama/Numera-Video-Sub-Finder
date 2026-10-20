use crate::state::{self, Destination, Host, Result, Source, blocking, display_name, lock};
use image::{DynamicImage, ImageFormat};
use nvsf_core::Project;
use serde::Serialize;
use std::{io::Cursor, path::PathBuf};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

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
            nvsf_app::media::MediaRuntime::load(&host.0.paths)
                .and_then(|runtime| runtime.probe(path))
                .map_err(|e| e.to_string())
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
    let media = probe(&path)?;
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
    let host = host.inner().clone();
    let operation = lock(&host.0.operation)?;
    if host.0.model_busy.load(std::sync::atomic::Ordering::Acquire)
        || host.0.closing.load(std::sync::atomic::Ordering::Acquire)
    {
        return Err("Wait for resource operations or shutdown before previewing".into());
    }
    if host
        .0
        .source_busy
        .swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        return Err("A source frame is already being decoded".into());
    }
    drop(operation);
    blocking(move || {
        struct Reset<'a>(&'a std::sync::atomic::AtomicBool);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.store(false, std::sync::atomic::Ordering::Release);
            }
        }
        let _reset = Reset(&host.0.source_busy);
        let path = host.source(&token)?;
        let rgb = nvsf_app::media::MediaRuntime::load(&host.0.paths)
            .and_then(|runtime| runtime.preview(&path, time_ms))
            .map_err(|e| e.to_string())?;
        if u64::from(rgb.width()) * u64::from(rgb.height()) > 33_177_600 {
            return Err("Preview exceeds 8K pixel limit".into());
        }
        let mut buffer = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(rgb)
            .write_to(&mut buffer, ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        let bytes = buffer.into_inner();
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("Preview exceeds 8MiB IPC bound; use a smaller source frame".into());
        }
        Ok(tauri::ipc::Response::new(bytes))
    })
    .await
}

#[tauri::command]
pub fn current_project(host: tauri::State<'_, Host>) -> Result<Option<Project>> {
    Ok(lock(&host.inner().0.project)?.clone())
}

#[tauri::command]
pub fn replace_project(host: tauri::State<'_, Host>, project: Project) -> Result<()> {
    let host = host.inner();
    let _operation = lock(&host.0.operation)?;
    host.ensure_idle()?;
    project.validate().map_err(|e| e.to_string())?;
    if serde_json::to_vec(&project)
        .map_err(|e| e.to_string())?
        .len()
        > 64 * 1024 * 1024
    {
        return Err("Project exceeds 64MiB workspace bound".into());
    }
    let mut current = lock(&host.0.project)?;
    let old = current
        .as_ref()
        .ok_or_else(|| "No project is open".to_string())?;
    if old.source_hash != project.source_hash
        || old.config_hash != project.config_hash
        || old.model_hash != project.model_hash
        || serde_json::to_value(&old.config).map_err(|e| e.to_string())?
            != serde_json::to_value(&project.config).map_err(|e| e.to_string())?
    {
        return Err("Only cue corrections may replace the current project; source/config/model identity must match".into());
    }
    if let Some(current) = current.as_mut() {
        current.cues = project.cues;
    }
    Ok(())
}

fn confirm_replace(app: &tauri::AppHandle, path: &std::path::Path) -> bool {
    !path.exists()
        || app
            .dialog()
            .message("This destination already exists. Replace it with your current corrections?")
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
    host.ensure_idle()?;
    let project = lock(&host.inner().0.project)?
        .clone()
        .ok_or_else(|| "No project is open".to_string())?;
    blocking(move || {
        let Some(file) = app
            .dialog()
            .file()
            .set_title("Save review project")
            .add_filter("Numera project", &["json"])
            .set_file_name("subtitles.nvsf.json")
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = file.into_path().map_err(|e| e.to_string())?;
        save_project_to(&project, &path, || Ok(confirm_replace(&app, &path)))
    })
    .await
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
            Ok(confirm_replace(&app, &path))
        })
    })
    .await
}

pub(crate) fn save_project_to(
    project: &Project,
    path: &std::path::Path,
    confirm: impl FnOnce() -> Result<bool>,
) -> Result<Option<String>> {
    let overwrite = path.exists();
    if !confirm()? {
        return Ok(None);
    }
    nvsf_app::storage::write_atomic(
        path,
        &serde_json::to_vec_pretty(project).map_err(|e| e.to_string())?,
        overwrite,
    )
    .map_err(|e| e.to_string())?;
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
    let overwrite = path.exists();
    if !confirm()? {
        return Ok(None);
    }
    nvsf_app::export_project(project, path, format, overwrite, bom, crlf)
        .map_err(|e| e.to_string())?;
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
            .set_title("Open review project")
            .add_filter("Numera project", &["json"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = file.into_path().map_err(|e| e.to_string())?;
        let project = nvsf_app::storage::load_project(&path).map_err(|e| e.to_string())?;
        let _operation = lock(&host.0.operation)?;
        host.ensure_idle()?;
        *lock(&host.0.project)? = Some(project.clone());
        Ok(Some(project))
    })
    .await
}

#[derive(Serialize)]
pub struct HistoryEntry {
    pub token: String,
    pub name: String,
    pub modified: Option<u64>,
}

#[tauri::command]
pub async fn list_history(host: tauri::State<'_, Host>) -> Result<Vec<HistoryEntry>> {
    let host = host.inner().clone();
    blocking(move || {
        let root = host
            .0
            .paths
            .projects
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let mut entries: Vec<(PathBuf, Option<u64>)> = Vec::new();
        for entry in std::fs::read_dir(&root)
            .map_err(|e| e.to_string())?
            .take(2000)
        {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if !entry.file_type().map_err(|e| e.to_string())?.is_file()
                || path.extension().is_none_or(|e| e != "json")
            {
                continue;
            }
            let canonical = path.canonicalize().map_err(|e| e.to_string())?;
            if !canonical.starts_with(&root) {
                continue;
            }
            let modified = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            entries.push((canonical, modified));
        }
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        let mut grants = lock(&host.0.history)?;
        grants.clear();
        Ok(entries
            .into_iter()
            .take(200)
            .map(|(path, modified)| {
                let token = host.token("history");
                grants.insert(token.clone(), path.clone());
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

#[tauri::command]
pub async fn open_history(host: tauri::State<'_, Host>, token: String) -> Result<Project> {
    host.ensure_idle()?;
    let host = host.inner().clone();
    let path = lock(&host.0.history)?
        .get(&token)
        .cloned()
        .ok_or_else(|| "Refresh history before opening this project".to_string())?;
    blocking(move || {
        let project = nvsf_app::storage::load_project(&path).map_err(|e| e.to_string())?;
        let _operation = lock(&host.0.operation)?;
        host.ensure_idle()?;
        *lock(&host.0.project)? = Some(project.clone());
        Ok(project)
    })
    .await
}
