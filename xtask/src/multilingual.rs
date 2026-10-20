//! Hosted synthetic language integration, never held-out subtitle-quality qualification.
use crate::{
    fixture::{bounded, clean_runtime_environment, sha},
    process::OwnedProcess,
};
use nvsf_app::{Paths, languages, models, storage};
use nvsf_core::{Rect, parse_srt};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    io::{Read, Write},
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const FONT_REVISION: &str = "1d0e46c7933245c8180fe83e47917961c913bbdb";

struct Font {
    id: &'static str,
    upstream: &'static str,
    name: &'static str,
    bytes: u64,
    sha256: &'static str,
    license_bytes: u64,
    license_sha256: &'static str,
}

const FONTS: [Font; 4] = [
    Font {
        id: "latin",
        upstream: "notosans",
        name: "NotoSans%5Bwdth%2Cwght%5D.ttf",
        bytes: 2_049_096,
        sha256: "bfb7bb691513f12e734dc346c03a03f784912432d7e3fa8e56efcf906fe86b3d",
        license_bytes: 4396,
        license_sha256: "cee9892f9f0cc8fe882c9e9537ee6a89621d86ee7ceaf70b02e2b2b1c25c061a",
    },
    Font {
        id: "arabic",
        upstream: "notosansarabic",
        name: "NotoSansArabic%5Bwdth%2Cwght%5D.ttf",
        bytes: 844_676,
        sha256: "63111b5b2e074dd48cc67692e0a2726d86ee94c1c37fe8598257b7b4e87e869e",
        license_bytes: 4382,
        license_sha256: "07fc70bfeb985cc1a87a8587d0a0c80bab11c86c9dc3fd95b6f0cb332f983e96",
    },
    Font {
        id: "indic",
        upstream: "notosansdevanagari",
        name: "NotoSansDevanagari%5Bwdth%2Cwght%5D.ttf",
        bytes: 641_944,
        sha256: "14ec4af41f27482216d1c2229f417ff9b1425e1babb014e57d1d40d03229853e",
        license_bytes: 4386,
        license_sha256: "a216f6f8d85c7228093e0ee5e258d9d377e6671f68acb4db1930b29583d0f331",
    },
    Font {
        id: "korean",
        upstream: "notosanskr",
        name: "NotoSansKR%5Bwght%5D.ttf",
        bytes: 10_414_588,
        sha256: "194018e6b2b293a7964f037b25c0249ce1418bc9ab3c971060a03aa57861e252",
        license_bytes: 4388,
        license_sha256: "1c05c68c34f9708415aada51f17e1b0092d2cea709bf4a94cd38114f9e73d7d9",
    },
];

struct Case {
    code: &'static str,
    font: &'static str,
    family: &'static str,
    text: &'static str,
    words: &'static [&'static str],
    provider: &'static str,
}

const CASES: [Case; 5] = [
    Case {
        code: "fra",
        font: "latin",
        family: "Noto Sans",
        text: "Bonjour monde",
        words: &["Bonjour", "monde"],
        provider: "recognizer=tesseract-cpu",
    },
    Case {
        code: "rus",
        font: "latin",
        family: "Noto Sans",
        text: "Привет мир",
        words: &["Привет", "мир"],
        provider: "recognizer=tesseract-cpu",
    },
    Case {
        code: "ara",
        font: "arabic",
        family: "Noto Sans Arabic",
        text: "مرحبا بالعالم",
        words: &["مرحبا", "بالعالم"],
        provider: "provider=cpu",
    },
    Case {
        code: "hin",
        font: "indic",
        family: "Noto Sans Devanagari",
        text: "नमस्ते दुनिया",
        words: &["नमस्ते", "दुनिया"],
        provider: "recognizer=tesseract-cpu",
    },
    Case {
        code: "kor",
        font: "korean",
        family: "Noto Sans KR",
        text: "안녕하세요 세계",
        words: &["안녕하세요", "세계"],
        provider: "recognizer=tesseract-cpu",
    },
];

