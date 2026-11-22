use crate::{Result, fail};
use nvsf_core::{ExtractConfig, Project};
use sha2::{Digest, Sha256};
use std::path::Path;

pub(super) struct SourceIdentity {
    pub(super) config: ExtractConfig,
    pub(super) hash: String,
    original_hash: String,
}

fn config_hash(config: &ExtractConfig) -> Result<String> {
    Ok(Sha256::digest(serde_json::to_vec(config)?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

impl SourceIdentity {
    pub(super) fn bind(mut config: ExtractConfig, input: &Path) -> Result<Self> {
        let original_hash = config_hash(&config)?;
        config.input = input
            .to_str()
            .ok_or_else(|| fail("Media input path must be UTF-8"))?
            .into();
        let hash = config_hash(&config)?;
        Ok(Self {
            config,
            hash,
            original_hash,
        })
    }

    pub(super) fn admit(&self, saved: &mut Project, source: &str, model: &str) -> Result<()> {
        if saved.source_hash != source
            || saved.model_hash != model
            || (saved.config_hash != self.hash && saved.config_hash != self.original_hash)
            || config_hash(&saved.config)? != saved.config_hash
        {
            return Err(fail(
                "Checkpoint source/configuration/model mismatch; choose a new output or restore compatible configuration",
            ));
        }
        saved.config = self.config.clone();
        saved.config_hash = self.hash.clone();
        Ok(())
    }
}

#[cfg(test)]
#[path = "extraction_identity_tests.rs"]
mod tests;
