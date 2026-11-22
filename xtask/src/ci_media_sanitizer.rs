#[path = "ci_media_sanitizer_symbols.rs"]
mod symbols;

use crate::{
    ci_fuzz::{
        NIGHTLY, TARGET, check_nightly, evidence, exact_source, fresh_log, hash, log,
        regular_directory, report, tracked, verify_head,
    },
    ci_guard::{self, Guard, Result},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

const PROFILE: &str = "MEDIA-ASAN";
const ADAPTER: &str = "libnvsf_media_native.so";
const FILE_LIMIT: u64 = 32 * 1024 * 1024;
const CLEAR: &[&str] = &[
    "RUSTFLAGS",
    "RUSTDOCFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "LLVM_PROFILE_FILE",
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET",
    "CARGO_BUILD_JOBS",
    "RUST_TEST_THREADS",
    "NVSF_RUNTIME_ROOT",
    "NVSF_MEDIA_SANITIZER_ROOT",
    "NVSF_MEDIA_SANITIZER",
    "ASAN_OPTIONS",
    "LSAN_OPTIONS",
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
    "RUSTUP_AUTO_INSTALL",
];
const INPUTS: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "crates/app/Cargo.toml",
    "crates/media-native/Cargo.toml",
    "crates/app/src/media.rs",
    "crates/app/src/media_preview.rs",
    "crates/core/src/media_abi.rs",
    "crates/media-native/src/lib.rs",
    "crates/app/tests/media_display_geometry.rs",
    "crates/app/tests/support/media_sanitizer_admission.rs",
    "xtask/src/ci_media_sanitizer.rs",
];

struct Admission<'a> {
    root: &'a Path,
    work: &'a Path,
    stage: &'a Path,
    report: &'a mut Value,
    log: &'a mut fs::File,
}

