use crate::{ProjectCommand, project_ops, status};
use nvsf_app::{
    AppError, Control, Paths, Result, assistant, crop_evidence,
    native_request::{self, Operation, Response},
    storage,
};
use nvsf_core::{Project, Rect};
use serde_json::json;
use std::path::Path;

pub(crate) fn run(paths: &Paths, command: ProjectCommand, control: &Control) -> Result<i32> {
    match command {
        ProjectCommand::Inspect { project } => {
            let loaded = storage::load_project(&project)?;
            let code = status::project(&loaded);
            println!(
                "{}",
                serde_json::to_string_pretty(&project_ops::inspect_value(&loaded, code == 2))?
            );
            Ok(0)
        }
        ProjectCommand::Cues {
            project,
            query,
            filter,
            offset,
            limit,
        } => {
            let loaded = storage::load_project(&project)?;
            let value = project_ops::filter_cues_value(&loaded, &query, &filter, offset, limit)?;
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(0)
        }
        ProjectCommand::EditCue {
            project,
            id,
            text,
            start_ms,
            end_ms,
            output,
        } => {
            let mut loaded = storage::load_project(&project)?;
            let updated = project_ops::edit_cue(&mut loaded, id, text, start_ms, end_ms)?;
            let dest = output.as_deref().unwrap_or(&project);
            storage::save_project(&loaded, dest)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "saved": dest,
                    "cue": updated,
                }))?
            );
            Ok(0)
        }
        ProjectCommand::AcceptCue {
            project,
            id,
            output,
        } => {
            let mut loaded = storage::load_project(&project)?;
            let updated = project_ops::accept_cue(&mut loaded, id)?;
            let dest = output.as_deref().unwrap_or(&project);
            storage::save_project(&loaded, dest)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "saved": dest,
                    "cue": updated,
                }))?
            );
            Ok(0)
        }
        ProjectCommand::RevertCue {
            project,
            id,
            output,
        } => {
            let mut loaded = storage::load_project(&project)?;
            let updated = project_ops::revert_cue(&mut loaded, id)?;
            let dest = output.as_deref().unwrap_or(&project);
            storage::save_project(&loaded, dest)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "saved": dest,
                    "cue": updated,
                }))?
            );
            Ok(0)
        }
        ProjectCommand::SplitCue {
            project,
            id,
            at_ms,
            output,
        } => {
            let mut loaded = storage::load_project(&project)?;
            let pair = project_ops::split_project_cue(&mut loaded, id, at_ms)?;
            let dest = output.as_deref().unwrap_or(&project);
            storage::save_project(&loaded, dest)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "saved": dest,
                    "split_id": id,
                    "at_ms": at_ms,
                    "total_cues": loaded.cues.len(),
                    "cues": pair,
                }))?
            );
            Ok(0)
        }
        ProjectCommand::MergeCues {
            project,
            first,
            second,
            output,
        } => {
            let mut loaded = storage::load_project(&project)?;
            let merged = project_ops::merge_project_cues(&mut loaded, first, second)?;
            let dest = output.as_deref().unwrap_or(&project);
            storage::save_project(&loaded, dest)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "saved": dest,
                    "merged_id": first,
                    "removed_id": second,
                    "total_cues": loaded.cues.len(),
                    "cue": merged,
                }))?
            );
            Ok(0)
        }
        ProjectCommand::ReOcr {
            project,
            id,
            input,
            language,
            region,
            at_ms,
            apply,
            output,
        } => run_re_ocr(
            paths,
            &project,
            id,
            &input,
            language,
            region,
            at_ms,
            apply,
            output.as_deref(),
            control,
        ),
        ProjectCommand::VisualRetry {
            project,
            id,
            input,
            opt_in,
            region,
            at_ms,
            apply,
            output,
        } => run_visual_retry(
            paths,
            &project,
            id,
            &input,
            opt_in,
            region,
            at_ms,
            apply,
            output.as_deref(),
            control,
        ),
        ProjectCommand::VisualRetryInstall { opt_in } => {
            if !opt_in {
                return Err(AppError::InvalidConfig(
                    "Visual retry model installation requires --opt-in".into(),
                ));
            }
            assistant::install(paths, &control.cancel)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "installed": true,
                    "models_dir": paths.models.join("assistant"),
                }))?
            );
            Ok(0)
        }
        ProjectCommand::CueEvidence {
            project,
            id,
            output,
            overwrite,
        } => {
            let loaded = storage::load_project(&project)?;
            let cue = project_ops::find_cue(&loaded, id)?;
            let crop = cue
                .crop
                .as_deref()
                .ok_or_else(|| nvsf_app::fail("Cue has no saved crop evidence"))?;
            let bytes = crop_evidence::read(paths, crop)?;
            storage::write_atomic(&output, &bytes, overwrite)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "cue_id": id,
                    "output": output,
                    "bytes": bytes.len(),
                }))?
            );
            Ok(0)
        }
        ProjectCommand::Save { project, output } => {
            let loaded = storage::load_project(&project)?;
            let dest = output.as_deref().unwrap_or(&project);
            storage::save_project(&loaded, dest)?;
            let review_needed = status::project(&loaded) == 2;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "saved": dest,
                    "cues": loaded.cues.len(),
                    "completed": loaded.completed,
                    "review_needed": review_needed,
                }))?
            );
            Ok(0)
        }
        ProjectCommand::History { limit } => {
            let value = project_ops::list_history_value(&paths.projects, limit)?;
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(0)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_re_ocr(
    paths: &Paths,
    project_path: &Path,
    id: u64,
    input: &Path,
    language: Option<String>,
    region: Option<Rect>,
    at_ms: Option<u64>,
    apply: bool,
    output: Option<&Path>,
    control: &Control,
) -> Result<i32> {
    let mut loaded = storage::load_project(project_path)?;
    verify_source_hash(&loaded, input)?;
    let (cue_start, cue_track) = {
        let cue = project_ops::find_cue(&loaded, id)?;
        (cue.start_ms, cue.track)
    };
    let lang = language.unwrap_or_else(|| loaded.config.language.clone());
    if nvsf_core::languages::find(&lang).is_none() {
        return Err(AppError::InvalidConfig(
            "Select a language from the official catalog".into(),
        ));
    }
    let rect = resolve_region(&loaded, cue_track, region)?;
    let time_ms = at_ms.unwrap_or(cue_start);
    let (response, _) = native_request::supervise(
        paths,
        input,
        Operation::Recognize {
            at_ms: time_ms,
            region: rect,
            language: lang.clone(),
        },
        &control.cancel,
    )?;
    let recognition = match response {
        Response::Recognize(r) => r,
        _ => return Err(nvsf_app::fail("Unexpected cue recognition response")),
    };
    let saved = if apply {
        let cue = project_ops::find_cue_mut(&mut loaded, id)?;
        let mut revised = cue.clone();
        revised.text = recognition.text.clone();
        revised.confidence = recognition.confidence;
        revised.edited = true;
        revised.reviewed = false;
        revised.validate()?;
        *cue = revised;
        let dest = output.unwrap_or(project_path);
        storage::save_project(&loaded, dest)?;
        Some(dest.to_path_buf())
    } else {
        None
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "cue_id": id,
            "at_ms": time_ms,
            "language": lang,
            "region": rect,
            "recognition": recognition,
            "applied": apply,
            "saved": saved,
        }))?
    );
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
fn run_visual_retry(
    paths: &Paths,
    project_path: &Path,
    id: u64,
    input: &Path,
    opt_in: bool,
    region: Option<Rect>,
    at_ms: Option<u64>,
    apply: bool,
    output: Option<&Path>,
    control: &Control,
) -> Result<i32> {
    if !opt_in {
        return Err(AppError::InvalidConfig(
            "Local visual retry requires explicit --opt-in".into(),
        ));
    }
    let mut loaded = storage::load_project(project_path)?;
    verify_source_hash(&loaded, input)?;
    let (cue_start, cue_track) = {
        let cue = project_ops::find_cue(&loaded, id)?;
        (cue.start_ms, cue.track)
    };
    let rect = resolve_region(&loaded, cue_track, region)?;
    let time_ms = at_ms.unwrap_or(cue_start);
    let proposal = assistant::propose(paths, input, time_ms, rect, true, &control.cancel)?;
    let mut applied = false;
    let mut saved = None;
    if apply {
        if proposal.unreadable {
            return Err(nvsf_app::fail(
                "Visual retry reported crop as unreadable; current cue preserved",
            ));
        }
        let proposed_text = proposal
            .text
            .clone()
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| nvsf_app::fail("Visual retry produced empty text; cue preserved"))?;
        let cue = project_ops::find_cue_mut(&mut loaded, id)?;
        let mut revised = cue.clone();
        revised.text = proposed_text;
        revised.edited = true;
        revised.reviewed = false;
        revised.validate()?;
        *cue = revised;
        let dest = output.unwrap_or(project_path);
        storage::save_project(&loaded, dest)?;
        applied = true;
        saved = Some(dest.to_path_buf());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "cue_id": id,
            "at_ms": time_ms,
            "region": rect,
            "proposal": proposal,
            "applied": applied,
            "saved": saved,
        }))?
    );
    Ok(0)
}

