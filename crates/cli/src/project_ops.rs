use nvsf_app::{AppError, Result};
use nvsf_core::{Cue, Project, merge_cues, split_cue};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(crate) fn find_cue(project: &Project, id: u64) -> Result<&Cue> {
    project
        .cues
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| nvsf_app::fail(format!("Cue {id} not found")))
}

pub(crate) fn find_cue_mut(project: &mut Project, id: u64) -> Result<&mut Cue> {
    project
        .cues
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| nvsf_app::fail(format!("Cue {id} not found")))
}

pub(crate) fn matches_cue(cue: &Cue, query_lower: &str, filter: &str) -> bool {
    let included = match filter {
        "unreviewed" => !cue.reviewed,
        "uncertain" => cue.confidence.is_none_or(|c| c < 0.7),
        "edited" => cue.edited,
        _ => true,
    };
    included && (query_lower.is_empty() || cue.text.to_lowercase().contains(query_lower))
}

pub(crate) fn inspect_value(project: &Project, review_needed: bool) -> Value {
    let total = project.cues.len();
    let reviewed = project.cues.iter().filter(|c| c.reviewed).count();
    let unreviewed = total - reviewed;
    let uncertain = project
        .cues
        .iter()
        .filter(|c| c.confidence.is_none_or(|v| v < 0.7))
        .count();
    let edited = project.cues.iter().filter(|c| c.edited).count();
    json!({
        "schema": project.schema,
        "source_hash": project.source_hash,
        "config_hash": project.config_hash,
        "model_hash": project.model_hash,
        "completed": project.completed,
        "processed_ms": project.processed_ms,
        "review_needed": review_needed,
        "warnings": project.warnings,
        "media": project.media,
        "config": project.config,
        "counts": {
            "total": total,
            "reviewed": reviewed,
            "unreviewed": unreviewed,
            "uncertain": uncertain,
            "edited": edited,
        },
    })
}

pub(crate) fn filter_cues_value(
    project: &Project,
    query: &str,
    filter: &str,
    offset: usize,
    limit: Option<usize>,
) -> Result<Value> {
    if !matches!(filter, "all" | "unreviewed" | "uncertain" | "edited") {
        return Err(AppError::InvalidConfig(
            "Filter must be all, unreviewed, uncertain or edited".into(),
        ));
    }
    let query_lower = query.to_lowercase();
    let matched: Vec<&Cue> = project
        .cues
        .iter()
        .filter(|c| matches_cue(c, &query_lower, filter))
        .collect();
    let page: Vec<&Cue> = match limit {
        Some(n) => matched.iter().skip(offset).take(n).copied().collect(),
        None => matched.iter().skip(offset).copied().collect(),
    };
    Ok(json!({
        "total": project.cues.len(),
        "matched": matched.len(),
        "offset": offset,
        "limit": limit,
        "filter": filter,
        "query": query,
        "cues": page,
    }))
}

pub(crate) fn edit_cue(
    project: &mut Project,
    id: u64,
    text: Option<String>,
    start_ms: Option<u64>,
    end_ms: Option<u64>,
) -> Result<Cue> {
    if text.is_none() && start_ms.is_none() && end_ms.is_none() {
        return Err(AppError::InvalidConfig(
            "edit-cue requires at least one of --text, --start-ms or --end-ms".into(),
        ));
    }
    let cue = find_cue_mut(project, id)?;
    let mut revised = cue.clone();
    if let Some(new_text) = text {
        revised.text = new_text;
    }
    if let Some(new_start) = start_ms {
        revised.start_ms = new_start;
    }
    if let Some(new_end) = end_ms {
        revised.end_ms = new_end;
    }
    revised.edited = true;
    revised.reviewed = false;
    revised.validate()?;
    *cue = revised.clone();
    project.validate()?;
    Ok(revised)
}

pub(crate) fn accept_cue(project: &mut Project, id: u64) -> Result<Cue> {
    let cue = find_cue_mut(project, id)?;
    cue.reviewed = true;
    let out = cue.clone();
    project.validate()?;
    Ok(out)
}

pub(crate) fn revert_cue(project: &mut Project, id: u64) -> Result<Cue> {
    let cue = find_cue_mut(project, id)?;
    let mut revised = cue.clone();
    revised.text = revised.raw_text.clone();
    revised.edited = false;
    revised.reviewed = false;
    revised.validate()?;
    *cue = revised.clone();
    project.validate()?;
    Ok(revised)
}

pub(crate) fn split_project_cue(project: &mut Project, id: u64, at_ms: u64) -> Result<Vec<Cue>> {
    split_cue(&mut project.cues, id, at_ms)?;
    project.validate()?;
    let idx = project
        .cues
        .iter()
        .position(|c| c.id == id)
        .ok_or_else(|| nvsf_app::fail("Split cue missing after split"))?;
    let pair = project
        .cues
        .get(idx..=(idx + 1))
        .ok_or_else(|| nvsf_app::fail("Split cue pair missing"))?
        .to_vec();
    Ok(pair)
}

