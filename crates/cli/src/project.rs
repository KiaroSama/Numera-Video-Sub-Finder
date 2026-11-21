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
