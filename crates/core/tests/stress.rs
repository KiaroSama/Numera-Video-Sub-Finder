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

#[test]
fn undo_history_keeps_exactly_fifty_recent_small_edits() {
    let mut cues = vec![Cue::new(1, 0, 0, 100, "0".into()).unwrap()];
    let mut history = nvsf_core::EditHistory::default();
    for revision in 1..=51 {
        history
            .apply(&mut cues, |c| {
                c[0].text = revision.to_string();
                Ok(())
            })
            .unwrap();
    }
    for expected in (1..=50).rev() {
        assert!(history.undo(&mut cues));
        assert_eq!(cues[0].text, expected.to_string());
    }
    assert!(!history.undo(&mut cues));
    assert_eq!(cues[0].text, "1");
}

#[test]
fn byte_budget_retains_two_small_snapshots_but_evicts_older_large_snapshots() {
    for (count, expected_undos) in [(200, 2), (300, 1)] {
        let mut cues: Vec<_> = (0..count)
            .map(|i| Cue::new(i + 1, 0, i * 100, i * 100 + 50, "a".repeat(40_000)).unwrap())
            .collect();
        let mut history = nvsf_core::EditHistory::default();
        for revision in [1, 2] {
            history
                .apply(&mut cues, |c| {
                    c[0].track = revision;
                    Ok(())
                })
                .unwrap();
        }
        let mut undos = 0;
        for _ in 0..=expected_undos {
            if !history.undo(&mut cues) {
                break;
            }
            undos += 1;
        }
        assert_eq!(undos, expected_undos, "cue_count={count}");
        assert_eq!(cues[0].track, if count == 200 { 0 } else { 1 });
        assert_eq!(cues[0].raw_text.len(), 40_000);
    }
}

#[test]
fn undo_byte_boundary_is_inclusive_and_preserves_one_oversized_snapshot() {
    let per_cue = 65_536 - std::mem::size_of::<Cue>() - "evidence.png".len();
    let mut original = Cue::new(1, 0, 0, 100, "a".repeat(per_cue / 2)).unwrap();
    original.raw_text = "b".repeat(per_cue - original.text.len());
    original.crop = Some("evidence.png".into());
    original.validate().unwrap();
    // 256 cues make each snapshot 16 MiB, so two snapshots exactly meet the 32 MiB contract.
    for (extra_byte, expected_undos) in [(false, 2), (true, 1)] {
        let mut cues: Vec<_> = (1..=256)
            .map(|id| {
                let mut cue = original.clone();
                cue.id = id;
                cue
            })
            .collect();
        if extra_byte {
            cues[0].text.push('x');
        }
        let mut history = nvsf_core::EditHistory::default();
        for track in [1, 2] {
            history
                .apply(&mut cues, |c| {
                    c[0].track = track;
                    Ok(())
                })
                .unwrap();
        }
        let mut undos = 0;
        for _ in 0..=expected_undos {
            if !history.undo(&mut cues) {
                break;
            }
            undos += 1;
        }
        assert_eq!(undos, expected_undos, "above_byte_budget={extra_byte}");
    }
    let mut cues: Vec<_> = (1..=600)
        .map(|id| {
            let mut cue = original.clone();
            cue.id = id;
            cue
        })
        .collect();
    let mut history = nvsf_core::EditHistory::default();
    history
        .apply(&mut cues, |c| {
            c[0].track = 1;
            Ok(())
        })
        .unwrap();
    assert!(history.can_undo());
    assert!(history.undo(&mut cues));
    assert_eq!(cues[0].track, 0);
    assert!(!history.undo(&mut cues));
}

#[test]
fn parser_enforces_the_exact_input_byte_limit_before_trimming() {
    let mut text = " ".repeat(67_108_864);
    assert!(parse_srt(&text).unwrap().is_empty());
    text.push(' ');
    assert!(matches!(
        parse_srt(&text),
        Err(nvsf_core::Error::Invalid(_))
    ));
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
