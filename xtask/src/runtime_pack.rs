use crate::{fixture::sha, package::validate_media_inventory};
use nvsf_app::runtimes::{Entry, Pack};
use std::{
    error::Error,
    io::Write,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const MAX_NOTICES: u64 = 32 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn notice_archive_contains_exact_regular_utf8_paths_and_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let notices = directory.path().join("notices");
        std::fs::create_dir_all(notices.join("ffmpeg")).unwrap();
        std::fs::write(notices.join("ffmpeg/copyright"), b"Native license\n").unwrap();
        let output = directory.path().join("notices.tar.gz");
        archive_notices(&notices, &output).unwrap();
        let decoder = flate2::read::GzDecoder::new(std::fs::File::open(output).unwrap());
        let mut archive = tar::Archive::new(decoder);
        let mut entries = archive.entries().unwrap();
        let mut entry = entries.next().unwrap().unwrap();
        assert_eq!(
            entry.path().unwrap().as_ref(),
            Path::new("ffmpeg/copyright")
        );
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"Native license\n");
        assert!(entries.next().is_none());
        std::fs::write(notices.join("unexpected.key"), b"not notice material").unwrap();
        let refused = directory.path().join("refused.gz");
        assert!(archive_notices(&notices, &refused).is_err());
        assert!(!refused.exists());
        for name in [
            "secrets.md",
            "explain-AI.md",
            "CLAUDE.md",
            "AGENTS.md",
            "reference.md",
        ] {
            let isolated = directory.path().join(name.replace('.', "-"));
            std::fs::create_dir(&isolated).unwrap();
            std::fs::write(isolated.join(name), b"synthetic excluded content").unwrap();
            let output = isolated.with_extension("tar.gz");
            assert!(
                archive_notices(&isolated, &output).is_err(),
                "admitted {name}"
            );
            assert!(!output.exists());
        }
    }

    #[test]
    fn runtime_inventory_uses_application_target_and_exact_payload_names() {
        let directory = tempfile::tempdir().unwrap();
        let names = [
            nvsf_app::media::NATIVE_LIBRARY_NAME,
            nvsf_app::runtimes::TESSERACT_LIBRARY,
            nvsf_app::runtimes::ONNX_LIBRARY,
            "runtime-notices.tar.gz",
        ];
        let files = names
            .iter()
            .map(|name| {
                std::fs::write(directory.path().join(name), b"fixture").unwrap();
                entry(directory.path(), name).unwrap()
            })
            .collect();
        let pack = Pack {
            schema: 1,
            version: env!("CARGO_PKG_VERSION").into(),
            target: nvsf_app::runtimes::target(),
            source_sha: nvsf_core::BUILD_SOURCE_SHA
                .unwrap_or(&"a".repeat(40))
                .into(),
            files,
        };
        pack.verify(directory.path()).unwrap();
        assert_eq!(
            asset_name(&pack, names[0]),
            format!("{}-{}", pack.target, names[0])
        );
        let mut wrong = pack.clone();
        wrong.target = "linux-arm64".into();
        assert!(wrong.validate().is_err());
        std::fs::write(directory.path().join(names[0]), b"tampered").unwrap();
        assert!(pack.verify(directory.path()).is_err());
        let mut missing = pack;
        missing.files.pop();
        assert!(missing.validate().is_err());
    }
}

fn entry(root: &Path, name: &str) -> Result<Entry> {
    let path = root.join(name);
    let metadata = std::fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 512 * 1024 * 1024 {
        return Err("Runtime asset must be a bounded regular file".into());
    }
    Ok(Entry {
        file: name.into(),
        bytes: metadata.len(),
        sha256: sha(&path)?,
    })
}

fn asset_name(pack: &Pack, name: &str) -> String {
    format!("{}-{name}", pack.target)
}

