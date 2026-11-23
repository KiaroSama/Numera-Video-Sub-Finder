use super::*;

#[test]
fn language_import_confirmation_denial_and_integrity_error_preserve_both_directory_generations() {
    for confirm in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let paths = nvsf_app::Paths::new(Some(&directory.path().join("data"))).unwrap();
        let log = nvsf_app::logging::Log::new(&paths, "nvsf-desktop");
        let host = Host::new(paths, log);
        let source = directory.path().join("selected.traineddata");
        std::fs::write(&source, b"invalid candidate").unwrap();
        let root = host.0.paths.models.join("tessdata");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("rus.traineddata"), b"previous pack").unwrap();
        let admitted = directory.path().join("admitted-tessdata");
        let mut prompted = false;
        let result = import_language_from(&host, "rus", &source, || {
            prompted = true;
            assert!(nvsf_app::storage::JobLock::acquire(&root.join("rus.download.lock")).is_err());
            std::fs::rename(&root, &admitted).unwrap();
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join("rus.traineddata"), b"unrelated replacement root").unwrap();
            confirm
        });
        assert!(prompted);
        if confirm {
            assert!(result.unwrap_err().contains("size or checksum mismatch"));
        } else {
            assert!(result.unwrap().is_none());
        }
        assert_eq!(
            std::fs::read(admitted.join("rus.traineddata")).unwrap(),
            b"previous pack"
        );
        assert_eq!(
            std::fs::read(root.join("rus.traineddata")).unwrap(),
            b"unrelated replacement root"
        );
        assert_eq!(std::fs::read_dir(&admitted).unwrap().count(), 2);
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 1);
        assert!(nvsf_app::storage::JobLock::acquire(&admitted.join("rus.download.lock")).is_ok());
    }
}
