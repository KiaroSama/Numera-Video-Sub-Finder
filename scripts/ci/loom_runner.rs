//! Hosted Linux x64 Loom runner for the production close-admission model.

use std::{
    error::Error,
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn utc_stamp(colon: bool) -> String {
    let out = Command::new("date")
        .args([
            "-u",
            if colon {
                "+%Y-%m-%d %H:%M:%S UTC"
            } else {
                "+%Y-%m-%d_%H-%M-%S_UTC"
            },
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_owned(),
        _ => {
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            format!("unix_{secs}_UTC")
        }
    }
}

fn sha256_file(path: &Path) -> Result<String, Box<dyn Error>> {
    let out = Command::new("sha256sum").arg(path).output()?;
    if !out.status.success() {
        return Err("sha256sum failed".into());
    }
    let text = String::from_utf8(out.stdout)?;
    let hex = text
        .split_whitespace()
        .next()
        .ok_or("Missing sha256sum digest")?
        .to_ascii_lowercase();
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Malformed sha256sum digest".into());
    }
    Ok(hex)
}

fn parse_libtest(text: &str) -> Result<(), Box<dyn Error>> {
    let mut matches = 0_usize;
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("test result: ok. ") else {
            continue;
        };
        let parts: Vec<&str> = rest.split(';').map(str::trim).collect();
        if parts.len() < 5 {
            return Err("Unexpected Loom libtest summary".into());
        }
        let passed = parts[0]
            .strip_suffix(" passed")
            .ok_or("Bad passed")?
            .parse::<u64>()?;
        let failed = parts[1]
            .strip_suffix(" failed")
            .ok_or("Bad failed")?
            .parse::<u64>()?;
        let ignored = parts[2]
            .strip_suffix(" ignored")
            .ok_or("Bad ignored")?
            .parse::<u64>()?;
        let measured = parts[3]
            .strip_suffix(" measured")
            .ok_or("Bad measured")?
            .parse::<u64>()?;
        let filtered = parts[4]
            .strip_suffix(" filtered out")
            .ok_or("Bad filtered")?
            .parse::<u64>()?;
        if passed != 1 || failed != 0 || ignored != 0 || measured != 0 || filtered != 0 {
            return Err("Failed, ignored, measured or filtered Loom model.".into());
        }
        matches += 1;
    }
    if matches != 1 {
        return Err("Loom must execute exactly one production-source model.".into());
    }
    Ok(())
}

fn log_line(log: &mut File, level: &str, msg: &str) {
    let line = format!("[{}] [{level}] [LOOM] {msg}", utc_stamp(true));
    let _ = writeln!(log, "{line}");
    let _ = log.flush();
    eprintln!("{line}");
}

