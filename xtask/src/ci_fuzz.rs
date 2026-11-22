use crate::{
    ci_guard::{self, Guard, Result},
    ci_tasks::sha256_hex,
};
use serde_json::{Value, json};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};

pub(crate) const NIGHTLY: &str = "nightly-2026-10-27";
pub(crate) const TARGET: &str = "x86_64-unknown-linux-gnu";

pub(crate) fn exact_source(message: &str) -> Result<String> {
    let sha = env::var("GITHUB_SHA").unwrap_or_default();
    if !hex(&sha, 40) {
        return Err(message.to_owned().into());
    }
    Ok(sha)
}

fn hex(value: &str, count: usize) -> bool {
    value.len() == count
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn verify_head(root: &Path, sha: &str, message: &str) -> Result<()> {
    let head = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()?;
    if !head.status.success() || std::str::from_utf8(&head.stdout)?.trim() != sha {
        return Err(message.to_owned().into());
    }
    Ok(())
}

pub(crate) fn tracked(root: &Path, path: &str, message: &str) -> Result<()> {
    let committed = Command::new("git")
        .current_dir(root)
        .args(["cat-file", "-e", &format!("HEAD:{path}")])
        .stderr(Stdio::null())
        .status()?;
    let clean = Command::new("git")
        .current_dir(root)
        .args(["diff", "--quiet", "HEAD", "--", path])
        .status()?;
    if !committed.success() || !clean.success() {
        return Err(message.to_owned().into());
    }
    Ok(())
}

pub(crate) fn hash(path: &Path) -> Result<String> {
    Ok(sha256_hex(path)?.1)
}

pub(crate) fn report(path: &Path, report: &Value) -> Result<()> {
    serde_json::to_writer_pretty(fs::File::create(path)?, report)?;
    Ok(())
}

pub(crate) fn log(file: &mut fs::File, component: &str, level: &str, message: &str) -> Result<()> {
    let line = format!(
        "[{}] [{level}] [{component}] {message}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    writeln!(file, "{line}")?;
    eprintln!("{line}");
    Ok(())
}

pub(crate) fn fresh_log(directory: &Path, prefix: &str) -> Result<fs::File> {
    let stamp = chrono::Utc::now().format("%Y-%m-%d_%H-%M-%S");
    Ok(fs::File::create_new(
        directory.join(format!("{prefix}_{stamp}_UTC.log")),
    )?)
}

pub(crate) fn evidence(root: &Path, name: &str, log: &Path, bound: u64) -> Result<String> {
    let prefix = format!("guard-{name}_");
    let mut entries = Vec::new();
    for item in fs::read_dir(root.join(".ci-work/logs"))? {
        let item = item?;
        let file = item.file_name();
        if file.to_string_lossy().starts_with(&prefix) && file.to_string_lossy().ends_with(".log") {
            entries.push(item.path());
        }
    }
    if entries.len() != 1
        || entries[0].as_path() != log
        || !ci_guard::unlinked(log)?.is_file()
        || fs::metadata(log)?.len() > bound
    {
        return Err(format!("Missing, duplicate or oversized guard evidence: {name}").into());
    }
    let text = fs::read_to_string(log)?;
    if !text.contains(&format!("Command finished name={name} exit_code=0 "))
        || !text.lines().any(|line| {
            line.split_once("[CI-GUARD] Cleanup verified pid=")
                .is_some_and(|(_, pid)| pid.parse::<u32>().is_ok_and(|n| n > 1))
        })
    {
        return Err(format!("Stage lacks successful exit and owned cleanup: {name}").into());
    }
    Ok(text)
}

struct Fuzz<'a> {
    root: &'a Path,
    work: &'a Path,
    report: &'a mut Value,
    log: &'a mut fs::File,
}

impl Fuzz<'_> {
    fn guard(
        &self,
        name: &str,
        wall_s: u32,
        idle_s: u32,
        mut command: Command,
        capture: Option<&Path>,
    ) -> Result<PathBuf> {
        command.current_dir(self.root);
        let outcome = ci_guard::run(
            Guard {
                name,
                wall_s,
                idle_s,
                capture,
                progress: None,
                stall: false,
            },
            command,
        )?;
        if outcome.code != 0 {
            return Err(format!("Fuzz stage {name} failed with exit code {}", outcome.code).into());
        }
        Ok(outcome.log)
    }

    fn run(&mut self, sha: &str, report_path: &Path) -> Result<()> {
        log(
            self.log,
            "SRT-FUZZ",
            "INFO",
            "Starting exact-source parser fuzz admission",
        )?;
        verify_head(
            self.root,
            sha,
            "Fuzz checkout SHA differs from workflow source.",
        )?;
        let inputs = [
            "fuzz/Cargo.toml",
            "fuzz/Cargo.lock",
            "fuzz/fuzz_targets/parse_srt.rs",
            "crates/core/src/export.rs",
        ];
        self.report["inputs"] = json!(
            inputs
                .iter()
                .map(|path| Ok(json!({"file":path,"sha256":hash(&self.root.join(path))?})))
                .collect::<Result<Vec<_>>>()?
        );
        for path in inputs {
            tracked(
                self.root,
                path,
                "Fuzz admission input differs from exact checked-out source.",
            )?;
        }
        let lock = self.root.join(inputs[1]);
        let lock_before = hash(&lock)?;
        let seed_dir = self.root.join("fuzz/seeds/parse_srt");
        for directory in [
            self.root.join("fuzz"),
            self.root.join("fuzz/seeds"),
            seed_dir.clone(),
        ] {
            regular_directory(&directory)?;
        }
        let mut seeds = Vec::new();
        for seed in fs::read_dir(&seed_dir)? {
            let seed = seed?;
            if !seed.file_type()?.is_dir() && !seed.file_name().to_string_lossy().starts_with('.') {
                seeds.push(seed);
            }
        }
        seeds.sort_unstable_by_key(fs::DirEntry::file_name);
        if !(6..=32).contains(&seeds.len()) {
            return Err("Fuzz maintained seed inventory must contain six to32 files.".into());
        }
        let mut seed_inventory = Vec::new();
        for seed in &seeds {
            let filename = seed.file_name();
            let name = filename
                .to_str()
                .ok_or("Unsafe or oversized maintained fuzz seed.")?;
            let path = format!("fuzz/seeds/parse_srt/{name}");
            tracked(
                self.root,
                &path,
                "Maintained fuzz seed differs from exact checked-out source.",
            )?;
            let file = seed.path();
            let meta = ci_guard::unlinked(&file)?;
            if !meta.is_file()
                || meta.len() > 4096
                || !name.strip_suffix(".srt").is_some_and(|stem| {
                    !stem.is_empty()
                        && stem
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                })
            {
                return Err("Unsafe or oversized maintained fuzz seed.".into());
            }
            seed_inventory.push(json!({"file":name,"bytes":meta.len(),"sha256":hash(&file)?}));
            fs::copy(&file, self.work.join("corpus").join(name))?;
        }
        self.report["seeds"] = json!(seed_inventory);
        self.report["seed_count"] = json!(seeds.len());
        report(report_path, self.report)?;

        let version = self.work.join("rustc-version.txt");
        let mut rustc = Command::new("rustup");
        rustc.args(["run", NIGHTLY, "rustc", "--version", "--verbose"]);
        self.guard("fuzz-rustc-version", 15, 5, rustc, Some(&version))?;
        let version = fs::read_to_string(version)?;
        check_nightly(&version, false)?;
        let tool_version = self.work.join("cargo-fuzz-version.txt");
        let mut tool = Command::new(self.root.join(".ci-work/tools/bin/cargo-fuzz"));
        tool.arg("--version");
        self.guard("fuzz-tool-version", 15, 5, tool, Some(&tool_version))?;
        if fs::read_to_string(tool_version)?.trim() != "cargo-fuzz 0.13.2" {
            return Err("Pinned cargo-fuzz version mismatch.".into());
        }
        self.report["rustc"] = json!(version.trim());
        let target_dir = self.work.join("target");
        let mut build = Command::new("cargo");
        build
            .args([
                format!("+{NIGHTLY}"),
                "fuzz".to_owned(),
                "build".to_owned(),
                "parse_srt".to_owned(),
                "--fuzz-dir".to_owned(),
            ])
            .arg(self.root.join("fuzz"))
            .args(["--target", TARGET, "--target-dir"])
            .arg(&target_dir)
            .args(["--sanitizer", "address", "--verbose"]);
        let build = self.guard("fuzz-build", 600, 180, build, None)?;
        let build = evidence(self.root, "fuzz-build", &build, 8 * 1024 * 1024)?;
        if !build.contains("sanitizer=address") || !build.contains("sanitizer-coverage") {
            return Err(
                "Fuzz build lacks actual ASan and coverage instrumentation evidence.".into(),
            );
        }
        if hash(&lock)? != lock_before {
            return Err("Fuzz build changed its committed lockfile.".into());
        }
        let binary = binary_evidence(&target_dir, self.report)?;
        log(
            self.log,
            "SRT-FUZZ",
            "INFO",
            "Instrumented parser built; replaying maintained seeds",
        )?;
        let arguments = || {
            let mut command = Command::new(&binary);
            command
                .args([
                    "-max_len=4096",
                    "-timeout=2",
                    "-rss_limit_mb=512",
                    "-malloc_limit_mb=64",
                ])
                .arg(format!(
                    "-artifact_prefix={}/",
                    self.work.join("artifacts").display()
                ))
                .arg("-print_final_stats=1");
            command
        };
        let mut replay = arguments();
        replay.arg("-runs=0").arg(self.work.join("corpus"));
        let replay = self.guard("fuzz-seed-replay", 30, 15, replay, None)?;
        let replay = evidence(self.root, "fuzz-seed-replay", &replay, 8 * 1024 * 1024)?;
        if !loaded(&replay)
            || !replay.lines().any(|line| {
                line.split_once("INITED")
                    .and_then(|(_, data)| data.split_once("cov:"))
                    .and_then(|(_, data)| first_count(data))
                    .is_some_and(|count| count > 0)
            })
        {
            return Err(
                "Seed replay lacks actual instrumentation or coverage observations.".into(),
            );
        }
        log(
            self.log,
            "SRT-FUZZ",
            "INFO",
            "Starting one30-second single-process coverage-guided campaign",
        )?;
        let mut campaign = arguments();
        campaign
            .args(["-max_total_time=30", "-seed=20261027"])
            .arg(self.work.join("corpus"));
        let campaign = self.guard("fuzz-campaign", 90, 45, campaign, None)?;
        let text = evidence(self.root, "fuzz-campaign", &campaign, 8 * 1024 * 1024)?;
        let (runs, edges, features) = stats(&text, seeds.len())?;
        if fs::read_dir(self.work.join("artifacts"))?.next().is_some() {
            return Err("Fuzz campaign produced failure artifacts.".into());
        }
        if hash(&lock)? != lock_before {
            return Err("Fuzz campaign changed its committed lockfile.".into());
        }
        for seed in &seed_inventory {
            let name = seed["file"].as_str().ok_or("Missing fuzz seed name.")?;
            if seed["sha256"] != hash(&seed_dir.join(name))? {
                return Err("Maintained fuzz seed changed during execution.".into());
            }
        }
        for item in self.report["inputs"]
            .as_array()
            .ok_or("Missing fuzz inputs.")?
        {
            let path = item["file"].as_str().ok_or("Missing fuzz input name.")?;
            if item["sha256"] != hash(&self.root.join(path))? {
                return Err("Fuzz admission source changed during execution.".into());
            }
        }
        self.report["executions"] = json!(runs);
        self.report["coverage_edges"] = json!(edges);
        self.report["features"] = json!(features);
        self.report["seed_replay_passed"] = json!(true);
        self.report["campaign_log_sha256"] = json!(hash(&campaign)?);
        self.report["cleanup_verified"] = json!(true);
        self.report["crash_artifacts"] = json!(0);
        log(
            self.log,
            "SRT-FUZZ",
            "INFO",
            &format!("Completed executions={runs} coverage_edges={edges} features={features}"),
        )
    }
}

