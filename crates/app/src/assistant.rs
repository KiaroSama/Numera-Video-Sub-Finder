use crate::{Paths, Result, fail, media, storage::hash_file};
use nvsf_core::Rect;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[derive(Deserialize)]
struct Artifact {
    file: String,
    url: String,
    bytes: u64,
    sha256: String,
}
#[derive(Deserialize)]
struct Pack {
    schema: u32,
    artifacts: Vec<Artifact>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Proposal {
    pub text: Option<String>,
    pub unreadable: bool,
    pub requires_review: bool,
    pub provider: String,
    pub warning: String,
}
fn artifacts() -> Result<Vec<Artifact>> {
    let pack: Pack = serde_json::from_str(include_str!("../../../ci/assistant-models.json"))?;
    if pack.schema != 1 || pack.artifacts.len() != 2 {
        return Err(fail("Invalid visual model pack"));
    }
    Ok(pack.artifacts)
}
pub fn install(paths: &Paths, cancel: &AtomicBool) -> Result<()> {
    let root = paths.models.join("assistant");
    std::fs::create_dir_all(&root)?;
    for a in artifacts()? {
        crate::model_download::install(&a.url, a.bytes, &a.sha256, &root.join(a.file), cancel)?;
    }
    Ok(())
}
fn verify(paths: &Paths) -> Result<Vec<PathBuf>> {
    let root = paths.models.join("assistant");
    let mut result = Vec::new();
    for a in artifacts()? {
        let file = root.join(a.file);
        if !file.is_file() || file.metadata()?.len() != a.bytes || hash_file(&file)? != a.sha256 {
            return Err(fail(
                "Visual retry pack missing or corrupt; explicitly install its model and projector",
            ));
        }
        let file = file.canonicalize()?;
        if !file.starts_with(root.canonicalize()?) {
            return Err(fail("Visual model path escapes its root"));
        }
        result.push(file);
    }
    Ok(result)
}

pub fn propose(
    paths: &Paths,
    input: &Path,
    at_ms: u64,
    region: Rect,
    enabled: bool,
    cancel: &AtomicBool,
) -> Result<Proposal> {
    if !enabled {
        return Err(fail("Local visual retry requires explicit opt-in"));
    }
    if cancel.load(Ordering::Acquire) {
        return Err(crate::AppError::Cancelled);
    }
    let (response, _) = crate::native_request::supervise(
        paths,
        input,
        crate::native_request::Operation::Visual { at_ms, region },
        cancel,
    )?;
    match response {
        crate::native_request::Response::Visual(proposal) => Ok(proposal),
        _ => Err(fail("Unexpected visual worker response")),
    }
}

pub(crate) fn run_proposal(
    paths: &Paths,
    input: &Path,
    at_ms: u64,
    region: Rect,
    directory: &Path,
) -> Result<Proposal> {
    let frame = media::MediaRuntime::load(paths)?.preview(input, at_ms)?;
    let (x, y, w, h) = region.pixels(frame.width(), frame.height())?;
    if u64::from(w) * u64::from(h) > 1_003_520 {
        return Err(fail(
            "Visual retry crop exceeds the model's pixel budget; choose a tighter region",
        ));
    }
    let crop = image::imageops::crop_imm(&frame, x, y, w, h).to_image();
    if constant_crop(&crop) {
        return Ok(unreadable("Blank/constant crop; no model invocation"));
    }
    let files = verify(paths)?;
    let runtime = paths.runtime(if cfg!(windows) {
        "llama-mtmd-cli.exe"
    } else {
        "llama-mtmd-cli"
    })?;
    let image = directory.join("crop.png");
    crop.save(&image).map_err(|e| fail(e.to_string()))?;
    let mut response = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(directory.join("model-output.txt"))?;
    let mut diagnostic = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(directory.join("model-diagnostic.txt"))?;
    let mut command = Command::new(runtime);
    command
        .args(["-m"])
        .arg(&files[0])
        .arg("--mmproj")
        .arg(&files[1])
        .arg("--image")
        .arg(&image)
        .args([
            "-p",
            "OCR:",
            "-n",
            "256",
            "-c",
            "4096",
            "-b",
            "512",
            "-ub",
            "512",
            "-t",
            "2",
            "--temp",
            "0",
            "--seed",
            "1",
            "-ngl",
            "0",
            "--no-mmproj-offload",
            "--jinja",
        ])
        .stdin(Stdio::null())
        .stdout(response.try_clone()?)
        .stderr(diagnostic.try_clone()?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    // Inherit the native-request group/job so parent cancellation owns the entire tree.
    let mut child = command.spawn()?;
    let start = Instant::now();
    let mut length = 0;
    let mut progress = Instant::now();
    loop {
        let now = response.metadata()?.len();
        if now > 65536 || diagnostic.metadata()?.len() > 1024 * 1024 {
            return Err(fail(
                "Visual worker response or diagnostic exceeds its byte bound",
            ));
        }
        if now != length {
            length = now;
            progress = Instant::now();
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                diagnostic.seek(SeekFrom::Start(0))?;
                let mut bytes = Vec::new();
                diagnostic.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
                let reason = diagnostic_reason(&String::from_utf8_lossy(&bytes));
                return Err(fail(format!(
                    "Local visual worker failed ({status}): {reason}; original OCR retained"
                )));
            }
            break;
        }
        if start.elapsed() > Duration::from_secs(120)
            || progress.elapsed() > Duration::from_secs(60)
        {
            diagnostic.seek(SeekFrom::Start(0))?;
            let mut bytes = Vec::new();
            diagnostic.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
            let phase = diagnostic_phase(&String::from_utf8_lossy(&bytes)).unwrap_or("unknown");
            return Err(fail(format!(
                "Local visual worker wall/idle deadline; phase={phase}; original OCR retained"
            )));
        }
        std::thread::park_timeout(Duration::from_millis(20));
    }
    response.seek(SeekFrom::Start(0))?;
    let mut text = String::new();
    response.take(65537).read_to_string(&mut text)?;
    validate_proposal(&text)
}

pub(crate) fn diagnostic_phase(text: &str) -> Option<&'static str> {
    [
        ("warmup with image size =", "vision-context-preparation"),
        ("encoding mtmd batch, n_chunks =", "vision-encoding-started"),
        ("mtmd batch encoding done in", "vision-encoding-completed"),
    ]
    .into_iter()
    .filter_map(|(marker, phase)| text.rfind(marker).map(|position| (position, phase)))
    .max_by_key(|(position, _)| *position)
    .map(|(_, phase)| phase)
}

