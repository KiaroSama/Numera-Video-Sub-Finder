use super::*;

#[test]
fn model_import_confirmation_denial_and_integrity_error_preserve_both_directory_generations() {
    for confirm in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let paths = nvsf_app::Paths::new(Some(&directory.path().join("data"))).unwrap();
        let log = nvsf_app::logging::Log::new(&paths, "nvsf-desktop");
        let host = Host::new(paths, log);
        let source = directory.path().join("selected.onnx");
        std::fs::write(&source, b"invalid candidate").unwrap();
        let root = &host.0.paths.models;
        std::fs::write(root.join("det.onnx"), b"previous model").unwrap();
        let admitted = directory.path().join("admitted-models");
        let mut prompted = false;
        let result = import_model_from(&host, "det", &source, || {
            prompted = true;
            std::fs::rename(root, &admitted).unwrap();
            std::fs::create_dir(root).unwrap();
            std::fs::write(root.join("det.onnx"), b"unrelated replacement root").unwrap();
            confirm
        });
        assert!(prompted);
        if confirm {
            assert!(result.unwrap_err().contains("Import checksum mismatch"));
        } else {
            assert!(result.unwrap().is_none());
        }
        assert_eq!(
            std::fs::read(admitted.join("det.onnx")).unwrap(),
            b"previous model"
        );
        assert_eq!(
            std::fs::read(root.join("det.onnx")).unwrap(),
            b"unrelated replacement root"
        );
        assert_eq!(std::fs::read_dir(admitted).unwrap().count(), 1);
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 1);
    }
}
