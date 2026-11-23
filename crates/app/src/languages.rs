use crate::{
    Paths, Result, fail,
    storage::{AtomicOutput, JobLock, hash_file, hash_reader},
};
use nvsf_core::languages::{self, Language};
use serde::Serialize;
use std::{
    ffi::OsStr,
    fs::File,
    io::{Read, Seek},
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
    prepare_import(paths, code, source, cancel)?
        .0
        .publish(cancel)
}

pub struct PreparedLanguageImport {
    language: Language,
    source: File,
    output: AtomicOutput,
    lock: JobLock,
}

pub fn prepare_import(
    paths: &Paths,
    code: &str,
    source: &Path,
    cancel: &AtomicBool,
) -> Result<(PreparedLanguageImport, bool)> {
    prepare_import_with_spec(paths, spec(code)?.clone(), source, cancel)
}

fn prepare_import_with_spec(
    paths: &Paths,
    language: Language,
    source: &Path,
    cancel: &AtomicBool,
) -> Result<(PreparedLanguageImport, bool)> {
    if cancel.load(Ordering::Acquire) {
        return Err(crate::AppError::Cancelled);
    }
    if !source.is_file() {
        return Err(fail("Language import size or checksum mismatch"));
    }
    let source = File::open(source)?;
    if !source.metadata()?.is_file() {
        return Err(fail("Language import size or checksum mismatch"));
    }
    let root = paths.models.join("tessdata");
    std::fs::create_dir_all(&root)?;
    let destination = root.join(format!("{}.traineddata", language.code));
    let (output, exists) = AtomicOutput::for_confirmation(&destination)?;
    let lock = output.lock_sibling(OsStr::new(&format!("{}.download.lock", language.code)))?;
    Ok((
        PreparedLanguageImport {
            language,
            source,
            output,
            lock,
        },
        exists,
    ))
}

impl PreparedLanguageImport {
    pub fn publish(self, cancel: &AtomicBool) -> Result<()> {
        let Self {
            language,
            mut source,
            output,
            lock,
        } = self;
        if cancel.load(Ordering::Acquire) {
            return Err(crate::AppError::Cancelled);
        }
        source.rewind()?;
        if source.metadata()?.len() != language.bytes
            || hash_reader((&mut source).take(language.bytes + 1))? != language.sha256
        {
            return Err(fail("Language import size or checksum mismatch"));
        }
        source.rewind()?;
        let result = output.write_with(|temporary| {
            std::io::copy(&mut (&mut source).take(language.bytes + 1), &mut *temporary)?;
            temporary.sync_all()?;
            if cancel.load(Ordering::Acquire) {
                return Err(crate::AppError::Cancelled);
            }
            temporary.rewind()?;
            if temporary.metadata()?.len() != language.bytes
                || hash_reader((&mut *temporary).take(language.bytes + 1))? != language.sha256
            {
                return Err(fail(
                    "Language pack changed during import; previous pack retained",
                ));
            }
            if cancel.load(Ordering::Acquire) {
                return Err(crate::AppError::Cancelled);
            }
            Ok(())
        });
        drop(lock);
        result
    }
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
    fingerprint_with_policy(paths, code, false)
}

pub fn fingerprint_with_policy(
    paths: &Paths,
    code: &str,
    required_acceleration: bool,
) -> Result<String> {
    let language = spec(code)?;
    let paddle = languages::paddle_model(code);
    if required_acceleration && paddle.is_none() {
        return Err(crate::AppError::Provider(
            "This language uses Tesseract CPU recognition; required acceleration cannot be met"
                .into(),
        ));
    }
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
    if required_acceleration {
        identities.push(format!("{}:verifier-disabled-by-policy", language.code));
    } else if verify(paths, code).is_ok() {
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

#[cfg(test)]
#[path = "languages_import_tests.rs"]
mod import_tests;
