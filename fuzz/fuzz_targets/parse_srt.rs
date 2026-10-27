#![no_main]

use libfuzzer_sys::fuzz_target;
use nvsf_core::{export_srt, parse_srt};

fuzz_target!(|data: &[u8]| {
    if data.len() > 4096 {
        return;
    }
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(cues) = parse_srt(text) else {
        return;
    };
    for cue in &cues {
        cue.validate().expect("Parser returned an invalid cue");
    }
    let exported = export_srt(&cues, false, false).expect("Validated cues failed export");
    let restored = parse_srt(&exported).expect("Exported subtitle failed to parse");
    assert_eq!(restored.len(), cues.len());
    for (actual, expected) in restored.iter().zip(&cues) {
        assert_eq!(actual.start_ms, expected.start_ms);
        assert_eq!(actual.end_ms, expected.end_ms);
        assert_eq!(actual.text, expected.text.replace('\r', ""));
    }
});
