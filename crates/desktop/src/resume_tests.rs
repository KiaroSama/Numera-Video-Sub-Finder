use crate::resume::check_checkpoint;
use nvsf_core::{Cue, ExtractConfig, MediaInfo, Project};
use std::path::Path;

#[test]
fn checkpoint_admission_preserves_exact_config_and_saved_corrections() {
    let root = std::env::current_dir().unwrap();
    let config = ExtractConfig {
        input: root.join("video.mp4").to_str().unwrap().into(),
        output: root.join("video.srt").to_str().unwrap().into(),
        ..ExtractConfig::default()
    };
    let mut cue = Cue::new(1, 0, 0, 1000, "Original OCR".into()).unwrap();
    cue.text = "Corrected فارسی 日本".into();
    cue.edited = true;
    let mut project = Project {
        schema: 1,
        source_hash: "a".repeat(64),
        config_hash: "b".repeat(64),
        model_hash: "c".repeat(64),
        config,
        media: MediaInfo {
            width: 1920,
            height: 1080,
            duration_ms: Some(10_000),
            codec: "h264".into(),
            time_base_num: 1,
            time_base_den: 1000,
            subtitle_streams: 0,
            rotation: 0,
        },
        cues: vec![cue],
        processed_ms: 1500,
        active_appearances: vec![],
        completed: false,
        warnings: vec![],
    };
    let checkpoint = Path::new(&project.config.output).with_extension("nvsf.json");
    let before = serde_json::to_vec(&project).unwrap();
    check_checkpoint(&project, &checkpoint).unwrap();
    assert_eq!(
        serde_json::to_vec(&project).unwrap(),
        before,
        "admission must not rewrite config/corrections"
    );
    assert!(check_checkpoint(&project, &root.join("copied.json")).is_err());
    project.completed = true;
    assert!(check_checkpoint(&project, &checkpoint).is_err());
    project.completed = false;
    project.config.input = "relative.mp4".into();
    assert!(check_checkpoint(&project, &checkpoint).is_err());
}
