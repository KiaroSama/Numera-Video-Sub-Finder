use nvsf_app::export_project;
use nvsf_core::{Cue, ExtractConfig, MediaInfo, Project};

fn project(cues: Vec<Cue>) -> Project {
    Project {
        schema: 1,
        source_hash: "a".repeat(64),
        config_hash: "b".repeat(64),
        model_hash: "c".repeat(64),
        config: ExtractConfig {
            input: "/media/input.mp4".into(),
            output: "/media/output.srt".into(),
            ..ExtractConfig::default()
        },
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
        cues,
        processed_ms: 5_000,
        active_appearances: vec![],
        completed: false,
        warnings: vec![],
    }
}

fn cue(id: u64, track: u32, start: u64, end: u64, text: &str) -> Cue {
    Cue::new(id, track, start, end, text.into()).unwrap()
}

#[test]
fn srt_export_honours_bom_and_line_ending_options() {
    let directory = tempfile::tempdir().unwrap();
    let project = project(vec![cue(1, 0, 0, 1_000, "سلام 日本")]);
    let plain = directory.path().join("plain.srt");
    export_project(&project, &plain, "srt", false, false, false).unwrap();
    let text = std::fs::read_to_string(&plain).unwrap();
    assert!(text.starts_with("1\n00:00:00,000 --> 00:00:01,000\n"));
    assert!(!text.contains('\r'));
    assert!(!text.starts_with('\u{feff}'));

    let marked = directory.path().join("marked.srt");
    export_project(&project, &marked, "srt", false, true, true).unwrap();
    let text = std::fs::read_to_string(&marked).unwrap();
    assert!(text.starts_with('\u{feff}'));
    assert!(text.contains("\r\n"));
    assert!(text.contains("سلام 日本"));
}

#[test]
fn alternate_formats_keep_expected_headers_and_unknown_formats_fail() {
    let directory = tempfile::tempdir().unwrap();
    let project = project(vec![cue(1, 0, 0, 1_000, "Hello")]);

    let vtt = directory.path().join("out.vtt");
    export_project(&project, &vtt, "vtt", false, false, false).unwrap();
    let text = std::fs::read_to_string(&vtt).unwrap();
    assert!(text.starts_with("WEBVTT\n\n"));
    assert!(text.contains("00:00:00.000 --> 00:00:01.000"));

    let ass = directory.path().join("out.ass");
    export_project(&project, &ass, "ass", false, false, false).unwrap();
    let text = std::fs::read_to_string(&ass).unwrap();
    assert!(text.contains("[Events]"));
    assert!(text.contains("Dialogue: 0,00:00:00.00,00:00:01.00,Default,,0,0,0,,Hello"));

    let json = directory.path().join("out.json");
    export_project(&project, &json, "json", false, false, false).unwrap();
    let restored: Project = serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
    assert_eq!(restored.cues, project.cues);

    let unsupported = directory.path().join("out.txt");
    assert!(export_project(&project, &unsupported, "txt", false, false, false).is_err());
    assert!(!unsupported.exists());
}

#[test]
fn simultaneous_tracks_export_separately_in_every_subtitle_format() {
    let directory = tempfile::tempdir().unwrap();
    let project = project(vec![
        cue(1, 0, 0, 1_000, "English"),
        cue(2, 1, 0, 1_000, "日本語"),
    ]);
    for format in ["srt", "vtt", "ass"] {
        let target = directory.path().join(format!("dual.{format}"));
        export_project(&project, &target, format, false, false, false).unwrap();
        let first =
            std::fs::read_to_string(directory.path().join(format!("dual.track-0.{format}")))
                .unwrap();
        let second =
            std::fs::read_to_string(directory.path().join(format!("dual.track-1.{format}")))
                .unwrap();
        assert!(first.contains("English") && !first.contains("日本語"));
        assert!(second.contains("日本語") && !second.contains("English"));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), first);
    }
}

