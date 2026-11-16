//! Bounded execution evidence, independent of provider registration or device enumeration.
use crate::{Result, fail};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub const PROFILE_LIMIT: usize = 8 * 1024 * 1024;
const EVENT_LIMIT: usize = 100_000;

/// Hashes the same bounded trace bytes that the caller parsed as placement evidence.
pub fn hash_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Serialize)]
pub struct Node {
    pub provider: String,
    pub operation: String,
    pub duration_us: f64,
    pub transfer: bool,
}

#[derive(Debug, Serialize)]
pub struct Placement {
    pub requested_provider: String,
    pub requested_compute: usize,
    pub other_compute: usize,
    pub transfer_events: usize,
    pub duration_us: f64,
    pub providers: BTreeMap<String, usize>,
    pub nodes: Vec<Node>,
}

#[derive(Debug, Serialize)]
pub struct ProfilePair {
    pub detector: Placement,
    pub recognizer: Placement,
}

#[derive(Debug, Serialize)]
pub struct ProfileFiles {
    pub detector: PathBuf,
    pub recognizer: PathBuf,
    pub runtime_build: String,
}

fn provider(requested: &str) -> Result<&'static str> {
    match requested {
        "cpu" => Ok("CPUExecutionProvider"),
        "cuda" => Ok("CUDAExecutionProvider"),
        "directml" => Ok("DmlExecutionProvider"),
        _ => Err(fail("Unsupported profile execution provider")),
    }
}

/// Requires actual requested-provider compute; strict mode rejects all other compute.
/// Transfer counts reflect observed trace events, not proof of absent uninstrumented transfers.
pub fn parse(bytes: &[u8], requested: &str, strict: bool) -> Result<Placement> {
    let requested = provider(requested)?;
    if bytes.len() > PROFILE_LIMIT {
        return Err(fail("Execution profile exceeds 8MiB"));
    }
    let events: Vec<Value> = serde_json::from_slice(bytes)?;
    if events.len() > EVENT_LIMIT {
        return Err(fail("Execution profile exceeds event limit"));
    }
    let mut result = Placement {
        requested_provider: requested.into(),
        requested_compute: 0,
        other_compute: 0,
        transfer_events: 0,
        duration_us: 0.0,
        providers: BTreeMap::new(),
        nodes: Vec::new(),
    };
    for event in events {
        if event["cat"] != "Node" {
            continue;
        }
        // ORT's optional fence events are not completed kernel execution evidence.
        if event["ph"] != "X" {
            return Err(fail("Execution profile contains an incomplete node event"));
        }
        let actual = event["args"]["provider"]
            .as_str()
            .filter(|name| {
                matches!(
                    *name,
                    "CPUExecutionProvider" | "CUDAExecutionProvider" | "DmlExecutionProvider"
                )
            })
            .ok_or_else(|| fail("Execution profile node has no admitted provider"))?;
        let operation = event["args"]["op_name"]
            .as_str()
            .filter(|name| {
                !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control)
            })
            .ok_or_else(|| fail("Execution profile node has an invalid operation"))?;
        let duration = event["dur"]
            .as_f64()
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| fail("Execution profile node has an invalid duration"))?;
        let transfer = operation.starts_with("Memcpy");
        if transfer {
            result.transfer_events += 1;
        } else if actual == requested {
            result.requested_compute += 1;
        } else {
            result.other_compute += 1;
        }
        result.duration_us += duration;
        if !result.duration_us.is_finite() {
            return Err(fail("Execution profile duration overflow"));
        }
        *result.providers.entry(actual.into()).or_default() += 1;
        result.nodes.push(Node {
            provider: actual.into(),
            operation: operation.into(),
            duration_us: duration,
            transfer,
        });
    }
    if result.requested_compute == 0 || (strict && result.other_compute != 0) {
        return Err(fail(
            "Execution profile does not prove requested compute placement",
        ));
    }
    Ok(result)
}

/// Both model stages must independently establish placement; detector-only evidence fails.
pub fn parse_pair(
    detector: &[u8],
    recognizer: &[u8],
    requested: &str,
    strict: bool,
) -> Result<ProfilePair> {
    Ok(ProfilePair {
        detector: parse(detector, requested, strict)?,
        recognizer: parse(recognizer, requested, strict)?,
    })
}

pub(crate) fn reserve(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(fail("Execution profile directory must be absolute and new"));
    }
    std::fs::create_dir(path)?;
    Ok(path.canonicalize()?)
}

pub(crate) fn profile_path(root: &Path, recorded: &str) -> Result<PathBuf> {
    let path = Path::new(recorded);
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > PROFILE_LIMIT as u64 {
        return Err(fail("Execution profile must be a bounded regular file"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(fail("Execution profile must not be a reparse point"));
        }
    }
    let path = path.canonicalize()?;
    if path.parent() != Some(root) {
        return Err(fail("Execution profile escaped its reserved directory"));
    }
    Ok(path)
}

#[cfg(test)]
#[path = "provider_profile_tests.rs"]
mod tests;
