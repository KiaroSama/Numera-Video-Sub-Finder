use nvsf_app::{extraction::adjudicate_verifier, fail};
use nvsf_core::Recognition;

fn observation(text: &str, confidence: f32) -> Recognition {
    Recognition {
        text: text.into(),
        confidence: Some(confidence),
        crop: Some("retained-original.png".into()),
    }
}

#[test]
fn independent_verifier_never_calibrates_scores_or_replaces_nonempty_primary() {
    for (primary_text, verifier_text, expected_text, expected_confidence) in [
        ("Hello", " Hello\n", "Hello", Some(0.6)),
        ("سلام ۱۶", "سلام ۱۷", "سلام ۱۶", None),
        ("日本", "", "日本", None),
        ("", "日本", "日本", None),
        ("", "", "", Some(0.6)),
    ] {
        let mut primary = observation(primary_text, 0.6);
        let mut verifier = observation(verifier_text, 0.99);
        verifier.crop = Some("different-verifier-crop.png".into());
        adjudicate_verifier(&mut primary, Ok(verifier)).unwrap();
        assert_eq!(primary.text, expected_text);
        assert_eq!(primary.confidence, expected_confidence);
        assert_eq!(primary.crop.as_deref(), Some("retained-original.png"));
    }
}

#[test]
fn independent_verifier_failure_preserves_primary_evidence_and_requires_review() {
    let mut primary = observation("سلام ۱۶ 日本", 0.6);
    let error =
        adjudicate_verifier(&mut primary, Err(fail("Native recognition timed out"))).unwrap_err();
    assert!(error.to_string().contains("timed out"));
    assert_eq!(primary.text, "سلام ۱۶ 日本");
    assert_eq!(primary.crop.as_deref(), Some("retained-original.png"));
    assert_eq!(primary.confidence, None);
}
