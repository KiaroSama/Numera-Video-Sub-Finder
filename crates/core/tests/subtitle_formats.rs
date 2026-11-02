use nvsf_core::{Cue, export_ass, export_vtt};

fn cue(start: u64, end: u64, text: &str) -> Cue {
    Cue::new(1, 0, start, end, text.into()).unwrap()
}

#[test]
fn vtt_encodes_literal_markup_entities_and_normalizes_cr_without_changing_evidence() {
    let cue = cue(1234, 5678, "<b>سلام &amp; 日本</b>\r\nnext > literal");
    let original = cue.clone();
    assert_eq!(
        export_vtt(&[cue.clone()]).unwrap(),
        "WEBVTT\n\n00:00:01.234 --> 00:00:05.678\n&lt;b&gt;سلام &amp;amp; 日本&lt;/b&gt;\nnext &gt; literal\n\n"
    );
    assert_eq!(cue, original);
}

#[test]
fn ass_encloses_subcentisecond_intervals_and_preserves_aligned_edges() {
    for (start, end, expected) in [
        (1, 9, "00:00:00.00,00:00:00.01"),
        (9, 10, "00:00:00.00,00:00:00.01"),
        (10, 11, "00:00:00.01,00:00:00.02"),
        (10, 20, "00:00:00.01,00:00:00.02"),
        (1234, 5678, "00:00:01.23,00:00:05.68"),
    ] {
        let exported = export_ass(&[cue(start, end, "سلام\r\n日本")]).unwrap();
        assert!(exported.contains(&format!(
            "Dialogue: 0,{expected},Default,,0,0,0,,سلام\\N日本\n"
        )));
        assert!(!exported.contains('\r'));
    }
}

#[test]
fn formats_keep_nonwrapping_hours_and_ass_rounding_does_not_overflow_u64() {
    let late = cue(360_000_001, 360_000_009, "late");
    assert!(
        export_vtt(&[late.clone()])
            .unwrap()
            .contains("100:00:00.001 --> 100:00:00.009")
    );
    assert!(
        export_ass(&[late])
            .unwrap()
            .contains("100:00:00.00,100:00:00.01")
    );
    let last = cue(u64::MAX - 1, u64::MAX, "last");
    assert!(
        export_vtt(&[last.clone()])
            .unwrap()
            .contains("5124095576030:25:51.614 --> 5124095576030:25:51.615")
    );
    assert!(
        export_ass(&[last])
            .unwrap()
            .contains("5124095576030:25:51.61,5124095576030:25:51.62")
    );
}
