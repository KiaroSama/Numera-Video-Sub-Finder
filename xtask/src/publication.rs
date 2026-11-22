//! Binds publication to a completed verification run and the requested release channel.

use serde_json::Value;
use std::{error::Error, path::Path};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn verify(
    reports: &Path,
    source: &str,
    evidence: &Path,
    repository: &str,
    publish: bool,
    prerelease: bool,
    tag: &str,
) -> Result<()> {
    if evidence.metadata()?.len() > 2 * 1024 * 1024 {
        return Err("Verification run metadata exceeds its byte bound".into());
    }
    let run: Value = serde_json::from_slice(&std::fs::read(evidence)?)?;
    verify_run(&run, source, repository)?;
    if publish && tag != format!("v{}", env!("CARGO_PKG_VERSION")) {
        return Err("Release tag must match the verified workspace version".into());
    }
    // Only an explicitly requested development prerelease may omit full qualification.
    // A qualification-only run remains strict even when its prerelease input is true.
    if publish && prerelease {
        crate::gate::validate(reports, source)?;
        eprintln!(
            "[{}] [WARNING] [RELEASE] Development prerelease only; full product qualification is not established",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        );
    } else {
        crate::gate::release(reports, source)?;
    }
    Ok(())
}

fn verify_run(run: &Value, source: &str, repository: &str) -> Result<()> {
    if source.len() != 40
        || !source.bytes().all(|c| c.is_ascii_hexdigit())
        || repository.is_empty()
        || run["head_sha"] != source
        || run["repository"]["full_name"] != repository
        || run["head_repository"]["full_name"] != repository
        || run["path"] != ".github/workflows/verify.yml"
        || run["event"] != "workflow_dispatch"
        || run["status"] != "completed"
        || run["conclusion"] != "success"
        || run["id"].as_u64().is_none_or(|id| id == 0)
    {
        return Err(
            "Release requires a successful exact-source Final verification run in this repository"
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "publication_tests.rs"]
mod tests;
