use crate::{
    files,
    queue::{self, QueueEntry},
    state::{Host, Result, Source, blocking, lock},
};
use nvsf_core::ExtractConfig;
use serde::Serialize;
use std::{
    collections::{HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
};

pub const MAX_DROP_DEPTH: usize = 4;
pub const MAX_DROP_FILES: usize = queue::MAX_ENTRIES;

const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "webm", "m4v", "ts", "mts", "m2ts", "mpeg", "wmv",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropResolution {
    pub videos: Vec<PathBuf>,
    pub skipped_unsupported: usize,
    pub truncated: bool,
    pub from_folder: bool,
}

#[derive(Clone, Serialize)]
pub struct DropOutcome {
    pub kind: String,
    pub source: Option<Source>,
    pub queued: Vec<QueueEntry>,
    pub added_count: usize,
    pub skipped_unsupported: usize,
    pub truncated: bool,
    pub errors: Vec<String>,
}

pub fn is_supported_video(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            VIDEO_EXTENSIONS
                .iter()
                .any(|allowed| ext.eq_ignore_ascii_case(allowed))
        })
}

pub fn resolve_dropped_paths(inputs: &[PathBuf]) -> DropResolution {
    let mut videos = Vec::new();
    let mut seen = HashSet::new();
    let mut skipped_unsupported = 0usize;
    let mut truncated = false;
    let mut from_folder = false;

    for raw in inputs {
        let Ok(meta) = fs::symlink_metadata(raw) else {
            skipped_unsupported += 1;
            continue;
        };
        if meta.file_type().is_symlink() {
            skipped_unsupported += 1;
            continue;
        }
        if meta.is_file() {
            if is_supported_video(raw) {
                push_canonical_video(raw, &mut seen, &mut videos, &mut truncated);
            } else {
                skipped_unsupported += 1;
            }
            continue;
        }
        if meta.is_dir() {
            from_folder = true;
            scan_directory(
                raw,
                &mut seen,
                &mut videos,
                &mut skipped_unsupported,
                &mut truncated,
            );
            continue;
        }
        skipped_unsupported += 1;
    }

    videos.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    DropResolution {
        videos,
        skipped_unsupported,
        truncated,
        from_folder,
    }
}

fn push_canonical_video(
    path: &Path,
    seen: &mut HashSet<PathBuf>,
    videos: &mut Vec<PathBuf>,
    truncated: &mut bool,
) {
    let Ok(canonical) = path.canonicalize() else {
        return;
    };
    if !canonical.is_file() {
        return;
    }
    if seen.contains(&canonical) {
        return;
    }
    if videos.len() >= MAX_DROP_FILES {
        *truncated = true;
        return;
    }
    seen.insert(canonical.clone());
    videos.push(canonical);
}

fn scan_directory(
    root: &Path,
    seen: &mut HashSet<PathBuf>,
    videos: &mut Vec<PathBuf>,
    skipped_unsupported: &mut usize,
    truncated: &mut bool,
) {
    let mut queue = VecDeque::new();
    queue.push_back((root.to_path_buf(), 0usize));

    while let Some((dir, depth)) = queue.pop_front() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut children: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .collect();
        children.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));

        for child in children {
            let Ok(meta) = fs::symlink_metadata(&child) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                *skipped_unsupported += 1;
                continue;
            }
            if meta.is_dir() {
                if depth < MAX_DROP_DEPTH {
                    queue.push_back((child, depth + 1));
                }
            } else if meta.is_file() {
                if is_supported_video(&child) {
                    push_canonical_video(&child, seen, videos, truncated);
                } else {
                    *skipped_unsupported += 1;
                }
            }
        }
    }
}

fn default_srt_destination(
    video: &Path,
    reserved: &mut HashSet<PathBuf>,
    overwrite: bool,
) -> PathBuf {
    let base = video.with_extension("srt");
    if !reserved.contains(&base) && (overwrite || !base.exists()) {
        reserved.insert(base.clone());
        return base;
    }
    let stem = video
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("subtitles");
    let parent = video.parent().unwrap_or_else(|| Path::new("."));
    for idx in 1..=999usize {
        let candidate = parent.join(format!("{stem}-{idx}.srt"));
        if !reserved.contains(&candidate) && (overwrite || !candidate.exists()) {
            reserved.insert(candidate.clone());
            return candidate;
        }
    }
    parent.join(format!("{stem}-nvsf.srt"))
}