pub(crate) fn sign(path: &Path, version: &str) -> Result<Vec<u8>> {
    let signature_path = path.with_file_name(format!(
        "{}.sig",
        path.file_name()
            .and_then(|n| n.to_str())
            .ok_or("Invalid signing filename")?
    ));
    if signature_path.exists() || signature_path.is_symlink() {
        return Err("Refusing to overwrite an artifact signature".into());
    }
    let mut signer = Command::new("cargo");
    signer
        .args(["tauri", "signer", "sign", "--app-version", version])
        .arg(path);
    crate::fixture::bounded(&mut signer, Duration::from_secs(30))?;
    if signature_path.metadata()?.len() > 4096 || signature_path.is_symlink() {
        return Err("Artifact signature exceeds limits".into());
    }
    Ok(std::fs::read(signature_path)?)
}

fn notice_paths(
    root: &Path,
    dir: &Path,
    paths: &mut Vec<std::path::PathBuf>,
    total: &mut u64,
    deadline: Instant,
) -> Result<()> {
    if dir.strip_prefix(root)?.components().count() > 16 {
        return Err("Native notice path exceeds depth budget".into());
    }
    for item in std::fs::read_dir(dir)? {
        if Instant::now() >= deadline {
            return Err("Native notice discovery exceeded deadline".into());
        }
        let item = item?;
        let path = item.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            notice_paths(root, &path, paths, total, deadline)?;
        } else if metadata.is_file() {
            let relative = path.strip_prefix(root)?;
            let text = relative
                .to_str()
                .ok_or("Notice filename is not UTF-8")?
                .replace('\\', "/");
            if text.len() > 512
                || text.split('/').any(|part| {
                    part.is_empty()
                        || part.starts_with('.')
                        || part.contains([':', '\0'])
                        || matches!(
                            part.to_ascii_lowercase().as_str(),
                            "secrets.md"
                                | "explain-ai.md"
                                | "claude.md"
                                | "agents.md"
                                | "reference.md"
                        )
                })
                || !matches!(
                    path.extension().and_then(|s| s.to_str()),
                    None | Some("txt" | "md" | "json" | "rtf" | "html" | "htm" | "pdf")
                )
            {
                return Err("Unexpected file in native license inventory".into());
            }
            *total = total
                .checked_add(metadata.len())
                .ok_or("Notice size overflow")?;
            if *total > MAX_NOTICES || paths.len() >= 4096 {
                return Err("Native notices exceed count or byte limits".into());
            }
            paths.push(relative.to_path_buf());
        } else {
            return Err("Native notices may not contain links or special files".into());
        }
    }
    Ok(())
}

fn archive_notices(root: &Path, output: &Path) -> Result<()> {
    let mut paths = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    notice_paths(root, root, &mut paths, &mut 0, deadline)?;
    if paths.is_empty() {
        return Err("Native notice inventory is empty".into());
    }
    paths.sort();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    let gzip = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    let mut archive = tar::Builder::new(gzip);
    for relative in paths {
        if Instant::now() >= deadline {
            return Err("Native notice archive exceeded deadline".into());
        }
        let path = root.join(&relative);
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.is_file() {
            return Err("Native notice changed during archive creation".into());
        }
        let mut header = tar::Header::new_gnu();
        header.set_size(metadata.len());
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_cksum();
        archive.append_data(&mut header, &relative, std::fs::File::open(path)?)?;
    }
    let file = archive.into_inner()?.finish()?;
    file.sync_all()?;
    Ok(())
}

