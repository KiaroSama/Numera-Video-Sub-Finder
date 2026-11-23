use super::*;

fn fixture() -> (tempfile::TempDir, Paths, Language, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let paths = Paths::new(Some(&directory.path().join("data"))).unwrap();
    let source = directory.path().join("selected 日本.traineddata");
    std::fs::write(&source, vec![b'L'; 131_079]).unwrap();
    // This bounded fixture substitutes catalog size/hash, not filesystem publication.
    let mut language = spec("rus").unwrap().clone();
    language.bytes = source.metadata().unwrap().len();
    language.sha256 = hash_file(&source).unwrap();
    (directory, paths, language, source)
}

#[test]
fn language_import_streams_to_the_admitted_parent_and_retains_its_lock_across_substitution() {
    let (directory, paths, language, source) = fixture();
    let cancel = AtomicBool::new(false);
    let root = paths.models.join("tessdata");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("rus.traineddata"), b"previous pack").unwrap();
    let (prepared, exists) = prepare_import_with_spec(&paths, language, &source, &cancel).unwrap();
    assert!(exists);
    let admitted = directory.path().join("admitted-tessdata");
    std::fs::rename(&root, &admitted).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("rus.traineddata"), b"unrelated replacement root").unwrap();
    assert!(crate::storage::JobLock::acquire(&admitted.join("rus.download.lock")).is_err());
    let selected = directory.path().join("retained-source.traineddata");
    std::fs::rename(&source, &selected).unwrap();
    std::fs::write(&source, b"unrelated source pathname").unwrap();
    prepared.publish(&cancel).unwrap();
    assert_eq!(
        std::fs::read(admitted.join("rus.traineddata")).unwrap(),
        vec![b'L'; 131_079]
    );
    assert_eq!(std::fs::read(&selected).unwrap(), vec![b'L'; 131_079]);
    assert_eq!(
        std::fs::read(&source).unwrap(),
        b"unrelated source pathname"
    );
    assert_eq!(
        std::fs::read(root.join("rus.traineddata")).unwrap(),
        b"unrelated replacement root"
    );
    assert_eq!(std::fs::read_dir(&admitted).unwrap().count(), 2);
    assert_eq!(std::fs::read_dir(root).unwrap().count(), 1);
    assert!(crate::storage::JobLock::acquire(&admitted.join("rus.download.lock")).is_ok());
}

#[test]
fn language_import_refuses_a_late_competitor_at_an_initially_absent_destination() {
    let (_directory, paths, language, source) = fixture();
    let cancel = AtomicBool::new(false);
    let destination = paths.models.join("tessdata/rus.traineddata");
    let (prepared, exists) = prepare_import_with_spec(&paths, language, &source, &cancel).unwrap();
    assert!(!exists);
    std::fs::write(&destination, b"late selected pack").unwrap();
    assert!(prepared.publish(&cancel).is_err());
    assert_eq!(std::fs::read(destination).unwrap(), b"late selected pack");
    assert_eq!(
        std::fs::read_dir(paths.models.join("tessdata"))
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn abandoned_cancelled_and_invalid_language_imports_preserve_the_pack_and_release_the_lock() {
    let (_directory, paths, language, source) = fixture();
    let cancel = AtomicBool::new(false);
    let root = paths.models.join("tessdata");
    std::fs::create_dir(&root).unwrap();
    let destination = root.join("rus.traineddata");
    std::fs::write(&destination, b"previous pack").unwrap();
    let (prepared, _) =
        prepare_import_with_spec(&paths, language.clone(), &source, &cancel).unwrap();
    drop(prepared);
    let (prepared, _) =
        prepare_import_with_spec(&paths, language.clone(), &source, &cancel).unwrap();
    cancel.store(true, Ordering::Release);
    assert!(matches!(
        prepared.publish(&cancel),
        Err(crate::AppError::Cancelled)
    ));
    cancel.store(false, Ordering::Release);
    let (prepared, _) = prepare_import_with_spec(&paths, language, &source, &cancel).unwrap();
    let mut changed = std::fs::read(&source).unwrap();
    changed[0] ^= 1;
    std::fs::write(&source, changed).unwrap();
    assert!(
        prepared
            .publish(&cancel)
            .unwrap_err()
            .to_string()
            .contains("size or checksum mismatch")
    );
    assert_eq!(std::fs::read(destination).unwrap(), b"previous pack");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
    assert!(crate::storage::JobLock::acquire(&root.join("rus.download.lock")).is_ok());
}

#[test]
fn language_import_rejects_directory_and_missing_sources_before_lock_admission() {
    let (directory, paths, language, _source) = fixture();
    for source in [
        directory.path().to_path_buf(),
        directory.path().join("missing.traineddata"),
    ] {
        let error =
            prepare_import_with_spec(&paths, language.clone(), &source, &AtomicBool::new(false))
                .err()
                .expect("nonregular source was admitted");
        assert!(error.to_string().contains("size or checksum mismatch"));
    }
    assert!(!paths.models.join("tessdata").exists());
}

#[test]
fn language_import_accepts_the_installed_pack_as_its_verified_source_and_releases_its_lock() {
    let (_directory, paths, language, source) = fixture();
    let cancel = AtomicBool::new(false);
    let expected = std::fs::read(&source).unwrap();
    let root = paths.models.join("tessdata");
    std::fs::create_dir(&root).unwrap();
    let destination = root.join("rus.traineddata");
    let lock_path = root.join("rus.download.lock");
    std::fs::rename(&source, &destination).unwrap();
    // The fixture substitutes only the catalog identity; the same-path import uses real files.
    let (prepared, exists) =
        prepare_import_with_spec(&paths, language, &destination, &cancel).unwrap();
    assert!(exists);
    assert!(crate::storage::JobLock::acquire(&lock_path).is_err());
    prepared.publish(&cancel).unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), expected);
    assert!(!source.exists());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
    let released = crate::storage::JobLock::acquire(&lock_path).unwrap();
    drop(released);
    assert!(std::fs::read(&lock_path).unwrap().is_empty());
}
