use super::Native;
use crate::{
    ci_guard::{Result, unlinked},
    ci_native_helpers::{Member, basename, files, lower_hex, path_text, sha},
};
use serde_json::{Map, Value, json};
use std::{fs, process::Command};

const ORT_URL: &str = "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/1.31.0/microsoft.ml.onnxruntime.1.31.0.nupkg";
const ORT_ARCHIVE: &str = "microsoft.ml.onnxruntime.1.31.0.nupkg";

fn ort_member(path: &str, prefix: &str) -> Option<bool> {
    if matches!(path, "LICENSE" | "ThirdPartyNotices.txt") {
        return Some(false);
    }
    let name = path.strip_prefix(prefix)?;
    let remainder = if cfg!(windows) {
        name.strip_prefix("onnxruntime")?.strip_suffix(".dll")?
    } else {
        name.strip_prefix("libonnxruntime")?.strip_suffix(".so")?
    };
    remainder
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        .then_some(true)
}

pub(super) fn onnx(native: &mut Native) -> Result<()> {
    let pin = &native.target.ort;
    let os = if cfg!(windows) { "win" } else { "linux" };
    if pin.url != ORT_URL
        || !lower_hex(&pin.sha256, 64)
        || pin.bytes == 0
        || pin.native != format!("runtimes/{os}-{}/native", native.target.arch)
    {
        return Err("ONNX package URL or pin differs from the reviewed native NuGet RID.".into());
    }
    let archive = native.work.join(ORT_ARCHIVE);
    let archive_text = path_text(&archive)?;
    native.run.run(
        "ort-download",
        "curl",
        [
            "--fail",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "30",
            "--max-time",
            "300",
            "--output",
            archive_text,
            ORT_URL,
        ],
        (330, 90),
    )?;
    Member {
        file: ORT_ARCHIVE.to_owned(),
        bytes: pin.bytes,
        sha256: pin.sha256.clone(),
    }
    .verify(&archive)?;
    let ort = tempfile::Builder::new()
        .prefix("ort-unpack-")
        .tempdir_in(&native.work)?;
    let unpack = path_text(ort.path())?;
    let prefix = format!("{}/", pin.native);
    let mut selected = std::collections::BTreeMap::new();
    for (name, size) in zip_members(native, archive_text)? {
        if let Some(library) = ort_member(&name, &prefix)
            && (!(1..=1_073_741_824).contains(&size)
                || selected.insert(name, (size, library)).is_some())
        {
            return Err("Unbounded or duplicate selected ONNX member.".into());
        }
    }
    let main = if cfg!(windows) {
        "onnxruntime.dll"
    } else {
        "libonnxruntime.so"
    };
    for name in [
        "LICENSE",
        "ThirdPartyNotices.txt",
        &format!("{prefix}{main}"),
    ] {
        if !selected.contains_key(name) {
            return Err(format!("ONNX package omitted required member: {name}").into());
        }
    }
    let mut extract = if cfg!(windows) {
        let mut cmd = Command::new("7z.exe");
        cmd.args(["x", "-y", "-mmt2", "-sccUTF-8", "-spd"])
            .arg(format!("-o{unpack}"))
            .arg(archive_text);
        cmd
    } else {
        let mut cmd = Command::new("unzip");
        cmd.args(["-o", archive_text]);
        cmd
    };
    extract.args(selected.keys());
    if !cfg!(windows) {
        extract.args(["-d", unpack]);
    }
    native
        .run
        .command("ort-unpack", extract, (300, 120), false, None)?;
    if files(ort.path())?.len() != selected.len() {
        return Err("Unexpected files in selected ONNX payload.".into());
    }
    let notices = native.notices.join("onnxruntime");
    fs::create_dir_all(&notices)?;
    for (path, (size, library)) in selected {
        let source = ort.path().join(path);
        let meta = unlinked(&source)?;
        if !meta.is_file() || meta.len() != size {
            return Err(
                "Selected ONNX package member bytes differ from the archive directory.".into(),
            );
        }
        if library {
            native.copy_runtime(&source)?;
        } else {
            fs::copy(&source, notices.join(basename(&source)?))?;
        }
    }
    if !cfg!(windows) {
        let soname = native.runtimes.join("libonnxruntime.so.1");
        if !soname.exists() {
            fs::copy(native.runtimes.join(main), soname)?;
        }
    }
    Ok(())
}