fn known_text(text: &str, words: &[&str]) -> bool {
    words.iter().all(|word| text.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_witness_requires_every_literal_word_not_merely_nonempty_ocr() {
        for (good, wrong, words) in [
            ("Bonjour monde", "Bonjour", &["Bonjour", "monde"][..]),
            ("Привет мир", "Hello world", &["Привет", "мир"][..]),
            ("مرحبا بالعالم", "نص مختلف", &["مرحبا", "بالعالم"][..]),
            ("नमस्ते दुनिया", "नमस्ते", &["नमस्ते", "दुनिया"][..]),
            ("안녕하세요 세계", "다른 글", &["안녕하세요", "세계"][..]),
        ] {
            assert!(known_text(good, words));
            assert!(!known_text(wrong, words));
            assert!(!known_text("", words));
        }
    }
}

fn download(
    client: &reqwest::blocking::Client,
    url: &str,
    destination: &Path,
    bytes: u64,
    expected: &str,
) -> Result<()> {
    if bytes == 0 || bytes > 12 * 1024 * 1024 || destination.exists() || destination.is_symlink() {
        return Err("Invalid or existing fixture font destination".into());
    }
    let mut response = client.get(url).send()?.error_for_status()?.take(bytes + 1);
    let mut payload = Vec::new();
    response.read_to_end(&mut payload)?;
    let hash: String = Sha256::digest(&payload)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if payload.len() as u64 != bytes || hash != expected {
        return Err("Pinned fixture font/license integrity failed".into());
    }
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    file.write_all(&payload)?;
    file.sync_all()?;
    Ok(())
}

pub fn provision_font(root: &Path, id: &str) -> Result<std::path::PathBuf> {
    let font = FONTS
        .iter()
        .find(|font| font.id == id)
        .ok_or("Unknown fixture font")?;
    let client = reqwest::blocking::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    for (kind, remote, local, bytes, hash) in [
        ("font", font.name, "fixture.ttf", font.bytes, font.sha256),
        (
            "license",
            "OFL.txt",
            "OFL.txt",
            font.license_bytes,
            font.license_sha256,
        ),
    ] {
        let directory = root.join("resources").join(font.id).join(kind);
        std::fs::create_dir_all(&directory)?;
        let url = format!(
            "https://raw.githubusercontent.com/google/fonts/{FONT_REVISION}/ofl/{}/{remote}",
            font.upstream
        );
        download(&client, &url, &directory.join(local), bytes, hash)?;
    }
    Ok(root
        .join("resources")
        .join(font.id)
        .join("font/fixture.ttf"))
}

fn render(ffmpeg: &Path, root: &Path, case: &Case) -> Result<()> {
    let script = format!(
        "[Script Info]\nScriptType: v4.00+\nPlayResX: 640\nPlayResY: 360\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,{},36,&H00000000,&H00000000,&H00FFFFFF,&H00FFFFFF,0,0,0,0,100,100,0,0,1,0,0,2,10,10,25,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:00.00,0:00:02.00,Default,,0,0,0,,{}\n",
        case.family, case.text
    );
    storage::write_atomic(
        &root.join(format!("{}.ass", case.code)),
        script.as_bytes(),
        false,
    )?;
    let filter = format!(
        "ass={}.ass:fontsdir=resources/{}/font:shaping=complex",
        case.code, case.font
    );
    let mut command = Command::new(ffmpeg);
    command
        .current_dir(root)
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "warning",
            "-filter_threads",
            "2",
            "-f",
            "lavfi",
            "-i",
            "color=c=white:s=640x360:r=2:d=2",
            "-vf",
            &filter,
            "-c:v",
            "mpeg4",
            "-threads",
            "2",
            "-q:v",
            "2",
            "-n",
        ])
        .arg(format!("{}.mp4", case.code));
    bounded(&mut command, Duration::from_secs(20))
}