impl Admission<'_> {
    fn guard(
        &mut self,
        name: &str,
        wall_s: u32,
        idle_s: u32,
        mut command: Command,
        capture: Option<&Path>,
    ) -> Result<String> {
        for key in CLEAR {
            command.env_remove(key);
        }
        for (key, _) in env::vars_os() {
            if key.to_str().is_some_and(|k| {
                k.starts_with("CARGO_LLVM_COV")
                    || k.starts_with("__CARGO_LLVM_COV")
                    || (k.starts_with("CARGO_TARGET_") && k.ends_with("_RUSTFLAGS"))
            }) {
                command.env_remove(key);
            }
        }
        command.envs([
            ("RUSTFLAGS", "-Zsanitizer=address -Cforce-frame-pointers=yes -Cdebuginfo=2 -Clink-arg=-Wl,--export-dynamic"),
            ("CARGO_BUILD_JOBS", "2"), ("RUST_TEST_THREADS", "1"), ("RUSTUP_AUTO_INSTALL", "0"),
            ("NVSF_MEDIA_SANITIZER", "address"),
            ("ASAN_OPTIONS", "halt_on_error=1:abort_on_error=1:detect_leaks=1:detect_stack_use_after_return=1:strict_string_checks=1"),
        ]).env("CARGO_TARGET_DIR", self.work.join("target")).env("NVSF_RUNTIME_ROOT", self.stage)
            .env("NVSF_MEDIA_SANITIZER_ROOT", self.stage).env("LD_LIBRARY_PATH", self.stage).current_dir(self.root);
        let guard_name = format!("media-asan-{name}");
        let outcome = ci_guard::run(
            Guard {
                name: &guard_name,
                wall_s,
                idle_s,
                capture,
                progress: None,
                stall: false,
            },
            command,
        )?;
        if outcome.code != 0 {
            return Err(
                format!("Media sanitizer stage failed: {name} exit={}", outcome.code).into(),
            );
        }
        let text = evidence(self.root, &guard_name, &outcome.log, FILE_LIMIT)?;
        if [
            "ERROR: AddressSanitizer",
            "ERROR: LeakSanitizer",
            "AddressSanitizer:DEADLYSIGNAL",
            "SUMMARY: AddressSanitizer",
            "SUMMARY: LeakSanitizer",
        ]
        .iter()
        .any(|marker| text.contains(marker))
        {
            return Err(format!("Sanitizer diagnostic in stage: {name}").into());
        }
        self.report["commands"]
            .as_array_mut()
            .ok_or("Missing sanitizer command inventory.")?
            .push(json!({"name":name,"log_sha256":hash(&outcome.log)?,"cleanup_verified":true}));
        Ok(text)
    }

    fn build(
        &mut self,
        started: Instant,
        name: &str,
        command: Command,
        capture: Option<&Path>,
    ) -> Result<String> {
        let remaining = Duration::from_secs(600)
            .saturating_sub(started.elapsed())
            .as_secs() as u32;
        if remaining == 0 {
            return Err("Aggregate media sanitizer build budget exhausted (600 seconds).".into());
        }
        self.guard(name, remaining, remaining.min(180), command, capture)
    }

    fn tests(
        &mut self,
        name: &str,
        executable: &Path,
        filter: Option<&str>,
        expected: Option<u64>,
    ) -> Result<String> {
        let capture = self
            .root
            .join(format!(".ci-work/logs/media-asan-{name}.stdout"));
        let mut command = Command::new(executable);
        command
            .args(filter)
            .args(["--test-threads=1", "--nocapture"]);
        self.guard(name, 120, 60, command, Some(&capture))?;
        let output = fs::read_to_string(capture)?;
        let (passed, filtered) = test_result(name, &output, expected)?;
        self.report["suites"].as_array_mut().ok_or("Missing sanitizer suite inventory.")?.push(json!({"name":name,"tests":passed,"failed":0,"skipped":0,"filtered":filtered,"binary_sha256":hash(executable)?}));
        self.report["tests"] = json!(
            self.report["tests"]
                .as_u64()
                .ok_or("Invalid sanitizer test inventory.")?
                .saturating_add(passed)
        );
        Ok(output)
    }

    fn run(&mut self, sha: &str) -> Result<()> {
        log(
            self.log,
            PROFILE,
            "INFO",
            "Starting actual media caller/cdylib ASan admission; release C closure remains uninstrumented",
        )?;
        verify_head(self.root, sha, "Sanitizer checkout SHA mismatch.")?;
        let mut inputs = Vec::new();
        for file in INPUTS {
            tracked(
                self.root,
                file,
                "Sanitizer input differs from checked-out source.",
            )?;
            inputs.push(json!({"file":file,"sha256":hash(&self.root.join(file))?}));
        }
        self.report["inputs"] = json!(inputs);
        artifact_contract(Some(&self.work.join("cargo-records-regression.jsonl")))?;
        log(
            self.log,
            PROFILE,
            "INFO",
            "Cargo record reader mixed-output and five rejection regressions passed",
        )?;
        let ordinary = self.root.join(".ci-work/runtimes");
        let inventory: Value = serde_json::from_reader(fs::File::open(
            self.root
                .join(".ci-work/native-notices/native-inventory.json"),
        )?)?;
        if inventory["schema"] != 1
            || inventory["source_sha"] != sha
            || inventory["target"] != "linux-x64"
            || inventory["rust_target"] != TARGET
            || inventory["linkage"] != "dynamic"
            || inventory["cargo_lock_sha256"] != hash(&self.root.join("Cargo.lock"))?
            || inventory["media_adapter"].is_null()
            || inventory["media_adapter"] == false
        {
            return Err("Sanitizer requires the current same-source staged native closure.".into());
        }
        let members = inventory["runtime_files"]
            .as_array()
            .filter(|members| (1..=512).contains(&members.len()))
            .ok_or("Invalid native closure inventory bound.")?;
        let mut known = HashSet::new();
        let mut staged = BTreeMap::new();
        for entry in members {
            let name = entry["file"]
                .as_str()
                .ok_or("Unsafe or duplicate native inventory member.")?;
            if name.is_empty()
                || name.contains(['/', '\\', ':'])
                || matches!(name, "." | "..")
                || !known.insert(name)
            {
                return Err("Unsafe or duplicate native inventory member.".into());
            }
            let file = ordinary.join(name);
            let size = regular_file(self.root, &file)?;
            let digest = hash(&file)?;
            if entry["bytes"] != size || entry["sha256"] != digest {
                return Err("Native closure bytes differ from current inventory.".into());
            }
            if shared_object(name) && name != ADAPTER {
                fs::copy(&file, self.stage.join(name))?;
                staged.insert(name, digest);
            }
        }
        if fs::read_dir(&ordinary)?
            .collect::<io::Result<Vec<_>>>()?
            .len()
            != known.len()
        {
            return Err("Uninventoried ordinary runtime files.".into());
        }
        for name in [
            "libavcodec.so",
            "libavformat.so",
            "libavutil.so",
            "libswscale.so",
        ] {
            if !staged.contains_key(name) {
                return Err(format!("Missing admitted FFmpeg member: {name}").into());
            }
        }

        let clock = Instant::now();
        self.build(
            clock,
            "nightly-install",
            cmd(
                "rustup",
                &[
                    "toolchain",
                    "install",
                    NIGHTLY,
                    "--profile",
                    "minimal",
                    "--component",
                    "rust-src",
                ],
            ),
            None,
        )?;
        let version = self.work.join("rustc-version.txt");
        self.build(
            clock,
            "version",
            cmd(
                "rustup",
                &["run", NIGHTLY, "rustc", "--version", "--verbose"],
            ),
            Some(&version),
        )?;
        let version = fs::read_to_string(version)?;
        check_nightly(&version, true)?;
        self.report["rustc"] = json!(version.trim());
        let target_dir = self.work.join("target");
        let cargo = |args: &[&str]| {
            let mut command = Command::new("cargo");
            command
                .arg(format!("+{NIGHTLY}"))
                .args(args)
                .args(["--locked", "--target", TARGET, "--target-dir"])
                .arg(&target_dir)
                .args([
                    "-Zbuild-std",
                    "-vv",
                    "--message-format=json-render-diagnostics",
                ]);
            command
        };
        let locations = self.root.join(".ci-work/logs");
        let native_capture = locations.join("media-asan-native-build.jsonl");
        let native_log = self.build(
            clock,
            "native-build",
            cargo(&["build", "-p", "nvsf-media-native"]),
            Some(&native_capture),
        )?;
        let native_tests = locations.join("media-asan-native-tests-build.jsonl");
        self.build(
            clock,
            "native-tests-build",
            cargo(&["test", "--no-run", "-p", "nvsf-media-native", "--lib"]),
            Some(&native_tests),
        )?;
        let app_capture = locations.join("media-asan-app-build.jsonl");
        let app_log = self.build(
            clock,
            "app-build",
            cargo(&[
                "test",
                "--no-run",
                "-p",
                "nvsf-app",
                "--lib",
                "--test",
                "media_display_geometry",
            ]),
            Some(&app_capture),
        )?;
        self.report["build_elapsed_ms"] = json!(clock.elapsed().as_millis());
        for (text, name) in [
            (native_log.as_str(), "nvsf_media_native"),
            (app_log.as_str(), "nvsf_app"),
        ] {
            if !text
                .split_whitespace()
                .zip(text.split_whitespace().skip(1))
                .any(|(flag, value)| flag == "--crate-name" && value == name)
                || !text.contains("-Zsanitizer=address")
            {
                return Err(
                    "Missing actual rustc sanitizer instrumentation evidence for caller/callee."
                        .into(),
                );
            }
        }
        let artifacts = read_artifacts(&fs::read_to_string(native_capture)?)?;
        for name in ["core", "alloc", "std"] {
            if !artifacts.iter().any(|row| {
                row["target"]["name"] == name
                    && row["package_id"]
                        .as_str()
                        .is_some_and(|p| p.contains("rust/library/"))
            }) {
                return Err(
                    format!("Missing rebuilt Rust standard library artifact: {name}").into(),
                );
            }
        }
        let libraries: Vec<_> = artifacts
            .iter()
            .filter(|row| {
                row["target"]["name"] == "nvsf_media_native"
                    && row["target"]["crate_types"]
                        .as_array()
                        .is_some_and(|types| types.iter().any(|kind| kind == "cdylib"))
            })
            .flat_map(|row| row["filenames"].as_array().into_iter().flatten())
            .filter_map(Value::as_str)
            .filter(|path| {
                Path::new(path)
                    .file_name()
                    .is_some_and(|file| file == ADAPTER)
            })
            .collect();
        let [library] = libraries.as_slice() else {
            return Err(
                "Expected exactly one actual instrumented media cdylib compiler artifact.".into(),
            );
        };
        let library = ci_guard::inside(
            &target_dir,
            Path::new(library),
            "Native sanitizer compiler artifact escaped its isolated target root.",
        )?;
        regular_file(self.root, &library)?;
        self.report["compiler_adapter_sha256"] = json!(hash(&library)?);
        let adapter = self.stage.join(ADAPTER);
        fs::copy(&library, &adapter)?;
        let mut patch = cmd("patchelf", &["--set-rpath", "$ORIGIN"]);
        patch.arg(&adapter);
        self.guard("rpath", 15, 5, patch, None)?;
        let mut nm = cmd("nm", &["-D"]);
        nm.arg(&adapter);
        let symbols = self.guard("adapter-symbols", 15, 5, nm, None)?;
        for name in [
            "nvsf_media_probe_v1",
            "nvsf_media_decode_v1",
            "nvsf_media_health_v1",
            "__asan_init",
        ] {
            if !symbols
                .lines()
                .any(|line| line.split_whitespace().last() == Some(name))
            {
                return Err(format!("Instrumented media ELF symbol missing: {name}").into());
            }
        }
        let adapter_hash = hash(&adapter)?;
        staged.insert(ADAPTER, adapter_hash.clone());
        self.report["adapter_sha256"] = json!(adapter_hash);
        report(&self.work.join("staged-libraries.json"), &json!(staged))?;
        let mut tests = read_artifacts(&fs::read_to_string(native_tests)?)?;
        tests.extend(read_artifacts(&fs::read_to_string(app_capture)?)?);
        let mut selected = BTreeMap::new();
        for row in tests
            .iter()
            .filter(|row| row["profile"]["test"] == true && row["executable"].is_string())
        {
            let executable = ci_guard::inside(
                &target_dir,
                Path::new(
                    row["executable"]
                        .as_str()
                        .ok_or("Missing compiler artifact executable.")?,
                ),
                "Sanitizer test executable escaped isolated compiler output.",
            )?;
            regular_file(self.root, &executable)?;
            if let Some(name @ ("nvsf_media_native" | "nvsf_app" | "media_display_geometry")) =
                row["target"]["name"].as_str()
                && selected.insert(name, executable).is_some()
            {
                return Err(
                    "Missing or ambiguous actual native/caller/geometry libtest executables."
                        .into(),
                );
            }
        }
        let get = |name| {
            selected
                .get(name)
                .ok_or("Missing or ambiguous actual native/caller/geometry libtest executables.")
        };
        let (callee, caller, geometry) = (
            get("nvsf_media_native")?,
            get("nvsf_app")?,
            get("media_display_geometry")?,
        );
        let mut symbol_check = Command::new(env::current_exe()?);
        symbol_check
            .arg("__ci-media-sanitizer-symbols")
            .arg(geometry);
        let receipt = self.guard("caller-symbols", 15, 5, symbol_check, None)?;
        if !receipt.lines().any(|line| {
            line.trim_end()
                .ends_with("defined_dynamic_symbol=__asan_init matches=1")
        }) {
            return Err("Actual caller executable does not export its Rust ASan runtime.".into());
        }
        self.tests("native-abi-geometry", callee, None, None)?;
        self.tests(
            "callbacks",
            caller,
            Some("media::callback_tests::"),
            Some(3),
        )?;
        let geometry = self.tests("display-geometry", geometry, None, Some(3))?;
        let mut loaded = Vec::new();
        for json in geometry.lines().filter_map(|line| {
            line.split_once("NVSF_MEDIA_ASAN_LOADED=")
                .map(|(_, payload)| payload)
        }) {
            let record: Value = serde_json::from_str(json.trim())?;
            if record["adapter_sha256"] != adapter_hash
                || record["libraries"].as_object().map_or_else(
                    || record["libraries"].as_array().map_or(0, Vec::len),
                    serde_json::Map::len,
                ) < 5
            {
                return Err("Loaded adapter identity/evidence mismatch.".into());
            }
            loaded.push(record);
        }
        if loaded.len() != 3 {
            return Err(
                "All three actual Y4M tests must report their live loaded media/FFmpeg closure."
                    .into(),
            );
        }
        self.report["loaded"] = json!(loaded);
        for (name, digest) in staged {
            if hash(&self.stage.join(name))? != digest {
                return Err("Sanitizer staged runtime changed during execution.".into());
            }
        }
        for entry in members {
            let name = entry["file"]
                .as_str()
                .ok_or("Missing inventory filename.")?;
            if entry["sha256"] != hash(&ordinary.join(name))? {
                return Err(
                    "Ordinary production runtime changed during sanitizer admission.".into(),
                );
            }
        }
        for entry in inputs {
            let path = entry["file"]
                .as_str()
                .ok_or("Missing sanitizer input filename.")?;
            if entry["sha256"] != hash(&self.root.join(path))? {
                return Err("Sanitizer source/lock input changed during admission.".into());
            }
        }
        self.report["cleanup_verified"] = json!(true);
        log(
            self.log,
            PROFILE,
            "INFO",
            &format!(
                "Completed actual instrumented media tests={}; loaded staged adapter and unchanged release C closure verified",
                self.report["tests"]
            ),
        )
    }
}

