//! Hosted real-CLI negative maintenance witnesses; never a signed-repair or OCR quality claim.
use crate::{fixture::clean_runtime_environment, process::OwnedProcess};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    error::Error,
    ffi::OsString,
    io::{ErrorKind, Read},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const CAPTURE_LIMIT: u64 = 1_048_576;

pub fn verify(cli: &Path, runtimes: &Path, output: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(55);
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
    {
        return Err("Maintenance CLI witnesses require GitHub-hosted Actions".into());
    }
    let source = std::env::var("GITHUB_SHA")?;
    if source.len() != 40 || !source.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("Maintenance report requires an exact source SHA".into());
    }
    let target = format!(
        "{}-{}",
        std::env::consts::OS,
        match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            _ => return Err("Unsupported maintenance witness architecture".into()),
        }
    );
    if std::env::var("NVSF_TARGET_ID")? != target {
        return Err("Maintenance witness target differs from hosted native target".into());
    }
    let output = owned_output(output)?;
    let report = output.join("maintenance-report.json");
    if report.exists() {
        return Err("Refusing to overwrite maintenance evidence".into());
    }
    let root = output.join(format!(
        "missing native 日本 {}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_micros()
    ));
    std::fs::create_dir(&root)?;
    let bin = root.join("bin");
    let empty = root.join("empty runtimes");
    let data = root.join("isolated data");
    let temp = root.join("temp");
    for path in [&bin, &empty, &temp] {
        std::fs::create_dir(path)?;
    }
    let original = cli.canonicalize()?;
    if !original.is_file() {
        return Err("Maintenance witness requires the actual native CLI".into());
    }
    let staged = bin.join(if cfg!(windows) { "nvsf.exe" } else { "nvsf" });
    std::fs::copy(&original, &staged)?;
    #[cfg(windows)]
    copy_imported_crt(&original, runtimes, &bin)?;
    #[cfg(not(windows))]
    let _ = runtimes;
    let environment = environment_snapshot();
    let proxy = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    proxy.set_nonblocking(true)?;
    let proxy_url = format!("http://{}", proxy.local_addr()?);
    let mut observed = Vec::new();
    {
        let mut invoke = |id: &str, args: &[&str], expected: i32| -> Result<String> {
            let capture = root.join(format!("{id}.txt"));
            let mut command = Command::new(&staged);
            clean_runtime_environment(&mut command);
            command
                .current_dir(&root)
                .arg("--data-root")
                .arg(&data)
                .args(args)
                .env("NVSF_RUNTIME_ROOT", &empty)
                .env("RUST_TEST_THREADS", "2")
                .env("OMP_NUM_THREADS", "2");
            for name in ["TEMP", "TMP", "TMPDIR"] {
                command.env(name, &temp);
            }
            // Tripwire covers proxy-observable HTTP attempts, not direct sockets/OS isolation.
            for name in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            ] {
                command.env(name, &proxy_url);
            }
            command.env("NO_PROXY", "").env("no_proxy", "");
            let text = run_cli(&mut command, &capture, expected, deadline, &proxy)?;
            observed.push(json!({"id":id,"exit_code":expected,"passed":true}));
            Ok(text)
        };
        let doctor: Value =
            serde_json::from_str(&invoke("doctor-empty", &["doctor", "--format", "json"], 0)?)?;
        if doctor["source_sha"] != source
            || doctor["os"] != std::env::consts::OS
            || doctor["architecture"] != std::env::consts::ARCH
            || Path::new(
                doctor["runtimes"]
                    .as_str()
                    .ok_or("Doctor omitted runtime root")?,
            )
            .canonicalize()?
                != empty.canonicalize()?
            || Path::new(
                doctor["data_root"]
                    .as_str()
                    .ok_or("Doctor omitted data root")?,
            )
            .canonicalize()?
                != data.canonicalize()?
        {
            return Err(
                "Reachable doctor did not report the isolated exact-source runtime/data identity"
                    .into(),
            );
        }
        let media = doctor["dependencies"]
            .as_array()
            .ok_or("Doctor omitted dependency diagnostics")?
            .iter()
            .find(|row| row["component"] == "FFmpeg native adapter")
            .ok_or("Doctor omitted FFmpeg diagnostic")?;
        if media["state"] != "missing-or-incompatible"
            || !media["message"]
                .as_str()
                .is_some_and(|s| s.contains("Missing packaged runtime"))
            || std::fs::read_dir(&empty)?.next().is_some()
        {
            return Err("Doctor did not diagnose absent native media before loading it".into());
        }
        let refused = invoke("repair-denied", &["dependencies", "--repair"], 4)?;
        if !refused.contains("Dependency repair requires --repair --consent; no payload downloaded")
        {
            return Err("Repair refusal did not reach the explicit consent boundary".into());
        }
        if data.join("runtime-selection.json").exists() || data.join("runtimes").exists() {
            return Err("Denied repair mutated managed runtime activation".into());
        }
        let invalid = invoke(
            "language-invalid",
            &["languages", "install", "../invalid-language"],
            4,
        )?;
        if !invalid.contains("Select a language from the official catalog") {
            return Err("Invalid language failed somewhere other than catalog validation".into());
        }
        let catalog: Value =
            serde_json::from_str(&invoke("language-catalog", &["languages", "list"], 0)?)?;
        let rows = catalog
            .as_array()
            .ok_or("CLI language catalog is not an array")?;
        let codes: HashSet<_> = rows.iter().filter_map(|r| r["code"].as_str()).collect();
        if rows.len() != 123 || codes.len() != 123 || codes.contains("osd") || codes.contains("equ")
        {
            return Err(
                "CLI official language catalog count, uniqueness or helper exclusion failed".into(),
            );
        }
        for (code, primary) in [
            ("eng", "Paddle en-ja"),
            ("fas", "Paddle fa"),
            ("jpn", "Paddle en-ja"),
            ("rus", "Tesseract CPU"),
            ("hin", "Tesseract CPU"),
        ] {
            if !rows
                .iter()
                .any(|r| r["code"] == code && r["primary"] == primary)
            {
                return Err(format!("CLI catalog identity/primary mismatch: {code}").into());
            }
        }
        if rows.iter().any(|r| {
            r["installed"] != false
                || r["verified"] != false
                || r["bytes"].as_u64().unwrap_or(0) == 0
        }) {
            return Err(
                "Empty witness data falsely claims installed or verified language packs".into(),
            );
        }
        let missing = invoke("language-alias-missing", &["languages", "verify", "en"], 4)?;
        if !missing.contains("Language pack eng is missing; explicitly install or import it") {
            return Err("Priority en alias did not reach canonical missing-pack diagnosis".into());
        }
    }
    reject_network_attempt(&proxy)?;
    if environment != environment_snapshot() {
        return Err("Maintenance witness modified the parent environment".into());
    }
    if data.join("runtime-selection.json").exists()
        || data.join("runtimes").exists()
        || std::fs::read_dir(data.join("models"))?.next().is_some()
        || std::fs::read_dir(&empty)?.next().is_some()
    {
        return Err(
            "Negative maintenance witnesses created runtime or language payload state".into(),
        );
    }
    let record = json!({"schema":1,"source_sha":source,"target":target,"passed":true,
        "tests":observed.len(),"failed":0,"skipped":0,"observations":observed,
        "proof":["actual relocated CLI doctor reachable without native media","explicit repair consent refusal","invalid language refusal before proxy-observable HTTP","123 official catalog rows and primary identity","en alias canonical missing-pack diagnosis"],
        "not_proved":["OS-enforced network isolation","signed successful repair/restart","native updater installation","nonpriority language recognition or subtitle quality"]});
    if Instant::now() >= deadline {
        return Err("Maintenance witness exceeded overall55s budget".into());
    }
    std::fs::write(report, serde_json::to_vec_pretty(&record)?)?;
    Ok(())
}

