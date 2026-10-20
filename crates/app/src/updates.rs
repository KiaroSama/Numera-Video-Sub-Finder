use crate::{Paths, Result, fail, storage::write_atomic};
use nvsf_core::maintenance::{Preferences, newer_stable};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Read, time::Duration};

pub const REPOSITORY: &str = "https://github.com/KiaroSama/Numera-Video-Sub-Finder";
pub const FEED: &str =
    "https://github.com/KiaroSama/Numera-Video-Sub-Finder/releases/latest/download/latest.json";
pub const MAX_UPDATE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub url: String,
    pub signature: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub platforms: BTreeMap<String, Artifact>,
}

pub fn preferences(paths: &Paths) -> Result<Preferences> {
    let path = paths.root.join("updates.json");
    match std::fs::File::open(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Preferences::default()),
        Err(e) => Err(e.into()),
        Ok(file) => {
            if path.is_symlink() || file.metadata()?.len() > 4096 {
                return Err(fail("Update preferences exceed limits or use a symlink"));
            }
            let value: Preferences = serde_json::from_reader(file.take(4097))?;
            if value.schema != 1 {
                return Err(fail("Unsupported update preferences schema"));
            }
            Ok(value)
        }
    }
}

pub fn save_preferences(paths: &Paths, value: &Preferences) -> Result<()> {
    let path = paths.root.join("updates.json");
    if path.is_symlink() {
        return Err(fail("Update preferences must not be a symlink"));
    }
    write_atomic(&path, &serde_json::to_vec(value)?, true)
}

pub fn official_asset(url: &str) -> Result<()> {
    let url = reqwest::Url::parse(url).map_err(|_| fail("Invalid official release URL"))?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url
            .path()
            .starts_with("/KiaroSama/Numera-Video-Sub-Finder/releases/download/")
        || url.path().contains("%")
        || url.path().contains("..")
    {
        return Err(fail(
            "Update payload is not an official project release asset",
        ));
    }
    let parts: Vec<_> = url.path().trim_start_matches('/').split('/').collect();
    if parts.len() != 6 || parts[4].is_empty() || parts[5].is_empty() {
        return Err(fail(
            "Release asset must include exactly one tag and filename",
        ));
    }
    Ok(())
}

pub fn verify_signature(bytes: &[u8], signature: &[u8]) -> Result<()> {
    if bytes.len() > 1024 * 1024 {
        return Err(fail("Signed metadata exceeds limits"));
    }
    let (key, signature) = signature_parts(signature)?;
    key.verify(bytes, &signature, false)
        .map_err(|_| fail("Release metadata signature verification failed"))
}

fn signature_parts(
    signature: &[u8],
) -> Result<(minisign_verify::PublicKey, minisign_verify::Signature)> {
    use base64::Engine;
    if signature.len() > 4096 {
        return Err(fail("Release signature exceeds limits"));
    }
    let decode = |text: &[u8]| -> Result<String> {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(text.trim_ascii())
            .map_err(|_| fail("Invalid signed metadata encoding"))?;
        String::from_utf8(decoded).map_err(|_| fail("Invalid signed metadata text"))
    };
    let key = decode(
        include_str!("../../../ci/updater-public.key")
            .trim()
            .as_bytes(),
    )?;
    let signature = decode(signature)?;
    let key = minisign_verify::PublicKey::decode(&key)
        .map_err(|_| fail("Invalid embedded release trust root"))?;
    let signature = minisign_verify::Signature::decode(&signature)
        .map_err(|_| fail("Invalid release signature"))?;
    Ok((key, signature))
}

pub fn verify_artifact_file(path: &std::path::Path, signature: &[u8], version: &str) -> Result<()> {
    let (key, signature) = signature_parts(signature)?;
    let metadata = std::fs::symlink_metadata(path)?;
    let length = metadata.len();
    if !metadata.is_file() || length == 0 || length > MAX_UPDATE_BYTES {
        return Err(fail("Signed artifact must be a bounded regular file"));
    }
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() != length {
        return Err(fail("Signed artifact changed before inspection"));
    }
    let mut verifier = key
        .verify_stream(&signature)
        .map_err(|_| fail("Artifact signature algorithm refused"))?;
    let mut file = file.take(length + 1);
    let mut buffer = [0u8; 65536];
    let mut received = 0u64;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(fail("Artifact signature inspection exceeded deadline"));
        }
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        received += count as u64;
        verifier.update(&buffer[..count]);
    }
    if received != length {
        return Err(fail("Artifact changed during signature inspection"));
    }
    verifier
        .finalize()
        .map_err(|_| fail("Artifact signature verification failed"))?;
    let versions: Vec<_> = signature
        .trusted_comment()
        .split('\t')
        .filter_map(|part| part.strip_prefix("version:"))
        .collect();
    if versions != [version] {
        return Err(fail(
            "Signed artifact version differs from release inventory",
        ));
    }
    Ok(())
}