fn verify_source_hash(project: &Project, input: &Path) -> Result<()> {
    let hash = storage::hash_file(input)?;
    if hash != project.source_hash {
        return Err(nvsf_app::fail(
            "Linked video does not match the project's source fingerprint",
        ));
    }
    Ok(())
}

fn resolve_region(project: &Project, track: u32, override_region: Option<Rect>) -> Result<Rect> {
    let rect = override_region
        .or_else(|| project.config.regions.get(track as usize).copied())
        .ok_or_else(|| AppError::InvalidConfig("Cue track has no configured region".into()))?;
    rect.validate()
        .map_err(|e| AppError::InvalidConfig(e.to_string()))?;
    Ok(rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProjectCommand;
    use nvsf_core::{Cue, ExtractConfig, MediaInfo};
    use std::path::PathBuf;

    fn sample_project(video: &Path, hash: String) -> Project {
        let mut cfg = ExtractConfig {
            input: video.to_string_lossy().into_owned(),
            output: video.with_extension("srt").to_string_lossy().into_owned(),
            ..ExtractConfig::default()
        };
        cfg.regions = vec![Rect::default()];
        let cue = Cue::new(1, 0, 1000, 3000, "Hello".into()).expect("cue");
        Project {
            schema: 1,
            source_hash: hash,
            config_hash: "b".repeat(64),
            model_hash: "c".repeat(64),
            config: cfg,
            media: MediaInfo {
                width: 1280,
                height: 720,
                duration_ms: Some(10_000),
                codec: "h264".into(),
                time_base_num: 1,
                time_base_den: 1000,
                subtitle_streams: 0,
                rotation: 0,
            },
            cues: vec![cue],
            processed_ms: 10_000,
            active_appearances: Vec::new(),
            completed: true,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn source_hash_region_resolution_and_fail_closed_retry_paths() {
        let dir = std::env::temp_dir().join(format!("nvsf-proj-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let paths = Paths::new(Some(&dir)).expect("paths");
        let video = dir.join("clip.mp4");
        std::fs::write(&video, b"deterministic-video-bytes").expect("write video");
        let other = dir.join("other.mp4");
        std::fs::write(&other, b"different-video-bytes").expect("write other");

        let hash = storage::hash_file(&video).expect("hash");
        let proj = sample_project(&video, hash);
        let proj_path = dir.join("clip.nvsf.json");
        storage::save_project(&proj, &proj_path).expect("save project");

        verify_source_hash(&proj, &video).expect("matching hash");
        assert!(verify_source_hash(&proj, &other).is_err());

        let r0 = resolve_region(&proj, 0, None).expect("track 0 region");
        assert_eq!(r0, Rect::default());
        let custom = Rect::new(0.1, 0.8, 0.8, 0.15).expect("custom");
        assert_eq!(
            resolve_region(&proj, 9, Some(custom)).expect("override"),
            custom
        );
        assert!(resolve_region(&proj, 9, None).is_err());

        // Missing cue ID and mismatched source hash fail closed in ReOcr and VisualRetry.
        let control = Control::default();
        assert!(
            run(
                &paths,
                ProjectCommand::ReOcr {
                    project: proj_path.clone(),
                    id: 999,
                    input: video.clone(),
                    language: None,
                    region: None,
                    at_ms: None,
                    apply: false,
                    output: None,
                },
                &control,
            )
            .is_err()
        );
        assert!(
            run(
                &paths,
                ProjectCommand::ReOcr {
                    project: proj_path.clone(),
                    id: 1,
                    input: other.clone(),
                    language: None,
                    region: None,
                    at_ms: None,
                    apply: false,
                    output: None,
                },
                &control,
            )
            .is_err()
        );
        assert!(
            run(
                &paths,
                ProjectCommand::VisualRetry {
                    project: proj_path.clone(),
                    id: 1,
                    input: video.clone(),
                    opt_in: false,
                    region: None,
                    at_ms: None,
                    apply: false,
                    output: None,
                },
                &control,
            )
            .is_err()
        );
        assert!(
            run(
                &paths,
                ProjectCommand::VisualRetry {
                    project: proj_path.clone(),
                    id: 999,
                    input: video.clone(),
                    opt_in: true,
                    region: None,
                    at_ms: None,
                    apply: false,
                    output: None,
                },
                &control,
            )
            .is_err()
        );
        assert!(
            run(
                &paths,
                ProjectCommand::VisualRetry {
                    project: proj_path,
                    id: 1,
                    input: video,
                    opt_in: true,
                    region: None,
                    at_ms: None,
                    apply: false,
                    output: Some(PathBuf::from("unused.nvsf.json")),
                },
                &control,
            )
            .is_err()
        );
        assert!(
            run(
                &paths,
                ProjectCommand::VisualRetryInstall { opt_in: false },
                &control,
            )
            .is_err()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
