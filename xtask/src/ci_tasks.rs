//! Hosted CI subcommands ported from stage-one PowerShell helpers into Rust.

use flate2::read::GzDecoder;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    error::Error,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const GUARD_SNIPPET: &str = "$n=$env:NVSF_GUARD_NAME; $w=[int]$env:NVSF_GUARD_WALL; $i=[int]$env:NVSF_GUARD_IDLE; $e=$env:NVSF_GUARD_EXE; $a=@(ConvertFrom-Json -InputObject $env:NVSF_GUARD_ARGS_JSON); $c=$env:NVSF_GUARD_CAPTURE; $d=$env:NVSF_GUARD_WORKDIR; Remove-Item Env:NVSF_GUARD_* -ErrorAction SilentlyContinue; $p=@{ Name=$n; WallSeconds=$w; IdleSeconds=$i; Executable=$e; Arguments=$a }; if($c){ $p.CaptureStdoutPath=$c }; if($d){ $p.WorkingDirectory=$d }; & './scripts/ci/guard.ps1' @p";

fn invoke_guard(
    name: &str,
    wall_s: u32,
    idle_s: u32,
    exe: &str,
    args: &[&str],
    capture: Option<&Path>,
) -> Result<()> {
    invoke_guard_in_dir(name, wall_s, idle_s, exe, args, capture, None)
}

pub(crate) fn invoke_guard_in_dir(
    name: &str,
    wall_s: u32,
    idle_s: u32,
    exe: &str,
    args: &[&str],
    capture: Option<&Path>,
    workdir: Option<&Path>,
) -> Result<()> {
    invoke_guard_with_env(name, wall_s, idle_s, exe, args, capture, workdir, &[], &[])
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn invoke_guard_with_env(
    name: &str,
    wall_s: u32,
    idle_s: u32,
    exe: &str,
    args: &[&str],
    capture: Option<&Path>,
    workdir: Option<&Path>,
    remove_env: &[String],
    set_env: &[(&str, &str)],
) -> Result<()> {
    let mut cmd = Command::new("pwsh");
    for key in remove_env {
        cmd.env_remove(key);
    }
    for (key, value) in set_env {
        cmd.env(key, value);
    }
    cmd.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        GUARD_SNIPPET,
    ])
    .env("NVSF_GUARD_NAME", name)
    .env("NVSF_GUARD_WALL", wall_s.to_string())
    .env("NVSF_GUARD_IDLE", idle_s.to_string())
    .env("NVSF_GUARD_EXE", exe)
    .env("NVSF_GUARD_ARGS_JSON", serde_json::to_string(args)?);
    if let Some(cap) = capture {
        cmd.env("NVSF_GUARD_CAPTURE", cap);
    } else {
        cmd.env_remove("NVSF_GUARD_CAPTURE");
    }
    if let Some(dir) = workdir {
        cmd.env("NVSF_GUARD_WORKDIR", dir);
    } else {
        cmd.env_remove("NVSF_GUARD_WORKDIR");
    }
    let status = cmd.status()?;
    if !status.success() {
        return Err(format!("Guarded command failed: {name}").into());
    }
    Ok(())
}

pub(crate) fn parse_suite_output(name: &str, text: &str) -> Result<u64> {
    let mut matched = 0_usize;
    let mut total_passed = 0_u64;
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("test result: ok. ") else {
            continue;
        };
        let parts: Vec<&str> = rest.split(';').map(str::trim).collect();
        if parts.len() < 5 {
            continue;
        }
        let passed = parts[0]
            .strip_suffix(" passed")
            .ok_or_else(|| format!("Malformed passed count: {name}"))?
            .parse::<u64>()?;
        let failed = parts[1]
            .strip_suffix(" failed")
            .ok_or_else(|| format!("Malformed failed count: {name}"))?
            .parse::<u64>()?;
        let ignored = parts[2]
            .strip_suffix(" ignored")
            .ok_or_else(|| format!("Malformed ignored count: {name}"))?
            .parse::<u64>()?;
        let _measured = parts[3]
            .strip_suffix(" measured")
            .ok_or_else(|| format!("Malformed measured count: {name}"))?
            .parse::<u64>()?;
        let filtered = parts[4]
            .strip_suffix(" filtered out")
            .ok_or_else(|| format!("Malformed filtered count: {name}"))?
            .parse::<u64>()?;
        if failed != 0 || ignored != 0 || filtered != 0 {
            return Err(format!("Failed/ignored/filtered required tests: {name}").into());
        }
        total_passed = total_passed.saturating_add(passed);
        matched += 1;
    }
    if matched == 0 {
        return Err(format!("No actual libtest result: {name}").into());
    }
    if total_passed == 0 {
        return Err(format!("Zero-test suite: {name}").into());
    }
    Ok(total_passed)
}

