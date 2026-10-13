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
    if !targets["targets"]
        .as_array()
        .ok_or("Missing targets")?
        .iter()
        .any(|r| r["id"] == target)
    {
        return Err("Unknown package target".into());
    }
    if !cli.is_file() || !gui.is_file() || !runtimes.is_dir() {
        return Err("Missing actual native package executable/runtime".into());
    }
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
    copy_tree(
        Path::new(".ci-work/native-notices"),
        &root.join("licenses/native"),
    )?;
    copy_tree(Path::new(".ci-work/data/models"), &root.join("data/models"))?;
    std::fs::write(root.join("portable.flag"), b"")?;
    relocate(&root.join(cli_name), target)?;
    relocate(&root.join(gui_name), target)?;
    let mut files = Vec::new();
    inventory(&root, &root, &mut files)?;
    files.sort_by(|a, b| a["file"].as_str().cmp(&b["file"].as_str()));
    let source = std::env::var("GITHUB_SHA")?;
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
                return Err("Excluded private artifact found in package".into());
            }
            out.push(serde_json::json!({"file":relative,"sha256":sha(&path)?,"bytes":path.metadata()?.len()}));
        }
    }
    Ok(())
}
