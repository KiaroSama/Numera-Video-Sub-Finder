//! Compiled only in CI's e2e artifact. Production has neither this command nor a driver.
use crate::state::{Destination, Host, Result, Source, display_name, lock};
use serde::Serialize;
use std::path::{Component, Path, PathBuf};

#[derive(Serialize)]
pub struct FixtureGrant {
    pub source: Source,
    pub output: Destination,
}

fn fixture_path(root: &Path, name: &str, existing: bool) -> Result<PathBuf> {
    if name.is_empty()
        || name.len() > 256
        || name.contains(['\0', '\\', ':'])
        || Path::new(name)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("Fixture path must be a relative name without traversal".into());
    }
    let candidate = root.join(name);
    let checked = if existing {
        candidate.canonicalize()
    } else {
        candidate
            .parent()
            .ok_or_else(|| "Missing fixture parent".to_string())?
            .canonicalize()
            .map(|parent| parent.join(candidate.file_name().unwrap_or_default()))
    }
    .map_err(|e| e.to_string())?;
    if !checked.starts_with(root) {
        return Err("Fixture path escapes its fixed CI root".into());
    }
    if checked.exists()
        && !checked
            .canonicalize()
            .map_err(|e| e.to_string())?
            .starts_with(root)
    {
        return Err("Fixture destination resolves outside CI root".into());
    }
    Ok(checked)
}

#[tauri::command]
pub async fn e2e_fixture(
    host: tauri::State<'_, Host>,
    fixture: String,
    output: String,
) -> Result<FixtureGrant> {
    let host = host.inner().clone();
    crate::state::blocking(move || {
        let _operation = lock(&host.0.operation)?;
        host.ensure_idle()?;
        let root = std::env::var_os("NVSF_E2E_FIXTURE_ROOT")
            .ok_or_else(|| "CI fixture root not configured".to_string())?;
        let root = PathBuf::from(root)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        if !root.is_dir() {
            return Err("CI fixture root must be an existing directory".into());
        }
        let source_path = fixture_path(&root, &fixture, true)?;
        let output_path = fixture_path(&root, &output, false)?;
        if !source_path.is_file()
            || output_path.extension().is_none_or(|e| e != "srt")
            || source_path == output_path
        {
            return Err("CI fixture needs a regular input and distinct SRT output".into());
        }
        let media = nvsf_app::media::MediaRuntime::load(&host.0.paths)
            .and_then(|runtime| runtime.probe(&source_path))
            .map_err(|e| e.to_string())?;
        let source_token = host.token("source");
        let output_token = host.token("output");
        lock(&host.0.sources)?.insert(source_token.clone(), source_path.clone());
        lock(&host.0.destinations)?.insert(output_token.clone(), output_path.clone());
        Ok(FixtureGrant {
            source: Source {
                token: source_token,
                name: display_name(&source_path),
                fingerprint: nvsf_app::storage::hash_file(&source_path)
                    .map_err(|e| e.to_string())?,
                media,
            },
            output: Destination {
                token: output_token,
                name: display_name(&output_path),
            },
        })
    })
    .await
}

#[tauri::command]
pub async fn e2e_save_edits(host: tauri::State<'_, Host>, output_token: String) -> Result<String> {
    let host = host.inner().clone();
    crate::state::blocking(move || {
        let _operation = lock(&host.0.operation)?;
        host.ensure_idle()?;
        let root = PathBuf::from(
            std::env::var_os("NVSF_E2E_FIXTURE_ROOT")
                .ok_or_else(|| "CI fixture root not configured".to_string())?,
        )
        .canonicalize()
        .map_err(|e| e.to_string())?;
        let fixed = fixture_path(&root, "ui-en.srt", false)?;
        let output = host.destination(&output_token)?;
        if output != fixed {
            return Err(
                "CI export accepts only the already granted fixed ui-en.srt destination".into(),
            );
        }
        let project = lock(&host.0.project)?
            .clone()
            .ok_or_else(|| "No review project is open".to_string())?;
        nvsf_app::export_project(&project, &output, "srt", true, false, false)
            .map_err(|e| e.to_string())?;
        Ok(display_name(&output))
    })
    .await
}

#[tauri::command]
pub fn e2e_focus(window: tauri::WebviewWindow) -> Result<bool> {
    window.set_focus().map_err(|e| e.to_string())?;
    window.is_focused().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unscoped_fixture_names_before_io() {
        for name in ["../input.mp4", "/input.mp4", "C:\\input.mp4", "input\0.mp4"] {
            assert!(fixture_path(Path::new("fixture-root"), name, true).is_err());
        }
    }
}
