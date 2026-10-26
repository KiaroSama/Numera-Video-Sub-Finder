use crate::{AppError, Paths, Result, fail, storage, worker::process};
use image::{DynamicImage, ImageFormat};
use nvsf_core::{MediaInfo, Recognition, Rect};
use serde::{Deserialize, Serialize};
use std::{
    io::{Cursor, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const JSON_LIMIT: u64 = 1024 * 1024;
const PREVIEW_LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "operation", deny_unknown_fields)]
pub enum Operation {
    Probe,
    Preview {
        at_ms: u64,
    },
    Suggest {
        language: String,
    },
    Visual {
        at_ms: u64,
        region: Rect,
    },
    Recognize {
        at_ms: u64,
        region: Rect,
        language: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema: u32,
    source_sha: Option<String>,
    data_root: PathBuf,
    runtimes: PathBuf,
    input: PathBuf,
    operation: Operation,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result", content = "data", deny_unknown_fields)]
pub enum Response {
    Probe(MediaInfo),
    Preview { width: u32, height: u32, bytes: u64 },
    Suggest(Vec<Rect>),
    Recognize(Recognition),
    Visual(crate::assistant::Proposal),
    Failed(String),
}

impl Operation {
    fn validate(&self) -> Result<()> {
        let (time, language) = match self {
            Self::Probe => (None, None),
            Self::Preview { at_ms } => (Some(*at_ms), None),
            Self::Suggest { language } => (None, Some(language)),
            Self::Visual { at_ms, region } => {
                region.validate()?;
                (Some(*at_ms), None)
            }
            Self::Recognize {
                at_ms,
                region,
                language,
            } => {
                region.validate()?;
                (Some(*at_ms), Some(language))
            }
        };
        if time.is_some_and(|at| at > 359_999_999_999)
            || language.is_some_and(|code| nvsf_core::languages::find(code).is_none())
        {
            return Err(fail("Invalid native request language or presentation time"));
        }
        Ok(())
    }

    fn accepts(&self, result: &Response) -> Result<()> {
        match (self, result) {
            (_, Response::Failed(message)) => Err(fail(message)),
            (Self::Probe, Response::Probe(info))
                if info.width > 0
                    && info.height > 0
                    && u64::from(info.width) * u64::from(info.height) <= 33_177_600
                    && info.time_base_num > 0
                    && info.time_base_den > 0 =>
            {
                Ok(())
            }
            (
                Self::Preview { .. },
                Response::Preview {
                    width,
                    height,
                    bytes,
                },
            ) if *width > 0
                && *height > 0
                && u64::from(*width) * u64::from(*height) <= 33_177_600
                && *bytes > 0
                && *bytes <= PREVIEW_LIMIT =>
            {
                Ok(())
            }
            (Self::Suggest { .. }, Response::Suggest(regions))
                if !regions.is_empty() && regions.len() <= 32 =>
            {
                for region in regions {
                    region.validate()?;
                }
                Ok(())
            }
            (Self::Recognize { .. }, Response::Recognize(text))
                if !text.text.trim().is_empty()
                    && text.text.len() <= 65_536
                    && text.crop.is_none()
                    && text
                        .confidence
                        .is_none_or(|c| c.is_finite() && (0.0..=1.0).contains(&c)) =>
            {
                Ok(())
            }
            (Self::Visual { .. }, Response::Visual(proposal))
                if proposal.requires_review
                    && proposal.provider == "local-visual-cpu"
                    && proposal.warning.len() <= 4096
                    && proposal.unreadable == proposal.text.is_none() =>
            {
                let checked =
                    crate::assistant::validate_proposal(proposal.text.as_deref().unwrap_or(""))?;
                if checked.text != proposal.text || checked.unreadable != proposal.unreadable {
                    return Err(fail("Invalid visual worker proposal"));
                }
                Ok(())
            }
            _ => Err(fail(
                "Native worker returned mismatched or invalid response",
            )),
        }
    }
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(fail("Native worker payload exceeds its byte bound"));
    }
    Ok(bytes)
}

/// Only the host supplies paths and operation; the frontend never controls a child command.
pub fn supervise(
    paths: &Paths,
    input: &Path,
    operation: Operation,
    cancel: &AtomicBool,
) -> Result<(Response, Vec<u8>)> {
    operation.validate()?;
    if cancel.load(Ordering::Acquire) {
        return Err(AppError::Cancelled);
    }
    let input = input.canonicalize()?;
    if !input.is_file() {
        return Err(fail("Native request needs a regular local source"));
    }
    let scratch = tempfile::tempdir_in(&paths.cache)?;
    let request = Request {
        schema: 1,
        source_sha: nvsf_core::BUILD_SOURCE_SHA.map(str::to_owned),
        data_root: paths.root.clone(),
        runtimes: paths.runtimes.canonicalize()?,
        input,
        operation,
    };
    let bytes = serde_json::to_vec(&request)?;
    if bytes.len() as u64 > JSON_LIMIT {
        return Err(fail("Native request exceeds1MiB"));
    }
    let file = scratch.path().join("request.json");
    storage::write_atomic(&file, &bytes, false)?;
    let log = crate::logging::Log::new(paths, "nvsf-native-supervisor");
    let mut command = Command::new(process::executable()?);
    command
        .arg("native-request")
        .arg("--request")
        .arg(&file)
        .env("NVSF_RUNTIME_ROOT", &paths.runtimes)
        .env("TEMP", scratch.path())
        .env("TMP", scratch.path())
        .env("TMPDIR", scratch.path())
        .env("OMP_NUM_THREADS", "2")
        .stdout(Stdio::null());
    let mut child = process::Process::spawn(&mut command)?;
    log.event("INFO", "NATIVE", &format!("Spawned pid={}", child.id()));
    let wall_seconds = if matches!(request.operation, Operation::Visual { .. }) {
        120
    } else {
        60
    };
    let deadline = Instant::now() + Duration::from_secs(wall_seconds);
    let observed = (|| -> Result<()> {
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(AppError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(fail(format!(
                    "Native request exceeded{wall_seconds}s wall deadline"
                )));
            }
            if child.exited()? {
                return Ok(());
            }
            std::thread::park_timeout(Duration::from_millis(10));
        }
    })();
    let status = child.finish();
    log.event(
        if observed.is_ok() && status.as_ref().is_ok_and(|s| s.success()) {
            "INFO"
        } else {
            "ERROR"
        },
        "NATIVE",
        &format!("Stopped pid={} monitor_ok={}", child.id(), observed.is_ok()),
    );
    let status = status?;
    observed?;
    if !status.success() {
        return Err(fail(format!("Native worker exited with {status}")));
    }
    if cancel.load(Ordering::Acquire) {
        return Err(AppError::Cancelled);
    }
    let result: Response = serde_json::from_slice(&read_bounded(
        &scratch.path().join("result.json"),
        JSON_LIMIT,
    )?)?;
    request.operation.accepts(&result)?;
    let preview = if let Response::Preview { bytes, .. } = &result {
        let data = read_bounded(&scratch.path().join("preview.png"), PREVIEW_LIMIT)?;
        if data.len() as u64 != *bytes || !data.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(fail("Native preview length or PNG signature mismatch"));
        }
        data
    } else {
        Vec::new()
    };
    Ok((result, preview))
}

