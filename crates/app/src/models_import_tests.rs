use super::*;

fn fixture() -> (tempfile::TempDir, Paths, ModelSpec, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let paths = Paths::new(Some(&directory.path().join("data"))).unwrap();
    let source = directory.path().join("selected 日本.onnx");
    std::fs::write(&source, b"bounded model integrity fixture").unwrap();
    // Only the expected artifact identity is substituted; publication uses real handles.
    let mut spec = specs().unwrap().remove(0);
    spec.bytes = source.metadata().unwrap().len();
    spec.sha256 = hash_file(&source).unwrap();
    (directory, paths, spec, source)
}

#[test]
fn model_import_retains_the_selected_source_and_destination_after_parent_substitution() {
    let (directory, paths, spec, source) = fixture();
    let name = format!("{}.onnx", spec.id);
    let expected = std::fs::read(&source).unwrap();
    std::fs::write(paths.models.join(&name), b"previous model").unwrap();
    let (prepared, exists) = prepare_import_with_spec(&paths, spec, &source).unwrap();
    assert!(exists);
    let admitted = directory.path().join("admitted-models");
    std::fs::rename(&paths.models, &admitted).unwrap();
    std::fs::create_dir(&paths.models).unwrap();
    std::fs::write(paths.models.join(&name), b"unrelated replacement root").unwrap();
    let selected = directory.path().join("retained-source.onnx");
    std::fs::rename(&source, &selected).unwrap();
    std::fs::write(&source, b"unrelated source pathname").unwrap();
    prepared.publish().unwrap();
    assert_eq!(std::fs::read(admitted.join(&name)).unwrap(), expected);
    assert_eq!(std::fs::read(selected).unwrap(), expected);
    assert_eq!(
        std::fs::read(&source).unwrap(),
        b"unrelated source pathname"
    );
    assert_eq!(
        std::fs::read(paths.models.join(&name)).unwrap(),
        b"unrelated replacement root"
    );
    assert_eq!(std::fs::read_dir(admitted).unwrap().count(), 1);
}

#[test]
fn model_import_refuses_a_late_competitor_at_an_initially_absent_destination() {
    let (_directory, paths, spec, source) = fixture();
    let destination = paths.models.join(format!("{}.onnx", spec.id));
    let (prepared, exists) = prepare_import_with_spec(&paths, spec, &source).unwrap();
    assert!(!exists);
    std::fs::write(&destination, b"late selected resource").unwrap();
    assert!(prepared.publish().is_err());
    assert_eq!(
        std::fs::read(destination).unwrap(),
        b"late selected resource"
    );
    assert_eq!(std::fs::read_dir(paths.models).unwrap().count(), 1);
}

#[test]
fn abandoned_and_invalid_model_imports_preserve_installed_bytes() {
    let (_directory, paths, spec, source) = fixture();
    let destination = paths.models.join(format!("{}.onnx", spec.id));
    std::fs::write(&destination, b"previous model").unwrap();
    let (prepared, exists) = prepare_import_with_spec(&paths, spec.clone(), &source).unwrap();
    assert!(exists);
    drop(prepared);
    assert_eq!(std::fs::read(&destination).unwrap(), b"previous model");
    let (prepared, _) = prepare_import_with_spec(&paths, spec, &source).unwrap();
    let mut changed = std::fs::read(&source).unwrap();
    changed[0] ^= 1;
    std::fs::write(&source, changed).unwrap();
    assert!(
        prepared
            .publish()
            .unwrap_err()
            .to_string()
            .contains("Import checksum mismatch")
    );
    assert_eq!(std::fs::read(destination).unwrap(), b"previous model");
    assert_eq!(std::fs::read_dir(paths.models).unwrap().count(), 1);
}

#[test]
fn model_import_rejects_directory_and_missing_sources_before_destination_admission() {
    let (directory, paths, spec, _source) = fixture();
    for source in [
        directory.path().to_path_buf(),
        directory.path().join("missing.onnx"),
    ] {
        let error = prepare_import_with_spec(&paths, spec.clone(), &source)
            .err()
            .expect("nonregular source was admitted");
        assert!(error.to_string().contains("regular ONNX model file"));
    }
    assert_eq!(std::fs::read_dir(paths.models).unwrap().count(), 0);
}
