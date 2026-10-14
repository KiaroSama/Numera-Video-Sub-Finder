use nvsf_core::{
    AppearanceTracker, Cue, EditHistory, Error, Recognition, Time, parse_srt, parse_time, split_cue,
};

#[test]
fn presentation_origin_uses_exact_microseconds_and_rejects_negative_or_overflow() {
    let origin = Time::from_pts(2_000_000, 1, 1_000_000).unwrap();
    assert_eq!(origin.relative_ms(origin).unwrap(), 0);
    let later = Time::from_pts(3_234_567, 1, 1_000_000).unwrap();
    assert_eq!(later.relative_ms(origin).unwrap(), 1_234);
    let earlier = Time::from_pts(1_999_999, 1, 1_000_000).unwrap();
    assert!(matches!(
        earlier.relative_ms(origin),
        Err(Error::Invalid(_))
    ));
    let maximum = Time::from_pts(i64::MAX, 1, 1_000_000).unwrap();
    let minimum = Time::from_pts(i64::MIN, 1, 1_000_000).unwrap();
    assert!(matches!(maximum.relative_ms(minimum), Err(Error::Overflow)));
}

#[test]
fn timecode_components_validate_structure_independently() {
    for value in [
        "0",
        "00:00:00",
        "00:00:00,1",
        "00:00:00,0000",
        ":00:00,000",
        "+0:00:00,000",
        "00:60:00,000",
        "00:00:60,000",
        "00:00:00,abc",
    ] {
        assert!(
            matches!(parse_time(value), Err(Error::Invalid(_))),
            "{value}"
        );
    }
    assert_eq!(parse_time("00:59:59,999").unwrap(), 3_599_999);
    assert_eq!(parse_time("100:00:00,001").unwrap(), 360_000_001);
}

#[test]
fn literal_subtitle_indices_become_positive_distinct_cue_identities() {
    let cues =
        parse_srt("1\n00:00:00,000 --> 00:00:00,500\nA\n\n2\n00:00:01,000 --> 00:00:01,500\nB\n\n")
            .unwrap();
    assert_eq!(cues.iter().map(|c| c.id).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(cues[1].text, "B");
}

#[test]
fn representative_evidence_changes_only_for_strictly_better_confidence() {
    let mut tracker = AppearanceTracker::new(0);
    for (start, confidence, crop, expected_confidence, expected_crop) in [
        (0, 0.2, "first.png", 0.2, "first.png"),
        (40, 0.9, "better.png", 0.9, "better.png"),
        (80, 0.2, "worse.png", 0.9, "better.png"),
        (120, 0.9, "equal.png", 0.9, "better.png"),
    ] {
        let recognition = Recognition {
            text: "15".into(),
            confidence: Some(confidence),
            crop: Some(crop.into()),
        };
        assert!(
            tracker
                .observe(start, start + 40, Some(recognition))
                .unwrap()
                .is_none()
        );
        let cue = tracker.active().unwrap();
        assert_eq!(cue.confidence, Some(expected_confidence));
        assert_eq!(cue.crop.as_deref(), Some(expected_crop));
    }
    let cue = tracker.finish().unwrap();
    assert_eq!((cue.start_ms, cue.end_ms), (0, 160));
    assert_eq!(cue.raw_text, "15");
}

#[test]
fn invalid_direct_splits_preserve_cues_without_an_edit_history_wrapper() {
    let original = Cue::new(2, 0, 100, 900, "Hello 日本".into()).unwrap();
    for at in [0, 100, 900, 1_000] {
        let mut cues = vec![original.clone()];
        assert!(split_cue(&mut cues, 2, at).is_err());
        assert_eq!(cues, [original.clone()]);
    }
}

#[test]
fn history_availability_follows_undo_redo_and_new_edit_transitions() {
    let original = Cue::new(1, 0, 0, 100, "first".into()).unwrap();
    let mut cues = vec![original.clone()];
    let mut history = EditHistory::default();
    assert!(!history.can_undo());
    assert!(!history.can_redo());
    history
        .apply(&mut cues, |c| {
            c[0].text = "second".into();
            Ok(())
        })
        .unwrap();
    assert!(history.can_undo());
    assert!(!history.can_redo());
    history
        .apply(&mut cues, |c| {
            c[0].text = "third".into();
            Ok(())
        })
        .unwrap();
    assert!(history.undo(&mut cues));
    assert_eq!(cues[0].text, "second");
    assert!(history.can_redo());
    assert!(history.undo(&mut cues));
    assert_eq!(cues[0], original);
    assert!(!history.can_undo());
    assert!(history.redo(&mut cues));
    assert_eq!(cues[0].text, "second");
    history
        .apply(&mut cues, |c| {
            c[0].text = "replacement".into();
            Ok(())
        })
        .unwrap();
    assert!(!history.can_redo());
    assert!(!history.redo(&mut cues));
}