fn cmd(exe: &str, args: &[&str]) -> Command {
    let mut command = Command::new(exe);
    command.args(args);
    command
}

fn regular_file(root: &Path, file: &Path) -> Result<u64> {
    let relative = file
        .strip_prefix(root)
        .map_err(|_| "Sanitizer artifact escaped the checkout.")?;
    let metadata = ci_guard::unlinked(file)?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(format!(
            "Expected a nonempty regular compiler/runtime file: {}",
            file.display()
        )
        .into());
    }
    for parent in relative
        .ancestors()
        .skip(1)
        .filter(|p| !p.as_os_str().is_empty())
    {
        regular_directory(&root.join(parent))
            .map_err(|_| "Sanitizer artifact ancestor must not be linked.")?;
    }
    Ok(metadata.len())
}

fn shared_object(name: &str) -> bool {
    name.rsplit_once(".so").is_some_and(|(_, suffix)| {
        suffix.is_empty()
            || suffix.strip_prefix('.').is_some_and(|tail| {
                !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit() || b == b'.')
            })
    })
}

fn read_artifacts(text: &str) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    let mut completed = 0;
    for line in text.lines().filter(|line| line.starts_with('{')) {
        let message: Value = serde_json::from_str(line)?;
        match message["reason"].as_str() {
            Some("compiler-artifact") => rows.push(message),
            Some("build-finished") if message["success"] == true => completed += 1,
            Some("build-finished") => {
                return Err("Cargo reported an unsuccessful sanitizer build.".into());
            }
            _ => (),
        }
    }
    if rows.is_empty() || completed != 1 {
        return Err(
            "Cargo requires actual compiler artifacts and one successful completion record.".into(),
        );
    }
    Ok(rows)
}

