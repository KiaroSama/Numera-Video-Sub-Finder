use crate::{PreferencesCommand, UpdatesCommand};
use nvsf_app::{AppError, Control, Paths, Result, storage, update_payload, updates};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const OFFER_CACHE_FILE: &str = "cli-update-offer.json";
const OFFER_CACHE_LIMIT: u64 = 1024 * 1024;
const SIGNATURE_FEED: &str =
    "https://github.com/KiaroSama/Numera-Video-Sub-Finder/releases/latest/download/latest.json.sig";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedOffer {
    schema: u32,
    token: String,
    version: String,
    notes: String,
    target: String,
    artifact: updates::Artifact,
    checked_at: u64,
    downloaded_path: Option<PathBuf>,
}

pub(crate) fn run_preferences(paths: &Paths, command: PreferencesCommand) -> Result<()> {
    match command {
        PreferencesCommand::Get => {
            let prefs = updates::preferences(paths)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "schema": prefs.schema,
                    "update_checks_enabled": prefs.enabled,
                    "last_update_attempt": prefs.last_attempt,
                    "data_root": paths.root,
                }))?
            );
            Ok(())
        }
        PreferencesCommand::Set {
            update_checks,
            reset_last_attempt,
        } => {
            if update_checks.is_none() && !reset_last_attempt {
                return Err(AppError::InvalidConfig(
                    "preferences set requires --update-checks or --reset-last-attempt".into(),
                ));
            }
            let mut prefs = updates::preferences(paths)?;
            if let Some(enabled) = update_checks {
                prefs.enabled = enabled;
            }
            if reset_last_attempt {
                prefs.last_attempt = None;
            }
            updates::save_preferences(paths, &prefs)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "schema": prefs.schema,
                    "update_checks_enabled": prefs.enabled,
                    "last_update_attempt": prefs.last_attempt,
                }))?
            );
            Ok(())
        }
    }
}

pub(crate) fn run_updates(paths: &Paths, command: UpdatesCommand, control: &Control) -> Result<()> {
    match command {
        UpdatesCommand::Status { format } => {
            validate_format(&format)?;
            let prefs = updates::preferences(paths)?;
            let target = updates::target(&format);
            let offer = load_cached_offer(paths).ok().flatten();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "current_version": env!("CARGO_PKG_VERSION"),
                    "target": target,
                    "enabled": prefs.enabled,
                    "last_attempt": prefs.last_attempt,
                    "offer": offer,
                }))?
            );
            Ok(())
        }
        UpdatesCommand::Config { enabled } => {
            let mut prefs = updates::preferences(paths)?;
            prefs.enabled = enabled;
            updates::save_preferences(paths, &prefs)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "schema": prefs.schema,
                    "enabled": prefs.enabled,
                    "last_attempt": prefs.last_attempt,
                }))?
            );
            Ok(())
        }
        UpdatesCommand::Check { format } => check_updates(paths, &format, control),
        UpdatesCommand::Download {
            token,
            consent,
            output,
            overwrite,
        } => download_update(
            paths,
            &token,
            consent,
            output.as_deref(),
            overwrite,
            control,
        ),
        UpdatesCommand::Verify {
            token,
            file,
            signature,
            version,
            sha256,
            target,
        } => verify_update(
            paths,
            token.as_deref(),
            file.as_deref(),
            signature.as_deref(),
            version.as_deref(),
            sha256.as_deref(),
            target.as_deref(),
        ),
    }
}

pub(crate) fn validate_format(format: &str) -> Result<()> {
    if !matches!(format, "portable" | "msi" | "appimage" | "deb" | "rpm") {
        return Err(AppError::InvalidConfig(
            "Update format must be portable, msi, appimage, deb or rpm".into(),
        ));
    }
    Ok(())
}

