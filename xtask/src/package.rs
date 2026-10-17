use crate::fixture::{sha, smoke};
use std::{
    error::Error,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn assemble(
    target: &str,
    cli: &Path,
    gui: &Path,
    runtimes: &Path,
    output: &Path,
) -> Result<()> {
    let targets: serde_json::Value = serde_json::from_slice(&std::fs::read("ci/targets.json")?)?;
    let native_target = targets["targets"]
        .as_array()
        .ok_or("Missing targets")?
        .iter()
        .find(|r| r["id"] == target)
        .ok_or("Unknown package target")?;
    if !cli.is_file() || !gui.is_file() || !runtimes.is_dir() {
        return Err("Missing actual native package executable/runtime".into());
    }
    let source = std::env::var("GITHUB_SHA")?;
    let provenance = Path::new(".ci-work/native-notices/native-inventory.json");
    if provenance.metadata()?.len() > 8_388_608 {
        return Err("Native runtime inventory exceeds its byte bound".into());
    }
    let provenance: serde_json::Value = serde_json::from_slice(&std::fs::read(provenance)?)?;
    validate_media_inventory(
        &provenance,
        target,
        native_target["target"]
            .as_str()
            .ok_or("Missing native Rust target")?,
        &source,
        &sha(Path::new("Cargo.lock"))?,
        runtimes,
    )?;
    let root = output.join(format!("Numera-Video-Sub-Finder-0.1.0-{target}"));
    std::fs::create_dir_all(&root)?;
    let cli_name = if cfg!(windows) { "nvsf.exe" } else { "nvsf" };
    let gui_name = if cfg!(windows) {
        "Numera Video Sub Finder.exe"
    } else {
        "numera-video-sub-finder"
    };
    std::fs::copy(cli, root.join(cli_name))?;
    std::fs::copy(gui, root.join(gui_name))?;
    copy_tree(runtimes, &root.join("runtimes"))?;
    if cfg!(windows) {
        // Windows imports are resolved before main; DLLs must also be beside the executables.
        for entry in std::fs::read_dir(root.join("runtimes"))? {
            let e = entry?;
            if e.path().extension().is_some_and(|s| s == "dll") {
                std::fs::copy(e.path(), root.join(e.file_name()))?;
            }
        }
    }
    for file in ["LICENSE", "THIRD_PARTY_NOTICES.md", "README.md"] {
        std::fs::copy(file, root.join(file))?;
    }
    std::fs::create_dir_all(root.join("assets"))?;
    std::fs::copy("assets/logo.svg", root.join("assets/logo.svg"))?;
    copy_tree(
        Path::new(".ci-work/native-notices"),
        &root.join("licenses/native"),
    )?;
    copy_tree(Path::new("licenses/data"), &root.join("licenses/data"))?;
    copy_tree(Path::new(".ci-work/data/models"), &root.join("data/models"))?;
    std::fs::write(root.join("portable.flag"), b"")?;
    relocate(&root.join(cli_name), target)?;
    relocate(&root.join(gui_name), target)?;
    let mut files = Vec::new();
    inventory(&root, &root, &mut files)?;
    files.sort_by(|a, b| a["file"].as_str().cmp(&b["file"].as_str()));
    let record = serde_json::json!({"schema":1,"target":target,"source_sha":source,"status":"development-unqualified","signing":"unsigned","files":files,"qualification":{"physical_gpu":"NOT_RUN","real_language_quality":"NOT_RUN"}});
    std::fs::write(
        root.join("manifest.json"),
        serde_json::to_vec_pretty(&record)?,
    )?;
    std::fs::create_dir_all(output.join("reports"))?;
    std::fs::write(
        output.join("reports").join(format!("{target}.json")),
        serde_json::to_vec_pretty(&record)?,
    )?;
    Ok(())
}

pub fn standalone(target: &str, packages: &Path, fixtures: &Path) -> Result<()> {
    let original = packages.join(format!("Numera-Video-Sub-Finder-0.1.0-{target}"));
    let owned = PathBuf::from(".ci-work/relocated");
    std::fs::create_dir_all(&owned)?;
    let root = owned.join(format!("Numera 日本 portable {target}"));
    copy_tree(&original, &root)?;
    let cli = root
        .join(if cfg!(windows) { "nvsf.exe" } else { "nvsf" })
        .canonicalize()?;
    let data = root.join("data").canonicalize()?;
    // Remove developer library overrides for the actual child without mutating this process.
    let mut command = Command::new(&cli);
    command
        .args(["--data-root"])
        .arg(&data)
        .arg("doctor")
        .env_remove("NVSF_RUNTIME_ROOT")
        .env_remove("LD_LIBRARY_PATH")
        .env_remove("DYLD_LIBRARY_PATH");
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let clean: Vec<_> = std::env::split_paths(&old_path)
        .filter(|p| {
            !p.to_string_lossy()
                .replace('\\', "/")
                .contains(".ci-work/native")
                && !p
                    .to_string_lossy()
                    .replace('\\', "/")
                    .contains(".ci-work/runtimes")
        })
        .collect();
    command.env("PATH", std::env::join_paths(clean)?);
    crate::fixture::bounded(&mut command, Duration::from_secs(15))?;
    // Extraction subprocess gets a clean explicit runtime/data root in its own relocated package.
    smoke(&cli, fixtures, &data)?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(original.join("manifest.json"))?)?;
    for file in manifest["files"].as_array().ok_or("No package inventory")? {
        let p = root.join(file["file"].as_str().ok_or("Missing package file")?);
        if sha(&p)? != file["sha256"].as_str().ok_or("Missing package hash")? {
            return Err("Relocated package bytes differ".into());
        }
    }
    std::fs::write(
        packages
            .join("reports")
            .join(format!("{target}-standalone.json")),
        serde_json::to_vec_pretty(
            &serde_json::json!({"schema":1,"source_sha":std::env::var("GITHUB_SHA")?,"target":target,"passed":true,"tests":3,"failed":0,"skipped":0,"artifact":original.file_name(),"proof":["relocated paths with spaces and Unicode","actual native CLI OCR","byte inventory"],"not_proved":["installer lifecycle","production GUI startup"]}),
        )?,
    )?;
    Ok(())
}

