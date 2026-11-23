use super::Checkpoint;

#[test]
fn explicit_checkpoint_is_selected_and_never_falls_back_to_an_output_sibling() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("subtitles.srt");
    let source = directory.path().join("source.mp4");
    let sibling = directory.path().join("subtitles.nvsf.json");
    let selected = directory.path().join("saved-project.json");
    std::fs::write(&sibling, b"unrelated checkpoint").unwrap();
    std::fs::write(&selected, b"selected checkpoint").unwrap();
    let checkpoint = Checkpoint::open(&output, Some(&selected), &source).unwrap();
    assert_eq!(checkpoint.path, selected.canonicalize().unwrap());
    crate::storage::write_atomic(&checkpoint.path, b"resumed checkpoint", true).unwrap();
    assert_eq!(std::fs::read(&sibling).unwrap(), b"unrelated checkpoint");
    assert_eq!(std::fs::read(&selected).unwrap(), b"resumed checkpoint");
    assert!(matches!(
        Checkpoint::open(&output, Some(&directory.path().join("missing.json")), &source),
        Err(crate::AppError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound
    ));
}

#[test]
fn a_selected_checkpoint_removed_after_admission_cannot_restart_the_job() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("subtitles.srt");
    let source = directory.path().join("source.mp4");
    let selected = directory.path().join("saved-project.json");
    std::fs::write(&selected, b"selected checkpoint").unwrap();
    let checkpoint = Checkpoint::open(&output, Some(&selected), &source).unwrap();
    std::fs::remove_file(&selected).unwrap();
    assert!(matches!(
        checkpoint.load(),
        Err(crate::AppError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound
    ));
    assert!(!output.exists());
    assert!(!output.with_extension("nvsf.json").exists());
}

#[test]
fn copied_checkpoints_still_share_the_output_job_lock() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("subtitles.srt");
    let source = directory.path().join("source.mp4");
    let first = directory.path().join("first.json");
    let second = directory.path().join("second.json");
    std::fs::write(&first, b"first checkpoint").unwrap();
    std::fs::write(&second, b"second checkpoint").unwrap();
    let held = Checkpoint::open(&output, Some(&first), &source).unwrap();
    assert!(Checkpoint::open(&output, Some(&second), &source).is_err());
    assert!(Checkpoint::open(&output, None, &source).is_err());
    drop(held);
    assert!(Checkpoint::open(&output, Some(&second), &source).is_ok());
}

#[test]
fn default_checkpoint_extension_preserves_a_distinct_existing_json_stem() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("subtitles.nvsf.json");
    let source = directory.path().join("source.mp4");
    let expected = directory.path().join("subtitles.nvsf.nvsf.json");
    assert_eq!(output.with_extension("nvsf.json"), expected);
    assert_ne!(output, expected);
    let checkpoint = Checkpoint::open(&output, None, &source).unwrap();
    assert_eq!(
        checkpoint.path,
        directory
            .path()
            .canonicalize()
            .unwrap()
            .join("subtitles.nvsf.nvsf.json")
    );
    assert!(checkpoint.load().unwrap().is_none());
    assert!(!output.exists());
    assert!(!checkpoint.path.exists());
}

#[test]
fn checkpoint_and_export_destination_must_be_distinct() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("subtitles.nvsf.json");
    let source = directory.path().join("source.mp4");
    std::fs::write(&output, b"selected checkpoint").unwrap();
    assert!(Checkpoint::open(&output, Some(&output), &source).is_err());
    assert_eq!(std::fs::read(&output).unwrap(), b"selected checkpoint");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn a_default_checkpoint_symlink_to_the_output_is_rejected_before_lock_creation() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("subtitles.srt");
    let source = directory.path().join("source.mp4");
    std::fs::write(&output, b"existing subtitle output").unwrap();
    std::os::unix::fs::symlink(&output, output.with_extension("nvsf.json")).unwrap();
    assert!(Checkpoint::open(&output, None, &source).is_err());
    assert_eq!(std::fs::read(&output).unwrap(), b"existing subtitle output");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn a_checkpoint_collision_cannot_replace_source_media_or_create_locks() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("subtitles.srt");
    let source = directory.path().join("subtitles.nvsf.json");
    std::fs::write(&source, b"media with an arbitrary extension").unwrap();
    let error = match Checkpoint::open(&output, None, &source) {
        Ok(_) => panic!("source was admitted as a checkpoint"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("source media"));
    assert_eq!(
        std::fs::read(&source).unwrap(),
        b"media with an arbitrary extension"
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn a_selected_checkpoint_cannot_alias_an_open_job_lock() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("subtitles.srt");
    let source = directory.path().join("source.mp4");
    let selected = directory.path().join("subtitles.nvsf.lock");
    std::fs::write(&source, b"original media").unwrap();
    std::fs::write(&selected, b"saved checkpoint bytes").unwrap();
    let guard = crate::storage::JobLock::acquire(&selected).unwrap();
    let error = match Checkpoint::open(&output, Some(&selected), &source) {
        Ok(_) => panic!("checkpoint aliased its job lock"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("Checkpoint and its locks"));
    assert_eq!(std::fs::read(&source).unwrap(), b"original media");
    assert!(crate::storage::JobLock::acquire(&selected).is_err());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    drop(guard);
    assert_eq!(std::fs::read(&selected).unwrap(), b"saved checkpoint bytes");
    assert!(crate::storage::JobLock::acquire(&selected).is_ok());
}

#[cfg(unix)]
#[test]
fn a_lock_symlink_to_a_selected_checkpoint_is_rejected_before_lock_creation() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("subtitles.srt");
    let source = directory.path().join("source.mp4");
    let selected = directory.path().join("saved.json");
    std::fs::write(&selected, b"saved checkpoint bytes").unwrap();
    std::os::unix::fs::symlink(&selected, directory.path().join("subtitles.nvsf.lock")).unwrap();
    assert!(Checkpoint::open(&output, Some(&selected), &source).is_err());
    assert_eq!(std::fs::read(&selected).unwrap(), b"saved checkpoint bytes");
    assert!(!directory.path().join("saved.lock").exists());
}

#[cfg(unix)]
#[test]
fn a_checkpoint_symlink_cannot_relocate_the_output_ownership_lock() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("subtitles.srt");
    let source = directory.path().join("source.mp4");
    let selected = directory.path().join("saved.json");
    let unrelated = directory.path().join("other.json");
    std::fs::write(&selected, b"selected checkpoint").unwrap();
    std::fs::write(&unrelated, b"other checkpoint").unwrap();
    std::os::unix::fs::symlink(&unrelated, output.with_extension("nvsf.json")).unwrap();
    let guard = crate::storage::JobLock::acquire(&output.with_extension("nvsf.lock")).unwrap();
    assert!(Checkpoint::open(&output, Some(&selected), &source).is_err());
    assert_eq!(std::fs::read(&selected).unwrap(), b"selected checkpoint");
    assert!(!unrelated.with_extension("lock").exists());
    drop(guard);
}