pub fn ci_tests(target_id: &str) -> Result<()> {
    let reports = Path::new(".ci-work/packages/reports");
    let captures = Path::new(".ci-work/captures");
    std::fs::create_dir_all(reports)?;
    std::fs::create_dir_all(captures)?;

    let suites: &[(&str, &[&str])] = &[
        (
            "media-native",
            &[
                "test",
                "--locked",
                "-p",
                "nvsf-media-native",
                "--",
                "--test-threads=2",
            ],
        ),
        (
            "core-app-xtask",
            &[
                "test",
                "--locked",
                "--no-fail-fast",
                "-p",
                "nvsf-core",
                "-p",
                "nvsf-app",
                "-p",
                "xtask",
                "--",
                "--test-threads=2",
            ],
        ),
        (
            "desktop",
            &[
                "test",
                "--locked",
                "-p",
                "nvsf-desktop",
                "--features",
                "e2e",
                "--",
                "--test-threads=2",
            ],
        ),
        (
            "ui",
            &[
                "test",
                "--locked",
                "-p",
                "nvsf-ui",
                "--",
                "--test-threads=2",
            ],
        ),
        (
            "cli",
            &[
                "test",
                "--locked",
                "-p",
                "nvsf-cli",
                "--",
                "--test-threads=2",
            ],
        ),
    ];

    let mut rows: Vec<Value> = Vec::with_capacity(suites.len());
    let mut sum_tests = 0_u64;
    for &(name, args) in suites {
        let capture = captures.join(format!("tests-{name}.txt"));
        invoke_guard(
            &format!("test-{name}"),
            1200,
            180,
            "cargo",
            args,
            Some(&capture),
        )
        .map_err(|_| format!("Required suite failed: {name}"))?;
        let text = std::fs::read_to_string(&capture)?;
        let passed = parse_suite_output(name, &text)?;
        sum_tests = sum_tests.saturating_add(passed);
        rows.push(json!({
            "id": name,
            "tests": passed,
            "failed": 0,
            "skipped": 0,
            "passed": true
        }));
    }

    let source_sha = std::env::var("GITHUB_SHA")?;
    let record = json!({
        "schema": 1,
        "source_sha": source_sha,
        "target": target_id,
        "passed": true,
        "tests": sum_tests,
        "failed": 0,
        "skipped": 0,
        "suites": rows
    });
    std::fs::write(
        reports.join(format!("{target_id}-suites.json")),
        serde_json::to_vec_pretty(&record)?,
    )?;
    Ok(())
}

pub fn virtual_desktop(executable: &Path, arguments_json: &str) -> Result<()> {
    if !cfg!(target_os = "linux") || std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return Err("Virtual desktop verification requires a hosted Linux runner".into());
    }
    let mut wm = Command::new("openbox")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "Window manager failed startup")?;
    if let Some(mut out) = wm.stdout.take() {
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut out, &mut std::io::sink());
        });
    }
    if let Some(mut err) = wm.stderr.take() {
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut err, &mut std::io::sink());
        });
    }

    let run_result: Result<()> = (|| {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while Instant::now() < deadline {
            let status = Command::new("wmctrl")
                .arg("-m")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if status.is_ok_and(|s| s.success()) {
                ready = true;
                break;
            }
            if wm.try_wait()?.is_some() {
                return Err("Window manager exited before readiness".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if !ready {
            return Err("Window manager readiness deadline exceeded".into());
        }
        let argv: Vec<String> = serde_json::from_str(arguments_json)?;
        let status = Command::new(executable).args(&argv).status()?;
        if !status.success() {
            return Err("Native production GUI verification failed".into());
        }
        Ok(())
    })();

    if wm.try_wait()?.is_none() {
        let _ = wm.kill();
    }
    let cleanup_deadline = Instant::now() + Duration::from_secs(3);
    let mut exited = false;
    while Instant::now() < cleanup_deadline {
        if wm.try_wait()?.is_some() {
            exited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !exited {
        return Err("Owned window manager cleanup failed".into());
    }
    run_result
}

pub(crate) fn check_arm64_pe(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 64 || u16::from_le_bytes([bytes[0], bytes[1]]) != 0x5A4D {
        return Err("Frontend tool has no PE header.".into());
    }
    let offset = u32::from_le_bytes([bytes[0x3C], bytes[0x3D], bytes[0x3E], bytes[0x3F]]) as usize;
    if offset > bytes.len().saturating_sub(6) {
        return Err("Frontend tool PE header is truncated.".into());
    }
    let sig = u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]);
    let machine = u16::from_le_bytes([bytes[offset + 4], bytes[offset + 5]]);
    if sig != 0x0000_4550 || machine != 0xAA64 {
        return Err("Frontend tool is not native ARM64.".into());
    }
    Ok(())
}

pub(crate) fn is_allowed_binaryen_path(name: &str) -> bool {
    if name.contains("..") {
        return false;
    }
    if name == "binaryen-version_133" {
        return true;
    }
    let Some(rest) = name.strip_prefix("binaryen-version_133/") else {
        return false;
    };
    if matches!(rest, "bin" | "include" | "lib") {
        return true;
    }
    for prefix in ["bin/", "include/", "lib/"] {
        if let Some(leaf) = rest.strip_prefix(prefix) {
            return !leaf.is_empty()
                && leaf
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
        }
    }
    false
}

fn sha256_hex(path: &Path) -> Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 65536];
    let mut total = 0_u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        hasher.update(&buf[..n]);
    }
    let digest = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok((total, digest))
}