fn artifact_contract(fixture: Option<&Path>) -> Result<()> {
    let compiler = r#"{"reason":"compiler-artifact","target":{"name":"literal"}}"#;
    let done = r#"{"reason":"build-finished","success":true}"#;
    let good = format!(
        "[compiler_builtins 0.1.160] cargo:rerun-if-env-changed=LIBM_BUILD_VERBOSE\n{compiler}\n{done}"
    );
    let check = |text: &str| {
        if let Some(path) = fixture {
            fs::write(path, text)?;
            read_artifacts(&fs::read_to_string(path)?)
        } else {
            read_artifacts(text)
        }
    };
    if check(&good)?.len() != 1 {
        return Err("Mixed Cargo stdout regression failed.".into());
    }
    for invalid in [
        format!("{{\n{compiler}\n{done}"),
        done.to_owned(),
        compiler.to_owned(),
        format!("{compiler}\n{{\"reason\":\"build-finished\",\"success\":false}}"),
        format!("{compiler}\n{done}\n{done}"),
    ] {
        if check(&invalid).is_ok() {
            return Err("Invalid Cargo record regression was accepted.".into());
        }
    }
    Ok(())
}

fn test_result(name: &str, text: &str, expected: Option<u64>) -> Result<(u64, u64)> {
    let mut rows = text.lines().filter_map(|line| {
        line.split_once("test result: ok. ")
            .map(|(_, result)| result)
    });
    let row = rows
        .next()
        .filter(|_| rows.next().is_none())
        .ok_or_else(|| format!("Missing unique actual libtest completion: {name}"))?;
    let labels = [
        " passed",
        " failed",
        " ignored",
        " measured",
        " filtered out",
    ];
    let nums = row
        .split(';')
        .zip(labels)
        .map(|(part, label)| {
            part.trim_start()
                .strip_suffix(label)
                .ok_or("Invalid libtest summary.")?
                .parse::<u64>()
                .map_err(|_| "Invalid libtest count.")
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let [passed, failed, ignored, measured, filtered] = nums.as_slice() else {
        return Err("Incomplete libtest summary.".into());
    };
    if *passed == 0 || expected.is_some_and(|expected| *passed != expected) {
        return Err(format!("Zero or changed selected sanitizer test inventory: {name}").into());
    }
    if *failed != 0 || *ignored != 0 || *measured != 0 {
        return Err(format!("Failed, ignored or measured-only sanitizer tests: {name}").into());
    }
    if name != "callbacks" && *filtered != 0 {
        return Err(format!("Unexpected filtered sanitizer tests: {name}").into());
    }
    Ok((*passed, *filtered))
}

fn admission() -> Result<()> {
    let message = "Media sanitizer admission requires a native Linux x64 GitHub-hosted runner.";
    ci_guard::hosted(message)?;
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err(message.into());
    }
    Ok(())
}

pub(crate) fn dispatch() -> Result<bool> {
    let mut args = env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("__ci-media-sanitizer-symbols")) {
        return Ok(false);
    }
    admission()?;
    let path = PathBuf::from(
        args.next()
            .ok_or("Missing caller executable for symbol inspection.")?,
    );
    if args.next().is_some() {
        return Err("Unexpected caller symbol inspection arguments.".into());
    }
    regular_file(&ci_guard::root(), &path)?;
    symbols::inspect_caller(&path)?;
    Ok(true)
}