fn repair_witness(stage: &Path, manifest: &[u8], signature: &[u8], work: &Path) -> Result<()> {
    use nvsf_app::{AppError, Paths, runtimes};
    use std::{io::Read, sync::atomic::AtomicBool};
    let temporary = tempfile::tempdir_in(work)?;
    let paths = Paths::new(Some(&temporary.path().join("Repair data 日本")))?;
    let project = paths.projects.join("preserved.json");
    let model = paths.models.join("preserved.onnx");
    std::fs::write(&project, b"existing project")?;
    std::fs::write(&model, b"existing model")?;
    let base = format!(
        "{}/releases/download/v{}",
        nvsf_app::updates::REPOSITORY,
        env!("CARGO_PKG_VERSION")
    );
    let cancel = AtomicBool::new(false);
    let no_fetch = |_: &Entry| -> nvsf_app::Result<std::fs::File> {
        panic!("Refused repair must not acquire a payload")
    };
    let denied =
        runtimes::repair_with_fetch(&paths, manifest, signature, &base, false, &cancel, no_fetch);
    if !denied
        .err()
        .is_some_and(|e| e.to_string().contains("approval"))
    {
        return Err("Repair denial did not fail before payload acquisition".into());
    }
    let mut tampered = manifest.to_vec();
    tampered.push(b' ');
    if runtimes::repair_with_fetch(&paths, &tampered, signature, &base, true, &cancel, no_fetch)
        .is_ok()
    {
        return Err("Tampered repair manifest was accepted".into());
    }
    let cancelled = AtomicBool::new(true);
    if !matches!(
        runtimes::repair_with_fetch(
            &paths, manifest, signature, &base, true, &cancelled, no_fetch,
        ),
        Err(AppError::Cancelled)
    ) {
        return Err("Pre-cancelled signed repair was not refused".into());
    }
    let pointer = paths.root.join("runtime-selection.json");
    if pointer.exists() || runtimes::managed(&paths)?.is_some() {
        return Err("Refused repair activated a runtime selection".into());
    }
    let fetch = |entry: &Entry| -> nvsf_app::Result<std::fs::File> {
        Ok(std::fs::File::open(stage.join(&entry.file))?)
    };
    let installed =
        runtimes::repair_with_fetch(&paths, manifest, signature, &base, true, &cancel, fetch)?;
    let selected = runtimes::managed(&paths)?.ok_or("Signed repair selection absent")?;
    if selected != installed || installed.parent() != Some(paths.root.join("runtimes").as_path()) {
        return Err("Signed repair did not activate its contained immutable pack".into());
    }
    let original = std::fs::read(&pointer)?;
    if runtimes::repair_with_fetch(&paths, manifest, signature, &base, true, &cancel, fetch)?
        != installed
    {
        return Err("Repeated signed repair changed the selected pack identity".into());
    }
    let short = runtimes::repair_with_fetch(
        &paths,
        manifest,
        signature,
        &base,
        true,
        &cancel,
        |_| -> nvsf_app::Result<std::io::Cursor<&[u8]>> { Ok(std::io::Cursor::new(b"corrupt")) },
    );
    if !short
        .err()
        .is_some_and(|e| e.to_string().contains("Runtime integrity failed"))
    {
        return Err("Short/corrupt repair did not fail signed payload verification".into());
    }
    struct CorruptRead(std::fs::File);
    impl Read for CorruptRead {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = self.0.read(buffer)?;
            if count > 0 {
                buffer[0] ^= 1;
            }
            Ok(count)
        }
    }
    let corrupt =
        runtimes::repair_with_fetch(&paths, manifest, signature, &base, true, &cancel, |entry| {
            Ok(CorruptRead(fetch(entry)?))
        });
    if !corrupt
        .err()
        .is_some_and(|e| e.to_string().contains("Runtime integrity failed"))
    {
        return Err("Same-size corrupt repair payload passed checksum verification".into());
    }
    let transport = runtimes::repair_with_fetch(
        &paths,
        manifest,
        signature,
        &base,
        true,
        &cancel,
        |_| -> nvsf_app::Result<std::fs::File> {
            Err(
                std::io::Error::new(std::io::ErrorKind::TimedOut, "controlled repair timeout")
                    .into(),
            )
        },
    );
    if !matches!(transport, Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::TimedOut) {
        return Err("Repair transport error was not propagated".into());
    }
    struct CancelRead<'a> {
        file: std::fs::File,
        cancel: &'a AtomicBool,
    }
    impl Read for CancelRead<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = self.file.read(buffer)?;
            self.cancel
                .store(true, std::sync::atomic::Ordering::Release);
            Ok(count)
        }
    }
    let interrupted =
        runtimes::repair_with_fetch(&paths, manifest, signature, &base, true, &cancel, |entry| {
            Ok(CancelRead {
                file: fetch(entry)?,
                cancel: &cancel,
            })
        });
    if !matches!(interrupted, Err(AppError::Cancelled)) {
        return Err("Cancellation during repair payload read did not stop activation".into());
    }
    if std::fs::read(&pointer)? != original
        || std::fs::read(&project)? != b"existing project"
        || std::fs::read(&model)? != b"existing model"
        || runtimes::managed(&paths)?.as_ref() != Some(&installed)
    {
        return Err("Failed repair changed the selected pack or application data".into());
    }
    let pack: Pack = serde_json::from_slice(manifest)?;
    pack.verify(&installed)?;
    if std::fs::read_dir(paths.root.join("runtimes"))?.count() != 2 {
        return Err("Repair left an undeclared staging directory".into());
    }
    let mut reopened = Paths::new(Some(&paths.root))?;
    reopened.runtimes = runtimes::managed(&reopened)?.ok_or("Reopened selection absent")?;
    if runtimes::diagnose(&reopened)
        .iter()
        .any(|row| row.state != "verified")
    {
        return Err("Reopened repaired pack failed actual native health".into());
    }
    Ok(())
}

