use crate::{
    Paths, Result, fail,
    storage::{hash_file, write_atomic},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub file: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Pack {
    pub schema: u32,
    pub version: String,
    pub target: String,
    pub source_sha: String,
    pub files: Vec<Entry>,
}
#[derive(Debug, Serialize)]
pub struct Diagnostic {
    pub component: String,
    pub state: String,
    pub message: String,
}

pub const TESSERACT_LIBRARY: &str = if cfg!(windows) {
    "tesseract55.dll"
} else if cfg!(target_os = "macos") {
    "libtesseract.dylib"
} else {
    "libtesseract.so"
};
pub const ONNX_LIBRARY: &str = if cfg!(windows) {
    "onnxruntime.dll"
} else if cfg!(target_os = "macos") {
    "libonnxruntime.dylib"
} else {
    "libonnxruntime.so"
};

pub fn target() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

impl Pack {
    pub fn validate(&self) -> Result<()> {
        if self.schema != 1
            || self.version != env!("CARGO_PKG_VERSION")
            || self.target != target()
            || self.source_sha.len() != 40
            || !self.source_sha.bytes().all(|c| c.is_ascii_hexdigit())
            || nvsf_core::BUILD_SOURCE_SHA.is_some_and(|sha| sha != self.source_sha)
            || self.files.is_empty()
            || self.files.len() > 512
        {
            return Err(fail(
                "Runtime pack version, target or inventory is incompatible",
            ));
        }
        let mut names = std::collections::HashSet::new();
        let mut total = 0u64;
        for entry in &self.files {
            let path = Path::new(&entry.file);
            if entry.file.is_empty()
                || entry.file.len() > 180
                || entry.file.starts_with('.')
                || entry.file.contains(['/', '\\', ':'])
                || !entry
                    .file
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
                || entry.file.ends_with('.')
                || entry.file.contains("..")
                || path
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                || !names.insert(entry.file.to_ascii_lowercase())
                || entry.bytes == 0
                || entry.bytes > 512 * 1024 * 1024
                || entry.sha256.len() != 64
                || !entry.sha256.bytes().all(|c| c.is_ascii_hexdigit())
            {
                return Err(fail("Unsafe or invalid runtime inventory entry"));
            }
            total = total
                .checked_add(entry.bytes)
                .ok_or_else(|| fail("Runtime pack size overflow"))?;
        }
        if total > 2 * 1024 * 1024 * 1024 {
            return Err(fail("Runtime pack exceeds 2GiB"));
        }
        for name in [
            crate::media::NATIVE_LIBRARY_NAME,
            TESSERACT_LIBRARY,
            ONNX_LIBRARY,
            "runtime-notices.tar.gz",
        ] {
            if !self.files.iter().any(|e| e.file == name) {
                return Err(fail(format!(
                    "Runtime pack is missing required component {name}"
                )));
            }
        }
        Ok(())
    }
    pub fn verify(&self, root: &Path) -> Result<()> {
        self.validate()?;
        let root = root.canonicalize()?;
        for entry in &self.files {
            let path = root.join(&entry.file);
            if path.is_symlink()
                || !path.is_file()
                || !path.canonicalize()?.starts_with(&root)
                || path.metadata()?.len() != entry.bytes
                || hash_file(&path)? != entry.sha256
            {
                return Err(fail(format!("Runtime integrity failed: {}", entry.file)));
            }
            #[cfg(unix)]
            if entry.file == "llama-mtmd-cli" {
                use std::os::unix::fs::PermissionsExt;
                let mode = path.metadata()?.permissions().mode();
                if mode & 0o100 == 0 || mode & 0o7022 != 0 {
                    return Err(fail(
                        "Managed visual helper must be owner-executable without special or shared-write permissions",
                    ));
                }
            }
        }
        for entry in std::fs::read_dir(&root)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| fail("Non-UTF-8 native runtime filename"))?;
            if !entry.file_type()?.is_file()
                || (!self.files.iter().any(|file| file.file == name)
                    && !matches!(name, "runtime-pack.json" | "runtime-pack.sig"))
            {
                return Err(fail("Uninventoried file in native runtime closure"));
            }
        }
        Ok(())
    }
}