#[test]
fn a_conflicting_track_preserves_the_entire_existing_export_set() {
    let directory = tempfile::tempdir().unwrap();
    let project = project(vec![
        cue(1, 0, 0, 1_000, "English"),
        cue(2, 1, 0, 1_000, "日本語"),
    ]);
    let combined = directory.path().join("dual.srt");
    let first = directory.path().join("dual.track-0.srt");
    let conflict = directory.path().join("dual.track-1.srt");
    std::fs::write(&combined, b"previous combined").unwrap();
    std::fs::write(&first, b"previous track").unwrap();
    std::fs::create_dir(&conflict).unwrap();
    assert!(export_project(&project, &combined, "srt", true, false, false).is_err());
    assert_eq!(std::fs::read(&combined).unwrap(), b"previous combined");
    assert_eq!(std::fs::read(&first).unwrap(), b"previous track");
    assert!(conflict.is_dir());
    assert_eq!(std::fs::read_dir(&conflict).unwrap().count(), 0);
}

#[test]
fn existing_destinations_are_preserved_unless_overwrite_is_explicit() {
    let directory = tempfile::tempdir().unwrap();
    let project = project(vec![cue(1, 0, 0, 1_000, "First")]);
    let target = directory.path().join("existing.srt");
    std::fs::write(&target, b"previous review").unwrap();
    assert!(export_project(&project, &target, "srt", false, false, false).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"previous review");

    export_project(&project, &target, "srt", true, false, false).unwrap();
    assert!(std::fs::read_to_string(&target).unwrap().contains("First"));

    let missing_parent = directory.path().join("absent").join("out.srt");
    assert!(export_project(&project, &missing_parent, "srt", true, false, false).is_err());
    assert!(!missing_parent.exists());
}

#[test]
fn export_cannot_replace_source_media_even_when_overwrite_is_enabled() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("original.mp4");
    std::fs::write(&source, b"original media bytes").unwrap();
    let mut project = project(vec![cue(1, 0, 0, 1_000, "Subtitle")]);
    project.config.input = source.to_string_lossy().into_owned();
    for format in ["srt", "vtt", "ass", "json"] {
        let error = export_project(&project, &source, format, true, false, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("source media"), "{error}");
        assert_eq!(std::fs::read(&source).unwrap(), b"original media bytes");
    }
    let error = nvsf_app::storage::save_project(&project, &source)
        .unwrap_err()
        .to_string();
    assert!(error.contains("source media"), "{error}");
    assert_eq!(std::fs::read(&source).unwrap(), b"original media bytes");
}

#[test]
fn track_output_source_collision_is_rejected_before_any_export_changes() {
    let directory = tempfile::tempdir().unwrap();
    let selected = directory.path().join("dual.srt");
    let source = directory.path().join("dual.track-1.srt");
    let first = directory.path().join("dual.track-0.srt");
    std::fs::write(&selected, b"previous subtitle").unwrap();
    std::fs::write(&first, b"previous first track").unwrap();
    std::fs::write(&source, b"original media with an arbitrary extension").unwrap();
    let mut project = project(vec![
        cue(1, 0, 0, 1_000, "English"),
        cue(2, 1, 0, 1_000, "日本語"),
    ]);
    project.config.input = source.to_string_lossy().into_owned();
    let error = export_project(&project, &selected, "srt", true, false, false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("source media"), "{error}");
    assert_eq!(std::fs::read(&selected).unwrap(), b"previous subtitle");
    assert_eq!(std::fs::read(&first).unwrap(), b"previous first track");
    assert_eq!(
        std::fs::read(&source).unwrap(),
        b"original media with an arbitrary extension"
    );
}