pub(crate) fn run() -> Result<()> {
    admission()?;
    let root = ci_guard::root();
    let sha = exact_source("Media sanitizer admission requires an exact source SHA.")?;
    let work = ci_guard::state_dir(&root)?.join("media-sanitizer");
    fs::create_dir(&work).map_err(|_| "Media sanitizer state must be a new unlinked directory.")?;
    let stage = work.join("runtimes");
    fs::create_dir(&stage)?;
    let reports = root.join(".ci-work/packages/reports");
    fs::create_dir_all(&reports)?;
    let path = reports.join("linux-x64-media-sanitizer.json");
    let mut writer =
        fresh_log(&root.join(".ci-work/logs"), "media-sanitizer").inspect_err(|_| {
            eprintln!("Media sanitizer file logging failed; no admission result was produced.")
        })?;
    let started = Instant::now();
    let mut output = json!({"schema":1,"source_sha":sha,"target":"linux-x64","rust_target":TARGET,"profile":"actual-media-rust-ffi-asan","nightly":NIGHTLY,"passed":false,"tests":0,"failed":1,"skipped":0,"sanitizer":"address","instrumented_std":true,"build_workers":2,"test_threads":1,"build_wall_seconds":600,"build_idle_seconds":180,"test_wall_seconds":120,"test_idle_seconds":60,"commands":[],"suites":[],"qualification":"Rust caller/cdylib and Rust std only; existing release FFmpeg/C closure uninstrumented; not UBSan, whole-program or cross-platform qualification"});
    report(&path, &output)?;
    let result = Admission {
        root: &root,
        work: &work,
        stage: &stage,
        report: &mut output,
        log: &mut writer,
    }
    .run(&sha);
    match &result {
        Ok(()) => {
            output["passed"] = json!(true);
            output["failed"] = json!(0);
        }
        Err(error) => {
            output["error"] = json!(error.to_string());
            let _ = log(&mut writer, PROFILE, "ERROR", &error.to_string());
        }
    }
    output["elapsed_ms"] = json!(started.elapsed().as_millis());
    report(&path, &output)?;
    result
}

#[cfg(test)]
#[path = "ci_media_sanitizer_tests.rs"]
mod tests;
