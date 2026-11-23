use super::Checkpoint;
use nvsf_core::{Cue, ExtractConfig, MediaInfo, Project};
use std::path::Path;

fn project(source: &Path, output: &Path, tracks: &[u32]) -> Project {
    std::fs::write(source, b"source media fixture").unwrap();
    Project {
        schema: 1,
        source_hash: crate::storage::hash_file(source).unwrap(),
        config_hash: "b".repeat(64),
        model_hash: "c".repeat(64),
        config: ExtractConfig {
            input: source.to_string_lossy().into_owned(),
            output: output.to_string_lossy().into_owned(),
            overwrite: true,
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
        cues: tracks
            .iter()
            .enumerate()
            .map(|(index, &track)| {
                Cue::new(
                    index as u64 + 1,
                    track,
                    0,
                    1_000,
                    format!("Subtitle for track {track}"),
                )
                .unwrap()
            })
            .collect(),
        processed_ms: 5_000,
        active_appearances: vec![],
        completed: true,
        warnings: vec![],
    }
}

#[test]
fn every_track_collision_preserves_the_checkpoint_and_entire_export_set() {
    for selected_track in [0, 1] {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.mp4");
        let output = directory.path().join("subtitles.srt");
        let project = project(&source, &output, &[0, 1]);
        let tracks = [
            directory.path().join("subtitles.track-0.srt"),
            directory.path().join("subtitles.track-1.srt"),
        ];
        std::fs::write(&output, b"previous primary export").unwrap();
        for track in &tracks {
            std::fs::write(track, b"previous track export").unwrap();
        }
        let selected = &tracks[selected_track];
        crate::storage::save_project(&project, selected).unwrap();
        let before: Vec<_> = [&output, &tracks[0], &tracks[1]]
            .into_iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect();
        let checkpoint = Checkpoint::open(&output, Some(selected), &source).unwrap();
        let saved = checkpoint.load().unwrap().unwrap();
        let error = checkpoint
            .export(&saved, &output, true, false, false)
            .unwrap_err();
        assert!(error.to_string().contains("Checkpoint and its locks"));
        drop(checkpoint);
        for (path, expected) in [&output, &tracks[0], &tracks[1]].into_iter().zip(before) {
            assert_eq!(std::fs::read(path).unwrap(), expected);
        }
        let retained = crate::storage::load_project(selected).unwrap();
        assert_eq!(retained.cues, saved.cues);
        assert_eq!(std::fs::read(source).unwrap(), b"source media fixture");
    }
}

#[test]
fn an_unused_track_filename_remains_a_valid_single_track_checkpoint() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.mp4");
    let output = directory.path().join("subtitles.srt");
    let selected = directory.path().join("subtitles.track-0.srt");
    let project = project(&source, &output, &[0]);
    crate::storage::save_project(&project, &selected).unwrap();
    let before = std::fs::read(&selected).unwrap();
    let checkpoint = Checkpoint::open(&output, Some(&selected), &source).unwrap();
    checkpoint
        .export(&project, &output, false, false, false)
        .unwrap();
    assert_eq!(std::fs::read(selected).unwrap(), before);
    assert!(
        std::fs::read_to_string(output)
            .unwrap()
            .contains("Subtitle for track 0")
    );
    assert!(!directory.path().join("subtitles.track-1.srt").exists());
}

#[test]
fn a_separate_checkpoint_exports_every_track_with_the_requested_srt_encoding() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.mp4");
    let output = directory.path().join("subtitles.srt");
    let selected = directory.path().join("review.json");
    let project = project(&source, &output, &[0, 1]);
    crate::storage::save_project(&project, &selected).unwrap();
    let before = std::fs::read(&selected).unwrap();
    let checkpoint = Checkpoint::open(&output, Some(&selected), &source).unwrap();
    checkpoint
        .export(&project, &output, false, true, true)
        .unwrap();
    assert_eq!(std::fs::read(selected).unwrap(), before);
    for (path, track) in [
        (output, 0),
        (directory.path().join("subtitles.track-0.srt"), 0),
        (directory.path().join("subtitles.track-1.srt"), 1),
    ] {
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.starts_with('\u{feff}'));
        assert!(text.contains("\r\n"));
        assert!(!text.replace("\r\n", "").contains('\n'));
        assert!(text.contains(&format!("Subtitle for track {track}")));
        assert!(!text.contains(&format!("Subtitle for track {}", 1 - track)));
    }
}

#[cfg(unix)]
#[test]
fn a_generated_track_cannot_replace_either_checkpoint_lock_alias() {
    for output_lock in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.mp4");
        let output = directory.path().join("subtitles.srt");
        let selected = directory.path().join("review.json");
        let track = directory.path().join("subtitles.track-1.srt");
        let first = directory.path().join("subtitles.track-0.srt");
        let project = project(&source, &output, &[0, 1]);
        crate::storage::save_project(&project, &selected).unwrap();
        let before = std::fs::read(&selected).unwrap();
        std::fs::write(&output, b"previous primary export").unwrap();
        std::fs::write(&first, b"previous first track").unwrap();
        std::fs::write(&track, b"locked track bytes").unwrap();
        let lock = if output_lock {
            output.with_extension("nvsf.lock")
        } else {
            selected.with_extension("lock")
        };
        std::os::unix::fs::symlink(&track, lock).unwrap();
        let checkpoint = Checkpoint::open(&output, Some(&selected), &source).unwrap();
        let error = checkpoint
            .export(&project, &output, true, false, false)
            .unwrap_err();
        assert!(error.to_string().contains("Checkpoint and its locks"));
        assert!(crate::storage::JobLock::acquire(&track).is_err());
        drop(checkpoint);
        assert_eq!(std::fs::read(selected).unwrap(), before);
        assert_eq!(std::fs::read(output).unwrap(), b"previous primary export");
        assert_eq!(std::fs::read(first).unwrap(), b"previous first track");
        assert_eq!(std::fs::read(track).unwrap(), b"locked track bytes");
    }
}
