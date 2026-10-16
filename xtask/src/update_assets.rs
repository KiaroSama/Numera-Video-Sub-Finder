use crate::{fixture::sha, runtime_pack::sign};
use nvsf_app::updates::{Artifact, Manifest};
use std::{collections::BTreeMap, error::Error, path::Path};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn asset_target(target: &str, format: &str) -> Result<String> {
    let (os, arch) = target.split_once('-').ok_or("Invalid native target")?;
    let arch = match arch {
        "x64" => "x86_64",
        "arm64" => "aarch64",
        _ => return Err("Unsupported update architecture".into()),
    };
    let (os, format) = match (os, format) {
        ("windows", "nsis") => ("windows", "nsis"),
        ("windows", "zip") => ("windows", "portable"),
        ("linux", "appimage" | "deb" | "rpm") => ("linux", format),
        ("linux", "tar.gz") => ("linux", "portable"),
        ("macos", "tar.gz") => ("darwin", "app"),
        _ => return Err("Unsupported update package format".into()),
    };
    Ok(format!("{os}-{arch}-{format}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_package_formats_map_to_explicit_application_update_targets() {
        for (target, format, expected) in [
            ("windows-arm64", "nsis", "windows-aarch64-nsis"),
            ("windows-x64", "zip", "windows-x86_64-portable"),
            ("linux-arm64", "appimage", "linux-aarch64-appimage"),
            ("linux-x64", "deb", "linux-x86_64-deb"),
            ("linux-x64", "tar.gz", "linux-x86_64-portable"),
            ("macos-x64", "tar.gz", "darwin-x86_64-app"),
        ] {
            assert_eq!(asset_target(target, format).unwrap(), expected);
        }
        for (target, format) in [
            ("linux-arm64", "nsis"),
            ("windows-x86", "zip"),
            ("macos-x64", "dmg"),
        ] {
            assert!(asset_target(target, format).is_err());
        }
    }
}

/// Candidate metadata only: no stable feed, release publication or native-signing claim.
pub fn generate(target: &str, packages: &Path) -> Result<()> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
        || std::env::var("NVSF_TARGET_ID").as_deref() != Ok(target)
    {
        return Err("Update candidates require the matching GitHub-hosted native row".into());
    }
    let source = std::env::var("GITHUB_SHA")?;
    if nvsf_core::BUILD_SOURCE_SHA != Some(source.as_str()) {
        return Err("Update candidate harness source differs from run".into());
    }
    let native = format!(
        "{}-{}",
        std::env::consts::OS,
        match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            _ => return Err("Unsupported native architecture".into()),
        }
    );
    if native != target {
        return Err("Update candidate target differs from native host".into());
    }
    let work = Path::new(".ci-work").canonicalize()?;
    if packages.is_symlink() {
        return Err("Package root must not be a symlink".into());
    }
    let packages = packages.canonicalize()?;
    if !packages.starts_with(&work) {
        return Err("Update candidates must remain in owned CI work".into());
    }
    let bundles = packages
        .join("reports")
        .join(format!("{target}-bundles.json"));
    if bundles.metadata()?.len() > 8 * 1024 * 1024 {
        return Err("Bundle report exceeds limits".into());
    }
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(&bundles)?)?;
    if report["schema"] != 1
        || report["source_sha"] != source
        || report["target"] != target
        || report["passed"] != true
        || report["failed"] != 0
        || report["skipped"] != 0
    {
        return Err("Same-source bundle verification is required before update candidates".into());
    }
    let rows = report["formats"]
        .as_array()
        .filter(|rows| !rows.is_empty() && rows.len() <= 6)
        .ok_or("Missing or oversized native format inventory")?;
    let output = packages.join(format!("{target}-updates"));
    if output.exists() || output.is_symlink() {
        return Err("Candidate output already exists".into());
    }
    std::fs::create_dir(&output)?;
    let mut platforms = BTreeMap::new();
    for row in rows {
        let format = row["format"].as_str().ok_or("Missing package format")?;
        if matches!(format, "app" | "dmg") {
            continue;
        }
        let identity = asset_target(target, format)?;
        let relative = row["file"].as_str().ok_or("Missing package asset path")?;
        let expected_prefix = format!("installers/{target}/");
        let name = relative
            .strip_prefix(&expected_prefix)
            .ok_or("Package asset escapes target inventory")?;
        if name.is_empty()
            || name.contains(['/', '\\', ':'])
            || name.starts_with('.')
            || name.contains("..")
        {
            return Err("Invalid native update asset name".into());
        }
        let input = packages.join(relative);
        let metadata = std::fs::symlink_metadata(&input)?;
        if !metadata.is_file()
            || !input.canonicalize()?.starts_with(&packages)
            || row["bytes"].as_u64() != Some(metadata.len())
            || row["sha256"].as_str() != Some(sha(&input)?.as_str())
        {
            return Err("Native update input differs from bundle inventory".into());
        }
        let extension = match format {
            "nsis" => "exe",
            "zip" => "zip",
            "appimage" => "AppImage",
            "deb" => "deb",
            "rpm" => "rpm",
            "tar.gz" => "tar.gz",
            _ => return Err("Unsupported candidate extension".into()),
        };
        let file_name = format!(
            "Numera-Video-Sub-Finder-{}-{target}-update.{extension}",
            env!("CARGO_PKG_VERSION")
        );
        let payload = output.join(&file_name);
        std::fs::copy(&input, &payload)?;
        let signature = sign(&payload, env!("CARGO_PKG_VERSION"))?;
        nvsf_app::updates::verify_artifact_file(&payload, &signature, env!("CARGO_PKG_VERSION"))?;
        if nvsf_app::updates::verify_artifact_file(&payload, &signature, "0.0.0").is_ok() {
            return Err("Signed payload accepted a different announced version".into());
        }
        // The parser checks the actual native payload, never an extension or a claimed target alone.
        nvsf_app::update_payload::validate(&std::fs::read(&payload)?, &identity)?;
        let artifact = Artifact {
            url: format!(
                "{}/releases/download/v{}/{file_name}",
                nvsf_app::updates::REPOSITORY,
                env!("CARGO_PKG_VERSION")
            ),
            signature: String::from_utf8(signature)?,
            sha256: sha(&payload)?,
            bytes: payload.metadata()?.len(),
        };
        nvsf_app::updates::validate_artifact(&artifact)?;
        if row["sha256"].as_str() != Some(artifact.sha256.as_str())
            || row["bytes"].as_u64() != Some(artifact.bytes)
            || platforms.insert(identity, artifact).is_some()
        {
            return Err("Candidate payload changed or duplicate update target".into());
        }
    }
    let expected = if target.starts_with("windows-") {
        2
    } else if target.starts_with("linux-") {
        4
    } else {
        1
    };
    if platforms.len() != expected {
        return Err("Native update candidate format inventory is incomplete".into());
    }
    let manifest = Manifest {
        version: env!("CARGO_PKG_VERSION").into(),
        notes: "Internal candidate; not approved for publication or stable updater delivery."
            .into(),
        platforms,
    };
    let manifest_path = output.join("candidate.json");
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    std::fs::write(&manifest_path, &bytes)?;
    let signature = sign(&manifest_path, &manifest.version)?;
    let final_bytes = std::fs::read(&manifest_path)?;
    if final_bytes != bytes {
        return Err("Candidate manifest changed during signing".into());
    }
    nvsf_app::updates::verify_signature(&final_bytes, &signature)?;
    if std::fs::read_dir(&output)?.count() != manifest.platforms.len() * 2 + 2 {
        return Err("Candidate output contains undeclared assets".into());
    }
    let result = serde_json::json!({"schema":1,"source_sha":source,"target":target,"passed":true,"tests":manifest.platforms.len()+1,"failed":0,"skipped":0,"status":"candidate-not-for-publication","proof":["actual package inventory bytes","payload signatures and signed version with wrong-version refusal","actual format/header admission","authenticated candidate metadata"],"not_proved":["platform-native signing/notarization","public stable feed","install/update/uninstall lifecycle"]});
    let path = packages
        .join("reports")
        .join(format!("{target}-update-assets.json"));
    use std::io::Write;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(&serde_json::to_vec_pretty(&result)?)?;
    Ok(())
}
