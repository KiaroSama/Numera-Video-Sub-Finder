use nvsf_core::{Cue, Rect, Time, export_srt, normalize_persian, parse_srt};

#[test]
fn source_time_and_long_unicode_exports_round_trip() {
    assert_eq!(
        Time::from_pts(90_000, 1, 90_000).unwrap().micros(),
        1_000_000
    );
    assert!(Time::from_pts(i64::MAX, i64::MAX, 1).is_err());
    assert!(Time::from_pts(1, 1, 0).is_err());
    let cue = Cue::new(
        1,
        0,
        360_001_234,
        360_003_567,
        "سلام Numera ۱۲\n日本語".into(),
    )
    .unwrap();
    let srt = export_srt(&[cue.clone()], false, false).unwrap();
    assert!(srt.contains("100:00:01,234 --> 100:00:03,567"));
    assert_eq!(parse_srt(&srt).unwrap()[0].text, cue.text);
    assert!(Cue::new(1, 0, 20, 20, "x".into()).is_err());
    assert!(Cue::new(1, 0, 0, 20, "\0".into()).is_err());
}

#[test]
fn region_and_normalization_preserve_boundaries() {
    let r = Rect::new(0.1, 0.8, 0.8, 0.2).unwrap();
    assert_eq!(r.pixels(100, 100).unwrap(), (10, 80, 80, 20));
    assert!(Rect::new(f64::NAN, 0.0, 1.0, 1.0).is_err());
    assert!(Rect::new(0.8, 0.0, 0.4, 1.0).is_err());
    assert_eq!(
        normalize_persian("كی مي‌روم ۱۲ Tokyo", true),
        "کی می‌روم ۱۲ Tokyo"
    );
    assert_eq!(normalize_persian("كی", false), "كی");
}

#[test]
fn parser_rejects_invalid_intervals_and_structure() {
    for text in [
        "1\n00:00:02,000 --> 00:00:01,000\nx\n",
        "1\n00:61:00,000 --> 00:62:00,000\nx\n",
        "2\n00:00:00,000 --> 00:00:01,000\nx\n",
    ] {
        assert!(parse_srt(text).is_err());
    }
    let bom = "\u{feff}1\r\n00:00:00,000 --> 00:00:01,000\r\n日本語\r\n\r\n";
    assert_eq!(parse_srt(bom).unwrap()[0].text, "日本語");
}

#[test]
fn checkpoint_continues_an_active_appearance_without_duplicate_cues() {
    let recognition = || {
        Some(nvsf_core::Recognition {
            text: "15".into(),
            confidence: Some(0.9),
            crop: None,
        })
    };
    let mut tracker = nvsf_core::AppearanceTracker::new(0);
    tracker.observe(0, 40, recognition()).unwrap();
    let saved = tracker.active().unwrap().clone();
    let mut restored = nvsf_core::AppearanceTracker::restore(0, saved).unwrap();
    assert!(restored.observe(40, 80, recognition()).unwrap().is_none());
    let cue = restored.observe(80, 120, None).unwrap().unwrap();
    assert_eq!((cue.start_ms, cue.end_ms, cue.text.as_str()), (0, 80, "15"));
    assert!(restored.finish().is_none());
    assert!(restored.observe(u64::MAX, u64::MAX, recognition()).is_err());
    assert!(restored.observe(100, 90, recognition()).is_err());
    assert!(nvsf_core::AppearanceTracker::restore(1, cue).is_err());
}

#[test]
fn project_rejects_duplicate_cue_identities() {
    let cue = Cue::new(7, 0, 0, 100, "Hello".into()).unwrap();
    let mut project = nvsf_core::Project {
        schema: 1,
        source_hash: "source".into(),
        config_hash: "config".into(),
        model_hash: "models".into(),
        config: nvsf_core::ExtractConfig {
            input: "video.mkv".into(),
            output: "video.srt".into(),
            ..Default::default()
        },
        media: nvsf_core::MediaInfo {
            width: 640,
            height: 360,
            duration_ms: Some(1000),
            codec: "ffv1".into(),
            time_base_num: 1,
            time_base_den: 1000,
            subtitle_streams: 0,
            rotation: 0,
        },
        cues: vec![cue.clone()],
        processed_ms: 100,
        active_appearances: vec![],
        completed: false,
        warnings: vec![],
    };
    assert!(project.validate().is_ok());
    project.active_appearances = vec![cue.clone()];
    assert!(project.validate().is_ok());
    for failure in ["track", "duplicate", "future", "completed", "interval"] {
        let mut invalid = project.clone();
        match failure {
            "track" => invalid.active_appearances[0].track = 1,
            "duplicate" => invalid.active_appearances.push(cue.clone()),
            "future" => invalid.processed_ms = 99,
            "completed" => invalid.completed = true,
            _ => invalid.active_appearances[0].end_ms = 0,
        }
        assert!(invalid.validate().is_err(), "{failure}");
    }
    project.active_appearances.clear();
    project.cues.push(cue);
    assert!(project.validate().is_err());
}

#[test]
fn edit_history_rolls_back_invalid_changes_and_preserves_raw_evidence() {
    let original = Cue::new(1, 0, 0, 1000, "سلام 15".into()).unwrap();
    let mut cues = vec![original.clone()];
    let mut history = nvsf_core::EditHistory::default();
    assert!(
        history
            .apply(&mut cues, |c| {
                c[0].end_ms = 0;
                Ok(())
            })
            .is_err()
    );
    assert_eq!(cues[0], original);
    assert!(!history.can_undo());
    history
        .apply(&mut cues, |c| {
            c[0].text = "سلام 16".into();
            c[0].edited = true;
            Ok(())
        })
        .unwrap();
    assert_eq!(cues[0].raw_text, "سلام 15");
    assert!(history.undo(&mut cues));
    assert_eq!(cues[0], original);
    assert!(history.redo(&mut cues));
    assert_eq!(cues[0].text, "سلام 16");
}

proptest::proptest! {
    #[test]
    fn exported_times_round_trip(start in 0u64..400_000_000, duration in 1u64..100_000) {
        let c = Cue::new(1, 0, start, start + duration, "A ۱۵ 日本".into()).unwrap();
        let parsed = parse_srt(&export_srt(&[c], false, false).unwrap()).unwrap();
        proptest::prop_assert_eq!(parsed[0].start_ms, start);
        proptest::prop_assert_eq!(parsed[0].end_ms, start + duration);
    }
}
