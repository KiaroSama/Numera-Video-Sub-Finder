use crate::{Result, fail, storage::write_atomic};
use nvsf_core::{Project, export_ass, export_srt, export_vtt};
use std::path::Path;

pub fn export_project(
    project: &Project,
    path: &Path,
    format: &str,
    overwrite: bool,
    bom: bool,
    crlf: bool,
) -> Result<()> {
    project.validate()?;
    let text = match format {
        "srt" => export_srt(&project.cues, bom, crlf)?,
        "vtt" => export_vtt(&project.cues)?,
        "ass" => export_ass(&project.cues)?,
        "json" => serde_json::to_string_pretty(project)?,
        _ => return Err(fail("Choose srt, vtt, ass or json")),
    };
    let mut outputs = vec![(path.to_path_buf(), text)];
    if format != "json" {
        let tracks: std::collections::BTreeSet<_> = project.cues.iter().map(|c| c.track).collect();
        if tracks.len() > 1 {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| fail("Track export needs a UTF-8 filename"))?;
            for track in tracks {
                let cues: Vec<_> = project
                    .cues
                    .iter()
                    .filter(|c| c.track == track)
                    .cloned()
                    .collect();
                outputs.push((
                    path.with_file_name(format!("{stem}.track-{track}.{format}")),
                    match format {
                        "srt" => export_srt(&cues, bom, crlf)?,
                        "vtt" => export_vtt(&cues)?,
                        "ass" => export_ass(&cues)?,
                        _ => unreachable!(),
                    },
                ));
            }
            // The selected path remains usable, but never silently combines distinct tracks.
            outputs[0].1 = outputs[1].1.clone();
        }
    }
    // Preflight the complete set before any export can replace an existing file.
    for (destination, _) in &outputs {
        if destination.is_symlink() || destination.is_dir() || (!overwrite && destination.exists())
        {
            return Err(fail(
                "Export set contains an existing or unsafe destination; no output changed",
            ));
        }
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if !parent.is_dir() {
            return Err(fail("Export destination directory unavailable"));
        }
    }
    // Each file is atomic; filesystems cannot atomically rename several separate track files.
    // Write separated tracks first and publish the selected primary track last.
    for (destination, text) in outputs.iter().skip(1).chain(outputs.iter().take(1)) {
        write_atomic(destination, text.as_bytes(), overwrite)?;
    }
    Ok(())
}
