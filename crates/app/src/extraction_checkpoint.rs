use crate::{Result, fail, storage::JobLock};
use std::path::{Path, PathBuf};

pub(super) struct Checkpoint {
    pub(super) path: PathBuf,
    required: bool,
    output_lock_path: PathBuf,
    selected_lock_path: PathBuf,
    _output_lock: JobLock,
    _selected_lock: Option<JobLock>,
}

fn resolved_destination(path: &Path) -> Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let name = path
                .file_name()
                .ok_or_else(|| fail("Missing destination filename"))?;
            Ok(parent.canonicalize()?.join(name))
        }
        Err(error) => Err(error.into()),
    }
}

impl Checkpoint {
    pub(super) fn open(output: &Path, selected: Option<&Path>, source: &Path) -> Result<Self> {
        let default_name = output.with_extension("nvsf.json");
        let output_lock = resolved_destination(&default_name.with_extension("lock"))?;
        let default = resolved_destination(&default_name)?;
        let path = match selected {
            Some(selected) => {
                let path = selected.canonicalize()?;
                if !path.is_file() {
                    return Err(fail("Selected checkpoint must be a regular file"));
                }
                path
            }
            None => default.clone(),
        };
        let output = resolved_destination(output)?;
        let selected_lock = resolved_destination(&path.with_extension("lock"))?;
        if path == output
            || path == output_lock
            || path == selected_lock
            || output_lock == output
            || selected_lock == output
        {
            return Err(fail(
                "Checkpoint and its locks must differ from the export destination",
            ));
        }
        for destination in [&path, &output_lock, &selected_lock] {
            crate::storage::protect_source(source, destination)?;
        }
        // Copies of one project still own the same output, so preserve the shared output lock.
        let output_guard = JobLock::acquire(&output_lock)?;
        let selected_guard = if selected_lock == output_lock {
            None
        } else {
            Some(JobLock::acquire(&selected_lock)?)
        };
        Ok(Self {
            path,
            required: selected.is_some(),
            output_lock_path: output_lock,
            selected_lock_path: selected_lock,
            _output_lock: output_guard,
            _selected_lock: selected_guard,
        })
    }

    pub(super) fn load(&self) -> Result<Option<nvsf_core::Project>> {
        if self.required || self.path.is_file() {
            crate::storage::load_project(&self.path).map(Some)
        } else {
            Ok(None)
        }
    }

    pub(super) fn export(
        &self,
        project: &nvsf_core::Project,
        output: &Path,
        overwrite: bool,
        bom: bool,
        crlf: bool,
    ) -> Result<()> {
        crate::export::export_project_checked(
            project,
            output,
            "srt",
            overwrite,
            bom,
            crlf,
            |destination| {
                for reserved in [&self.path, &self.output_lock_path, &self.selected_lock_path] {
                    if destination.conflicts_with(reserved)? {
                        return Err(fail(
                            "Checkpoint and its locks must differ from every export destination",
                        ));
                    }
                }
                Ok(())
            },
        )
    }
}

#[cfg(test)]
#[path = "extraction_checkpoint_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "extraction_checkpoint_export_tests.rs"]
mod export_tests;
