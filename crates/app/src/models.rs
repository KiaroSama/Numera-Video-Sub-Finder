use crate::{
    Paths, Result, fail,
    storage::{hash_file, write_atomic},
};
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Read, path::Path, sync::atomic::AtomicBool};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelSpec {
    pub id: String,
    pub upstream: String,
    pub revision: String,
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
    pub metadata: String,
    pub metadata_sha256: String,
    pub languages: Vec<String>,
    pub license: String,
    pub status: String,
}
#[derive(Deserialize)]
struct Manifest {
    schema: u32,
    models: Vec<ModelSpec>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
    pub id: String,
    pub languages: Vec<String>,
    pub installed: bool,
    pub verified: bool,
    pub message: String,
}

pub fn specs() -> Result<Vec<ModelSpec>> {
    let manifest: Manifest = serde_json::from_str(include_str!("../../../ci/models.json"))?;
    if manifest.schema != 1 {
        return Err(fail("Unsupported model manifest schema"));
    }
    Ok(manifest.models)
}

pub fn metadata(id: &str) -> Result<&'static str> {
    match id {
        "det" => Ok(include_str!("../../../assets/model-metadata/det.yml")),
        "fa" => Ok(include_str!("../../../assets/model-metadata/fa.yml")),
        "en-ja" => Ok(include_str!("../../../assets/model-metadata/en-ja.yml")),
        "ja-specialist" => Ok(include_str!(
            "../../../assets/model-metadata/ja-specialist.yml"
        )),
        _ => Err(fail("Unknown model")),
    }
}

pub fn verify(paths: &Paths, id: &str) -> Result<std::path::PathBuf> {
    let spec = specs()?
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| fail("Unknown model identifier"))?;
    let path = paths.models.join(format!("{id}.onnx"));
    if !path.is_file() {
        return Err(fail(format!(
            "Model {id} not installed; explicitly run models install or import"
        )));
    }
    if path.metadata()?.len() != spec.bytes || hash_file(&path)? != spec.sha256 {
        return Err(fail(format!(
            "Model {id} checksum failed; reimport a valid pack"
        )));
    }
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(paths.models.canonicalize()?) {
        return Err(fail("Model symlink escapes model root"));
    }
    Ok(canonical)
}

pub fn catalog(paths: &Paths) -> Result<Vec<ModelStatus>> {
    specs()?
        .into_iter()
        .map(|m| {
            let installed = paths.models.join(format!("{}.onnx", m.id)).is_file();
            let checked = if installed {
                verify(paths, &m.id)
            } else {
                Err(fail("Not installed"))
            };
            Ok(ModelStatus {
                id: m.id,
                languages: m.languages,
                installed,
                verified: checked.is_ok(),
                message: match checked {
                    Ok(_) => "Hash verified; quality qualification is separate".into(),
                    Err(e) => e.to_string(),
                },
            })
        })
        .collect()
}

pub fn install_defaults(paths: &Paths) -> Result<()> {
    install(paths, None, &AtomicBool::new(false))
}

#[derive(Deserialize)]
struct TessSpec {
    id: String,
    url: String,
    sha256: String,
    bytes: u64,
}
#[derive(Deserialize)]
struct TessManifest {
    schema: u32,
    models: Vec<TessSpec>,
}

pub fn install(paths: &Paths, selected: Option<&str>, cancel: &AtomicBool) -> Result<()> {
    if let Some(id) = selected
        && !specs()?.iter().any(|s| s.id == id)
    {
        return Err(fail("Unknown model identifier"));
    }
    for spec in specs()?
        .into_iter()
        .filter(|s| selected.map_or(s.id != "ja-specialist", |id| s.id == id))
    {
        crate::model_download::install(
            &spec.url,
            spec.bytes,
            &spec.sha256,
            &paths.models.join(format!("{}.onnx", spec.id)),
            cancel,
        )?;
    }
    if selected.is_none() {
        let manifest: TessManifest =
            serde_json::from_str(include_str!("../../../ci/tessdata.json"))?;
        if manifest.schema != 1 {
            return Err(fail("Unsupported Tesseract manifest"));
        }
        let root = paths.models.join("tessdata");
        std::fs::create_dir_all(&root)?;
        for spec in manifest.models {
            crate::model_download::install(
                &spec.url,
                spec.bytes,
                &spec.sha256,
                &root.join(format!("{}.traineddata", spec.id)),
                cancel,
            )?;
        }
    }
    Ok(())
}

pub fn verify_tessdata(paths: &Paths, id: &str) -> Result<std::path::PathBuf> {
    crate::languages::verify(paths, id)
}

pub fn seed_bundled(paths: &Paths) -> Result<()> {
    let Some(resources) = paths.runtimes.parent() else {
        return Ok(());
    };
    let source = resources.join("data/models");
    if !source.is_dir() {
        return Ok(());
    }
    let source = source.canonicalize()?;
    if source == paths.models.canonicalize()? {
        return Ok(());
    }
    let _lock = crate::storage::JobLock::acquire(&paths.models.join("seed.lock"))?;
    let mut rows: Vec<_> = specs()?
        .into_iter()
        .map(|m| (format!("{}.onnx", m.id), m.bytes, m.sha256))
        .collect();
    let tess: TessManifest = serde_json::from_str(include_str!("../../../ci/tessdata.json"))?;
    rows.extend(
        tess.models
            .into_iter()
            .map(|m| (format!("tessdata/{}.traineddata", m.id), m.bytes, m.sha256)),
    );
    for (name, size, hash) in rows {
        let destination = paths.models.join(&name);
        if destination.exists() {
            continue;
        }
        let candidate = source.join(&name);
        if !candidate.is_file() {
            continue;
        }
        let candidate = candidate.canonicalize()?;
        if !candidate.starts_with(&source)
            || candidate.metadata()?.len() != size
            || hash_file(&candidate)? != hash
        {
            return Err(fail(
                "Bundled model integrity failed; writable resources unchanged",
            ));
        }
        let parent = destination
            .parent()
            .ok_or_else(|| fail("Model destination has no parent"))?;
        std::fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        std::io::copy(&mut File::open(candidate)?, &mut temporary)?;
        temporary.as_file().sync_all()?;
        if hash_file(temporary.path())? != hash {
            return Err(fail("Bundled model changed during copy"));
        }
        temporary
            .persist_noclobber(destination)
            .map_err(|e| e.error)?;
    }
    Ok(())
}

pub fn export_model(paths: &Paths, id: &str, destination: &Path) -> Result<()> {
    let source = verify(paths, id)?;
    if destination.exists() {
        return Err(fail("Model export destination already exists"));
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    std::io::copy(&mut File::open(source)?, &mut tmp)?;
    tmp.as_file().sync_all()?;
    tmp.persist_noclobber(destination).map_err(|e| e.error)?;
    Ok(())
}

pub fn import_model(paths: &Paths, id: &str, source: &Path) -> Result<()> {
    let spec = specs()?
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| fail("Unknown model identifier"))?;
    if source.metadata()?.len() != spec.bytes || hash_file(source)? != spec.sha256 {
        return Err(fail("Import checksum mismatch"));
    }
    let mut bytes = Vec::with_capacity(spec.bytes as usize);
    File::open(source)?
        .take(spec.bytes + 1)
        .read_to_end(&mut bytes)?;
    use sha2::{Digest, Sha256};
    let hash: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if bytes.len() as u64 != spec.bytes || hash != spec.sha256 {
        return Err(fail(
            "Model changed during import; existing model preserved",
        ));
    }
    write_atomic(&paths.models.join(format!("{id}.onnx")), &bytes, true)
}