fn check_updates(paths: &Paths, format: &str, control: &Control) -> Result<()> {
    validate_format(format)?;
    if control.cancel.load(Ordering::Acquire) {
        return Err(AppError::Cancelled);
    }
    let current = env!("CARGO_PKG_VERSION");
    let target = updates::target(format);
    let client = updates::client(Duration::from_secs(20))?;
    let mut prefs = updates::preferences(paths)?;
    prefs.last_attempt = Some(now_secs());
    updates::save_preferences(paths, &prefs)?;

    let response = client
        .get(updates::FEED)
        .send()
        .map_err(|e| nvsf_app::fail(format!("Update check unavailable: {e}")))?;
    if response.status().as_u16() == 404 {
        clear_cached_offer(paths);
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "current_version": current,
                "target": target,
                "update_available": false,
                "state": "up_to_date",
                "token": null,
                "version": null,
            }))?
        );
        return Ok(());
    }
    let response = response
        .error_for_status()
        .map_err(|e| nvsf_app::fail(format!("Update metadata unavailable: {e}")))?;
    let bytes = read_bounded_response(response, 1024 * 1024, control)?;

    let sig_response = client
        .get(SIGNATURE_FEED)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| nvsf_app::fail(format!("Signed update metadata unavailable: {e}")))?;
    let signature = read_bounded_response(sig_response, 4096, control)?;
    updates::verify_signature(&bytes, &signature)?;

    match updates::manifest(&bytes, current, &target)? {
        None => {
            clear_cached_offer(paths);
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "current_version": current,
                    "target": target,
                    "update_available": false,
                    "state": "up_to_date",
                    "token": null,
                    "version": null,
                }))?
            );
        }
        Some(manifest) => {
            let artifact = manifest
                .platforms
                .get(&target)
                .cloned()
                .ok_or_else(|| nvsf_app::fail("No matching target/format update asset"))?;
            let token = format!("upd-{}-{}", manifest.version, now_secs());
            let offer = CachedOffer {
                schema: 1,
                token: token.clone(),
                version: manifest.version.clone(),
                notes: manifest.notes.clone(),
                target: target.clone(),
                artifact: artifact.clone(),
                checked_at: now_secs(),
                downloaded_path: None,
            };
            save_cached_offer(paths, &offer)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "current_version": current,
                    "target": target,
                    "update_available": true,
                    "state": "available",
                    "token": token,
                    "version": manifest.version,
                    "notes": manifest.notes,
                    "artifact": artifact,
                }))?
            );
        }
    }
    Ok(())
}

fn download_update(
    paths: &Paths,
    token: &str,
    consent: bool,
    output: Option<&Path>,
    overwrite: bool,
    control: &Control,
) -> Result<()> {
    if !consent {
        return Err(AppError::InvalidConfig(
            "Update download requires explicit --consent".into(),
        ));
    }
    let mut offer = load_cached_offer(paths)?
        .filter(|o| o.token == token)
        .ok_or_else(|| {
            nvsf_app::fail("Update offer token not found or expired; run updates check")
        })?;
    updates::validate_artifact(&offer.artifact)?;
    updates::official_asset(&offer.artifact.url)?;

    let updates_dir = paths.cache.join("updates");
    std::fs::create_dir_all(&updates_dir)?;
    let filename = offer
        .artifact
        .url
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty() && !n.contains(".."))
        .unwrap_or("update-artifact.bin");
    let dest = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| updates_dir.join(filename));
    if dest.exists() && !overwrite {
        return Err(nvsf_app::fail(
            "Update destination exists; pass --overwrite explicitly",
        ));
    }

    let staging = updates_dir.join(format!("staging-{}-{}.tmp", std::process::id(), now_secs()));
    let result = (|| -> Result<String> {
        let client = updates::client(Duration::from_secs(20))?;
        let mut response = client
            .get(&offer.artifact.url)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| nvsf_app::fail(format!("Update download failed: {e}")))?;
        if response
            .content_length()
            .is_some_and(|n| n != offer.artifact.bytes)
        {
            return Err(nvsf_app::fail(
                "Update payload size differs from release inventory",
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(600);
        let mut file = std::fs::File::create(&staging)?;
        let mut received = 0u64;
        let mut buf = [0u8; 65_536];
        loop {
            if Instant::now() >= deadline {
                return Err(nvsf_app::fail(
                    "Update transfer exceeded 600-second deadline",
                ));
            }
            if control.cancel.load(Ordering::Acquire) {
                return Err(AppError::Cancelled);
            }
            let n = response.read(&mut buf)?;
            if n == 0 {
                break;
            }
            received = received
                .checked_add(n as u64)
                .ok_or_else(|| nvsf_app::fail("Update size overflow"))?;
            if received > offer.artifact.bytes || received > updates::MAX_UPDATE_BYTES {
                return Err(nvsf_app::fail("Update exceeds declared payload size"));
            }
            file.write_all(&buf[..n])?;
        }
        file.sync_all()?;
        drop(file);
        if received != offer.artifact.bytes {
            return Err(nvsf_app::fail("Downloaded update payload truncated"));
        }
        let digest = storage::hash_file(&staging)?;
        if !digest.eq_ignore_ascii_case(&offer.artifact.sha256) {
            return Err(nvsf_app::fail("Update payload SHA-256 verification failed"));
        }
        updates::verify_artifact_file(
            &staging,
            offer.artifact.signature.as_bytes(),
            &offer.version,
        )?;
        let payload_bytes = std::fs::read(&staging)?;
        update_payload::validate(&payload_bytes, &offer.target)?;
        storage::write_atomic(&dest, &payload_bytes, overwrite)?;
        Ok(digest)
    })();
    let _ = std::fs::remove_file(&staging);
    let digest = result?;
    offer.downloaded_path = Some(dest.clone());
    save_cached_offer(paths, &offer)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "token": offer.token,
            "version": offer.version,
            "target": offer.target,
            "path": dest,
            "bytes": offer.artifact.bytes,
            "sha256": digest,
            "verified": true,
        }))?
    );
    Ok(())
}

