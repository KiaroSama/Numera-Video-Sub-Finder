use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs, io,
    path::{Path, PathBuf},
};

pub(super) const MAX_DROP_DEPTH: usize = 4;
pub(super) const MAX_DROP_FILES: usize = crate::queue::MAX_ENTRIES;
const MAX_SCAN_ENTRIES: usize = 4096;
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "webm", "m4v", "ts", "mts", "m2ts", "mpeg", "wmv",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DropResolution {
    pub videos: Vec<PathBuf>,
    pub skipped_unsupported: usize,
    pub truncated: bool,
    pub from_folder: bool,
    pub errors: Vec<String>,
}

enum ScanError {
    Limit,
    Io(io::Error),
}

fn bounded_children(
    entries: impl IntoIterator<Item = io::Result<PathBuf>>,
    remaining: &mut usize,
) -> Result<Vec<PathBuf>, ScanError> {
    let mut children = Vec::new();
    for entry in entries {
        if *remaining == 0 {
            return Err(ScanError::Limit);
        }
        *remaining -= 1;
        children.push(entry.map_err(ScanError::Io)?);
    }
    children.sort();
    Ok(children)
}

fn is_supported_video(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            VIDEO_EXTENSIONS
                .iter()
                .any(|allowed| ext.eq_ignore_ascii_case(allowed))
        })
}

pub(super) fn resolve_dropped_paths(inputs: &[PathBuf]) -> DropResolution {
    resolve_with_budget(inputs, MAX_SCAN_ENTRIES)
}

fn resolve_with_budget(inputs: &[PathBuf], budget: usize) -> DropResolution {
    let mut result = DropResolution {
        videos: Vec::new(),
        skipped_unsupported: 0,
        truncated: false,
        from_folder: false,
        errors: Vec::new(),
    };
    let limit_message = format!(
        "Reached the {budget}-entry scan budget. Drop a smaller folder or select video files directly."
    );
    if inputs.len() > budget {
        result.truncated = true;
        result.errors.push(limit_message);
        return result;
    }
    let mut roots = inputs.to_vec();
    roots.sort();
    roots.dedup();
    let mut remaining = budget - roots.len();
    let mut pending: VecDeque<_> = roots.into_iter().map(|path| (path, 0usize)).collect();
    let mut seen_files = HashSet::new();
    let mut seen_directories = HashMap::new();

    while let Some((path, depth)) = pending.pop_front() {
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) => {
                result.errors.push(format!(
                    "Cannot inspect {}: {error}. Drop accessible video files directly.",
                    path.display()
                ));
                continue;
            }
        };
        if meta.file_type().is_symlink() {
            result.skipped_unsupported += 1;
            continue;
        }
        if meta.is_file() {
            if !is_supported_video(&path) {
                result.skipped_unsupported += 1;
                continue;
            }
            if let Ok(canonical) = path.canonicalize()
                && canonical.is_file()
                && seen_files.insert(canonical.clone())
            {
                result.videos.push(canonical);
                if result.videos.len() == MAX_DROP_FILES {
                    if !pending.is_empty() {
                        result.truncated = true;
                        result.errors.push(format!(
                            "Reached the {MAX_DROP_FILES}-video drop limit. Drop the remaining videos separately."
                        ));
                    }
                    break;
                }
            }
            continue;
        }
        if !meta.is_dir() {
            result.skipped_unsupported += 1;
            continue;
        }
        result.from_folder = true;
        if depth > MAX_DROP_DEPTH {
            result.truncated = true;
            let message = format!(
                "Reached the {MAX_DROP_DEPTH}-level folder depth limit. Drop a deeper folder or select video files directly."
            );
            if !result.errors.contains(&message) {
                result.errors.push(message);
            }
            continue;
        }
        if let Ok(canonical) = path.canonicalize() {
            if seen_directories
                .get(&canonical)
                .is_some_and(|previous_depth| *previous_depth <= depth)
            {
                continue;
            }
            seen_directories.insert(canonical, depth);
        }
        let children = fs::read_dir(&path)
            .map_err(ScanError::Io)
            .and_then(|entries| {
                bounded_children(
                    entries.map(|entry| entry.map(|entry| entry.path())),
                    &mut remaining,
                )
            });
        match children {
            Ok(children) => {
                for child in children.into_iter().rev() {
                    pending.push_front((child, depth + 1));
                }
            }
            Err(ScanError::Limit) => {
                // Reject the partial directory rather than depending on filesystem enumeration order.
                result.truncated = true;
                result.errors.push(limit_message);
                break;
            }
            Err(ScanError::Io(error)) => result.errors.push(format!(
                "Cannot scan {}: {error}. Drop accessible video files directly.",
                path.display()
            )),
        }
    }
    result.videos.sort();
    result
}

#[cfg(test)]
#[path = "drop_scan_tests.rs"]
mod tests;
