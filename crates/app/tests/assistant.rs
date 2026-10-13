use nvsf_app::assistant::validate_proposal;

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
