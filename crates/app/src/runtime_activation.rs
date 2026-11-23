use super::{Pack, read_bounded};
use crate::{Result, fail};
use std::path::Path;

/// Promotes an already authenticated, verified and native-admitted stage, then publishes selection.
/// A corrupt previous directory is retained until both promotion and selection succeed.
pub(super) fn activate(
    pack: &Pack,
    manifest: &[u8],
    signature: &[u8],
    stage: &Path,
    destination: &Path,
    selection: &Path,
    publish: impl FnOnce() -> Result<()>,
) -> Result<()> {
    activate_with_barrier(
        pack,
        manifest,
        signature,
        stage,
        destination,
        selection,
        (sync_directory, publish),
    )
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn activate_with_barrier(
    pack: &Pack,
    manifest: &[u8],
    signature: &[u8],
    stage: &Path,
    destination: &Path,
    selection: &Path,
    callbacks: (
        impl FnOnce(&Path) -> Result<()>,
        impl FnOnce() -> Result<()>,
    ),
) -> Result<()> {
    let (sync_parent, publish) = callbacks;
    if destination.is_symlink() {
        return Err(fail("Runtime activation destination must not be a symlink"));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| fail("Runtime destination has no parent"))?;
    let backup = tempfile::tempdir_in(parent)?;
    let selection_backup = backup.path().join("selection");
    let had_selection = match std::fs::symlink_metadata(selection) {
        Ok(metadata) if metadata.is_file() => {
            std::fs::copy(selection, &selection_backup)?;
            std::fs::OpenOptions::new()
                .write(true)
                .open(&selection_backup)?
                .sync_all()?;
            true
        }
        Ok(_) => return Err(fail("Runtime selection must be a regular file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    let reusable = pack.verify(destination).is_ok()
        && read_bounded(&destination.join("runtime-pack.json"), 1024 * 1024)
            .is_ok_and(|bytes| bytes == manifest)
        && read_bounded(&destination.join("runtime-pack.sig"), 4096)
            .is_ok_and(|bytes| bytes == signature);
    let previous = backup.path().join("previous");
    let had_previous = destination.exists();
    if !reusable && had_previous {
        std::fs::rename(destination, &previous)?;
    }
    if !reusable && let Err(error) = std::fs::rename(stage, destination) {
        if had_previous && let Err(restore) = std::fs::rename(&previous, destination) {
            let retained = backup.keep();
            return Err(fail(format!(
                "Runtime promotion failed ({error}); restoration failed ({restore}); previous bytes retained at {}",
                retained.display()
            )));
        }
        return Err(error.into());
    }
    // The pointer lives outside this directory, so its own sync cannot persist these renames.
    let promoted = if reusable {
        Ok(())
    } else {
        sync_parent(parent)
    };
    if let Err(error) = promoted.and_then(|()| publish()) {
        let selection_restored = if had_selection {
            std::fs::rename(&selection_backup, selection)
        } else {
            match std::fs::remove_file(selection) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
        };
        if let Err(restore) = selection_restored {
            let retained = backup.keep();
            return Err(fail(format!(
                "Runtime selection failed ({error}); pointer restoration failed ({restore}); recovery bytes retained at {}",
                retained.display()
            )));
        }
        #[cfg(unix)]
        if let Err(restore) = std::fs::File::open(selection.parent().unwrap_or(Path::new(".")))
            .and_then(|directory| directory.sync_all())
        {
            let retained = backup.keep();
            return Err(fail(format!(
                "Runtime selection failed ({error}); restored pointer durability failed ({restore}); recovery bytes retained at {}",
                retained.display()
            )));
        }
        let restored = if reusable {
            Ok(())
        } else {
            std::fs::rename(destination, stage).and_then(|()| {
                if had_previous {
                    std::fs::rename(&previous, destination)
                } else {
                    Ok(())
                }
            })
        };
        if let Err(restore) = restored {
            let retained = backup.keep();
            return Err(fail(format!(
                "Runtime selection failed ({error}); restoration failed ({restore}); previous bytes retained at {}",
                retained.display()
            )));
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
#[path = "runtime_activation_tests.rs"]
mod tests;