fn diagnostic_reason(text: &str) -> &'static str {
    let text = text.to_ascii_lowercase();
    if text
        .lines()
        .any(|line| line.contains("tensor '") && line.contains("not found"))
    {
        return "required model tensor missing";
    }
    for (needle, reason) in [
        ("invalid argument", "runtime argument rejected"),
        (
            "unknown model architecture",
            "unsupported model architecture",
        ),
        (
            "unsupported model architecture",
            "unsupported model architecture",
        ),
        ("unknown pre-tokenizer", "unsupported model tokenizer"),
        ("unable to find tensor", "required model tensor missing"),
        ("failed to find tensor", "required model tensor missing"),
        (
            "failed to load vision model",
            "vision initialization failed",
        ),
        (
            "model does not have chat template",
            "model chat template missing",
        ),
        (
            "unsupported chat template",
            "model chat template unsupported",
        ),
        (
            "this custom template is not supported",
            "model chat template unsupported",
        ),
        ("failed to encode", "vision encoding failed"),
        ("unable to eval", "multimodal evaluation failed"),
        ("failed to decode", "token decoding failed"),
        ("failed to allocate", "native memory allocation failed"),
        (
            "failed to initialize the context",
            "model context initialization failed",
        ),
        (
            "failed to load clip model",
            "vision model initialization failed",
        ),
        ("failed to load model", "model initialization failed"),
    ] {
        if text.contains(needle) {
            return reason;
        }
    }
    "unclassified native runtime failure"
}