/// Hidden CLI entry: must arm the owner lease before loading any native library.
pub fn run(file: &Path) -> Result<()> {
    crate::worker::guard_parent_lifetime()?;
    let request: Request = serde_json::from_slice(&read_bounded(file, JSON_LIMIT)?)?;
    if request.schema != 1
        || request.source_sha.as_deref() != nvsf_core::BUILD_SOURCE_SHA
        || !request.data_root.is_absolute()
        || !request.runtimes.is_absolute()
        || !request.input.is_absolute()
        || !request.input.is_file()
    {
        return Err(fail("Native request identity or paths are invalid"));
    }
    request.operation.validate()?;
    let mut paths = Paths::new(Some(&request.data_root))?;
    paths.runtimes = request.runtimes.clone();
    let log = crate::logging::Log::new(&paths, "nvsf-native-request");
    log.event("INFO", "NATIVE", "Started bounded native request");
    let directory = file
        .parent()
        .ok_or_else(|| fail("Native request directory missing"))?;
    let result = execute(&paths, &request, directory);
    let response = match result {
        Ok(response) => {
            request.operation.accepts(&response)?;
            response
        }
        Err(error) => Response::Failed(error.to_string().chars().take(4096).collect()),
    };
    storage::write_atomic(
        &directory.join("result.json"),
        &serde_json::to_vec(&response)?,
        false,
    )?;
    log.event(
        if matches!(response, Response::Failed(_)) {
            "ERROR"
        } else {
            "INFO"
        },
        "NATIVE",
        "Native request completed",
    );
    Ok(())
}

fn execute(paths: &Paths, request: &Request, directory: &Path) -> Result<Response> {
    Ok(match &request.operation {
        Operation::Probe => {
            Response::Probe(crate::media::MediaRuntime::load(paths)?.probe(&request.input)?)
        }
        Operation::Preview { at_ms } => {
            let rgb = crate::media::MediaRuntime::load(paths)?.preview(&request.input, *at_ms)?;
            let (width, height) = rgb.dimensions();
            let mut buffer = Cursor::new(Vec::new());
            DynamicImage::ImageRgb8(rgb)
                .write_to(&mut buffer, ImageFormat::Png)
                .map_err(|e| fail(e.to_string()))?;
            let bytes = buffer.into_inner();
            if bytes.len() as u64 > PREVIEW_LIMIT {
                return Err(fail("Preview exceeds8MiB IPC bound"));
            }
            storage::write_atomic(&directory.join("preview.png"), &bytes, false)?;
            Response::Preview {
                width,
                height,
                bytes: bytes.len() as u64,
            }
        }
        Operation::Suggest { language } => {
            Response::Suggest(crate::regions::suggest(paths, &request.input, language)?)
        }
        Operation::Visual { at_ms, region } => Response::Visual(crate::assistant::run_proposal(
            paths,
            &request.input,
            *at_ms,
            *region,
            directory,
        )?),
        Operation::Recognize {
            at_ms,
            region,
            language,
        } => Response::Recognize(crate::regions::recognize_cue(
            paths,
            &request.input,
            *at_ms,
            *region,
            language,
        )?),
    })
}

#[cfg(test)]
#[path = "native_request_tests.rs"]
mod tests;
