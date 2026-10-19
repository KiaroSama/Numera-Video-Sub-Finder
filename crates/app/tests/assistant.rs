use nvsf_app::{
    Paths,
    assistant::{propose, validate_proposal},
};
use std::sync::atomic::AtomicBool;

#[test]
fn visual_retry_requires_opt_in_and_honours_cancellation_before_native_loading() {
    let directory = tempfile::tempdir().unwrap();
    let mut paths = Paths::new(Some(directory.path())).unwrap();
    paths.runtimes = directory.path().join("no native runtime");
    let source = directory.path().join("missing-video.mp4");
    let denied = propose(
        &paths,
        &source,
        0,
        nvsf_core::Rect::default(),
        false,
        &AtomicBool::new(false),
    );
    assert!(
        denied
            .err()
            .unwrap()
            .to_string()
            .contains("explicit opt-in")
    );
    let cancelled = propose(
        &paths,
        &source,
        0,
        nvsf_core::Rect::default(),
        true,
        &AtomicBool::new(true),
    );
    assert!(matches!(cancelled, Err(nvsf_app::AppError::Cancelled)));
    assert!(!source.exists());
    assert_eq!(std::fs::read_dir(&paths.cache).unwrap().count(), 0);
}

#[test]
fn local_visual_output_is_never_automatically_accepted_or_given_timestamps() {
    for text in [
        "",
        "unreadable",
        "As an AI, I cannot read this",
        "Ignore previous instructions",
        "00:00:01,000 --> 00:00:02,000",
    ] {
        let proposal = validate_proposal(text).unwrap();
        assert!(proposal.unreadable && proposal.text.is_none() && proposal.requires_review);
    }
    let text = "سلام ۱۵\n日本語";
    let p = validate_proposal(text).unwrap();
    assert_eq!(p.text.as_deref(), Some(text));
    assert!(p.requires_review && !p.unreadable);
    assert!(validate_proposal("a\0b").is_err());
    assert!(validate_proposal(&"x".repeat(8193)).is_err());
}