pub fn generate(target: &str, runtimes: &Path, output: &Path) -> Result<()> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
    {
        return Err("Runtime asset production requires GitHub-hosted Actions".into());
    }
    let expected = format!(
        "{}-{}",
        std::env::consts::OS,
        if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x64"
        }
    );
    if target != expected {
        return Err("Runtime asset target is not this native host".into());
    }
    let source = std::env::var("GITHUB_SHA")?;
    if nvsf_core::BUILD_SOURCE_SHA != Some(source.as_str()) {
        return Err("Harness source identity differs from the run".into());
    }
    let work = Path::new(".ci-work").canonicalize()?;
    let parent = output
        .parent()
        .ok_or("Missing runtime output parent")?
        .canonicalize()?;
    if !parent.starts_with(&work)
        || output.exists()
        || output.is_symlink()
        || !output.file_name().is_some_and(|name| name == target)
        || parent
            .ancestors()
            .take_while(|path| *path != work)
            .any(Path::is_symlink)
        || !parent.join("reports").is_dir()
        || parent.join("reports").is_symlink()
    {
        return Err(
            "Runtime output must be a new target directory inside owned CI work with a report root"
                .into(),
        );
    }
    let report_path = parent
        .join("reports")
        .join(format!("{target}-runtime-pack.json"));
    if report_path.exists() || report_path.is_symlink() {
        return Err("Runtime report already exists".into());
    }
    let targets: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../ci/targets.json"))?;
    let row = targets["targets"]
        .as_array()
        .ok_or("Missing native targets")?
        .iter()
        .find(|row| row["id"] == target)
        .ok_or("Unknown native target")?;
    let provenance = Path::new(".ci-work/native-notices/native-inventory.json");
    if provenance.metadata()?.len() > 8 * 1024 * 1024 {
        return Err("Native provenance exceeds limits".into());
    }
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(provenance)?)?;
    validate_media_inventory(
        &record,
        target,
        row["target"].as_str().ok_or("Missing Rust target")?,
        &source,
        &sha(Path::new("Cargo.lock"))?,
        runtimes,
    )?;
    let temporary = tempfile::tempdir_in(&work)?;
    let stage = temporary.path().join("runtimes");
    std::fs::create_dir(&stage)?;
    for item in record["runtime_files"]
        .as_array()
        .ok_or("Missing runtime files")?
    {
        let name = item["file"].as_str().ok_or("Missing native filename")?;
        std::fs::copy(runtimes.join(name), stage.as_path().join(name))?;
    }
    archive_notices(
        Path::new(".ci-work/native-notices"),
        &stage.as_path().join("runtime-notices.tar.gz"),
    )?;
    let mut names = std::fs::read_dir(stage.as_path())?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    names.sort();
    let files = names
        .iter()
        .map(|name| {
            entry(
                stage.as_path(),
                name.to_str().ok_or("Invalid native filename")?,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let pack = Pack {
        schema: 1,
        version: env!("CARGO_PKG_VERSION").into(),
        target: nvsf_app::runtimes::target(),
        source_sha: source.clone(),
        files,
    };
    pack.verify(stage.as_path())?;
    let manifest = stage.as_path().join("runtime-pack.json");
    let bytes = serde_json::to_vec_pretty(&pack)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&manifest)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    let signature = sign(&manifest, &pack.version)?;
    let signature_path = stage.join("runtime-pack.json.sig");
    nvsf_app::runtimes::authenticate(&bytes, &signature)?;
    let mut tampered = bytes.clone();
    tampered.push(b' ');
    if nvsf_app::runtimes::authenticate(&tampered, &signature).is_ok() {
        return Err("Runtime metadata tampering was not rejected".into());
    }
    std::fs::rename(signature_path, stage.join("runtime-pack.sig"))?;
    // Signer secrets never reach a native resource or application subprocess.
    let cli = Path::new("target/release")
        .join(if cfg!(windows) { "nvsf.exe" } else { "nvsf" })
        .canonicalize()?;
    let mut doctor = Command::new(cli);
    crate::fixture::clean_runtime_environment(&mut doctor);
    doctor
        .arg("--data-root")
        .arg(temporary.path().join("health-data"))
        .args(["doctor", "--format", "json"])
        .env("NVSF_RUNTIME_ROOT", stage.as_path());
    for key in [
        "TAURI_SIGNING_PRIVATE_KEY",
        "TAURI_SIGNING_PRIVATE_KEY_PATH",
        "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
    ] {
        doctor.env_remove(key);
    }
    let capture = temporary.path().join("health.stdout");
    let mut child = crate::process::OwnedProcess::spawn(&mut doctor, &capture)?;
    child.wait(&capture, Duration::from_secs(30), Duration::from_secs(10))?;
    if capture.metadata()?.len() > 1024 * 1024 {
        return Err("Native health output exceeds limits".into());
    }
    pack.verify(&stage)?;
    let health: serde_json::Value = serde_json::from_slice(&std::fs::read(capture)?)?;
    if health["source_sha"] != source
        || health["dependencies"]
            .as_array()
            .is_none_or(|rows| rows.len() != 3 || rows.iter().any(|r| r["state"] != "verified"))
    {
        return Err("Actual staged runtime health failed".into());
    }
    std::fs::create_dir(output)?;
    for item in &pack.files {
        std::fs::copy(
            stage.as_path().join(&item.file),
            output.join(asset_name(&pack, &item.file)),
        )?;
    }
    std::fs::write(output.join(asset_name(&pack, "runtime-pack.json")), &bytes)?;
    std::fs::write(
        output.join(asset_name(&pack, "runtime-pack.sig")),
        signature,
    )?;
    for item in &pack.files {
        let actual = entry(output, &asset_name(&pack, &item.file))?;
        if actual.bytes != item.bytes || actual.sha256 != item.sha256 {
            return Err("Final runtime asset bytes differ from signed inventory".into());
        }
    }
    let final_manifest = std::fs::read(output.join(asset_name(&pack, "runtime-pack.json")))?;
    let final_signature = std::fs::read(output.join(asset_name(&pack, "runtime-pack.sig")))?;
    if final_manifest != bytes {
        return Err("Final runtime manifest differs from admitted bytes".into());
    }
    nvsf_app::runtimes::authenticate(&final_manifest, &final_signature)?;
    repair_witness(&stage, &final_manifest, &final_signature, &work)?;
    if std::fs::read_dir(output)?.count() != pack.files.len() + 2 {
        return Err("Final runtime output contains undeclared assets".into());
    }
    let report = serde_json::json!({"schema":1,"source_sha":source,"target":target,"passed":true,"tests":6,"failed":0,"skipped":0,"status":"candidate-not-for-publication","proof":["same-source native closure and notices","signed manifest authenticated with embedded key","modified signed metadata refused","actual staged native API health","final output payload hashes and authenticated metadata","shared authenticated repair admission, cancellation and activation with local payload acquisition"],"not_proved":["platform-native signing","HTTP/TLS repair download and process restart","installer/update lifecycle"]});
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(report_path)?
        .write_all(&serde_json::to_vec_pretty(&report)?)?;
    Ok(())
}