pub(crate) fn regular_directory(path: &Path) -> Result<()> {
    if !ci_guard::unlinked(path)?.is_dir() {
        return Err(format!("Expected an unlinked directory: {}", path.display()).into());
    }
    Ok(())
}

fn binary_evidence(target: &Path, report: &mut Value) -> Result<PathBuf> {
    let release = target.join(TARGET).join("release");
    for directory in [target, &target.join(TARGET), &release] {
        regular_directory(directory)?;
    }
    let binary = release.join("parse_srt");
    let meta = ci_guard::unlinked(&binary)?;
    if !meta.is_file() {
        return Err("Fuzz executable must be a regular file, not a symbolic link.".into());
    }
    let digest = hash(&binary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() > 1 {
            let package = release.join("build/nvsf-parser-fuzz");
            regular_directory(&release.join("build"))?;
            regular_directory(&package)?;
            let mut entries = fs::read_dir(&package)?;
            let mut units = Vec::new();
            for entry in entries.by_ref() {
                let entry = entry?;
                if entry.file_type()?.is_dir() || entry.file_type()?.is_symlink() {
                    units.push(entry);
                }
            }
            let [unit] = units.as_slice() else {
                return Err("Fuzz compiler artifact requires one exact regular build unit.".into());
            };
            if !hex(&unit.file_name().to_string_lossy(), 16) {
                return Err("Fuzz compiler artifact requires one exact regular build unit.".into());
            }
            let unit = unit.path();
            regular_directory(&unit)?;
            let output = unit.join("out");
            regular_directory(&output)?;
            let compiled = output.join("parse_srt");
            if !ci_guard::unlinked(&compiled)?.is_file() || hash(&compiled)? != digest {
                return Err(
                    "Fuzz hard-linked executable differs from its exact regular compiler artifact."
                        .into(),
                );
            }
            report["compiler_artifact"] = json!(compiled.strip_prefix(target)?);
            report["binary_link_type"] = json!("HardLink");
        } else {
            report["binary_link_type"] = Value::Null;
        }
    }
    #[cfg(not(unix))]
    {
        report["binary_link_type"] = Value::Null;
    }
    report["binary_sha256"] = json!(digest);
    Ok(binary)
}