pub(crate) fn validate_media_inventory(
    record: &serde_json::Value,
    target: &str,
    rust_target: &str,
    source: &str,
    lock_sha: &str,
    runtimes: &Path,
) -> Result<()> {
    let name = if target.starts_with("windows-") {
        "nvsf_media_native.dll"
    } else if target.starts_with("macos-") {
        "libnvsf_media_native.dylib"
    } else {
        "libnvsf_media_native.so"
    };
    let adapter = &record["media_adapter"];
    if record["schema"] != 1
        || record["target"] != target
        || record["rust_target"] != rust_target
        || record["source_sha"] != source
        || record["cargo_lock_sha256"] != lock_sha
        || adapter["file"] != name
        || adapter["abi_version"] != 1
        || adapter["rust_target"] != rust_target
        || adapter["profile"] != "release"
        || adapter["cargo_lock_sha256"] != lock_sha
    {
        return Err("Missing or mismatched same-source native media adapter provenance".into());
    }
    let files = record["runtime_files"]
        .as_array()
        .filter(|files| !files.is_empty() && files.len() <= 512)
        .ok_or("Invalid native runtime inventory size")?;
    let mut names = std::collections::HashSet::new();
    for file in files {
        let name = file["file"]
            .as_str()
            .ok_or("Missing native runtime filename")?;
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains(['/', '\\', ':'])
            || !names.insert(name)
        {
            return Err("Unsafe or duplicate native runtime inventory filename".into());
        }
        let path = runtimes.join(name);
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.is_file()
            || metadata.len() == 0
            || file["bytes"].as_u64() != Some(metadata.len())
            || file["sha256"].as_str() != Some(sha(&path)?.as_str())
        {
            return Err("Native runtime bytes differ from the finalized loader inventory".into());
        }
    }
    if !names.contains(name)
        || files
            .iter()
            .find(|file| file["file"] == name)
            .map(|file| &file["sha256"])
            != Some(&adapter["sha256"])
        || std::fs::read_dir(runtimes)?.count() != files.len()
    {
        return Err("Native media adapter/loader closure inventory is incomplete".into());
    }
    Ok(())
}

#[cfg(test)]
mod media_inventory_tests {
    use super::*;

    #[test]
    fn rejected_package_member_reports_only_escaped_relative_path() {
        let directory = tempfile::tempdir().unwrap();
        let notices = directory.path().join("licenses/native");
        std::fs::create_dir_all(&notices).unwrap();
        let name = ".owned-notice-fixture.txt";
        std::fs::write(notices.join(name), b"private fixture body").unwrap();
        let error = inventory(directory.path(), directory.path(), &mut Vec::new())
            .unwrap_err()
            .to_string();
        assert!(error.contains("licenses/native/.owned-notice-fixture.txt"));
        assert!(!error.contains(&directory.path().display().to_string()));
        assert!(!error.contains("private fixture body"));
        assert_eq!(
            std::fs::read(notices.join(name)).unwrap(),
            b"private fixture body"
        );
    }