fn constant_crop(crop: &image::RgbImage) -> bool {
    let mut min = [255u8; 3];
    let mut max = [0u8; 3];
    for pixel in crop.pixels() {
        for channel in 0..3 {
            min[channel] = min[channel].min(pixel[channel]);
            max[channel] = max[channel].max(pixel[channel]);
        }
    }
    (0..3).all(|channel| max[channel].saturating_sub(min[channel]) < 4)
}

fn unreadable(warning: &str) -> Proposal {
    Proposal {
        text: None,
        unreadable: true,
        requires_review: true,
        provider: "local-visual-cpu".into(),
        warning: warning.into(),
    }
}
pub fn validate_proposal(text: &str) -> Result<Proposal> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(unreadable("Model returned no readable text"));
    }
    if text.len() > 8192 || text.contains('\0') || text.contains("\n\n") || text.lines().count() > 8
    {
        return Err(fail("Malformed visual proposal; original OCR retained"));
    }
    let lower = text.to_lowercase();
    if [
        "as an ai",
        "i cannot",
        "unreadable",
        "ignore previous",
        "system prompt",
        "assistant:",
        "<|",
        "```",
        " --> ",
    ]
    .iter()
    .any(|p| lower.contains(p))
    {
        return Ok(unreadable(
            "Refusal, instruction-like or timestamp-bearing output rejected",
        ));
    }
    Ok(Proposal{text:Some(text.into()),unreadable:false,requires_review:true,provider:"local-visual-cpu".into(),warning:"Unqualified crop transcription proposal only; CPU fallback profile, original text/timing unchanged until explicit review".into()})
}

#[cfg(test)]
mod tests {
    #[test]
    fn runtime_phase_reports_only_observed_categories_not_private_suffixes() {
        assert_eq!(super::diagnostic_phase("private prompt/path"), None);
        assert_eq!(
            super::diagnostic_phase("get_dummy_batch: warmup with image size = private"),
            Some("vision-context-preparation")
        );
        assert_eq!(
            super::diagnostic_phase(
                "warmup with image size = private\nencoding mtmd batch, n_chunks = private"
            ),
            Some("vision-encoding-started")
        );
        assert_eq!(
            super::diagnostic_phase(
                "encoding mtmd batch, n_chunks = private\nmtmd batch encoding done in private"
            ),
            Some("vision-encoding-completed")
        );
    }

    #[test]
    fn runtime_diagnostic_is_a_fixed_category_never_raw_model_text_or_paths() {
        assert_eq!(
            super::diagnostic_reason("error: invalid argument: /private/user"),
            "runtime argument rejected"
        );
        assert_eq!(
            super::diagnostic_reason("unknown model architecture: private-model"),
            "unsupported model architecture"
        );
        assert_eq!(
            super::diagnostic_reason("failed to load model: tensor 'private' not found"),
            "required model tensor missing"
        );
        assert_eq!(
            super::diagnostic_reason(
                "failed to load model: unsupported model architecture private"
            ),
            "unsupported model architecture"
        );
        assert_eq!(
            super::diagnostic_reason("arbitrary private text"),
            "unclassified native runtime failure"
        );
    }

    #[test]
    fn constant_colored_crops_are_blank_but_spatial_text_is_not() {
        let mut crop = image::RgbImage::from_pixel(4, 2, image::Rgb([23, 32, 42]));
        assert!(super::constant_crop(&crop));
        crop.put_pixel(1, 1, image::Rgb([230, 230, 230]));
        assert!(!super::constant_crop(&crop));
    }
}
