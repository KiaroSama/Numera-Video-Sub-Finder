use nvsf_app::{Paths, models};
use std::sync::atomic::AtomicBool;

fn paths() -> (tempfile::TempDir, Paths) {
    let directory = tempfile::tempdir().unwrap();
    let paths = Paths::new(Some(&directory.path().join("Model data 日本"))).unwrap();
    (directory, paths)
}

#[test]
fn manifest_lists_the_four_admitted_models_with_full_identity() {
    let specs = models::specs().unwrap();
    let ids: Vec<_> = specs.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["det", "fa", "en-ja", "ja-specialist"]);
    for spec in &specs {
        assert!(spec.url.starts_with("https://"));
        assert_eq!(spec.sha256.len(), 64);
        assert_eq!(spec.metadata_sha256.len(), 64);
        assert!(spec.bytes > 0);
        assert!(!spec.license.is_empty());
        assert!(!spec.languages.is_empty());
    }
}

#[test]
fn metadata_is_embedded_only_for_admitted_models() {
    for id in ["det", "fa", "en-ja", "ja-specialist"] {
        assert!(!models::metadata(id).unwrap().is_empty());
    }
    assert!(models::metadata("../det").is_err());
    assert!(models::metadata("").is_err());
}

#[test]
fn empty_data_root_reports_every_model_as_missing_without_writing() {
    let (_directory, paths) = paths();
    let catalog = models::catalog(&paths).unwrap();
    assert_eq!(catalog.len(), 4);
    assert!(catalog.iter().all(|row| !row.installed && !row.verified));
    assert!(
        catalog
            .iter()
            .all(|row| row.message.to_lowercase().contains("not installed"))
    );
    assert!(models::verify(&paths, "det").is_err());
    assert!(models::verify(&paths, "unknown").is_err());
    assert!(!paths.models.join("det.onnx").exists());
}

#[test]
fn import_rejects_unknown_ids_and_mismatched_bytes_before_activation() {
    let (_directory, paths) = paths();
    let source = paths.root.join("candidate.onnx");
    std::fs::write(&source, b"not the admitted model").unwrap();
    assert!(models::import_model(&paths, "unknown", &source).is_err());
    assert!(models::import_model(&paths, "det", &source).is_err());
    assert!(!paths.models.join("det.onnx").exists());
    assert!(models::export_model(&paths, "det", &paths.root.join("out.onnx")).is_err());
}

#[test]
fn cancelled_install_returns_cancel_without_downloading() {
    let (_directory, paths) = paths();
    let cancel = AtomicBool::new(true);
    assert!(matches!(
        models::install(&paths, Some("det"), &cancel),
        Err(nvsf_app::AppError::Cancelled)
    ));
    assert!(models::install(&paths, Some("unknown"), &cancel).is_err());
    assert!(!paths.models.join("det.onnx").exists());
}

#[test]
fn bundled_seeding_is_a_no_op_without_packaged_resources() {
    let (_directory, paths) = paths();
    models::seed_bundled(&paths).unwrap();
    assert!(
        models::catalog(&paths)
            .unwrap()
            .iter()
            .all(|row| !row.installed)
    );
}