#[cfg(unix)]
fn parent_lease_loss(cli: &Path, paths: &Paths, root: &Path) -> Result<()> {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::Stdio;
    let control = root.join("parent-lease");
    std::fs::create_dir(&control)?;
    storage::write_atomic(&control.join("pause"), b"pause", false)?;
    let checkpoint = control.join("paused.nvsf.json");
    let config = nvsf_core::ExtractConfig {
        input: root
            .join("rus.mp4")
            .to_str()
            .ok_or("Invalid fixture path")?
            .into(),
        output: control
            .join("paused.srt")
            .to_str()
            .ok_or("Invalid output path")?
            .into(),
        data_root: Some(paths.root.to_str().ok_or("Invalid data path")?.into()),
        language: "rus".into(),
        device: "cpu".into(),
        ..Default::default()
    };
    let request = control.join("request.json");
    storage::write_atomic(&request, &serde_json::to_vec(&config)?, false)?;
    let capture = control.join("worker.log");
    let log = std::fs::File::create(&capture)?;
    let mut command = Command::new(cli);
    clean_runtime_environment(&mut command);
    command
        .args(["worker", "--request"])
        .arg(&request)
        .arg("--control")
        .arg(&control)
        .env("NVSF_RUNTIME_ROOT", &paths.runtimes)
        .env("OMP_NUM_THREADS", "2")
        .stdin(Stdio::piped())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0);
    let child = command.spawn()?;
    struct Paused(std::process::Child);
    impl Drop for Paused {
        fn drop(&mut self) {
            if matches!(self.0.try_wait(), Ok(Some(_))) {
                return;
            }
            // SAFETY: this still-owned child was created as its own group leader above.
            unsafe { kill(-(self.0.id() as i32), 9) };
            let _ = self.0.wait();
        }
    }
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    let mut child = Paused(child);
    eprintln!(
        "[INFO] [LANGUAGE-WITNESS] Owned parent-lease regression pid={}",
        child.0.id()
    );
    let lease = child.0.stdin.take().ok_or("Missing parent lease writer")?;
    let ready = Instant::now() + Duration::from_secs(15);
    loop {
        if child.0.try_wait()?.is_some() {
            return Err("Actual worker exited before paused readiness".into());
        }
        if checkpoint.exists() {
            let project = storage::load_project(&checkpoint)?;
            if project.completed || project.config.language != "rus" {
                return Err("Paused worker checkpoint did not establish real readiness".into());
            }
            break;
        }
        if Instant::now() >= ready {
            return Err("Paused worker readiness exceeded15s".into());
        }
        std::thread::park_timeout(Duration::from_millis(20));
    }
    drop(lease);
    let stopped = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.0.try_wait()? {
            if status.signal() != Some(9) || control.join("paused.srt").exists() {
                return Err("Lost parent lease did not terminate only the paused worker".into());
            }
            break;
        }
        if Instant::now() >= stopped {
            return Err("Worker survived parent lease EOF beyond3s".into());
        }
        std::thread::park_timeout(Duration::from_millis(20));
    }
    Ok(())
}