pub fn diagnose(paths: &Paths) -> Vec<Diagnostic> {
    let media = crate::media::MediaRuntime::load(paths);
    let mut rows = vec![Diagnostic {
        component: "FFmpeg native adapter".into(),
        state: if media.is_ok() {
            "verified"
        } else {
            "missing-or-incompatible"
        }
        .into(),
        message: match media {
            Ok(runtime) => format!(
                "ABI and native library versions verified: {:?}",
                runtime.health().versions
            ),
            Err(e) => e.to_string(),
        },
    }];
    for (component, result) in [
        ("Tesseract", crate::runtime_health::tesseract(paths)),
        ("ONNX Runtime", crate::runtime_health::onnx(paths)),
    ] {
        rows.push(Diagnostic { component: component.into(), state: if result.is_ok() { "verified" } else { "missing-or-incompatible" }.into(), message: match result { Ok(version) => format!("Native API/version verified: {version}; model/operator/provider qualification is separate"), Err(e) => e.to_string() } });
    }
    if let Err(error) = managed(paths) {
        rows.push(Diagnostic {
            component: "Managed runtime selection".into(),
            state: "invalid".into(),
            message: format!(
                "Ignored invalid selection; bundled resources remain selected: {error}"
            ),
        });
    }
    rows
}

pub fn managed(paths: &Paths) -> Result<Option<PathBuf>> {
    let pointer = paths.root.join("runtime-selection.json");
    if !pointer.exists() {
        return Ok(None);
    }
    if pointer.is_symlink() || pointer.metadata()?.len() > 256 {
        return Err(fail("Invalid managed runtime selection"));
    }
    let id: String = serde_json::from_slice(&std::fs::read(pointer)?)?;
    if id.len() != 64 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(fail("Invalid managed runtime identity"));
    }
    let parent = paths.root.join("runtimes");
    let root = parent.join(&id);
    if parent.is_symlink()
        || root.is_symlink()
        || !root.canonicalize()?.starts_with(paths.root.canonicalize()?)
    {
        return Err(fail("Managed runtime root escapes application data"));
    }
    let record = root.join("runtime-pack.json");
    let manifest = read_bounded(&record, 1024 * 1024)?;
    let signature = read_bounded(&root.join("runtime-pack.sig"), 4096)?;
    authenticate(&manifest, &signature)?;
    if hash_file(&record)? != id {
        return Err(fail(
            "Managed runtime selection hash differs from signed inventory",
        ));
    }
    let pack: Pack = serde_json::from_slice(&manifest)?;
    pack.verify(&root)?;
    Ok(Some(root))
}

pub fn authenticate(manifest: &[u8], signature: &[u8]) -> Result<()> {
    if manifest.len() > 1024 * 1024 || signature.len() > 4096 {
        return Err(fail("Runtime signature or manifest exceeds limits"));
    }
    crate::updates::verify_signature(manifest, signature)?;
    let pack: Pack = serde_json::from_slice(manifest)?;
    pack.validate()
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    if path.is_symlink() || !path.is_file() {
        return Err(fail("Runtime metadata must be a regular file"));
    }
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > limit {
        return Err(fail("Runtime metadata exceeds limits"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(fail("Runtime metadata exceeds limits"));
    }
    Ok(bytes)
}

pub fn fetch() -> Result<(Vec<u8>, Vec<u8>, String)> {
    // An exact application version only: a newer release's native ABI is not a repair pack.
    let base = format!(
        "{}/releases/download/v{}",
        crate::updates::REPOSITORY,
        env!("CARGO_PKG_VERSION")
    );
    let client = crate::updates::client(Duration::from_secs(20))?;
    let read = |name: &str, limit: u64| -> Result<Vec<u8>> {
        let url = format!("{base}/{}-{name}", target());
        crate::updates::official_asset(&url)?;
        let response = client
            .get(url)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| fail(format!("Matching dependency pack is not available: {e}")))?;
        crate::updates::metadata(response, limit)
    };
    let manifest = read("runtime-pack.json", 1024 * 1024)?;
    let signature = read("runtime-pack.sig", 4096)?;
    authenticate(&manifest, &signature)?;
    Ok((manifest, signature, base))
}

pub fn repair(
    paths: &Paths,
    manifest: &[u8],
    signature: &[u8],
    base: &str,
    consent: bool,
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    let mut client = None;
    repair_with_fetch(paths, manifest, signature, base, consent, cancel, |entry| {
        if client.is_none() {
            client = Some(crate::updates::client(Duration::from_secs(20))?);
        }
        client
            .as_ref()
            .ok_or_else(|| fail("Runtime transfer client unavailable"))?
            .get(format!("{base}/{}-{}", target(), entry.file))
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| fail(e.to_string()))
    })
}

