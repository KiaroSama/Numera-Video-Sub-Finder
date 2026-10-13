use nvsf_app::{
    Paths,
    logging::Log,
    storage::{JobLock, hash_file, write_atomic},
};

#[test]
fn data_integrity_locking_and_logs_are_real_local_resources() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Numera 日本 path with spaces");
    let paths = Paths::new(Some(&root)).unwrap();
    let known = root.join("known.txt");
    write_atomic(&known, b"hello world", false).unwrap();
    assert_eq!(
        hash_file(&known).unwrap(),
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
    );
    let lock_path = root.join("job.lock");
    let lock = JobLock::acquire(&lock_path).unwrap();
    assert!(JobLock::acquire(&lock_path).is_err());
    drop(lock);
    assert!(JobLock::acquire(&lock_path).is_ok());
    let old = paths.logs.join("fixture_2000-01-01_00-00-00_UTC.log");
    std::fs::write(&old, b"owned historical diagnostic").unwrap();
    let file = std::fs::OpenOptions::new().write(true).open(&old).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::UNIX_EPOCH))
        .unwrap();
    drop(file);
    let first = Log::new(&paths, "fixture");
    assert!(!old.exists());
    let second = Log::new(&paths, "fixture");
    assert_ne!(first.path, second.path);
    first.event("WARNING", "TEST", "Controlled diagnostic warning");
    first.event("ERROR", "TEST", "Controlled failure preserved");
    first.event(
        "WARNING",
        "TEST",
        "password=fixture-secret Authorization: Bearer fixture-token",
    );
    let log = first.path.clone().unwrap();
    drop(first);
    drop(second);
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(log.file_name().unwrap().to_str().unwrap().contains("_UTC"));
    for phrase in [
        "UTC] [INFO] [APP] Startup",
        "[WARNING] [TEST]",
        "[ERROR] [TEST]",
        "Shutdown; log handler closed",
    ] {
        assert!(text.contains(phrase));
    }
    assert!(!text.contains("hello world"));
    assert!(!text.contains("fixture-secret"));
    assert!(!text.contains("fixture-token"));
    assert!(text.contains("[REDACTED]"));
    assert!(write_atomic(&known, b"replacement", false).is_err());
    assert_eq!(std::fs::read(&known).unwrap(), b"hello world");
}

#[test]
fn complete_partial_download_activates_without_network_and_bad_hash_preserves_model() {
    use std::sync::atomic::AtomicBool;
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("model.onnx");
    let partial = target.with_extension("partial");
    let hash = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
    std::fs::write(&partial, b"hello world").unwrap();
    nvsf_app::model_download::install(
        "https://invalid.example/model",
        11,
        hash,
        &target,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"hello world");
    assert!(!partial.exists());
    std::fs::write(&partial, b"wrong bytes").unwrap();
    assert!(
        nvsf_app::model_download::install(
            "https://invalid.example/model",
            11,
            &"a".repeat(64),
            &target,
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"hello world");
    assert!(partial.is_file());
    assert!(
        nvsf_app::model_download::install(
            "https://invalid.example/model",
            11,
            &"b".repeat(64),
            &target,
            &AtomicBool::new(true)
        )
        .is_err()
    );
}

#[test]
fn malformed_model_import_preserves_existing_bytes_and_unknown_ids_fail() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(Some(dir.path())).unwrap();
    let existing = paths.models.join("det.onnx");
    std::fs::write(&existing, b"retained").unwrap();
    let bad = dir.path().join("bad.onnx");
    std::fs::write(&bad, b"not a model").unwrap();
    assert!(nvsf_app::models::import_model(&paths, "det", &bad).is_err());
    assert!(nvsf_app::models::import_model(&paths, "../det", &bad).is_err());
    assert_eq!(std::fs::read(&existing).unwrap(), b"retained");
    assert!(nvsf_app::models::verify(&paths, "det").is_err());
    assert!(nvsf_app::models::verify_tessdata(&paths, "fas").is_err());
}
