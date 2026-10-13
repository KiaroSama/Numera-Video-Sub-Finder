use nvsf_core::{Cue, EditHistory, Error, merge_cues, split_cue};

#[test]
fn split_merge_and_history_keep_positive_intervals_and_originals() {
    let original = Cue::new(2, 0, 100, 900, "Hello\n日本".into()).unwrap();
    let mut cues = vec![original.clone()];
    let mut history = EditHistory::default();
    for at in [0, 100, 900, 1000] {
        assert!(history.apply(&mut cues, |c| split_cue(c, 2, at)).is_err());
        assert_eq!(cues, vec![original.clone()]);
    }
    assert!(split_cue(&mut cues, 999, 500).is_err());
    history.apply(&mut cues, |c| split_cue(c, 2, 500)).unwrap();
    assert_eq!(cues.len(), 2);
    assert_eq!((cues[0].end_ms, cues[1].start_ms), (500, 500));
    assert_eq!(cues[1].id, 3);
    assert!(
        cues.iter()
            .all(|c| c.edited && c.raw_text == original.raw_text)
    );
    assert!(history.undo(&mut cues));
    assert_eq!(cues, vec![original.clone()]);
    assert!(history.redo(&mut cues));
    history.apply(&mut cues, |c| merge_cues(c, 2, 3)).unwrap();
    assert_eq!(cues.len(), 1);
    assert_eq!((cues[0].start_ms, cues[0].end_ms), (100, 900));
    assert!(merge_cues(&mut cues, 2, 2).is_err());
    assert!(merge_cues(&mut cues, 2, 999).is_err());
    let mut oversized = vec![
        Cue::new(1, 0, 0, 100, "a".repeat(40_000)).unwrap(),
        Cue::new(2, 0, 100, 200, "b".repeat(40_000)).unwrap(),
    ];
    let preserved = oversized.clone();
    assert!(merge_cues(&mut oversized, 1, 2).is_err());
    assert_eq!(oversized, preserved);
    assert!(
        history
            .apply(&mut cues, |_| Err(Error::Invalid("controlled".into())))
            .is_err()
    );
}

#[test]
fn configuration_and_geometry_reject_every_invalid_boundary() {
    let valid = nvsf_core::ExtractConfig {
        input: "a.mp4".into(),
        output: "a.srt".into(),
        ..Default::default()
    };
    assert!(valid.validate().is_ok());
    let mut variants = Vec::new();
    let mut c = valid.clone();
    c.input.clear();
    variants.push(c);
    let mut c = valid.clone();
    c.output = c.input.clone();
    variants.push(c);
    let mut c = valid.clone();
    c.language = "xx".into();
    variants.push(c);
    let mut c = valid.clone();
    c.device = "unknown".into();
    variants.push(c);
    let mut c = valid.clone();
    c.quality = "unknown".into();
    variants.push(c);
    let mut c = valid.clone();
    c.threads = 0;
    variants.push(c);
    let mut c = valid.clone();
    c.threads = 17;
    variants.push(c);
    let mut c = valid.clone();
    c.regions.clear();
    variants.push(c);
    let mut c = valid.clone();
    c.regions = vec![nvsf_core::Rect::default(); 9];
    variants.push(c);
    let mut c = valid.clone();
    c.exclusions = vec![nvsf_core::Rect::default(); 33];
    variants.push(c);
    let mut c = valid.clone();
    c.min_confidence = f32::NAN;
    variants.push(c);
    let mut c = valid.clone();
    c.min_confidence = 1.1;
    variants.push(c);
    let mut c = valid.clone();
    c.end_ms = Some(0);
    variants.push(c);
    let mut c = valid.clone();
    c.device = "cpu".into();
    c.gpu_required = true;
    variants.push(c);
    for c in variants {
        assert!(c.validate().is_err());
    }
    for value in ["", "a,0,1,1", "0,0,1", "0,0,1,1,1", "-1,0,1,1", "0,0,0,1"] {
        assert!(value.parse::<nvsf_core::Rect>().is_err());
    }
    let rect = "0,0,1,1".parse::<nvsf_core::Rect>().unwrap();
    assert_eq!(rect.pixels(2, 2).unwrap(), (0, 0, 2, 2));
    assert!(rect.pixels(0, 1).is_err());
    assert!(rect.pixels(10000, 10000).is_err());
    let tiny = nvsf_core::Rect::new(0., 0., 0.0001, 0.0001).unwrap();
    assert!(tiny.pixels(2, 2).is_err());
}

#[test]
fn exports_have_known_independent_wire_format() {
    let c = Cue::new(1, 0, 1234, 5678, "A ۱۵\n日本".into()).unwrap();
    assert_eq!(
        nvsf_core::export_srt(&[c.clone()], true, true).unwrap(),
        "\u{feff}1\r\n00:00:01,234 --> 00:00:05,678\r\nA ۱۵\r\n日本\r\n\r\n"
    );
    assert_eq!(
        nvsf_core::export_vtt(&[c.clone()]).unwrap(),
        "WEBVTT\n\n00:00:01.234 --> 00:00:05.678\nA ۱۵\n日本\n\n"
    );
    let ass = nvsf_core::export_ass(&[c]).unwrap();
    assert!(ass.contains("Dialogue: 0,00:00:01.23,00:00:05.67,Default,,0,0,0,,A ۱۵\\N日本"));
    assert_eq!(nvsf_core::parse_time("01:02:03,004").unwrap(), 3_723_004);
    assert!(nvsf_core::parse_time("18446744073709551615:00:00,000").is_err());
}