#[tauri::command]
pub async fn accept_dropped_paths(
    host: tauri::State<'_, Host>,
    paths: Vec<String>,
    config: ExtractConfig,
) -> Result<DropOutcome> {
    let host = host.inner().clone();
    blocking(move || {
        let raw_paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        let resolved = resolve_dropped_paths(&raw_paths);
        if resolved.videos.is_empty() {
            return Err(
                "No supported video files were found in the dropped files or folders.".into(),
            );
        }

        if resolved.videos.len() == 1 && !resolved.from_folder {
            let source = files::grant_source_with(&host, &resolved.videos[0], None, |path| {
                files::probe_source(&host, path)
            })?;
            return Ok(DropOutcome {
                kind: "single".into(),
                source: Some(source),
                queued: Vec::new(),
                added_count: 1,
                skipped_unsupported: resolved.skipped_unsupported,
                truncated: resolved.truncated,
                errors: Vec::new(),
            });
        }

        let mut reserved: HashSet<PathBuf> = {
            let store = lock(&host.0.queue)?;
            store.check()?;
            store
                .file
                .records
                .iter()
                .map(|r| r.output.clone())
                .collect()
        };

        let mut added_count = 0usize;
        let mut errors = Vec::new();
        for video in &resolved.videos {
            let source = match files::grant_source_with(&host, video, None, |path| {
                files::probe_source(&host, path)
            }) {
                Ok(source) => source,
                Err(err) => {
                    errors.push(format!("{}: {err}", video.display()));
                    continue;
                }
            };
            let output_path = default_srt_destination(video, &mut reserved, config.overwrite);
            let destination = match files::grant_output(&host, &output_path) {
                Ok(dest) => dest,
                Err(err) => {
                    errors.push(format!("{}: {err}", video.display()));
                    continue;
                }
            };
            let _operation = lock(&host.0.operation)?;
            match queue::register_with(
                &host,
                &source.token,
                &destination.token,
                config.clone(),
                |_| Ok(source.media.clone()),
            ) {
                Ok(_) => added_count += 1,
                Err(err) => errors.push(format!("{}: {err}", video.display())),
            }
        }

        let queued = lock(&host.0.queue)?.entries()?;
        if added_count == 0 {
            let summary = errors
                .first()
                .cloned()
                .unwrap_or_else(|| "Dropped videos could not be added to the queue.".into());
            return Err(summary);
        }

        Ok(DropOutcome {
            kind: "queue".into(),
            source: None,
            queued,
            added_count,
            skipped_unsupported: resolved.skipped_unsupported,
            truncated: resolved.truncated,
            errors,
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn resolves_single_video_file() {
        let dir = tempdir().expect("tempdir");
        let video = dir.path().join("clip.MP4");
        fs::write(&video, b"stub").expect("write");

        let resolved = resolve_dropped_paths(&[video.clone()]);
        assert_eq!(resolved.videos.len(), 1);
        assert!(!resolved.from_folder);
        assert_eq!(resolved.skipped_unsupported, 0);
        assert!(!resolved.truncated);
    }

    #[test]
    fn resolves_folder_recursively_and_skips_non_video() {
        let dir = tempdir().expect("tempdir");
        let sub = dir.path().join("nested");
        fs::create_dir_all(&sub).expect("mkdir");
        let v1 = dir.path().join("a.mkv");
        let v2 = sub.join("b.webm");
        let txt = sub.join("notes.txt");
        fs::write(&v1, b"1").expect("write");
        fs::write(&v2, b"2").expect("write");
        fs::write(&txt, b"ignore").expect("write");

        let resolved = resolve_dropped_paths(&[dir.path().to_path_buf()]);
        assert!(resolved.from_folder);
        assert_eq!(resolved.videos.len(), 2);
        assert_eq!(resolved.skipped_unsupported, 1);
        assert!(!resolved.truncated);
    }

    #[test]
    fn enforces_max_drop_depth_and_deduplicates() {
        let dir = tempdir().expect("tempdir");
        let mut cursor = dir.path().to_path_buf();
        for i in 0..=MAX_DROP_DEPTH + 1 {
            cursor = cursor.join(format!("d{i}"));
            fs::create_dir_all(&cursor).expect("mkdir");
        }
        let too_deep = cursor.join("deep.mp4");
        let shallow = dir.path().join("d0").join("ok.mov");
        fs::write(&too_deep, b"1").expect("write");
        fs::write(&shallow, b"2").expect("write");

        let resolved = resolve_dropped_paths(&[dir.path().to_path_buf(), shallow.clone()]);
        assert_eq!(resolved.videos.len(), 1);
        assert!(resolved.videos[0].ends_with("ok.mov"));
    }

    #[test]
    fn default_srt_destination_avoids_collisions() {
        let dir = tempdir().expect("tempdir");
        let video = dir.path().join("episode.mp4");
        let mut reserved = HashSet::new();
        let first = default_srt_destination(&video, &mut reserved, false);
        let second = default_srt_destination(&video, &mut reserved, false);
        assert_eq!(first, dir.path().join("episode.srt"));
        assert_eq!(second, dir.path().join("episode-1.srt"));
    }
}