fn verify_update(
    paths: &Paths,
    token: Option<&str>,
    file: Option<&Path>,
    signature: Option<&str>,
    version: Option<&str>,
    sha256: Option<&str>,
    target: Option<&str>,
) -> Result<()> {
    let (artifact_path, sig_bytes, ver, expected_sha, expected_target) = if let Some(tok) = token {
        let offer = load_cached_offer(paths)?
            .filter(|o| o.token == tok)
            .ok_or_else(|| nvsf_app::fail("Cached update token not found"))?;
        let path = file
            .map(Path::to_path_buf)
            .or(offer.downloaded_path)
            .ok_or_else(|| {
                nvsf_app::fail("No downloaded file for token; run updates download or pass --file")
            })?;
        (
            path,
            offer.artifact.signature.into_bytes(),
            offer.version,
            Some(offer.artifact.sha256),
            Some(offer.target),
        )
    } else {
        let path = file
            .ok_or_else(|| {
                AppError::InvalidConfig("Pass --token or --file/--signature/--version".into())
            })?
            .to_path_buf();
        let sig_arg = signature
            .ok_or_else(|| AppError::InvalidConfig("--signature is required with --file".into()))?;
        let ver = version
            .ok_or_else(|| AppError::InvalidConfig("--version is required with --file".into()))?
            .to_owned();
        let sig_bytes = if Path::new(sig_arg).is_file() {
            std::fs::read(sig_arg)?
        } else {
            sig_arg.as_bytes().to_vec()
        };
        (
            path,
            sig_bytes,
            ver,
            sha256.map(str::to_owned),
            target.map(str::to_owned),
        )
    };

    let digest = storage::hash_file(&artifact_path)?;
    if let Some(expected) = expected_sha.as_deref()
        && !digest.eq_ignore_ascii_case(expected)
    {
        return Err(nvsf_app::fail("Update payload SHA-256 verification failed"));
    }
    updates::verify_artifact_file(&artifact_path, &sig_bytes, &ver)?;
    if let Some(tgt) = expected_target.as_deref() {
        let bytes = std::fs::read(&artifact_path)?;
        update_payload::validate(&bytes, tgt)?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "path": artifact_path,
            "version": ver,
            "sha256": digest,
            "target": expected_target,
            "verified": true,
        }))?
    );
    Ok(())
}

fn read_bounded_response(
    mut response: impl Read,
    limit: u64,
    control: &Control,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        if control.cancel.load(Ordering::Acquire) {
            return Err(AppError::Cancelled);
        }
        let n = response.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if (out.len() + n) as u64 > limit {
            return Err(nvsf_app::fail("Release metadata exceeds byte budget"));
        }
        out.extend_from_slice(&buf[..n]);
    }
    Ok(out)
}

fn load_cached_offer(paths: &Paths) -> Result<Option<CachedOffer>> {
    let path = paths.cache.join(OFFER_CACHE_FILE);
    if !path.exists() {
        return Ok(None);
    }
    if path.is_symlink() || path.metadata()?.len() > OFFER_CACHE_LIMIT {
        return Err(nvsf_app::fail(
            "Cached update offer is invalid or oversized",
        ));
    }
    let offer: CachedOffer = serde_json::from_slice(&std::fs::read(&path)?)?;
    if offer.schema != 1 {
        return Err(nvsf_app::fail("Unsupported cached update offer schema"));
    }
    Ok(Some(offer))
}

fn save_cached_offer(paths: &Paths, offer: &CachedOffer) -> Result<()> {
    let path = paths.cache.join(OFFER_CACHE_FILE);
    storage::write_atomic(&path, &serde_json::to_vec_pretty(offer)?, true)
}

fn clear_cached_offer(paths: &Paths) {
    let _ = std::fs::remove_file(paths.cache.join(OFFER_CACHE_FILE));
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
#[path = "updates_cmd_tests.rs"]
mod tests;
