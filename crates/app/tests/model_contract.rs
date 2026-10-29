use nvsf_app::{model_contract::validate_metadata, models};

#[test]
fn admitted_metadata_preserves_exact_ctc_vocabulary_and_detector_profile() {
    for (id, classes) in [
        ("det", None),
        ("fa", Some(749)),
        ("en-ja", Some(18_385)),
        ("ja-specialist", Some(4401)),
    ] {
        let metadata = validate_metadata(id, models::metadata(id).unwrap()).unwrap();
        assert_eq!(metadata.classes, classes, "{id}");
        assert_eq!(metadata.dictionary.len(), classes.unwrap_or(0), "{id}");
        if classes.is_some() {
            assert_eq!(metadata.dictionary.first().unwrap(), "");
            assert_eq!(metadata.dictionary.last().unwrap(), " ");
        }
        if id == "ja-specialist" {
            assert!(metadata.dictionary.iter().filter(|s| *s == " ").count() >= 2);
        }
    }
}

#[test]
fn unknown_models_and_changed_metadata_fail_before_admission() {
    assert!(validate_metadata("../det", models::metadata("det").unwrap()).is_err());
    assert!(validate_metadata("unknown", "{}").is_err());
    let changed = models::metadata("det").unwrap().replace("BGR", "RGB");
    assert!(validate_metadata("det", &changed).is_err());
    assert!(validate_metadata("det", "not: [valid").is_err());
}