pub(crate) fn check_nightly(version: &str, exact: bool) -> Result<()> {
    let commit = version
        .lines()
        .find_map(|line| line.strip_prefix("commit-hash: "))
        .unwrap_or_default();
    let correct_commit = if exact {
        commit == "16d1ffac17ad8b739e32483ac2e65fe7e1804977"
    } else {
        commit.starts_with("16d1ffac1") && hex(commit, 40)
    };
    if !version
        .lines()
        .any(|line| line.starts_with("rustc 1.101.0-nightly"))
        || !correct_commit
        || !version
            .lines()
            .any(|line| line == "host: x86_64-unknown-linux-gnu")
    {
        return Err(if exact {
            "Pinned sanitizer nightly compiler/host identity mismatch."
        } else {
            "Pinned fuzz nightly version/host mismatch."
        }
        .into());
    }
    Ok(())
}

fn first_count(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.parse().ok()
}

fn loaded(text: &str) -> bool {
    text.lines().any(|line| {
        line.split_once("INFO: Loaded ")
            .and_then(|(_, data)| {
                let (count, units) = data.split_once(' ')?;
                Some(count.parse::<u64>().is_ok() && units.starts_with("modules"))
            })
            .unwrap_or(false)
    })
}

fn stats(text: &str, seeds: usize) -> Result<(u64, u64, u64)> {
    let runs = text.lines().find_map(|line| {
        line.split_once("stat::number_of_executed_units:")
            .and_then(|(_, rest)| first_count(rest))
    });
    let done = text.lines().find_map(|line| {
        let (head, tail) = line.split_once("DONE")?;
        head.split_whitespace()
            .last()?
            .strip_prefix('#')?
            .parse::<u64>()
            .ok()?;
        let (coverage, features) = tail.trim_start().strip_prefix("cov:")?.split_once("ft:")?;
        Some((first_count(coverage)?, first_count(features)?))
    });
    match (runs, done) {
        (Some(runs), Some((coverage, features)))
            if runs > seeds as u64 && coverage > 0 && features > 0 && loaded(text) =>
        {
            Ok((runs, coverage, features))
        }
        _ => Err(
            "Fuzz campaign lacks nonzero actual execution/coverage completion statistics.".into(),
        ),
    }
}