fn owned_output(path: &Path) -> Result<PathBuf> {
    let project = std::env::current_dir()?.canonicalize()?;
    let base = project.join(".ci-work");
    if base.is_symlink() {
        return Err("CI work root may not be a symlink".into());
    }
    std::fs::create_dir_all(&base)?;
    let base = base.canonicalize()?;
    let output = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project.join(path)
    };
    if !output.starts_with(&base)
        || output
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(
            "Maintenance output must remain inside this project's owned CI work root".into(),
        );
    }
    let mut ancestor = output.as_path();
    while ancestor != base {
        if ancestor.is_symlink() {
            return Err("Maintenance output ancestors may not be symlinks".into());
        }
        ancestor = ancestor.parent().ok_or("Invalid maintenance output root")?;
    }
    std::fs::create_dir_all(&output)?;
    let output = output.canonicalize()?;
    if !output.starts_with(base) {
        return Err("Maintenance output escaped its owned CI root".into());
    }
    Ok(output)
}

fn environment_snapshot() -> Vec<Option<OsString>> {
    [
        "PATH",
        "LD_LIBRARY_PATH",
        "DYLD_LIBRARY_PATH",
        "NVSF_RUNTIME_ROOT",
        "NVSF_DATA_ROOT",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
    ]
    .into_iter()
    .map(std::env::var_os)
    .collect()
}

