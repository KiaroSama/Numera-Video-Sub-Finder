use ndarray::Array3;
use nvsf_app::{inference::decode_ctc, vision};

#[test]
fn ctc_preserves_unicode_and_collapses_only_adjacent_nonblank_labels() {
    let dict = vec![String::new(), "日".into(), "15".into(), "本".into()];
    let ids = [1, 1, 0, 1, 2, 3];
    let mut output = Array3::zeros((1, ids.len(), dict.len()));
    for (t, id) in ids.into_iter().enumerate() {
        output[[0, t, id]] = 0.9;
    }
    let (text, score) = decode_ctc(output.view().into_dyn(), &dict).unwrap();
    assert_eq!(text, "日日15本");
    assert!((score - 0.9).abs() < 0.0001);
    assert!(decode_ctc(output.view().into_dyn(), &dict[..3]).is_err());
    output[[0, 0, 0]] = f32::NAN;
    assert!(decode_ctc(output.view().into_dyn(), &dict).is_err());
}

#[test]
fn detector_contract_rejects_invalid_shape_and_preserves_image_limits() {
    let output = ndarray::Array4::<f32>::zeros((1, 1, 2, 2));
    assert!(vision::db_regions(output.view().into_dyn(), 0, 2, 640, 360).is_err());
    assert!(vision::db_regions(output.view().into_dyn(), 4000, 4000, 640, 360).is_err());
    assert!(
        vision::db_regions(output.view().into_dyn(), 2, 2, 640, 360)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn model_metadata_matches_immutable_manifest_hashes() {
    use sha2::{Digest, Sha256};
    for spec in nvsf_app::models::specs().unwrap() {
        let bytes = nvsf_app::models::metadata(&spec.id).unwrap().as_bytes();
        let actual: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(actual, spec.metadata_sha256, "{} metadata", spec.id);
    }
}