pub(crate) fn run() -> Result<()> {
    let hosted = "SRT fuzz admission requires a public Linux x64 hosted runner.";
    ci_guard::hosted(hosted)?;
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err(hosted.into());
    }
    let root = ci_guard::root();
    let sha = exact_source("Fuzz admission requires an exact source SHA.")?;
    let work = ci_guard::state_dir(&root)?.join("fuzz");
    fs::create_dir(&work).map_err(|_| "Fuzz work directory must be new for this run.")?;
    for name in ["corpus", "artifacts", "logs"] {
        fs::create_dir(work.join(name))?;
    }
    let path = work.join("fuzz-report.json");
    let mut writer = fresh_log(&work.join("logs"), "fuzz").inspect_err(|_| {
        eprintln!("Fuzz file logging initialization failed; no admission result was produced.")
    })?;
    let started = Instant::now();
    let mut output = json!({"schema":1,"source_sha":sha,"target":"linux-x64","profile":"bounded-srt-libfuzzer","passed":false,"tests":1,"failed":1,"skipped":0,"sanitizer":"address","workers":1,"nightly":NIGHTLY,"cargo_fuzz":"0.13.2","libfuzzer_sys":"0.4.13","limits":{"campaign_seconds":30,"wall_seconds":90,"idle_seconds":45,"max_len":4096,"input_timeout_seconds":2,"rss_mb":512,"malloc_mb":64},"qualification":"bounded parser campaign only; not native FFI, Loom or full CT-06 qualification"});
    report(&path, &output)?;
    let result = Fuzz {
        root: &root,
        work: &work,
        report: &mut output,
        log: &mut writer,
    }
    .run(&sha, &path);
    match &result {
        Ok(()) => {
            output["passed"] = json!(true);
            output["failed"] = json!(0);
        }
        Err(error) => {
            output["error"] = json!(error.to_string());
            let _ = log(&mut writer, "SRT-FUZZ", "ERROR", &error.to_string());
        }
    }
    output["elapsed_ms"] = json!(started.elapsed().as_millis());
    report(&path, &output)?;
    result
}

#[cfg(test)]
mod tests {
    use super::stats;
    #[test]
    fn campaign_requires_executions_and_instrumentation() {
        let log =
            "INFO: Loaded 1 modules\n#99 DONE cov: 2 ft: 3\nstat::number_of_executed_units: 99";
        assert_eq!(stats(log, 6).expect("campaign"), (99, 2, 3));
        assert!(stats(log, 99).is_err());
        assert!(stats(&log.replace("cov: 2", "cov: 0"), 6).is_err());
        assert!(stats(&log.replace("1 modules", "modules"), 6).is_err());
    }
}
