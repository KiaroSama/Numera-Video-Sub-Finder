use nvsf_app::{Control, Result, storage, worker};
use nvsf_core::{ExtractConfig, Project};
use std::path::Path;

pub(crate) fn run(checkpoint: &Path, control: &Control) -> Result<Project> {
    run_with(checkpoint, control, |config, control, checkpoint| {
        worker::supervise_resume(
            config,
            control,
            |progress| eprintln!("{} {}ms", progress.stage, progress.position_ms),
            checkpoint,
        )
    })
}

fn run_with(
    checkpoint: &Path,
    control: &Control,
    runner: impl FnOnce(ExtractConfig, &Control, &Path) -> Result<Project>,
) -> Result<Project> {
    let checkpoint = checkpoint.canonicalize()?;
    let project = storage::load_project(&checkpoint)?;
    runner(project.config, control, &checkpoint)
}

#[cfg(test)]
#[path = "resume_tests.rs"]
mod tests;