pub fn manifest(bytes: &[u8], current: &str, target: &str) -> Result<Option<Manifest>> {
    if bytes.len() > 1024 * 1024 {
        return Err(fail("Update metadata exceeds 1MiB"));
    }
    let value: Manifest = serde_json::from_slice(bytes)?;
    if value.notes.len() > 16 * 1024 || value.platforms.len() > 48 {
        return Err(fail("Update metadata exceeds catalog limits"));
    }
    if !newer_stable(current, &value.version)? {
        return Ok(None);
    }
    let artifact = value
        .platforms
        .get(target)
        .ok_or_else(|| fail("No matching target/format update asset"))?;
    validate_artifact(artifact)?;
    Ok(Some(value))
}

pub fn validate_artifact(artifact: &Artifact) -> Result<()> {
    official_asset(&artifact.url)?;
    if artifact.bytes == 0
        || artifact.bytes > MAX_UPDATE_BYTES
        || artifact.sha256.len() != 64
        || !artifact.sha256.bytes().all(|c| c.is_ascii_hexdigit())
        || artifact.signature.is_empty()
        || artifact.signature.len() > 4096
    {
        return Err(fail("Invalid signed update artifact contract"));
    }
    Ok(())
}

pub fn client(timeout: Duration) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .user_agent(concat!(
            "Numera-Video-Sub-Finder/",
            env!("CARGO_PKG_VERSION")
        ))
        .redirect(redirect_policy())
        .build()
        .map_err(|e| fail(e.to_string()))
}

pub(crate) fn metadata(response: reqwest::blocking::Response, limit: u64) -> Result<Vec<u8>> {
    read_metadata(
        response,
        limit,
        std::time::Instant::now() + Duration::from_secs(20),
    )
}

fn read_metadata(response: impl Read, limit: u64, deadline: std::time::Instant) -> Result<Vec<u8>> {
    let mut response = response.take(limit + 1);
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(fail("Release metadata exceeded transfer deadline"));
        }
        let count = response.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() as u64 > limit {
            return Err(fail("Release metadata exceeds byte budget"));
        }
    }
    Ok(bytes)
}

fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let url = attempt.url();
        if attempt.previous().len() >= 4
            || url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || !matches!(
                url.host_str().unwrap_or_default(),
                "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        {
            attempt.error("Release redirect refused")
        } else {
            attempt.follow()
        }
    })
}

async fn async_metadata(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(fail("Release metadata exceeds byte budget"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| fail(e.to_string()))? {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(fail("Release metadata exceeds byte budget"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub async fn check(current: &str, target: &str) -> Result<Option<Manifest>> {
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .user_agent(concat!(
            "Numera-Video-Sub-Finder/",
            env!("CARGO_PKG_VERSION")
        ))
        .redirect(redirect_policy())
        .build()
        .map_err(|e| fail(e.to_string()))?;
    let response = client
        .get(FEED)
        .send()
        .await
        .map_err(|e| fail(format!("Update check unavailable: {e}")))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let response = response
        .error_for_status()
        .map_err(|e| fail(format!("Update metadata unavailable: {e}")))?;
    let bytes = async_metadata(response, 1024 * 1024).await?;
    let response = client
        .get(concat!(
            "https://github.com/KiaroSama/Numera-Video-Sub-Finder/releases/latest/download/",
            "latest.json.sig"
        ))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| fail(format!("Signed update metadata unavailable: {e}")))?;
    let signature = async_metadata(response, 4096).await?;
    verify_signature(&bytes, &signature)?;
    manifest(&bytes, current, target)
}

pub fn target(format: &str) -> String {
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else {
        std::env::consts::OS
    };
    format!("{os}-{}-{format}", std::env::consts::ARCH)
}

#[cfg(test)]
mod transfer_tests {
    use super::*;
    use std::{
        io::{self, Cursor},
        time::Instant,
    };

    #[test]
    fn release_metadata_reads_are_bounded_and_fail_on_transport_errors() {
        let deadline = || Instant::now() + Duration::from_secs(2);
        assert_eq!(
            read_metadata(Cursor::new(b"{}"), 2, deadline()).unwrap(),
            b"{}"
        );
        assert!(
            read_metadata(Cursor::new(b"{}x"), 2, deadline())
                .unwrap_err()
                .to_string()
                .contains("byte budget")
        );
        assert!(
            read_metadata(Cursor::new(b"{}"), 2, Instant::now())
                .unwrap_err()
                .to_string()
                .contains("transfer deadline")
        );
        struct FailedRead;
        impl Read for FailedRead {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "controlled metadata timeout",
                ))
            }
        }
        assert!(
            matches!(read_metadata(FailedRead, 2, deadline()), Err(crate::AppError::Io(e)) if e.kind() == io::ErrorKind::TimedOut)
        );
    }
}