pub(crate) fn merge_project_cues(project: &mut Project, first: u64, second: u64) -> Result<Cue> {
    merge_cues(&mut project.cues, first, second)?;
    project.validate()?;
    Ok(find_cue(project, first)?.clone())
}

pub(crate) fn list_history_value(projects_dir: &Path, limit: usize) -> Result<Value> {
    let root = projects_dir.canonicalize()?;
    let mut entries: Vec<(PathBuf, Option<u64>)> = Vec::new();
    for entry in std::fs::read_dir(&root)?.take(2000) {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() || path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let canonical = path.canonicalize()?;
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
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let total = entries.len();
    let bounded: Vec<Value> = entries
        .into_iter()
        .take(limit)
        .map(|(path, modified)| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            json!({
                "name": name,
                "path": path,
                "modified_unix_secs": modified,
            })
        })
        .collect();
    Ok(json!({
        "projects_dir": root,
        "total": total,
        "entries": bounded,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nvsf_core::{ExtractConfig, MediaInfo};

    fn sample_project() -> Project {
        let mut c1 = Cue::new(1, 0, 1000, 3000, "Hello world".into()).expect("cue 1");
        c1.confidence = Some(0.95);
        c1.reviewed = true;
        let mut c2 = Cue::new(2, 0, 3500, 6000, "Second line".into()).expect("cue 2");
        c2.confidence = Some(0.45);
        let mut c3 = Cue::new(3, 0, 6500, 9000, "Edited hello".into()).expect("cue 3");
        c3.raw_text = "Raw hello".into();
        c3.confidence = Some(0.88);
        c3.edited = true;
        Project {
            schema: 1,
            source_hash: "a".repeat(64),
            config_hash: "b".repeat(64),
            model_hash: "c".repeat(64),
            config: ExtractConfig {
                input: "in.mp4".into(),
                output: "out.srt".into(),
                ..Default::default()
            },
            media: MediaInfo {
                width: 1920,
                height: 1080,
                duration_ms: Some(10_000),
                codec: "h264".into(),
                time_base_num: 1,
                time_base_den: 1000,
                subtitle_streams: 0,
                rotation: 0,
            },
            cues: vec![c1, c2, c3],
            processed_ms: 10_000,
            active_appearances: vec![],
            completed: true,
            warnings: vec![],
        }
    }

    #[test]
    fn filter_and_search_cues_match_ui_semantics() {
        let proj = sample_project();
        let all = filter_cues_value(&proj, "hello", "all", 0, None).expect("query all");
        assert_eq!(all["matched"], 2);
        let unrev = filter_cues_value(&proj, "", "unreviewed", 0, None).expect("unreviewed");
        assert_eq!(unrev["matched"], 2);
        let unc = filter_cues_value(&proj, "", "uncertain", 0, None).expect("uncertain");
        assert_eq!(unc["matched"], 1);
        let ed = filter_cues_value(&proj, "", "edited", 0, None).expect("edited");
        assert_eq!(ed["matched"], 1);
        let paged = filter_cues_value(&proj, "", "all", 1, Some(1)).expect("paged");
        assert_eq!(paged["cues"].as_array().expect("arr").len(), 1);
        assert_eq!(paged["cues"][0]["id"], 2);
    }

    #[test]
    fn edit_accept_revert_split_merge_round_trip() {
        let mut proj = sample_project();
        let edited = edit_cue(&mut proj, 2, Some("Updated".into()), Some(3600), Some(5800))
            .expect("edit cue");
        assert!(edited.edited);
        assert!(!edited.reviewed);
        assert_eq!(edited.text, "Updated");
        assert_eq!(edited.raw_text, "Second line");

        let accepted = accept_cue(&mut proj, 2).expect("accept cue");
        assert!(accepted.reviewed);

        let reverted = revert_cue(&mut proj, 2).expect("revert cue");
        assert_eq!(reverted.text, "Second line");
        assert!(!reverted.edited);
        assert!(!reverted.reviewed);

        let pair = split_project_cue(&mut proj, 2, 4700).expect("split cue");
        assert_eq!(pair.len(), 2);
        assert_eq!(pair[0].end_ms, 4700);
        assert_eq!(pair[1].start_ms, 4700);
        let split_second_id = pair[1].id;

        let merged = merge_project_cues(&mut proj, 2, split_second_id).expect("merge cues");
        assert_eq!(merged.start_ms, 3600);
        assert_eq!(merged.end_ms, 5800);
        assert_eq!(proj.cues.len(), 3);
    }
}
