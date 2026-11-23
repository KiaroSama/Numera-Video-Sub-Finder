use crate::{
    files,
    queue::{self, QueueEntry},
    state::{Host, Result, Source, blocking, lock},
};
use nvsf_core::ExtractConfig;
use serde::Serialize;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
use tauri::Emitter;

#[path = "drop_scan.rs"]
mod scan;
use scan::resolve_dropped_paths;
const MAX_PENDING_PATHS: usize = 1024;

pub struct PendingDrop {
    token: String,
    paths: Vec<PathBuf>,
    truncated: bool,
}

pub(crate) fn remember_native_drop(host: &Host, paths: Vec<PathBuf>) -> Result<String> {
    let token = host.token("drop");
    let truncated = paths.len() > MAX_PENDING_PATHS;
    *lock(&host.0.pending_drop)? = Some(PendingDrop {
        token: token.clone(),
        paths: paths.into_iter().take(MAX_PENDING_PATHS).collect(),
        truncated,
    });
    Ok(token)
}

pub(crate) fn publish_native_drop(
    app: &tauri::AppHandle,
    host: &Host,
    paths: Vec<PathBuf>,
) -> Result<String> {
    let token = remember_native_drop(host, paths)?;
    app.emit_to("main", "nvsf://files-dropped", &token)
        .map_err(|e| e.to_string())?;
    Ok(token)
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

pub(crate) fn accept_dropped_paths_with<F>(
    host: &Host,
    paths: Vec<PathBuf>,
    config: ExtractConfig,
    mut probe: F,
) -> Result<DropOutcome>
where
    F: FnMut(&Path) -> Result<nvsf_core::MediaInfo>,
{
    let resolved = resolve_dropped_paths(&paths);
    if resolved.videos.is_empty() {
        return Err(resolved.errors.into_iter().next().unwrap_or_else(|| {
            "No supported video files were found in the dropped files or folders.".into()
        }));
    }

    if resolved.videos.len() == 1 && !resolved.from_folder {
        let source = files::grant_source_with(host, &resolved.videos[0], None, |path| probe(path))?;
        return Ok(DropOutcome {
            kind: "single".into(),
            source: Some(source),
            queued: Vec::new(),
            added_count: 1,
            skipped_unsupported: resolved.skipped_unsupported,
            truncated: resolved.truncated,
            errors: resolved.errors,
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
    let mut errors = resolved.errors;
    for video in &resolved.videos {
        let source = match files::grant_source_with(host, video, None, |path| probe(path)) {
            Ok(source) => source,
            Err(err) => {
                errors.push(format!("{}: {err}", video.display()));
                continue;
            }
        };
        let output_path = default_srt_destination(video, &mut reserved, config.overwrite);
        let destination = match files::grant_output(host, &output_path) {
            Ok(dest) => dest,
            Err(err) => {
                errors.push(format!("{}: {err}", video.display()));
                continue;
            }
        };
        let _operation = lock(&host.0.operation)?;
        match queue::register_with(
            host,
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
}

pub(crate) fn accept_native_drop_with(
    host: &Host,
    token: &str,
    config: ExtractConfig,
    probe: impl FnMut(&Path) -> Result<nvsf_core::MediaInfo>,
) -> Result<DropOutcome> {
    let pending = {
        let mut pending = lock(&host.0.pending_drop)?;
        if pending.as_ref().is_none_or(|drop| drop.token != token) {
            return Err("Drop request expired; drop the files onto the window again".into());
        }
        pending.take().ok_or_else(|| {
            "Drop request expired; drop the files onto the window again".to_string()
        })?
    };
    let mut outcome = accept_dropped_paths_with(host, pending.paths, config, probe)?;
    outcome.truncated |= pending.truncated;
    Ok(outcome)
}

#[tauri::command]
pub async fn accept_dropped_paths(
    host: tauri::State<'_, Host>,
    token: String,
    config: ExtractConfig,
) -> Result<DropOutcome> {
    let host = host.inner().clone();
    blocking(move || {
        accept_native_drop_with(&host, &token, config, |path| {
            files::probe_source(&host, path)
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use scan::{MAX_DROP_DEPTH, MAX_DROP_FILES};
    use std::fs;
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
        fs::write(&first, b"existing").expect("existing srt");
        let mut fresh = HashSet::new();
        let preserved = default_srt_destination(&video, &mut fresh, false);
        assert_eq!(preserved, dir.path().join("episode-1.srt"));
        let mut overwrite = HashSet::new();
        let replaced = default_srt_destination(&video, &mut overwrite, true);
        assert_eq!(replaced, first);
    }

    fn test_host(root: &Path) -> Host {
        let paths = nvsf_app::Paths::new(Some(root)).expect("paths");
        let log = nvsf_app::logging::Log::new(&paths, "nvsf-desktop");
        Host::new(paths, log)
    }

    fn sample_media() -> nvsf_core::MediaInfo {
        nvsf_core::MediaInfo {
            width: 1280,
            height: 720,
            duration_ms: Some(4000),
            codec: "h264".into(),
            time_base_num: 1,
            time_base_den: 1000,
            subtitle_streams: 0,
            rotation: 0,
        }
    }

    #[test]
    fn native_drop_tokens_reject_forgery_and_replay_without_losing_pending_paths() {
        let dir = tempdir().expect("tempdir");
        let host = test_host(dir.path());
        let video = dir.path().join("native drop 日本.mp4");
        fs::write(&video, b"native-drop-video").expect("write video");
        let reject = |token: &str| {
            accept_native_drop_with(&host, token, ExtractConfig::default(), |_| {
                panic!("ungranted drops must not probe media")
            })
            .err()
            .expect("ungranted drop")
        };
        assert!(reject("invented-token").contains("Drop request expired"));
        let token = remember_native_drop(&host, vec![video.clone()]).expect("native grant");
        assert!(reject(&video.to_string_lossy()).contains("Drop request expired"));
        assert!(reject("wrong-token").contains("Drop request expired"));
        assert!(lock(&host.0.sources).unwrap().is_empty());
        let outcome = accept_native_drop_with(&host, &token, ExtractConfig::default(), |path| {
            assert_eq!(path, video.canonicalize().unwrap());
            Ok(sample_media())
        })
        .expect("native drop admitted");
        assert_eq!(outcome.kind, "single");
        assert_eq!(outcome.source.unwrap().name, "native drop 日本.mp4");
        assert_eq!(lock(&host.0.sources).unwrap().len(), 1);
        assert!(reject(&token).contains("Drop request expired"));
        assert_eq!(lock(&host.0.sources).unwrap().len(), 1);
        assert_eq!(fs::read(&video).unwrap(), b"native-drop-video");
    }

    #[test]
    fn a_new_native_drop_expires_the_previous_batch_and_caps_pending_paths() {
        let dir = tempdir().expect("tempdir");
        let host = test_host(dir.path());
        let first = dir.path().join("first.mp4");
        let second = dir.path().join("second.mp4");
        fs::write(&first, b"first-video").unwrap();
        fs::write(&second, b"second-video").unwrap();
        let old = remember_native_drop(&host, vec![first.clone()]).unwrap();
        let current = remember_native_drop(&host, vec![second.clone()]).unwrap();
        assert_ne!(old, current);
        assert!(
            accept_native_drop_with(&host, &old, ExtractConfig::default(), |_| {
                panic!("expired drop must not probe media")
            })
            .is_err()
        );
        let admitted = accept_native_drop_with(&host, &current, ExtractConfig::default(), |_| {
            Ok(sample_media())
        })
        .unwrap();
        assert_eq!(admitted.source.unwrap().name, "second.mp4");
        let mut batch = vec![first; MAX_PENDING_PATHS];
        batch.push(second);
        let token = remember_native_drop(&host, batch).unwrap();
        let capped = accept_native_drop_with(&host, &token, ExtractConfig::default(), |_| {
            Ok(sample_media())
        })
        .unwrap();
        assert_eq!(capped.kind, "single");
        assert_eq!(capped.source.unwrap().name, "first.mp4");
        assert!(capped.truncated);
        assert!(lock(&host.0.queue).unwrap().entries().unwrap().is_empty());
    }

    #[test]
    fn accept_dropped_single_video_opens_extract_source() {
        let dir = tempdir().expect("tempdir");
        let host = test_host(dir.path());
        let video = dir.path().join("clip.mp4");
        fs::write(&video, b"fake-mp4-bytes").expect("write video");
        let outcome =
            accept_dropped_paths_with(&host, vec![video], ExtractConfig::default(), |_| {
                Ok(sample_media())
            })
            .expect("single drop");
        assert_eq!(outcome.kind, "single");
        assert!(outcome.source.is_some());
        assert_eq!(outcome.added_count, 1);
        assert!(outcome.queued.is_empty());
        assert!(outcome.errors.is_empty());
    }

    #[test]
    fn accept_dropped_folder_queues_videos_and_skips_unsupported() {
        let dir = tempdir().expect("tempdir");
        let host = test_host(dir.path());
        let folder = dir.path().join("batch");
        fs::create_dir_all(&folder).expect("mkdir");
        let first = folder.join("a.mp4");
        let second = folder.join("b.mkv");
        let ignored = folder.join("notes.txt");
        fs::write(&first, b"video-a").expect("write a");
        fs::write(&second, b"video-b").expect("write b");
        fs::write(&ignored, b"text").expect("write txt");
        let outcome =
            accept_dropped_paths_with(&host, vec![folder], ExtractConfig::default(), |_| {
                Ok(sample_media())
            })
            .expect("folder drop");
        assert_eq!(outcome.kind, "queue");
        assert!(outcome.source.is_none());
        assert_eq!(outcome.added_count, 2);
        assert_eq!(outcome.skipped_unsupported, 1);
        assert_eq!(outcome.queued.len(), 2);
        assert!(outcome.errors.is_empty());
    }

    #[test]
    fn accept_dropped_paths_rejects_empty_and_reports_probe_errors() {
        let dir = tempdir().expect("tempdir");
        let host = test_host(dir.path());
        let note = dir.path().join("readme.txt");
        fs::write(&note, b"text").expect("write txt");
        let empty_err =
            accept_dropped_paths_with(&host, vec![note], ExtractConfig::default(), |_| {
                Ok(sample_media())
            })
            .err()
            .expect("no videos");
        assert!(empty_err.contains("No supported video files"));

        let good = dir.path().join("good.mp4");
        let bad = dir.path().join("bad.mp4");
        fs::write(&good, b"good-video").expect("write good");
        fs::write(&bad, b"bad-video").expect("write bad");
        let partial = accept_dropped_paths_with(
            &host,
            vec![good.clone(), bad.clone()],
            ExtractConfig::default(),
            |path| {
                if path.ends_with("bad.mp4") {
                    Err("corrupt stream".into())
                } else {
                    Ok(sample_media())
                }
            },
        )
        .expect("partial queue");
        assert_eq!(partial.kind, "queue");
        assert_eq!(partial.added_count, 1);
        assert_eq!(partial.errors.len(), 1);
        assert!(partial.errors[0].contains("corrupt stream"));

        let all_fail =
            accept_dropped_paths_with(&host, vec![good, bad], ExtractConfig::default(), |_| {
                Err("probe unavailable".into())
            })
            .err()
            .expect("all failed");
        assert!(all_fail.contains("probe unavailable"));
    }

    #[test]
    fn resolve_dropped_paths_deduplicates_and_truncates_at_cap() {
        let dir = tempdir().expect("tempdir");
        let folder = dir.path().join("many");
        fs::create_dir_all(&folder).expect("mkdir");
        for idx in 0..=(MAX_DROP_FILES + 2) {
            fs::write(folder.join(format!("clip-{idx:03}.mp4")), b"x").expect("write clip");
        }
        let resolved = resolve_dropped_paths(&[folder.clone(), folder]);
        assert!(resolved.from_folder);
        assert!(resolved.truncated);
        assert_eq!(resolved.videos.len(), MAX_DROP_FILES);
    }
}