pub fn frontend_tools() -> Result<()> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
    {
        return Err("Frontend tool provisioning is restricted to GitHub-hosted Actions.".into());
    }
    if !cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        return Err("This tool admission requires native Windows ARM64.".into());
    }

    let root = std::env::current_dir()?;
    let tools = root.join(".ci-work/tools");
    let bin = tools.join("bin");
    let stage = tools.join(format!("frontend-{}", std::process::id()));
    std::fs::create_dir_all(&stage)?;
    std::fs::create_dir_all(&bin)?;

    let tools_str = tools.to_str().ok_or("Non-UTF-8 tools path")?;
    invoke_guard(
        "wasm-bindgen-install",
        1200,
        180,
        "cargo",
        &[
            "install",
            "wasm-bindgen-cli",
            "--version",
            "0.2.129",
            "--locked",
            "--bin",
            "wasm-bindgen",
            "--root",
            tools_str,
        ],
        None,
    )
    .map_err(|_| "Native wasm-bindgen provisioning failed.")?;

    let bindgen = bin.join("wasm-bindgen.exe");
    check_arm64_pe(&std::fs::read(&bindgen)?)?;
    let bindgen_ver = stage.join("bindgen-version.txt");
    invoke_guard(
        "wasm-bindgen-version",
        15,
        5,
        bindgen.to_str().ok_or("Non-UTF-8 bindgen path")?,
        &["--version"],
        Some(&bindgen_ver),
    )?;
    if std::fs::read_to_string(&bindgen_ver)?.trim() != "wasm-bindgen 0.2.129" {
        return Err("Native wasm-bindgen version mismatch.".into());
    }

    let archive = stage.join("binaryen.tar.gz");
    let archive_str = archive.to_str().ok_or("Non-UTF-8 archive path")?;
    invoke_guard(
        "binaryen-download",
        210,
        60,
        "curl",
        &[
            "--fail",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "15",
            "--max-time",
            "180",
            "--output",
            archive_str,
            "https://github.com/WebAssembly/binaryen/releases/download/version_133/binaryen-version_133-arm64-windows.tar.gz",
        ],
        None,
    )?;
    let (arc_len, arc_sha) = sha256_hex(&archive)?;
    if arc_len != 109_735_160
        || arc_sha != "492a8e1847a0be1554bb9a7f384227981d60bc013aedc02d8ba1372c3943178c"
    {
        return Err("Native Binaryen archive identity mismatch.".into());
    }

    let output = bin.join("wasm-opt.exe");
    if output.exists() {
        return Err("Refusing to replace an existing optimizer.".into());
    }
    let mut tar = tar::Archive::new(GzDecoder::new(File::open(&archive)?));
    let mut seen = HashSet::new();
    let mut count = 0_usize;
    let mut total = 0_u64;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut buf = vec![0_u8; 65536];

    for item in tar.entries()? {
        let mut entry = item?;
        count += 1;
        let raw_path = entry.path()?.to_string_lossy().into_owned();
        let name = raw_path.trim_end_matches('/').to_owned();
        if Instant::now() >= deadline
            || count > 32
            || !seen.insert(name.clone())
            || !is_allowed_binaryen_path(&name)
        {
            return Err("Unexpected Binaryen archive path, count or deadline.".into());
        }
        let kind = entry.header().entry_type();
        let declared = entry.size();
        if kind.is_dir() {
            if declared != 0 {
                return Err("Binaryen directory contains data.".into());
            }
            continue;
        }
        if !kind.is_file() || !(1..=536_870_912).contains(&declared) {
            return Err(
                "Binaryen archive contains a link, special file or oversized entry.".into(),
            );
        }
        total = total.saturating_add(declared);
        if total > 700_000_000 {
            return Err("Binaryen archive exceeds its uncompressed byte budget.".into());
        }
        let mut dest = if name == "binaryen-version_133/bin/wasm-opt.exe" {
            Some(
                OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&output)?,
            )
        } else {
            None
        };
        let mut received = 0_u64;
        loop {
            let n = entry.read(&mut buf)?;
            if n == 0 {
                break;
            }
            if Instant::now() >= deadline {
                return Err("Binaryen inspection exceeded deadline.".into());
            }
            received += n as u64;
            if received > declared {
                return Err("Binaryen entry exceeds its declared size.".into());
            }
            if let Some(ref mut f) = dest {
                f.write_all(&buf[..n])?;
            }
        }
        if received != declared {
            return Err("Truncated Binaryen entry.".into());
        }
        if let Some(mut f) = dest {
            f.flush()?;
            f.sync_all()?;
        }
    }

    let (opt_len, opt_sha) = sha256_hex(&output)?;
    if count != 21
        || opt_len != 11_148_288
        || opt_sha != "d6c01791133b2eded195b6461e9393de02a9bc36bdc3379918870780ca1c6a4d"
    {
        return Err("Native optimizer inventory or bytes mismatch.".into());
    }
    check_arm64_pe(&std::fs::read(&output)?)?;
    let opt_ver = stage.join("optimizer-version.txt");
    invoke_guard(
        "binaryen-version",
        15,
        5,
        output.to_str().ok_or("Non-UTF-8 wasm-opt path")?,
        &["--version"],
        Some(&opt_ver),
    )?;
    let ver_txt = std::fs::read_to_string(&opt_ver)?;
    let ver_trimmed = ver_txt.trim();
    if ver_trimmed != "wasm-opt version 133" && !ver_trimmed.starts_with("wasm-opt version 133 ") {
        return Err("Native Binaryen version mismatch.".into());
    }

    let license = stage.join("BINARYEN-LICENSE.txt");
    let license_str = license.to_str().ok_or("Non-UTF-8 license path")?;
    invoke_guard(
        "binaryen-license",
        45,
        15,
        "curl",
        &[
            "--fail",
            "--proto",
            "=https",
            "--connect-timeout",
            "10",
            "--max-time",
            "30",
            "--output",
            license_str,
            "https://raw.githubusercontent.com/WebAssembly/binaryen/fba5005a132d85d578be5bd010e0487ed8e2ce3e/LICENSE",
        ],
        None,
    )?;
    let (lic_len, lic_sha) = sha256_hex(&license)?;
    if lic_len != 11_535
        || lic_sha != "64c0d02491e16eced74826440ecd2bcf7722d37fa586faf457b2494293afbffe"
    {
        return Err("Binaryen notice identity mismatch.".into());
    }

    let gh_path = std::env::var("GITHUB_PATH")?;
    let mut path_file = OpenOptions::new().create(true).append(true).open(gh_path)?;
    writeln!(path_file, "{}", bin.display())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cargo_libtest_summary_with_trailing_duration() {
        let sample = "running 3 tests\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n";
        assert_eq!(parse_suite_output("sample", sample).expect("ok"), 3);
        let bad = "test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s\n";
        assert!(parse_suite_output("sample", bad).is_err());
    }

    #[test]
    fn binaryen_archive_allowlist_and_arm64_pe_header() {
        assert!(is_allowed_binaryen_path("binaryen-version_133"));
        assert!(is_allowed_binaryen_path(
            "binaryen-version_133/bin/wasm-opt.exe"
        ));
        assert!(!is_allowed_binaryen_path(
            "binaryen-version_133/bin/../wasm-opt.exe"
        ));
        assert!(!is_allowed_binaryen_path("other/wasm-opt.exe"));

        let mut pe = vec![0_u8; 128];
        pe[0] = b'M';
        pe[1] = b'Z';
        pe[0x3C] = 64;
        pe[64..68].copy_from_slice(&0x0000_4550_u32.to_le_bytes());
        pe[68..70].copy_from_slice(&0xAA64_u16.to_le_bytes());
        assert!(check_arm64_pe(&pe).is_ok());
        pe[68..70].copy_from_slice(&0x8664_u16.to_le_bytes());
        assert!(check_arm64_pe(&pe).is_err());
    }
}