fn zip_members(native: &mut Native, archive: &str) -> Result<Vec<(String, u64)>> {
    if cfg!(windows) {
        let listing = native.run.capture(
            "ort-members",
            "7z.exe",
            ["l", "-ba", "-slt", "-sccUTF-8", archive],
            (60, 20),
        )?;
        let mut members = Vec::new();
        let mut path = None;
        for line in listing.lines() {
            if let Some(name) = line.strip_prefix("Path = ") {
                if path.replace(name).is_some() {
                    return Err("ONNX archive listing omitted a member size.".into());
                }
            } else if let Some(size) = line.strip_prefix("Size = ") {
                let path = path
                    .take()
                    .ok_or("ONNX archive listing omitted a member path.")?;
                members.push((path.replace('\\', "/"), size.parse()?));
            }
        }
        if path.is_some() {
            return Err("ONNX archive listing omitted a member size.".into());
        }
        return Ok(members);
    }
    let listing = native
        .run
        .capture("ort-members", "unzip", ["-l", archive], (60, 20))?;
    let mut members = Vec::new();
    for line in listing.lines() {
        let mut parts = line.split_ascii_whitespace();
        if let (Some(size), Some(_date), Some(_time), Some(path), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) && let Ok(size) = size.parse::<u64>()
        {
            members.push((path.to_owned(), size));
        }
    }
    Ok(members)
}