pub fn verify(cli: &Path, ffmpeg: &Path, data_root: &Path, output: &Path) -> Result<()> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
    {
        return Err("Multilingual OCR witnesses require GitHub-hosted Actions".into());
    }
    let started = Instant::now();
    let source = std::env::var("GITHUB_SHA")?;
    let target = std::env::var("NVSF_TARGET_ID")?;
    if source.len() != 40 || !source.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Multilingual report requires an exact source SHA".into());
    }
    let expected_target = format!(
        "{}-{}",
        std::env::consts::OS,
        match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            _ => return Err("Unsupported native architecture".into()),
        }
    );
    if nvsf_core::BUILD_SOURCE_SHA != Some(source.as_str()) || target != expected_target {
        return Err("Multilingual witness source/target differs from native run".into());
    }
    let work = Path::new(".ci-work").canonicalize()?;
    if output.exists()
        || output.is_symlink()
        || output
            .parent()
            .ok_or("Missing evidence parent")?
            .canonicalize()?
            != work
    {
        return Err(
            "Multilingual evidence must be a new directory directly inside owned CI work".into(),
        );
    }
    std::fs::create_dir(output)?;
    let root = output.canonicalize()?;
    let cli = cli.canonicalize()?;
    let ffmpeg = ffmpeg.canonicalize()?;
    let baseline = Paths::new(Some(data_root))?;
    // Extra packs stay outside the baseline data consumed by package assembly.
    let paths = Paths::new(Some(&root.join("data")))?;
    for id in ["det", "fa"] {
        let from = models::verify(&baseline, id)?;
        let to = paths.models.join(format!("{id}.onnx"));
        if !to.exists() {
            std::fs::copy(from, &to)?;
        }
        models::verify(&paths, id)?;
    }
    for font in &FONTS {
        provision_font(&root, font.id)?;
    }
    let mut rows = Vec::new();
    for case in &CASES {
        if started.elapsed() >= Duration::from_secs(480) {
            return Err("Multilingual witness exceeded overall deadline".into());
        }
        eprintln!(
            "[INFO] [LANGUAGE-WITNESS] Installing explicit pack and recognizing {}",
            case.code
        );
        let mut install = Command::new(&cli);
        clean_runtime_environment(&mut install);
        install
            .arg("--data-root")
            .arg(&paths.root)
            .args(["languages", "install", case.code]);
        let capture = root.join(format!("{}-install.log", case.code));
        let mut child = OwnedProcess::spawn(&mut install, &capture)?;
        child.wait(&capture, Duration::from_secs(60), Duration::from_secs(30))?;
        languages::verify(&paths, case.code)?;
        if case.code == "rus" {
            let exported = root.join("exported-rus.traineddata");
            languages::export(&paths, case.code, &exported)?;
            if languages::export(&paths, case.code, &exported).is_ok() {
                return Err("Language export replaced an existing destination".into());
            }
            let imported = Paths::new(Some(&root.join("imported-language")))?;
            languages::import(
                &imported,
                case.code,
                &exported,
                &std::sync::atomic::AtomicBool::new(false),
            )?;
            let selected = languages::verify(&imported, case.code)?;
            if sha(&selected)? != sha(&exported)? {
                return Err("Imported language differs from the verified export".into());
            }
            if !matches!(
                languages::import(
                    &imported,
                    case.code,
                    &exported,
                    &std::sync::atomic::AtomicBool::new(true)
                ),
                Err(nvsf_app::AppError::Cancelled)
            ) {
                return Err("Cancelled language import was not refused".into());
            }
            languages::verify(&imported, case.code)?;
        }
        render(&ffmpeg, &root, case)?;
        let input = root.join(format!("{}.mp4", case.code));
        let subtitle = root.join(format!("{}.srt", case.code));
        let mut extract = Command::new(&cli);
        clean_runtime_environment(&mut extract);
        extract
            .arg("--data-root")
            .arg(&paths.root)
            .args(["extract", "--input"])
            .arg(&input)
            .arg("--output")
            .arg(&subtitle)
            .args([
                "--language",
                case.code,
                "--device",
                "cpu",
                "--quality",
                "accurate",
                "--threads",
                "2",
            ]);
        let capture = root.join(format!("{}-extract.log", case.code));
        let mut child = OwnedProcess::spawn(&mut extract, &capture)?;
        child.wait(&capture, Duration::from_secs(45), Duration::from_secs(20))?;
        let cues = parse_srt(&std::fs::read_to_string(&subtitle)?)?;
        let project = storage::load_project(&subtitle.with_extension("nvsf.json"))?;
        let diagnostic = std::fs::read_to_string(&capture)?;
        if !cues.iter().any(|cue| known_text(&cue.text, case.words))
            || cues
                .iter()
                .any(|cue| cue.end_ms <= cue.start_ms || cue.end_ms > 2000)
            || !project.completed
            || project.config.language != case.code
            || project.source_hash != sha(&input)?
            || project.model_hash != languages::fingerprint(&paths, case.code)?
            || !diagnostic.contains(case.provider)
        {
            return Err(format!(
                "Actual {} extraction/text/timing/provider witness failed",
                case.code
            )
            .into());
        }
        rows.push(json!({"language":case.code,"expected":case.text,"actual":cues.iter().map(|cue|&cue.text).collect::<Vec<_>>(),"provider":case.provider,"synthetic":true}));
    }
    eprintln!(
        "[INFO] [LANGUAGE-WITNESS] Re-recognizing Russian cue through the application service"
    );
    let cue = nvsf_app::regions::recognize_cue(
        &paths,
        &root.join("rus.mp4"),
        500,
        Rect::default(),
        "rus",
    )?;
    if !known_text(&cue.text, &["Привет", "мир"]) || started.elapsed() >= Duration::from_secs(480)
    {
        return Err(
            "Actual selected-cue multilingual re-recognition failed or exceeded deadline".into(),
        );
    }
    #[cfg(unix)]
    parent_lease_loss(&cli, &paths, &root)?;
    if started.elapsed() >= Duration::from_secs(480) {
        return Err("Multilingual witness exceeded overall deadline".into());
    }
    let report = json!({"schema":1,"source_sha":source,"target":target,"passed":true,"tests":6 + u64::from(cfg!(unix)),"failed":0,"skipped":0,"synthetic":true,"languages":rows,"rerecognition":{"language":"rus","actual":cue.text,"seam":"app service"},"worker_parent_lease":if cfg!(unix) { "EOF terminated actual paused isolated worker with SIGKILL" } else { "Unix process-group contract not applicable" },"font_revision":FONT_REVISION,"elapsed_ms":started.elapsed().as_millis(),"not_proved":["real subtitle CER/recall/boundary quality","other catalog languages","host reOCR token/picker lifecycle","physical acceleration"]});
    storage::write_atomic(
        &root.join("multilingual-report.json"),
        &serde_json::to_vec_pretty(&report)?,
        false,
    )?;
    Ok(())
}