#[test]
fn replacing_a_separate_hard_link_does_not_replace_the_source_file() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("original.mp4");
    let selected = directory.path().join("subtitle.srt");
    std::fs::write(&source, b"original media bytes").unwrap();
    std::fs::hard_link(&source, &selected).unwrap();
    let mut project = project(vec![cue(1, 0, 0, 1_000, "Subtitle")]);
    project.config.input = source.to_string_lossy().into_owned();
    project.source_hash = nvsf_app::storage::hash_file(&source).unwrap();
    export_project(&project, &selected, "srt", true, false, false).unwrap();
    assert_eq!(std::fs::read(&source).unwrap(), b"original media bytes");
    assert!(
        std::fs::read_to_string(&selected)
            .unwrap()
            .contains("Subtitle")
    );
}

#[cfg(unix)]
#[test]
fn a_source_selected_through_a_symlink_is_protected_by_its_resolved_path() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("original.mp4");
    let alias = directory.path().join("selected.mp4");
    std::fs::write(&source, b"original media bytes").unwrap();
    std::os::unix::fs::symlink(&source, &alias).unwrap();
    let mut project = project(vec![cue(1, 0, 0, 1_000, "Subtitle")]);
    project.config.input = alias.to_string_lossy().into_owned();
    let error = export_project(&project, &source, "srt", true, false, false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("source media"), "{error}");
    assert_eq!(std::fs::read(&source).unwrap(), b"original media bytes");
    assert!(alias.is_symlink());
}

#[cfg(windows)]
#[test]
fn source_media_is_protected_when_the_destination_uses_different_case() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("Original.MP4");
    let destination = directory.path().join("original.mp4");
    std::fs::write(&source, b"original media bytes").unwrap();
    let mut project = project(vec![cue(1, 0, 0, 1_000, "Subtitle")]);
    project.config.input = source.to_string_lossy().into_owned();
    let error = export_project(&project, &destination, "srt", true, false, false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("source media"), "{error}");
    assert_eq!(std::fs::read(&source).unwrap(), b"original media bytes");
}

#[test]
fn a_relocated_legacy_project_preserves_its_source_by_content_identity() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("relocated-original.mp4");
    std::fs::write(&source, b"hello world").unwrap();
    let mut project = project(vec![cue(1, 0, 0, 1_000, "Subtitle")]);
    // Model a project opened from a different working directory without changing process CWD.
    project.config.input = "unavailable-original-directory/video.mp4".into();
    project.source_hash = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9".into();
    assert!(export_project(&project, &source, "srt", true, false, false).is_err());
    assert!(nvsf_app::storage::save_project(&project, &source).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"hello world");
    let safe = directory.path().join("review.srt");
    export_project(&project, &safe, "srt", false, false, false).unwrap();
    assert!(std::fs::read_to_string(safe).unwrap().contains("Subtitle"));
}

#[test]
fn a_reused_absolute_source_path_does_not_disable_recorded_identity_protection() {
    let directory = tempfile::tempdir().unwrap();
    let old_source = directory.path().join("old-original.mp4");
    let relocated_source = directory.path().join("relocated-original.mp4");
    std::fs::write(&old_source, b"an unrelated replacement").unwrap();
    std::fs::write(&relocated_source, b"hello world").unwrap();
    let mut project = project(vec![cue(1, 0, 0, 1_000, "Subtitle")]);
    project.config.input = old_source.to_string_lossy().into_owned();
    project.source_hash = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9".into();
    for format in ["srt", "vtt", "ass", "json"] {
        let error = export_project(&project, &relocated_source, format, true, false, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("source media"), "{error}");
    }
    assert!(nvsf_app::storage::save_project(&project, &relocated_source).is_err());
    assert_eq!(std::fs::read(&relocated_source).unwrap(), b"hello world");
    assert_eq!(
        std::fs::read(&old_source).unwrap(),
        b"an unrelated replacement"
    );
    let safe = directory.path().join("review.srt");
    export_project(&project, &safe, "srt", false, false, false).unwrap();
    assert!(std::fs::read_to_string(safe).unwrap().contains("Subtitle"));
}