fn reject_network_attempt(proxy: &TcpListener) -> Result<()> {
    match proxy.accept() {
        Ok((stream, _)) => {
            drop(stream);
            Err(
                "Unexpected proxy-observable HTTP attempt during negative CLI maintenance witness"
                    .into(),
            )
        }
        Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn run_cli(
    command: &mut Command,
    capture: &Path,
    expected: i32,
    deadline: Instant,
    proxy: &TcpListener,
) -> Result<String> {
    if Instant::now() >= deadline {
        return Err("Maintenance witness exhausted its overall55s budget".into());
    }
    let mut child = OwnedProcess::spawn(command, capture)?;
    let mut last_progress = Instant::now();
    let mut size = 0;
    loop {
        reject_network_attempt(proxy)?;
        let length = std::fs::metadata(capture)?.len();
        if length > CAPTURE_LIMIT {
            return Err("CLI maintenance capture exceeded1MiB".into());
        }
        if size != length {
            size = length;
            last_progress = Instant::now();
        }
        if let Some(status) = child.status()? {
            if status.code() != Some(expected) {
                return Err(
                    format!("CLI maintenance expected exit{expected}, observed{status}").into(),
                );
            }
            let mut text = String::new();
            std::fs::File::open(capture)?
                .take(CAPTURE_LIMIT + 1)
                .read_to_string(&mut text)?;
            if text.len() as u64 > CAPTURE_LIMIT {
                return Err("CLI maintenance capture exceeded1MiB".into());
            }
            reject_network_attempt(proxy)?;
            if Instant::now() >= deadline || child.started.elapsed() >= Duration::from_secs(10) {
                return Err("CLI maintenance completed after its wall deadline".into());
            }
            return Ok(text);
        }
        if child.started.elapsed() >= Duration::from_secs(10)
            || last_progress.elapsed() >= Duration::from_secs(5)
            || Instant::now() >= deadline
        {
            return Err("CLI maintenance exceeded10s wall/5s idle or overall55s budget".into());
        }
        // Bounded process-status polling; output, not this wait, advances the idle deadline.
        std::thread::park_timeout(Duration::from_millis(20));
    }
}

#[cfg(windows)]
fn copy_imported_crt(cli: &Path, runtimes: &Path, bin: &Path) -> Result<()> {
    let mut queue = vec![cli.to_path_buf()];
    let mut seen = HashSet::new();
    while let Some(binary) = queue.pop() {
        if seen.len() > 32 {
            return Err("CLI CRT import closure exceeded32 files".into());
        }
        for import in pe_imports(&binary)? {
            let name = import.to_ascii_lowercase();
            if !seen.insert(name.clone()) {
                continue;
            }
            let crt = matches!(
                name.as_str(),
                "concrt140.dll"
                    | "msvcp140.dll"
                    | "msvcp140_1.dll"
                    | "msvcp140_2.dll"
                    | "msvcp140_atomic_wait.dll"
                    | "msvcp140_codecvt_ids.dll"
                    | "vcruntime140.dll"
                    | "vcruntime140_1.dll"
                    | "vcruntime140_threads.dll"
            );
            if crt {
                let source = runtimes.join(&name);
                if !source.is_file() || source.is_symlink() {
                    return Err(format!("Required staged CLI CRT missing: {name}").into());
                }
                let dest = bin.join(name);
                std::fs::copy(source, &dest)?;
                queue.push(dest);
            } else if name.starts_with("api-ms-")
                || name.starts_with("ext-ms-")
                || [
                    "kernel32.dll",
                    "kernelbase.dll",
                    "ntdll.dll",
                    "advapi32.dll",
                    "user32.dll",
                    "userenv.dll",
                    "gdi32.dll",
                    "ole32.dll",
                    "oleaut32.dll",
                    "combase.dll",
                    "shell32.dll",
                    "shlwapi.dll",
                    "ws2_32.dll",
                    "bcrypt.dll",
                    "bcryptprimitives.dll",
                    "crypt32.dll",
                    "ncrypt.dll",
                    "secur32.dll",
                    "normaliz.dll",
                    "iphlpapi.dll",
                    "msvcrt.dll",
                    "ucrtbase.dll",
                    "rpcrt4.dll",
                    "version.dll",
                    "dbghelp.dll",
                    "synchronization.dll",
                ]
                .contains(&name.as_str())
            {
                continue;
            } else {
                return Err(
                    format!("CLI has a non-system/non-release-CRT startup import: {name}").into(),
                );
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn pe_imports(path: &Path) -> Result<Vec<String>> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > 134_217_728 {
        return Err("CLI PE exceeds128MiB inspection budget".into());
    }
    let mut bytes = Vec::new();
    file.take(134_217_729).read_to_end(&mut bytes)?;
    let u16_at = |offset: usize| -> Result<u16> {
        Ok(u16::from_le_bytes(
            bytes
                .get(offset..offset + 2)
                .ok_or("Truncated PE u16")?
                .try_into()?,
        ))
    };
    let u32_at = |offset: usize| -> Result<u32> {
        Ok(u32::from_le_bytes(
            bytes
                .get(offset..offset + 4)
                .ok_or("Truncated PE u32")?
                .try_into()?,
        ))
    };
    if bytes.get(..2) != Some(b"MZ") {
        return Err("CLI has no PE DOS header".into());
    }
    let pe = u32_at(0x3c)? as usize;
    if bytes.get(pe..pe + 4) != Some(b"PE\0\0")
        || u16_at(pe + 4)?
            != if cfg!(target_arch = "aarch64") {
                0xaa64
            } else {
                0x8664
            }
    {
        return Err("CLI/CRT PE signature or target machine mismatch".into());
    }
    let sections = u16_at(pe + 6)? as usize;
    let optional = pe + 24;
    let optional_size = u16_at(pe + 20)? as usize;
    if !(1..=96).contains(&sections) || optional_size < 128 || u16_at(optional)? != 0x20b {
        return Err("Unsupported CLI PE32+ optional header".into());
    }
    let section_table = optional + optional_size;
    let offset = |rva: u32| -> Result<usize> {
        for index in 0..sections {
            let s = section_table + index * 40;
            let address = u32_at(s + 12)?;
            let length = u32_at(s + 16)?;
            if let Some(delta) = rva.checked_sub(address)
                && delta < length
            {
                let raw = u32_at(s + 20)?
                    .checked_add(delta)
                    .ok_or("PE RVA overflow")? as usize;
                if raw < bytes.len() {
                    return Ok(raw);
                }
            }
        }
        Err("CLI PE import RVA is outside raw sections".into())
    };
    let import_rva = u32_at(optional + 120)?;
    let import_size = u32_at(optional + 124)? as usize;
    if import_rva == 0 || !(20..=20 * 512).contains(&import_size) {
        return Err("CLI PE import table absent or exceeds bound".into());
    }
    let table = offset(import_rva)?;
    let mut imports = Vec::new();
    for index in 0..(import_size / 20).min(512) {
        let descriptor = table + index * 20;
        let name_rva = u32_at(descriptor + 12)?;
        if name_rva == 0 {
            if imports.is_empty() {
                return Err("CLI/CRT PE has no startup imports".into());
            }
            return Ok(imports);
        }
        let start = offset(name_rva)?;
        let name = bytes
            .get(start..)
            .ok_or("Invalid PE import name")?
            .iter()
            .take(181)
            .position(|b| *b == 0)
            .filter(|n| (1..=180).contains(n))
            .ok_or("Invalid PE import name length")?;
        let name = std::str::from_utf8(&bytes[start..start + name])?;
        if !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
        {
            return Err("Unsafe PE import name".into());
        }
        imports.push(name.to_owned());
    }
    Err("CLI PE import descriptors lack a bounded terminator".into())
}
