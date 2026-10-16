use crate::{Paths, Result, fail, storage::hash_file};
use nvsf_core::languages::{self, Language};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Serialize)]
pub struct LanguageStatus {
    pub code: String,
    pub name: String,
    pub installed: bool,
    pub verified: bool,
    pub bytes: u64,
    pub primary: String,
    pub message: String,
}

fn spec(code: &str) -> Result<&'static Language> {
    languages::find(code).ok_or_else(|| fail("Select a language from the official catalog"))
}

pub fn verify(paths: &Paths, code: &str) -> Result<PathBuf> {
    let language = spec(code)?;
    let path = paths
        .models
        .join("tessdata")
        .join(format!("{}.traineddata", language.code));
    if path.is_symlink() || !path.is_file() {
        return Err(fail(format!(
            "Language pack {} is missing; explicitly install or import it",
            language.code
        )));
    }
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(paths.models.canonicalize()?)
        || canonical.metadata()?.len() != language.bytes
        || hash_file(&canonical)? != language.sha256
    {
        return Err(fail("Language pack integrity or containment check failed"));
    }
    Ok(canonical)
}

pub fn catalog(paths: &Paths) -> Vec<LanguageStatus> {
    languages::catalog()
        .iter()
        .map(|language| {
            let installed = paths
                .models
                .join("tessdata")
                .join(format!("{}.traineddata", language.code))
                .is_file();
            let verified = installed && verify(paths, &language.code).is_ok();
            LanguageStatus {
                code: language.code.clone(),
                name: language.name.clone(),
                installed,
                verified,
                bytes: language.bytes,
                primary: languages::paddle_model(&language.code)
                    .map_or_else(|| "Tesseract CPU".into(), |id| format!("Paddle {id}")),
                message: if verified {
                    "Hash verified; subtitle quality not established"
                } else if installed {
                    "Pack needs integrity verification"
                } else {
                    "Explicit installation required; subtitle quality unmeasured"
                }
                .into(),
            }
        })
        .collect()
}

pub fn install(paths: &Paths, code: &str, cancel: &AtomicBool) -> Result<()> {
    let language = spec(code)?;
    let root = paths.models.join("tessdata");
    std::fs::create_dir_all(&root)?;
    crate::model_download::install(
        &language.url,
        language.bytes,
        &language.sha256,
        &root.join(format!("{}.traineddata", language.code)),
        cancel,
    )
}

pub fn import(paths: &Paths, code: &str, source: &Path, cancel: &AtomicBool) -> Result<()> {
    let language = spec(code)?;
    if cancel.load(Ordering::Acquire) {
        return Err(crate::AppError::Cancelled);
    }
    if !source.is_file()
        || source.metadata()?.len() != language.bytes
        || hash_file(source)? != language.sha256
    {
        return Err(fail("Language import size or checksum mismatch"));
    }
    let root = paths.models.join("tessdata");
    std::fs::create_dir_all(&root)?;
    let destination = root.join(format!("{}.traineddata", language.code));
    let _lock = crate::storage::JobLock::acquire(&destination.with_extension("download.lock"))?;
    if destination.is_symlink() {
        return Err(fail("Language destination must not be a symlink"));
    }
    let mut temporary = tempfile::NamedTempFile::new_in(&root)?;
    use std::io::Read;
    std::io::copy(
        &mut std::fs::File::open(source)?.take(language.bytes + 1),
        &mut temporary,
    )?;
    temporary.as_file().sync_all()?;
    if cancel.load(Ordering::Acquire) {
        return Err(crate::AppError::Cancelled);
    }
    if temporary.as_file().metadata()?.len() != language.bytes
        || hash_file(temporary.path())? != language.sha256
    {
        return Err(fail(
            "Language pack changed during import; previous pack retained",
        ));
    }
    temporary.persist(destination).map_err(|e| e.error)?;
    Ok(())
}

pub fn export(paths: &Paths, code: &str, destination: &Path) -> Result<()> {
    let source = verify(paths, code)?;
    if destination.exists() || destination.is_symlink() {
        return Err(fail("Language export destination already exists"));
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    std::io::copy(&mut std::fs::File::open(source)?, &mut temporary)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(destination)
        .map_err(|e| e.error)?;
    Ok(())
}

pub fn fingerprint(paths: &Paths, code: &str) -> Result<String> {
    let language = spec(code)?;
    let mut identities = Vec::new();
    for id in [Some("det"), languages::paddle_model(code)]
        .into_iter()
        .flatten()
    {
        let model = crate::models::specs()?
            .into_iter()
            .find(|m| m.id == id)
            .ok_or_else(|| fail("Recognition model identity missing"))?;
        identities.push(format!("{id}:{}:{}", model.sha256, model.metadata_sha256));
    }
    if verify(paths, code).is_ok() {
        identities.push(format!("{}:{}", language.code, language.sha256));
    } else if languages::paddle_model(code).is_none() {
        verify(paths, code)?;
    } else {
        identities.push(format!("{}:verifier-unavailable", language.code));
    }
    if language.code == "jpn" && crate::models::verify(paths, "ja-specialist").is_ok() {
        let model = crate::models::specs()?
            .into_iter()
            .find(|m| m.id == "ja-specialist")
            .ok_or_else(|| fail("Specialist identity missing"))?;
        identities.push(format!(
            "ja-specialist:{}:{}",
            model.sha256, model.metadata_sha256
        ));
    }
    Ok(identities.join(":"))
}
