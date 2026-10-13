use nvsf_core::{AppearanceTracker, Cue, Recognition, export_srt, parse_srt};

#[test]
fn bounded_high_volume_export_and_interleaved_tracks_preserve_every_cue() {
    let cues: Vec<_> = (0..100_000u64)
        .map(|i| Cue::new(i + 1, (i % 2) as u32, i * 1000, i * 1000 + 500, "x".into()).unwrap())
        .collect();
    let text = export_srt(&cues, false, false).unwrap();
    let actual = parse_srt(&text).unwrap();
    assert_eq!(actual.len(), 100_000);
    assert_eq!(actual[99_999].start_ms, 99_999_000);
}

#[test]
fn actual_threaded_trackers_are_independent_and_numeric_gaps_are_not_merged() {
    std::thread::scope(|scope| {
        let tasks: Vec<_> = (0..2u32)
            .map(|track| {
                scope.spawn(move || {
                    let mut t = AppearanceTracker::new(track);
                    let mut finished = Vec::new();
                    for i in 0..1000u64 {
                        let r = if i % 3 == 2 {
                            None
                        } else {
                            Some(Recognition {
                                text: if i % 3 == 0 { "15" } else { "16" }.into(),
                                confidence: Some(0.9),
                                crop: None,
                            })
                        };
                        if let Some(c) = t.observe(i * 40, i * 40 + 40, r).unwrap() {
                            finished.push(c);
                        }
                    }
                    if let Some(c) = t.finish() {
                        finished.push(c);
                    }
                    assert!(
                        finished
                            .iter()
                            .all(|c| c.track == track && c.end_ms > c.start_ms)
                    );
                    finished
                })
            })
            .collect();
        let first = tasks
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(first[0].len(), 667);
        assert_eq!(first[1].len(), 667);
        assert_ne!(first[0][0].id, first[1][0].id);
    });
}

proptest::proptest! {
    #[test]
    fn hostile_subtitle_parser_never_panics(text in ".{0,2048}") {let _=parse_srt(&text);}
    #[test]
    fn source_time_preserves_generated_positive_rational_boundaries(pts in 0i64..1_000_000, numerator in 1i64..1000, denominator in 1i64..1_000_000) {
        let time=nvsf_core::Time::from_pts(pts,numerator,denominator).unwrap();
        proptest::prop_assert!(time.micros()>=0);
        proptest::prop_assert!(time.relative_ms(nvsf_core::Time::from_pts(0,1,1).unwrap()).is_ok());
    }
}
