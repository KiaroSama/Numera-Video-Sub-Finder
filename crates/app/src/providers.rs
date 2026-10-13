use crate::{AppError, Result, fail};
use ort::{environment::Environment, session::Session};
use std::path::Path;

pub fn session(
    env: &Environment,
    path: &Path,
    requested: &str,
    threads: usize,
    required: bool,
) -> Result<(Session, String)> {
    let selected = if requested == "auto" {
        if cfg!(feature = "cuda") {
            "cuda"
        } else if cfg!(feature = "coreml") {
            "coreml"
        } else if cfg!(feature = "directml") {
            "directml"
        } else {
            "cpu"
        }
    } else {
        requested
    };
    if required && selected == "cpu" {
        return Err(AppError::Provider(
            "Required acceleration unavailable; this package selects CPU".into(),
        ));
    }
    let attempt = build(env, path, selected, threads, required);
    match attempt {
        Ok(s) => Ok((s, selected.into())),
        Err(e) if requested == "auto" && !required => {
            eprintln!("WARNING: {selected} unavailable ({e}); using CPU");
            Ok((build(env, path, "cpu", threads, false)?, "cpu".into()))
        }
        Err(e) => Err(e),
    }
}

fn build(
    env: &Environment,
    path: &Path,
    provider: &str,
    threads: usize,
    required: bool,
) -> Result<Session> {
    let mut builder = Session::builder(env)
        .map_err(|e| fail(e.to_string()))?
        .with_intra_threads(threads)
        .map_err(|e| fail(e.to_string()))?;
    match provider {
        "cpu" => {}
        #[cfg(feature = "cuda")]
        "cuda" => {
            builder = builder
                .with_execution_providers([ort::ep::CUDA::default().build().error_on_failure()])
                .map_err(|e| AppError::Provider(e.to_string()))?;
        }
        #[cfg(feature = "directml")]
        "directml" => {
            builder = builder
                .with_parallel_execution(false)
                .map_err(|e| fail(e.to_string()))?
                .with_memory_pattern(false)
                .map_err(|e| fail(e.to_string()))?
                .with_execution_providers([ort::ep::DirectML::default().build().error_on_failure()])
                .map_err(|e| AppError::Provider(e.to_string()))?;
        }
        #[cfg(feature = "coreml")]
        "coreml" => {
            if required {
                return Err(AppError::Provider(
                    "CoreML OS-managed placement cannot establish strict GPU-only execution".into(),
                ));
            }
            builder = builder
                .with_execution_providers([ort::ep::CoreML::default().build().error_on_failure()])
                .map_err(|e| AppError::Provider(e.to_string()))?;
        }
        _ => {
            return Err(AppError::Provider(format!(
                "{provider} was not enabled for this native runtime package"
            )));
        }
    }
    if required {
        builder = builder
            .with_disable_cpu_fallback()
            .map_err(|e| fail(e.to_string()))?;
    }
    builder
        .commit_from_file(path)
        .map_err(|e| AppError::Provider(e.to_string()))
}
