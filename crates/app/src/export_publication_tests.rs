use super::export_project_checked;
use nvsf_core::{Cue, ExtractConfig, MediaInfo, Project};

#[test]
fn all_track_publications_keep_the_primary_directory_after_an_admission_callback_swap() {
    let directory = tempfile::tempdir().unwrap();
    let selected = directory.path().join("selected");
    let media = directory.path().join("media");
    let admitted = directory.path().join("admitted");
    std::fs::create_dir(&selected).unwrap();
    std::fs::create_dir(&media).unwrap();
    let source = media.join("original.srt");
    std::fs::write(&source, b"original media").unwrap();
    let output = selected.join("original.srt");
    let project = Project {
        schema: 1,
        source_hash: crate::storage::hash_file(&source).unwrap(),
        config_hash: "b".repeat(64),
        model_hash: "c".repeat(64),
        config: ExtractConfig {
            input: source.to_string_lossy().into_owned(),
            output: output.to_string_lossy().into_owned(),
            ..ExtractConfig::default()
        },
        media: MediaInfo {
            width: 640,
            height: 360,
            duration_ms: Some(1000),
            codec: "ffv1".into(),
            time_base_num: 1,
            time_base_den: 1000,
            subtitle_streams: 0,
            rotation: 0,
        },
        cues: vec![
            Cue::new(1, 0, 0, 1000, "English".into()).unwrap(),
            Cue::new(2, 1, 0, 1000, "日本語".into()).unwrap(),
        ],
        processed_ms: 1000,
        active_appearances: vec![],
        completed: true,
        warnings: vec![],
    };
    let mut admitted_count = 0;
    export_project_checked(&project, &output, "srt", true, false, false, |_| {
        if admitted_count == 0 {
            // Later track admission must use the first writer's directory handle too.
            std::fs::rename(&selected, &admitted)?;
            std::fs::rename(&media, &selected)?;
        }
        admitted_count += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(admitted_count, 3);
    assert_eq!(
        std::fs::read(selected.join("original.srt")).unwrap(),
        b"original media"
    );
    assert_eq!(std::fs::read_dir(&selected).unwrap().count(), 1);
    assert!(
        std::fs::read_to_string(admitted.join("original.srt"))
            .unwrap()
            .contains("English")
    );
    assert!(
        std::fs::read_to_string(admitted.join("original.track-0.srt"))
            .unwrap()
            .contains("English")
    );
    assert!(
        std::fs::read_to_string(admitted.join("original.track-1.srt"))
            .unwrap()
            .contains("日本語")
    );
    assert_eq!(std::fs::read_dir(&admitted).unwrap().count(), 3);
}