/// Acquisition adapter for authenticated packs; all admission and activation policy is shared.
/// Callers must supply only the payload for the requested signed entry.
pub fn repair_with_fetch<R: Read>(
    paths: &Paths,
    manifest: &[u8],
    signature: &[u8],
    base: &str,
    consent: bool,
    cancel: &AtomicBool,
    mut fetch: impl FnMut(&Entry) -> Result<R>,
) -> Result<PathBuf> {
    if !consent {
        return Err(fail(
            "Explicit app-local dependency repair approval required",
        ));
    }
    authenticate(manifest, signature)?;
    let pack: Pack = serde_json::from_slice(manifest)?;
    pack.validate()?;
    crate::updates::official_asset(&format!("{base}/runtime-pack.json"))?;
    let root = paths.root.join("runtimes");
    if root.is_symlink() {
        return Err(fail("Runtime installation root must not be a symlink"));
    }
    std::fs::create_dir_all(&root)?;
    let _lock = crate::storage::JobLock::acquire(&root.join("repair.lock"))?;
    let stage = tempfile::tempdir_in(&root)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    for entry in &pack.files {
        if cancel.load(Ordering::Acquire) {
            return Err(crate::AppError::Cancelled);
        }
        let mut response = fetch(entry)?;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(stage.path().join(&entry.file))?;
        let mut received = 0u64;
        let mut bytes = [0u8; 65536];
        loop {
            if std::time::Instant::now() >= deadline {
                return Err(fail("Dependency transfer exceeded 600-second deadline"));
            }
            if cancel.load(Ordering::Acquire) {
                return Err(crate::AppError::Cancelled);
            }
            let count = response.read(&mut bytes)?;
            if count == 0 {
                break;
            }
            received = received
                .checked_add(count as u64)
                .ok_or_else(|| fail("Runtime download overflow"))?;
            if received > entry.bytes {
                return Err(fail("Runtime payload exceeds declared size"));
            }
            file.write_all(&bytes[..count])?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if entry.file == "llama-mtmd-cli" {
                0o700
            } else {
                0o600
            };
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        file.sync_all()?;
    }
    pack.verify(stage.path())?;
    let mut selected = paths.clone();
    selected.runtimes = stage.path().to_path_buf();
    crate::media::MediaRuntime::load(&selected)?;
    crate::runtime_health::tesseract(&selected)?;
    crate::runtime_health::onnx(&selected)?;
    write_atomic(&stage.path().join("runtime-pack.json"), manifest, false)?;
    write_atomic(&stage.path().join("runtime-pack.sig"), signature, false)?;
    let id = hash_file(&stage.path().join("runtime-pack.json"))?;
    let destination = root.join(&id);
    if destination.is_symlink() {
        return Err(fail("Runtime activation destination must not be a symlink"));
    }
    if destination.exists() {
        pack.verify(&destination)?;
        authenticate(
            &read_bounded(&destination.join("runtime-pack.json"), 1024 * 1024)?,
            &read_bounded(&destination.join("runtime-pack.sig"), 4096)?,
        )?;
        if hash_file(&destination.join("runtime-pack.json"))? != id {
            return Err(fail("Existing runtime inventory identity differs"));
        }
    } else {
        std::fs::rename(stage.path(), &destination)?;
    }
    if cancel.load(Ordering::Acquire) {
        return Err(crate::AppError::Cancelled);
    }
    write_atomic(
        &paths.root.join("runtime-selection.json"),
        &serde_json::to_vec(&id)?,
        true,
    )?;
    Ok(destination)
}
