use nvsf_app::{Paths, languages};
use std::sync::atomic::AtomicBool;

#[test]
fn missing_and_unadmitted_languages_cannot_mutate_resources_or_change_unrelated_identity() {
    let directory = tempfile::tempdir().unwrap();
    let paths = Paths::new(Some(&directory.path().join("Language 日本 data"))).unwrap();
    let before = languages::fingerprint(&paths, "en").unwrap();
    assert_eq!(before, languages::fingerprint(&paths, "eng").unwrap());
    assert!(languages::verify(&paths, "fra").is_err());
    assert!(languages::fingerprint(&paths, "fra").is_err());
    let source = directory.path().join("corrupt.traineddata");
    std::fs::write(&source, b"not a language model").unwrap();
    let cancel = AtomicBool::new(false);
    assert!(languages::import(&paths, "fra", &source, &cancel).is_err());
    assert!(languages::import(&paths, "../eng", &source, &cancel).is_err());
    assert!(!paths.models.join("tessdata/fra.traineddata").exists());
    assert_eq!(languages::fingerprint(&paths, "en").unwrap(), before);
    let catalog = languages::catalog(&paths);
    assert_eq!(catalog.len(), 123);
    assert!(
        catalog
            .iter()
            .all(|entry| !entry.verified && !entry.installed)
    );
    assert!(
        catalog
            .iter()
            .find(|entry| entry.code == "fra")
            .unwrap()
            .primary
            .contains("Tesseract")
    );
}
