use nvsf_core::{Cue, export_srt, parse_srt};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

pub fn provision_models(root: &Path) -> Result<(), Box<dyn Error>> {
    let model_dir = root.join("models");
    std::fs::create_dir_all(&model_dir)?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()?;
    for (manifest, subdir, suffix) in [
        ("ci/models.json", "", "onnx"),
        ("ci/tessdata.json", "tessdata", "traineddata"),
    ] {
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest)?)?;
        let rows = json["models"].as_array().ok_or("Missing model inventory")?;
        for row in rows {
            let id = row["id"].as_str().ok_or("Missing model id")?;
            let hash = row["sha256"].as_str().ok_or("Missing model hash")?;
            let bytes = row["bytes"].as_u64().ok_or("Missing model size")?;
            let dir = model_dir.join(subdir);
            std::fs::create_dir_all(&dir)?;
            let output = dir.join(format!("{id}.{suffix}"));
            if output.is_file() && sha(&output)? == hash {
                continue;
            }
            let response = client
                .get(row["url"].as_str().ok_or("Missing model URL")?)
                .send()?
                .error_for_status()?;
            let mut data = Vec::new();
            response.take(bytes + 1).read_to_end(&mut data)?;
            if data.len() as u64 != bytes
                || Sha256::digest(&data)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
                    != hash
            {
                return Err("Model download integrity failed".into());
            }
            std::fs::write(output, data)?;
        }
    }
    Ok(())
}

pub fn generate(ffmpeg: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(output)?;
    let font = if cfg!(windows) {
        PathBuf::from("C:/Windows/Fonts/arial.ttf")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("/System/Library/Fonts/Supplemental/Arial.ttf")
    } else {
        PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")
    };
    if !font.is_file() {
        return Err("Fixture English font unavailable".into());
    }
    let cues = vec![
        Cue::new(1, 0, 0, 2000, "Hello world".into())?,
        Cue::new(2, 0, 3000, 5000, "Hello world".into())?,
        Cue::new(3, 0, 5000, 6000, "16".into())?,
    ];
    std::fs::write(
        output.join("en.expected.srt"),
        export_srt(&cues, false, false)?,
    )?;
    let subtitle = output.join("en.txt");
    std::fs::write(&subtitle, "Hello world")?;
    let escape = |p: &Path| {
        p.to_string_lossy()
            .replace('\\', "/")
            .replace(':', "\\:")
            .replace('\'', "\\'")
    };
    let filter = format!(
        "drawtext=fontfile='{}':textfile='{}':fontsize=40:fontcolor=white:borderw=2:x=(w-tw)/2:y=h-70:enable='lt(t,2)+gte(t,3)*lt(t,5)',drawtext=fontfile='{}':text='16':fontsize=40:fontcolor=white:borderw=2:x=(w-tw)/2:y=h-70:enable='gte(t,5)'",
        escape(&font),
        escape(&subtitle),
        escape(&font)
    );
    let mut c = Command::new(ffmpeg);
    c.args([
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=0x17202a:s=640x360:r=10:d=6",
        "-vf",
        &filter,
        "-c:v",
        "mpeg4",
        "-q:v",
        "2",
        "-y",
    ])
    .arg(output.join("en.mp4"));
    bounded(&mut c, Duration::from_secs(30))?;
    std::fs::write(
        output.join("CORPUS_STATUS.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"synthetic":true,"real_world_quality":"NOT_RUN","language":"en","purpose":"timing/recognition smoke, not held-out qualification"}),
        )?,
    )?;
    Ok(())
}

pub fn smoke(cli: &Path, fixture_dir: &Path, data_root: &Path) -> Result<(), Box<dyn Error>> {
    let cli = cli.canonicalize()?;
    let input = fixture_dir.join("en.mp4").canonicalize()?;
    let label = if cli.to_string_lossy().contains("relocated") {
        "en.package"
    } else {
        "en.actual"
    };
    let output = fixture_dir.join(format!("{label}.srt"));
    let mut c = Command::new(&cli);
    clean_runtime_environment(&mut c);
    c.arg("--data-root")
        .arg(data_root)
        .args(["extract", "--input"])
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .args([
            "--language",
            "en",
            "--device",
            "cpu",
            "--quality",
            "accurate",
            "--overwrite",
        ]);
    bounded(&mut c, Duration::from_secs(180))?;
    let actual = parse_srt(&std::fs::read_to_string(&output)?)?;
    if actual.is_empty() {
        return Err("Real ONNX extraction produced no fixture cues".into());
    }
    if !actual
        .iter()
        .any(|c| c.text.to_lowercase().contains("hello"))
    {
        return Err("OCR failed to recognize known visible Hello text".into());
    }
    if actual.iter().any(|c| c.end_ms <= c.start_ms) {
        return Err("Invalid extracted timing".into());
    }
    if actual
        .iter()
        .filter(|c| c.text.to_lowercase().contains("hello"))
        .count()
        < 2
    {
        return Err("Repeated appearance after absence was merged or lost".into());
    }
    let mut invalid = Command::new(&cli);
    clean_runtime_environment(&mut invalid);
    invalid.args([
        "extract",
        "--input",
        "missing-video",
        "--output",
        "out.srt",
        "--language",
        "xx",
    ]);
    if bounded(&mut invalid, Duration::from_secs(10)).is_ok() {
        return Err("Invalid input incorrectly succeeded".into());
    }
    println!(
        "Actual native CLI/FFmpeg/ONNX English synthetic smoke passed; real en/fa/ja quality still unqualified"
    );
    Ok(())
}

pub fn clean_runtime_environment(command: &mut Command) {
    command
        .env_remove("LD_LIBRARY_PATH")
        .env_remove("DYLD_LIBRARY_PATH");
    let paths = std::env::var_os("PATH").unwrap_or_default();
    let clean: Vec<_> = std::env::split_paths(&paths)
        .filter(|p| {
            let p = p.to_string_lossy().replace('\\', "/");
            !p.contains(".ci-work/native") && !p.contains(".ci-work/runtimes")
        })
        .collect();
    if let Ok(path) = std::env::join_paths(clean) {
        command.env("PATH", path);
    }
    if command
        .get_program()
        .to_string_lossy()
        .contains("relocated")
    {
        command.env_remove("NVSF_RUNTIME_ROOT");
    }
}

pub fn bounded(command: &mut Command, timeout: Duration) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(".ci-work/captures")?;
    let capture = tempfile::NamedTempFile::new_in(".ci-work/captures")?;
    let mut child = crate::process::OwnedProcess::spawn(command, capture.path())?;
    child.wait(
        capture.path(),
        timeout,
        timeout.min(Duration::from_secs(60)),
    )
}
pub fn sha(path: &Path) -> Result<String, Box<dyn Error>> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
