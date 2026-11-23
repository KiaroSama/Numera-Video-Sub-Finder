use nvsf_app::storage::AtomicOutput;

#[test]
fn prepared_output_stays_in_its_directory_after_ancestor_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let selected = directory.path().join("selected");
    let media = directory.path().join("media");
    let admitted = directory.path().join("admitted");
    std::fs::create_dir(&selected).unwrap();
    std::fs::create_dir(&media).unwrap();
    let source = media.join("original.mp4");
    std::fs::write(&source, b"original media").unwrap();
    let output = AtomicOutput::new(&selected.join("original.mp4"), true).unwrap();
    output.protect_source(&source).unwrap();
    // Substitute the ancestor after admission and before both creation and publication.
    std::fs::rename(&selected, &admitted).unwrap();
    std::fs::rename(&media, &selected).unwrap();
    output.write(b"subtitle output").unwrap();
    assert_eq!(
        std::fs::read(selected.join("original.mp4")).unwrap(),
        b"original media"
    );
    assert_eq!(
        std::fs::read(admitted.join("original.mp4")).unwrap(),
        b"subtitle output"
    );
    assert_eq!(std::fs::read_dir(&admitted).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn prepared_output_ignores_a_symlink_substituted_for_its_ancestor() {
    let directory = tempfile::tempdir().unwrap();
    let selected = directory.path().join("selected");
    let media = directory.path().join("media");
    let admitted = directory.path().join("admitted");
    std::fs::create_dir(&selected).unwrap();
    std::fs::create_dir(&media).unwrap();
    let source = media.join("original.mp4");
    std::fs::write(&source, b"original media").unwrap();
    let output = AtomicOutput::new(&selected.join("original.mp4"), true).unwrap();
    output.protect_source(&source).unwrap();
    std::fs::rename(&selected, &admitted).unwrap();
    std::os::unix::fs::symlink(&media, &selected).unwrap();
    output.write(b"subtitle output").unwrap();
    assert_eq!(std::fs::read(&source).unwrap(), b"original media");
    assert_eq!(
        std::fs::read(admitted.join("original.mp4")).unwrap(),
        b"subtitle output"
    );
    assert_eq!(std::fs::read_dir(&media).unwrap().count(), 1);
}

#[test]
fn source_validation_uses_the_admitted_directory_after_ancestor_changes() {
    let directory = tempfile::tempdir().unwrap();
    let selected = directory.path().join("selected");
    let relocated = directory.path().join("relocated");
    std::fs::create_dir(&selected).unwrap();
    let destination = selected.join("original.mp4");
    std::fs::write(&destination, b"original media").unwrap();
    let output = AtomicOutput::new(&destination, true).unwrap();
    std::fs::rename(&selected, &relocated).unwrap();
    std::fs::create_dir(&selected).unwrap();
    std::fs::write(&destination, b"unrelated replacement").unwrap();
    let error = output
        .protect_source(&relocated.join("original.mp4"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("source media"), "{error}");
    assert_eq!(
        std::fs::read(relocated.join("original.mp4")).unwrap(),
        b"original media"
    );
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"unrelated replacement"
    );
}

#[test]
fn publication_without_overwrite_preserves_a_competitor_created_after_admission() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("日本 fa.srt");
    let output = AtomicOutput::new(&destination, false).unwrap();
    std::fs::write(&destination, b"external edit after admission").unwrap();
    assert!(output.write(b"new subtitles").is_err());
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"external edit after admission"
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn overwrite_permission_does_not_replace_a_source_moved_to_an_initially_absent_output() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("original.mp4");
    let destination = directory.path().join("review.srt");
    std::fs::write(&source, b"original media").unwrap();
    let output = AtomicOutput::new(&destination, true).unwrap();
    output.protect_source(&source).unwrap();
    std::fs::rename(&source, &destination).unwrap();
    assert!(output.write(b"subtitles").is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"original media");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn confirmation_state_comes_from_the_bound_entry_and_keeps_late_arrivals_safe() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("review.json");
    let (output, needs_confirmation) = AtomicOutput::for_confirmation(&destination).unwrap();
    assert!(!needs_confirmation);
    std::fs::write(&destination, b"external review").unwrap();
    assert!(output.write(b"new review").is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"external review");
    let (output, needs_confirmation) = AtomicOutput::for_confirmation(&destination).unwrap();
    assert!(needs_confirmation);
    output.write(b"confirmed review").unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), b"confirmed review");
}

#[test]
fn directory_syntax_never_turns_into_permission_to_replace_a_regular_file() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("existing.srt");
    std::fs::write(&destination, b"previous subtitles").unwrap();
    let mut suffixes = vec!["/", "/.", "/./", "//"];
    if cfg!(windows) {
        suffixes.extend(["\\", "\\.", "\\.\\"]);
    }
    for suffix in suffixes {
        let mut raw = destination.as_os_str().to_os_string();
        raw.push(suffix);
        assert!(
            nvsf_app::storage::write_atomic(std::path::Path::new(&raw), b"replacement", true)
                .is_err()
        );
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous subtitles");
    }
    nvsf_app::storage::write_atomic(&destination, b"explicit replacement", true).unwrap();
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"explicit replacement"
    );
}

#[test]
fn reserved_names_follow_the_admitted_directory_even_before_the_file_exists() {
    let directory = tempfile::tempdir().unwrap();
    let selected = directory.path().join("selected");
    let admitted = directory.path().join("admitted");
    std::fs::create_dir(&selected).unwrap();
    let destination = selected.join("project.nvsf.json");
    let output = AtomicOutput::new(&destination, false).unwrap();
    std::fs::rename(&selected, &admitted).unwrap();
    std::fs::create_dir(&selected).unwrap();
    assert!(
        output
            .conflicts_with(&admitted.join("project.nvsf.json"))
            .unwrap()
    );
    assert!(!output.conflicts_with(&destination).unwrap());
}

#[test]
fn atomic_output_rejects_missing_parents_directories_and_invalid_names_without_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("original.mp4");
    std::fs::write(&source, b"original media").unwrap();
    assert!(AtomicOutput::new(&directory.path().join("missing/out.srt"), true).is_err());
    assert!(AtomicOutput::new(directory.path(), true).is_err());
    assert!(AtomicOutput::new(&directory.path().join("invalid\0.srt"), true).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"original media");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn a_final_symlink_is_refused_without_touching_its_target_or_leaving_a_temporary() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("original.mp4");
    let output = directory.path().join("subtitle.srt");
    std::fs::write(&source, b"original media").unwrap();
    std::os::unix::fs::symlink(&source, &output).unwrap();
    assert!(AtomicOutput::new(&output, true).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"original media");
    assert!(output.is_symlink());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[cfg(windows)]
#[test]
fn reserved_destination_names_use_windows_case_semantics() {
    let directory = tempfile::tempdir().unwrap();
    let output = AtomicOutput::new(&directory.path().join("Project.NVSF.JSON"), false).unwrap();
    assert!(
        output
            .conflicts_with(&directory.path().join("project.nvsf.json"))
            .unwrap()
    );
}