pub(super) fn llama(native: &mut Native) -> Result<Map<String, Value>> {
    const TAG: &str = "69ecae922d2b60c1fa8c590279f4e2099af75111";
    const COMMIT: &str = "d81235049384534c167caea52b85a694f6103d14";
    const PATCH: &str = "df3a06e51cf1496f6cf19865f2137c4214693b19c191769c904babd3f47e4c9a";
    let source = native.work.join("llama-source");
    let repo = path_text(&source)?;
    if !source.exists() {
        fs::create_dir_all(&source)?;
        native
            .run
            .run("llama-init", "git", ["-C", repo, "init"], (300, 120))?;
        native.run.run(
            "llama-fetch",
            "git",
            [
                "-C",
                repo,
                "fetch",
                "--depth=1",
                "https://github.com/ggml-org/llama.cpp.git",
                "refs/tags/v0.6.0:refs/tags/v0.6.0",
            ],
            (600, 180),
        )?;
    }
    for (name, rev, expected) in [
        ("llama-tag", "refs/tags/v0.6.0", TAG),
        ("llama-commit", "refs/tags/v0.6.0^{}", COMMIT),
    ] {
        if native
            .run
            .capture(name, "git", ["-C", repo, "rev-parse", rev], (300, 120))?
            != expected
        {
            return Err(
                "Stable llama.cpp tag/peeled-source revision changed or mismatched.".into(),
            );
        }
    }
    native.run.run(
        "llama-line-endings",
        "git",
        ["-C", repo, "config", "core.autocrlf", "false"],
        (300, 120),
    )?;
    native.run.run(
        "llama-checkout",
        "git",
        ["-C", repo, "checkout", "--detach", COMMIT],
        (300, 120),
    )?;
    let patch = native
        .root
        .join("ci")
        .join("patches")
        .join("llama-bf16-neon.patch");
    let kernel = source
        .join("ggml")
        .join("src")
        .join("ggml-cpu")
        .join("vec.cpp");
    if sha(&patch)? != PATCH
        || sha(&kernel)? != "31e43ec66a1e2920a549dd2f71e6ce474e68f2fc298c4d2942404ca327e763d1"
    {
        return Err("Native BF16 patch or exact upstream preimage mismatch.".into());
    }
    native.run.run(
        "llama-patch-check",
        "git",
        ["-C", repo, "apply", "--check", path_text(&patch)?],
        (300, 120),
    )?;
    native.run.run(
        "llama-patch",
        "git",
        ["-C", repo, "apply", path_text(&patch)?],
        (300, 120),
    )?;
    if sha(&kernel)? != "ad14c129561bb7d9a0e5e0dbb843158760513bdbd900cdadc39490fd29eea21c" {
        return Err("Native BF16 patched source identity mismatch.".into());
    }
    let build = native.work.join("llama-build");
    let source_option = format!("-DNVSF_LLAMA_SOURCE={repo}");
    let native_tests = native.root.join("ci").join("native-tests");
    let mut configure = vec![
        "-S",
        path_text(&native_tests)?,
        &source_option,
        "-B",
        path_text(&build)?,
        "-DCMAKE_BUILD_TYPE=Release",
        "-DBUILD_SHARED_LIBS=OFF",
        "-DLLAMA_BUILD_IS_DEV=OFF",
        "-DLLAMA_BUILD_COMMON=ON",
        "-DLLAMA_BUILD_TOOLS=ON",
        "-DLLAMA_BUILD_TESTS=OFF",
        "-DLLAMA_BUILD_EXAMPLES=OFF",
        "-DLLAMA_BUILD_SERVER=OFF",
        "-DLLAMA_BUILD_APP=OFF",
        "-DLLAMA_OPENSSL=OFF",
        "-DLLAMA_SUBPROCESS=OFF",
        "-DMTMD_VIDEO=OFF",
        "-DGGML_NATIVE=OFF",
        "-DGGML_METAL=OFF",
        "-DGGML_BLAS=OFF",
        "-DGGML_CUDA=OFF",
        "-DGGML_OPENMP=OFF",
        "-DGGML_CPU_KLEIDIAI=OFF",
    ];
    let arm = native.target.arch == "arm64";
    if arm {
        configure.push("-DNVSF_REQUIRE_ARM_NEON=ON");
    }
    if cfg!(windows) {
        configure.extend(["-A", if arm { "ARM64" } else { "x64" }, "-T", "ClangCL"]);
        if arm {
            configure.push("-DGGML_CPU_ARM_ARCH=armv8-a");
        }
    }
    native
        .run
        .run("llama-configure", "cmake", configure, (300, 120))?;
    native.run.run(
        "llama-build",
        "cmake",
        [
            "--build",
            path_text(&build)?,
            "--config",
            "Release",
            "--target",
            "llama-mtmd-cli",
            "nvsf-llama-bf16",
            "--parallel",
            "2",
        ],
        (3600, 600),
    )?;
    let witness = build.join(if cfg!(windows) {
        "witness-bin/nvsf-llama-bf16.exe"
    } else {
        "witness-bin/nvsf-llama-bf16"
    });
    let output = native.run.capture(
        "llama-bf16-reference",
        witness,
        [if arm { "--require-arm64" } else { "--native" }],
        (15, 5),
    )?;
    let evidence: Value = serde_json::from_str(&output)?;
    if evidence["passed"] != true
        || evidence["tests"] != 312
        || evidence["failed"] != 0
        || evidence["skipped"] != 0
        || (arm && (evidence["runtime_arch"] != "aarch64" || evidence["neon"] != 1))
    {
        return Err("Actual native BF16 scalar-reference evidence is incomplete.".into());
    }
    native.run.log(
        "INFO",
        &format!(
            "BF16 scalar-reference tests=312 failed=0 arch={} neon={}",
            evidence["runtime_arch"], evidence["neon"]
        ),
    )?;
    let name = if cfg!(windows) {
        "llama-mtmd-cli.exe"
    } else {
        "llama-mtmd-cli"
    };
    let binaries = files(&build.join("bin"))?
        .into_iter()
        .filter(|path| path.file_name().is_some_and(|file| file == name))
        .collect::<Vec<_>>();
    let [binary] = binaries.as_slice() else {
        return Err("Native llama.cpp build did not produce one exact helper executable.".into());
    };
    let installed = native.runtimes.join(name);
    fs::copy(binary, &installed)?;
    let notices = native.notices.join("llama.cpp");
    fs::create_dir_all(&notices)?;
    fs::copy(source.join("LICENSE"), notices.join("LICENSE"))?;
    fs::copy(patch, notices.join("BF16-NEON-backport.txt"))?;
    for file in files(&source.join("vendor"))? {
        if legal_notice(basename(&file)?) {
            let dest = notices.join(file.strip_prefix(&source)?);
            fs::create_dir_all(dest.parent().ok_or("Invalid llama.cpp notice path.")?)?;
            fs::copy(file, dest)?;
        }
    }
    if cfg!(windows) {
        super::platform::media_machine(&installed, &native.target.arch)?;
    }
    native
        .run
        .run("llama-help", installed, ["--help"], (30, 10))?;
    Ok(Map::from_iter([
        ("llama_tag_object".into(), json!(TAG)),
        ("llama_source_commit".into(), json!(COMMIT)),
        ("llama_patch_sha256".into(), json!(PATCH)),
        ("llama_bf16_reference".into(), evidence),
        (
            "llama_cli_status".into(),
            json!("EXPERIMENTAL_MODEL_NOT_LOADED"),
        ),
    ]))
}

fn legal_notice(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    ["LICENSE", "COPYING", "NOTICE"].iter().any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(['.', '_', '-']))
    })
}