fn main() {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
        || !cfg!(all(target_os = "linux", target_arch = "x86_64"))
    {
        eprintln!("Close-admission Loom requires a native Linux x64 hosted runner.");
        std::process::exit(1);
    }
    let root = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Working directory unavailable: {e}");
            std::process::exit(1);
        }
    };
    let ci_work = root.join(".ci-work");
    let work = ci_work.join("loom");
    if std::fs::symlink_metadata(&ci_work).is_ok_and(|m| m.file_type().is_symlink())
        || work.exists()
    {
        eprintln!("Loom evidence requires a new unlinked owned directory.");
        std::process::exit(1);
    }
    if let Err(e) = std::fs::create_dir_all(&work) {
        eprintln!("Failed to create Loom directory: {e}");
        std::process::exit(1);
    }
    let log_path = work.join(format!("loom_{}.log", utc_stamp(false)));
    let mut log = match OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&log_path)
    {
        Ok(f) => f,
        Err(_) => {
            eprintln!("Loom file logging failed; no model evidence was produced.");
            std::process::exit(1);
        }
    };

    let sha = std::env::var("GITHUB_SHA").unwrap_or_default();
    let mut prod_hash = String::new();
    let mut executions = 0_u64;
    let outcome: Result<(), Box<dyn Error>> = (|| {
        if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Loom requires an exact source SHA.".into());
        }
        let head = Command::new("git").args(["rev-parse", "HEAD"]).output()?;
        if !head.status.success() || String::from_utf8_lossy(&head.stdout).trim() != sha {
            return Err("Loom checkout source mismatch.".into());
        }
        let source = root.join("crates/core/src/close_admission.rs");
        prod_hash = sha256_file(&source)?;
        let capture = work.join("model.stdout");
        log_line(
            &mut log,
            "INFO",
            "Model-checking the same production close-admission source with two contenders; no native application execution.",
        );
        let xtask = root.join(if cfg!(windows) {
            "target/release/xtask.exe"
        } else {
            "target/release/xtask"
        });
        let status = Command::new(&xtask)
            .args([
                "ci-guard",
                "--name",
                "close-loom",
                "--wall-seconds",
                "180",
                "--idle-seconds",
                "60",
                "--executable",
                "cargo",
                "--capture-stdout-path",
            ])
            .arg(&capture)
            .args([
                "--",
                "test",
                "--locked",
                "--manifest-path",
                "ci/loom/Cargo.toml",
                "--lib",
                "--",
                "--test-threads=1",
                "--nocapture",
            ])
            .env("RUSTFLAGS", "--cfg=loom")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env("CARGO_TARGET_DIR", work.join("target"))
            .env("RUST_TEST_THREADS", "1")
            .env("CARGO_BUILD_JOBS", "2")
            .status()?;
        if !status.success() {
            return Err("Production close-admission model failed.".into());
        }
        let text = std::fs::read_to_string(&capture)?;
        parse_libtest(&text)?;
        let exec_lines: Vec<u64> = text
            .lines()
            .filter_map(|l| {
                l.trim_end_matches('\r')
                    .strip_prefix("close_admission_executions=")
            })
            .filter_map(|v| v.parse::<u64>().ok())
            .collect();
        if exec_lines.len() != 1 || exec_lines[0] <= 1 {
            return Err("Loom requires actual multiple model executions.".into());
        }
        executions = exec_lines[0];

        let mut guards: Vec<PathBuf> = Vec::new();
        for entry in std::fs::read_dir(ci_work.join("logs"))? {
            let entry = entry?;
            let name = entry.file_name();
            let s = name.to_string_lossy();
            if s.starts_with("guard-close-loom_")
                && s.ends_with("_UTC.log")
                && entry.file_type()?.is_file()
            {
                guards.push(entry.path());
            }
        }
        if guards.len() != 1 || guards[0].metadata()?.len() > 8_388_608 {
            return Err("Missing unique bounded Loom guard evidence.".into());
        }
        let guard_text = std::fs::read_to_string(&guards[0])?;
        let cleaned = guard_text.contains("Command finished name=close-loom exit_code=0 ")
            && guard_text
                .split("Cleanup verified pid=")
                .nth(1)
                .is_some_and(|tail| tail.bytes().next().is_some_and(|b| b.is_ascii_digit()));
        if !cleaned {
            return Err("Loom requires successful owned cleanup.".into());
        }
        if sha256_file(&source)? != prod_hash {
            return Err("Production model source changed during execution.".into());
        }
        log_line(
            &mut log,
            "INFO",
            "Production close-admission model passed; full desktop operation/install/job concurrency remains separately qualified.",
        );
        Ok(())
    })();

    let passed = outcome.is_ok();
    if let Err(ref e) = outcome {
        log_line(&mut log, "ERROR", &e.to_string());
    }
    let report = if passed {
        format!(
            "{{\"schema\":1,\"source_sha\":\"{sha}\",\"target\":\"linux-x64\",\"passed\":true,\"tests\":1,\"failed\":0,\"skipped\":0,\"scope\":\"production close-request lease only\",\"suites\":[{{\"name\":\"actual_close_lease_excludes_duplicates_and_reopens_after_drop\",\"tests\":1,\"failed\":0,\"skipped\":0}}],\"production_source_sha256\":\"{prod_hash}\",\"loom_version\":\"0.7.2\",\"model_executions\":{executions},\"cleanup_verified\":true}}\n"
        )
    } else {
        format!(
            "{{\"schema\":1,\"source_sha\":\"{sha}\",\"target\":\"linux-x64\",\"passed\":false,\"tests\":0,\"failed\":1,\"skipped\":0,\"scope\":\"production close-request lease only\",\"suites\":[]}}\n"
        )
    };
    let _ = std::fs::write(work.join("loom-report.json"), report);
    if outcome.is_err() {
        std::process::exit(1);
    }
}
