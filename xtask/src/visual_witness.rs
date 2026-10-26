//! Positive native-model admission, not held-out OCR-quality qualification.
use crate::fixture::sha;
use nvsf_app::{Paths, assistant};
use nvsf_core::Rect;
use serde_json::json;
use std::{error::Error, path::Path, sync::atomic::AtomicBool, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn validate(proposal: &assistant::Proposal) -> Result<()> {
    if proposal.text.as_deref() != Some("16")
        || proposal.unreadable
        || !proposal.requires_review
        || proposal.provider != "local-visual-cpu"
    {
        return Err(
            "Visual model did not return the exact synthetic numeric review proposal".into(),
        );
    }
    Ok(())
}

fn hosted() -> Result<()> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
    {
        return Err("Visual model admission requires GitHub-hosted Actions".into());
    }
    Ok(())
}

pub fn provision(data_root: &Path) -> Result<()> {
    hosted()?;
    let paths = Paths::new(Some(data_root))?;
    assistant::install(&paths, &AtomicBool::new(false))?;
    Ok(())
}

pub fn verify(fixture_dir: &Path, data_root: &Path, output: &Path) -> Result<()> {
    hosted()?;
    let source = std::env::var("GITHUB_SHA")?;
    let target = std::env::var("NVSF_TARGET_ID")?;
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return Err("Unsupported native visual witness architecture".into()),
    };
    if nvsf_core::BUILD_SOURCE_SHA != Some(source.as_str())
        || target != format!("{}-{architecture}", std::env::consts::OS)
    {
        return Err("Visual admission source/target identity mismatch".into());
    }
    let work = Path::new(".ci-work").canonicalize()?;
    if output.exists()
        || output.is_symlink()
        || output
            .parent()
            .ok_or("Missing visual evidence parent")?
            .canonicalize()?
            != work
    {
        return Err("Visual evidence needs a new directory directly under owned CI work".into());
    }
    std::fs::create_dir(output)?;
    let marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture_dir.join("CORPUS_STATUS.json"))?)?;
    if marker["synthetic"] != true || marker["language"] != "en" {
        return Err("Only the declared generated English fixture is admitted".into());
    }
    let paths = Paths::new(Some(data_root))?;
    let input = fixture_dir.join("en.mp4").canonicalize()?;
    let original = sha(&input)?;
    let cancel = AtomicBool::new(false);
    eprintln!(
        "[INFO] [VISUAL-WITNESS] Running actual numeric crop through the supervised native model"
    );
    let started = Instant::now();
    let mut report = json!({
        "schema":1,"source_sha":source,"target":target,"passed":false,
        "tests":1,"failed":1,"skipped":0,"synthetic":true,
        "expected":"16","at_ms":5500,"source_sha256":original,
        "real_corpus_qualified":false,"accelerator_qualified":false
    });
    let persist = |record: &serde_json::Value| -> Result<()> {
        std::fs::write(
            output.join("visual-model-report.json"),
            serde_json::to_vec_pretty(record)?,
        )?;
        Ok(())
    };
    persist(&report)?;
    let result = (|| -> Result<()> {
        let proposal = assistant::propose(
            &paths,
            &input,
            5500,
            Rect::new(0.3, 0.72, 0.4, 0.22)?,
            true,
            &cancel,
        )?;
        report["actual"] = json!(proposal.text);
        report["requires_review"] = json!(proposal.requires_review);
        report["provider"] = json!(proposal.provider);
        persist(&report)?;
        validate(&proposal)?;
        if sha(&input)? != original || std::fs::read_dir(&paths.cache)?.next().is_some() {
            return Err(
                "Visual inference changed source bytes or retained private crop scratch".into(),
            );
        }
        report["source_unchanged"] = json!(true);
        report["scratch_clean"] = json!(true);
        report["passed"] = json!(true);
        report["failed"] = json!(0);
        Ok(())
    })();
    report["elapsed_ms"] = json!(started.elapsed().as_millis());
    if let Err(error) = &result {
        report["error"] = json!(error.to_string());
    }
    persist(&report)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_admission_requires_literal_review_only_model_output() {
        let good = assistant::validate_proposal("16").unwrap();
        validate(&good).unwrap();
        for text in ["", "sixteen", "161", "Hello world", "unreadable"] {
            assert!(validate(&assistant::validate_proposal(text).unwrap()).is_err());
        }
        let mut accepted = assistant::validate_proposal("16").unwrap();
        accepted.requires_review = false;
        assert!(validate(&accepted).is_err());
    }
}