    #[test]
    fn final_adapter_and_loader_bytes_require_exact_source_target_and_lock() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let name = "libnvsf_media_native.so";
        std::fs::write(root.join(name), b"adapter").unwrap();
        std::fs::write(root.join("libavcodec.so.63"), b"native dependency").unwrap();
        let files: Vec<_> = [name, "libavcodec.so.63"]
            .into_iter()
            .map(|name| {
                let path = root.join(name);
                serde_json::json!({"file":name,"bytes":path.metadata().unwrap().len(),"sha256":sha(&path).unwrap()})
            })
            .collect();
        let source = "a".repeat(40);
        let lock = "b".repeat(64);
        let record = serde_json::json!({
            "schema":1,"target":"linux-x64","rust_target":"x86_64-unknown-linux-gnu",
            "source_sha":source,"cargo_lock_sha256":lock,"runtime_files":files,
            "media_adapter":{"file":name,"abi_version":1,"rust_target":"x86_64-unknown-linux-gnu",
                "profile":"release","cargo_lock_sha256":lock,"sha256":sha(&root.join(name)).unwrap()}
        });
        let validate = |record: &serde_json::Value| {
            validate_media_inventory(
                record,
                "linux-x64",
                "x86_64-unknown-linux-gnu",
                &source,
                &lock,
                root,
            )
        };
        validate(&record).unwrap();
        for field in ["source_sha", "target", "rust_target", "cargo_lock_sha256"] {
            let mut wrong = record.clone();
            wrong[field] = "wrong".into();
            assert!(validate(&wrong).is_err(), "mismatched {field} accepted");
        }
        for field in [
            "file",
            "abi_version",
            "profile",
            "rust_target",
            "cargo_lock_sha256",
            "sha256",
        ] {
            let mut wrong = record.clone();
            wrong["media_adapter"][field] = "wrong".into();
            assert!(
                validate(&wrong).is_err(),
                "mismatched adapter {field} accepted"
            );
        }
        std::fs::write(root.join(name), b"tampered adapter").unwrap();
        assert!(validate(&record).is_err());
        std::fs::write(root.join(name), b"adapter").unwrap();
        std::fs::write(root.join("libavcodec.so.63"), b"tampered dependency").unwrap();
        assert!(validate(&record).is_err());
        std::fs::write(root.join("libavcodec.so.63"), b"native dependency").unwrap();
        validate(&record).unwrap();
        let mut unsafe_record = record.clone();
        unsafe_record["runtime_files"][0]["file"] = "../escaped.so".into();
        assert!(validate(&unsafe_record).is_err());
        let mut missing = record.clone();
        missing["media_adapter"] = serde_json::Value::Null;
        assert!(validate(&missing).is_err());
        std::fs::write(root.join("unlisted.dll"), b"unexpected").unwrap();
        assert!(validate(&record).is_err());
    }
}

fn relocate(binary: &Path, target: &str) -> Result<()> {
    if target.starts_with("linux") {
        let mut c = Command::new("patchelf");
        c.args(["--set-rpath", "$ORIGIN/runtimes"]).arg(binary);
        crate::fixture::bounded(&mut c, Duration::from_secs(10))?;
    }
    Ok(())
}

fn copy_tree(source: &Path, dest: &Path) -> Result<()> {
    if !source.is_dir() {
        return Err(format!("Missing required package directory: {}", source.display()).into());
    }
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(source)? {
        let e = entry?;
        let path = e.path();
        let target = dest.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&path, &target)?;
        } else if path.is_file() {
            std::fs::copy(path, target)?;
        } else {
            return Err("Unsupported package input entry".into());
        }
    }
    Ok(())
}
fn inventory(root: &Path, dir: &Path, out: &mut Vec<serde_json::Value>) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let path = e.path();
        if e.file_type()?.is_dir() {
            inventory(root, &path, out)?;
        } else if path.is_file() {
            let relative = path
                .strip_prefix(root)?
                .to_str()
                .ok_or("Non-UTF8 package path")?
                .replace('\\', "/");
            if relative
                .split('/')
                .any(|p| p.starts_with('.') || p == "secrets.md" || p == "explain-AI.md")
            {
                return Err(format!(
                    "Excluded package member rejected: {}",
                    relative.escape_default()
                )
                .into());
            }
            out.push(serde_json::json!({"file":relative,"sha256":sha(&path)?,"bytes":path.metadata()?.len()}));
        }
    }
    Ok(())
}
