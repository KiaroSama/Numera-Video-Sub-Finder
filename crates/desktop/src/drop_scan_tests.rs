use super::*;
use std::cell::Cell;
use tempfile::tempdir;

#[test]
fn directory_enumeration_stops_at_budget_without_accepting_an_arbitrary_prefix() {
    let reads = Cell::new(0);
    let entries = std::iter::repeat_with(|| {
        reads.set(reads.get() + 1);
        Ok(PathBuf::from(format!("clip-{}.mp4", reads.get())))
    });
    let mut remaining = 8;
    assert!(matches!(
        bounded_children(entries, &mut remaining),
        Err(ScanError::Limit)
    ));
    assert_eq!(reads.get(), 9);
    assert_eq!(remaining, 0);

    let dir = tempdir().expect("tempdir");
    for name in ["d.mp4", "b.mp4", "a.mp4", "c.mp4"] {
        fs::write(dir.path().join(name), b"video").expect("video");
    }
    let result = resolve_with_budget(&[dir.path().to_path_buf()], 4);
    assert!(result.from_folder);
    assert!(result.truncated);
    assert!(result.videos.is_empty());
    assert_eq!(result.errors.len(), 1);
    assert!(result.errors[0].contains("4-entry scan budget"));
    assert!(result.errors[0].contains("Drop a smaller folder or select video files directly"));
}

#[test]
fn exact_budget_directories_are_sorted_and_do_not_report_truncation() {
    let dir = tempdir().expect("tempdir");
    for name in ["c.mp4", "a.mp4", "b.mp4"] {
        fs::write(dir.path().join(name), b"video").expect("video");
    }
    let result = resolve_with_budget(&[dir.path().to_path_buf()], 4);
    assert!(!result.truncated);
    assert!(result.errors.is_empty());
    assert_eq!(result.videos.len(), 3);
    assert!(result.videos[0].ends_with("a.mp4"));
    assert!(result.videos[1].ends_with("b.mp4"));
    assert!(result.videos[2].ends_with("c.mp4"));
}

#[test]
fn video_limit_stops_before_traversing_remaining_folders() {
    let dir = tempdir().expect("tempdir");
    for index in 0..MAX_DROP_FILES {
        fs::write(dir.path().join(format!("{index:03}.mp4")), b"video").expect("video");
    }
    let late = dir.path().join("z-later-folder");
    fs::create_dir(&late).expect("folder");
    for index in 0..20 {
        fs::write(late.join(format!("{index}.txt")), b"unsupported").expect("text");
    }
    let result = resolve_dropped_paths(&[dir.path().to_path_buf()]);
    assert_eq!(result.videos.len(), MAX_DROP_FILES);
    assert_eq!(result.skipped_unsupported, 0);
    assert!(result.truncated);
    assert!(
        result
            .errors
            .iter()
            .any(|error| error.contains("100-video drop limit"))
    );
    assert!(result.videos[0].ends_with("000.mp4"));
    assert!(result.videos[MAX_DROP_FILES - 1].ends_with("099.mp4"));
}

#[test]
fn scan_budget_is_shared_across_nested_and_duplicate_roots() {
    let dir = tempdir().expect("tempdir");
    let first = dir.path().join("a-small");
    let second = dir.path().join("b-wide");
    fs::create_dir(&first).expect("first");
    fs::create_dir(&second).expect("second");
    fs::write(first.join("a.mp4"), b"video").expect("video");
    for index in 0..4 {
        fs::write(second.join(format!("{index}.mp4")), b"video").expect("video");
    }
    let forward = resolve_with_budget(&[first.clone(), second.clone(), first.clone()], 6);
    let reversed = resolve_with_budget(&[second, first], 6);
    assert_eq!(forward, reversed);
    assert!(forward.truncated);
    assert_eq!(forward.videos.len(), 1);
    assert!(forward.videos[0].ends_with("a.mp4"));
}

#[test]
fn metadata_failures_report_the_path_and_os_error_and_keep_other_roots() {
    let dir = tempdir().expect("tempdir");
    let missing = dir.path().join("a-missing.mp4");
    let good = dir.path().join("z-present.mp4");
    fs::write(&good, b"video").expect("video");
    let os_error = fs::symlink_metadata(&missing)
        .expect_err("missing input")
        .to_string();
    let result = resolve_dropped_paths(&[missing.clone(), good.clone()]);
    assert_eq!(
        result.videos,
        [good.canonicalize().expect("canonical source")]
    );
    assert_eq!(result.skipped_unsupported, 0);
    assert_eq!(result.errors.len(), 1);
    assert!(result.errors[0].contains(&missing.display().to_string()));
    assert!(result.errors[0].contains(&os_error));
    assert!(result.errors[0].contains("Cannot inspect"));
}

#[test]
fn explicit_nested_roots_restore_their_depth_allowance_without_resetting_the_scan_budget() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path().join("r");
    let nested = root.join("a");
    let mut deepest = nested.clone();
    for depth in 0..MAX_DROP_DEPTH {
        deepest = deepest.join(format!("d{depth}"));
    }
    fs::create_dir_all(&deepest).expect("nested tree");
    let shallow_video = nested.join("seen.mp4");
    let deep_video = deepest.join("deep.mp4");
    fs::write(&shallow_video, b"shallow").expect("shallow video");
    fs::write(&deep_video, b"deep").expect("deep video");

    let nested_only = resolve_dropped_paths(&[nested.clone()]);
    assert_eq!(nested_only.videos.len(), 2);
    assert!(!nested_only.truncated);
    let overlapping = resolve_dropped_paths(&[root.clone(), nested.clone()]);
    assert_eq!(overlapping.videos, nested_only.videos);
    assert_eq!(
        overlapping
            .videos
            .iter()
            .filter(|path| path.ends_with("seen.mp4"))
            .count(),
        1
    );
    assert!(overlapping.truncated);
    assert!(
        overlapping
            .errors
            .iter()
            .any(|error| error.contains("folder depth limit"))
    );
    assert_eq!(
        overlapping,
        resolve_dropped_paths(&[nested.clone(), root.clone()])
    );

    let budget = 2 * MAX_DROP_DEPTH + 5;
    let limited = resolve_with_budget(&[root, nested], budget);
    assert!(limited.truncated);
    assert_eq!(
        limited.videos,
        [shallow_video.canonicalize().expect("canonical source")]
    );
    assert!(
        limited
            .errors
            .iter()
            .any(|error| error.contains(&format!("{budget}-entry scan budget")))
    );
}
