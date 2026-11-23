use crate::{Result, fail, storage::AtomicOutput};
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
    export_project_checked(project, path, format, overwrite, bom, crlf, |_| Ok(()))
}

pub(crate) fn export_project_checked(
    project: &Project,
    path: &Path,
    format: &str,
    overwrite: bool,
    bom: bool,
    crlf: bool,
    mut admit: impl FnMut(&AtomicOutput) -> Result<()>,
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
    // Every generated track shares the primary's pinned parent. Admit the complete
    // set before publication, retaining each writer through both checks and rename.
    let mut prepared: Vec<(AtomicOutput, String)> = Vec::with_capacity(outputs.len());
    for (destination, text) in outputs {
        let output = match prepared.first() {
            Some((primary, _)) => primary.sibling(
                destination
                    .file_name()
                    .ok_or_else(|| fail("Missing export filename"))?,
            )?,
            None => AtomicOutput::new(&destination, overwrite)?,
        };
        output.protect_project_source(project)?;
        admit(&output)?;
        prepared.push((output, text));
    }
    // Each file is atomic; filesystems cannot atomically rename several separate track files.
    // Write separated tracks first and publish the selected primary track last.
    prepared.rotate_left(1);
    for (output, text) in prepared {
        output.write(text.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "export_publication_tests.rs"]
mod publication_tests;
