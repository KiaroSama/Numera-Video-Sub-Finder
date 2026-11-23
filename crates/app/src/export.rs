use crate::{Result, fail, storage::AtomicOutput};
use nvsf_core::{Project, export_ass, export_srt, export_vtt};
use std::path::Path;

/// Retains every validated output and its admission policy until publication.
pub struct PreparedExport {
    outputs: Vec<(AtomicOutput, String)>,
}

impl PreparedExport {
    /// Binds the complete output set before asking whether existing entries may be replaced.
    pub fn for_confirmation(
        project: &Project,
        path: &Path,
        format: &str,
        bom: bool,
        crlf: bool,
    ) -> Result<(Self, bool)> {
        let prepared = prepare_export(project, path, format, true, bom, crlf, |_| Ok(()))?;
        let requires_confirmation = prepared
            .outputs
            .iter()
            .any(|(output, _)| output.replaces_existing());
        Ok((prepared, requires_confirmation))
    }

    pub fn publish(mut self) -> Result<()> {
        // Each file is atomic; filesystems cannot atomically rename several separate track files.
        // Write separated tracks first and publish the selected primary track last.
        self.outputs.rotate_left(1);
        for (output, text) in self.outputs {
            output.write(text.as_bytes())?;
        }
        Ok(())
    }
}

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
    admit: impl FnMut(&AtomicOutput) -> Result<()>,
) -> Result<()> {
    prepare_export(project, path, format, overwrite, bom, crlf, admit)?.publish()
}

fn prepare_export(
    project: &Project,
    path: &Path,
    format: &str,
    overwrite: bool,
    bom: bool,
    crlf: bool,
    mut admit: impl FnMut(&AtomicOutput) -> Result<()>,
) -> Result<PreparedExport> {
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
    Ok(PreparedExport { outputs: prepared })
}

#[cfg(test)]
#[path = "export_publication_tests.rs"]
mod publication_tests;
