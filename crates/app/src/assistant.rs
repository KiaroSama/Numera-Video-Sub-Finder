use crate::{Paths, Result, fail, media, storage::hash_file};
use nvsf_core::Rect;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
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
    let frame = media::preview(input, at_ms)?;
    let (x, y, w, h) = region.pixels(frame.width(), frame.height())?;
    if u64::from(w) * u64::from(h) > 1_003_520 {
        return Err(fail(
            "Visual retry crop exceeds the model's pixel budget; choose a tighter region",
        ));
    }
    let crop = image::imageops::crop_imm(&frame, x, y, w, h).to_image();
    let min = crop
        .pixels()
        .map(|p| p.0.iter().copied().min().unwrap_or(0))
        .min()
        .unwrap_or(0);
    let max = crop
        .pixels()
        .map(|p| p.0.iter().copied().max().unwrap_or(0))
        .max()
        .unwrap_or(0);
    if max.saturating_sub(min) < 4 {
        return Ok(unreadable("Blank/constant crop; no model invocation"));
    }
    let files = verify(paths)?;
    let runtime = paths.runtime(if cfg!(windows) {
        "llama-mtmd-cli.exe"
    } else {
        "llama-mtmd-cli"
    })?;
    let scratch = tempfile::tempdir_in(&paths.cache)?;
    let image = scratch.path().join("crop.png");
    crop.save(&image).map_err(|e| fail(e.to_string()))?;
    let mut response = tempfile::NamedTempFile::new_in(scratch.path())?;
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
            "--no-display-prompt",
            "--simple-io",
        ])
        .stdin(Stdio::null())
        .stdout(response.as_file().try_clone()?)
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut owned = Owned(command.spawn()?);
    let start = Instant::now();
    let mut length = 0;
    let mut progress = Instant::now();
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(crate::AppError::Cancelled);
        }
        let now = response.as_file().metadata()?.len();
        if now > 65536 {
            return Err(fail("Visual worker response exceeds 64KiB"));
        }
        if now != length {
            length = now;
            progress = Instant::now();
        }
        if let Some(status) = owned.0.try_wait()? {
            if !status.success() {
                return Err(fail("Local visual worker failed; original OCR retained"));
            }
            break;
        }
        if start.elapsed() > Duration::from_secs(120)
            || progress.elapsed() > Duration::from_secs(60)
        {
            return Err(fail(
                "Local visual worker wall/idle deadline; original OCR retained",
            ));
        }
        std::thread::park_timeout(Duration::from_millis(20));
    }
    response.as_file_mut().seek(SeekFrom::Start(0))?;
    let mut text = String::new();
    response
        .as_file_mut()
        .take(65537)
        .read_to_string(&mut text)?;
    validate_proposal(&text)
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
struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let pid = self.0.id().to_string();
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let _ = Command::new("taskkill")
                    .args(["/PID", &pid, "/T", "/F"])
                    .creation_flags(0x08000000)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            #[cfg(unix)]
            {
                let _ = Command::new("kill")
                    .args(["-KILL", "--", &format!("-{pid}")])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
