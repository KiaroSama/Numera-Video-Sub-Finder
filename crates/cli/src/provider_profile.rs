use nvsf_app::{AppError, Result, providers::profiles};
use profiles::{Placement, ProfileFiles};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const EVIDENCE_FILE: &str = "provider-evidence.json";
const EVIDENCE_LIMIT: usize = 1024 * 1024;
const FALLBACK_LIMIT: usize = 64;
const NAME_LIMIT: usize = 256;
const RUNTIME_LIMIT: usize = 16 * 1024;

fn invalid(message: &str) -> AppError {
    AppError::InvalidConfig(message.into())
}

pub fn validate_options(directory: &Path, device: &str, language: &str) -> Result<()> {
    if !directory.is_absolute()
        || directory.file_name().is_none()
        || directory
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(invalid(
            "--provider-profile requires a new absolute directory without parent components",
        ));
    }
    if !matches!(device, "cpu" | "cuda" | "directml" | "coreml") {
        return Err(invalid(
            "--provider-profile requires explicit --device cpu, cuda, directml or coreml; auto is not placement evidence",
        ));
    }
    if nvsf_core::languages::paddle_model(language).is_none() {
        return Err(invalid(
            "--provider-profile requires a language with an ONNX recognizer for two-stage evidence",
        ));
    }
    Ok(())
}

pub fn validate_new_directory(directory: &Path) -> Result<()> {
    match std::fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err(invalid(
            "--provider-profile directory already exists; choose a new directory",
        )),
    }
}

#[derive(Serialize)]
struct ProfileIdentity {
    file: String,
    bytes: usize,
    sha256: String,
}

#[derive(Serialize)]
struct StageEvidence {
    requested_provider: String,
    requested_compute: usize,
    other_compute: usize,
    transfer_events: usize,
    duration_us: f64,
    providers: BTreeMap<String, usize>,
    compute_providers: BTreeMap<String, usize>,
    cpu_fallback_nodes: Vec<String>,
    cpu_fallback_nodes_omitted: usize,
    profile: ProfileIdentity,
}

#[derive(Serialize)]
struct Evidence {
    schema: u32,
    requested_device: String,
    strict: bool,
    runtime_build: String,
    detector: StageEvidence,
    recognizer: StageEvidence,
}

fn stage(path: &Path, bytes: &[u8], placement: Placement) -> Result<StageEvidence> {
    let file = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| {
            !name.is_empty() && name.len() <= NAME_LIMIT && !name.chars().any(char::is_control)
        })
        .ok_or_else(|| nvsf_app::fail("Profile filename is not bounded UTF-8"))?;
    let mut compute_providers = BTreeMap::new();
    for node in &placement.nodes {
        if !node.transfer {
            *compute_providers.entry(node.provider.clone()).or_default() += 1;
        }
    }
    let mut cpu_fallback_nodes = Vec::new();
    let mut cpu_fallback_count = 0;
    if placement.requested_provider != "CPUExecutionProvider" {
        // Placement validation has already rejected incomplete or unadmitted node events.
        let events: Vec<serde_json::Value> = serde_json::from_slice(bytes)?;
        for event in events {
            if event["cat"] != "Node"
                || event["args"]["provider"] != "CPUExecutionProvider"
                || event["args"]["op_name"]
                    .as_str()
                    .is_some_and(|operation| operation.starts_with("Memcpy"))
            {
                continue;
            }
            let name = event["name"]
                .as_str()
                .filter(|name| {
                    !name.is_empty()
                        && name.len() <= NAME_LIMIT
                        && !name.chars().any(char::is_control)
                })
                .ok_or_else(|| nvsf_app::fail("CPU fallback node has no bounded profile name"))?;
            cpu_fallback_count += 1;
            if cpu_fallback_nodes.len() < FALLBACK_LIMIT {
                cpu_fallback_nodes.push(name.to_owned());
            }
        }
    }
    Ok(StageEvidence {
        requested_provider: placement.requested_provider,
        requested_compute: placement.requested_compute,
        other_compute: placement.other_compute,
        transfer_events: placement.transfer_events,
        duration_us: placement.duration_us,
        providers: placement.providers,
        compute_providers,
        cpu_fallback_nodes_omitted: cpu_fallback_count - cpu_fallback_nodes.len(),
        cpu_fallback_nodes,
        profile: ProfileIdentity {
            file: file.into(),
            bytes: bytes.len(),
            sha256: profiles::hash_bytes(bytes),
        },
    })
}

fn build_record(
    requested_device: &str,
    strict: bool,
    files: &ProfileFiles,
    detector: &[u8],
    recognizer: &[u8],
) -> Result<Evidence> {
    if files.runtime_build.is_empty()
        || files.runtime_build.len() > RUNTIME_LIMIT
        || files.runtime_build.contains('\0')
        || files.detector == files.recognizer
        || files.detector.parent() != files.recognizer.parent()
    {
        return Err(nvsf_app::fail(
            "Invalid profile file identity or runtime build metadata",
        ));
    }
    let pair = profiles::parse_pair(detector, recognizer, requested_device, strict).map_err(|error| {
        AppError::Provider(format!(
            "Provider profiles do not prove both stages on requested device {requested_device}: {error}"
        ))
    })?;
    Ok(Evidence {
        schema: 1,
        requested_device: requested_device.into(),
        strict,
        runtime_build: files.runtime_build.clone(),
        detector: stage(&files.detector, detector, pair.detector)?,
        recognizer: stage(&files.recognizer, recognizer, pair.recognizer)?,
    })
}

fn read_profile(root: &Path, path: &Path) -> Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > profiles::PROFILE_LIMIT as u64 {
        return Err(nvsf_app::fail(
            "Profile evidence requires a bounded regular file",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(nvsf_app::fail(
                "Profile evidence must not be a reparse point",
            ));
        }
    }
    if path.canonicalize()?.parent() != Some(root) {
        return Err(nvsf_app::fail(
            "Profile evidence escaped its reserved directory",
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(profiles::PROFILE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > profiles::PROFILE_LIMIT {
        return Err(nvsf_app::fail("Profile evidence exceeds 8MiB"));
    }
    Ok(bytes)
}

pub fn write_evidence(
    directory: &Path,
    requested_device: &str,
    strict: bool,
    files: &ProfileFiles,
) -> Result<PathBuf> {
    let root = directory.canonicalize()?;
    let detector = read_profile(&root, &files.detector)?;
    let recognizer = read_profile(&root, &files.recognizer)?;
    let record = build_record(requested_device, strict, files, &detector, &recognizer)?;
    let bytes = serde_json::to_vec_pretty(&record)?;
    if bytes.len() > EVIDENCE_LIMIT {
        return Err(nvsf_app::fail("Provider evidence exceeds 1MiB"));
    }
    let path = root.join(EVIDENCE_FILE);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    output.write_all(&bytes)?;
    output.sync_all()?;
    Ok(path)
}

#[cfg(test)]
#[path = "provider_profile_tests.rs"]
mod tests;
