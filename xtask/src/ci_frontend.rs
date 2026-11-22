//! Stage-two CI helper ported from `scripts/ci/frontend.ps1` into Rust.
//! Scrubs native LLVM coverage and helper overrides from the child Trunk
//! build without mutating the parent process environment.

use crate::ci_guard::{self, Guard};
use std::error::Error;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const STATIC_SCRUB_KEYS: &[&str] = &[
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "LLVM_PROFILE_FILE",
    "CARGO_LLVM_COV",
    "CARGO_LLVM_COV_SHOW_ENV",
    "TRUNK_TOOLS_WASM_BINDGEN",
    "TRUNK_TOOLS_WASM_OPT",
];

const WIN_ARM64_WASM_BINDGEN: &str = "0.2.129";
const WIN_ARM64_WASM_OPT: &str = "version_133";

/// Computes the exact environment variable keys to remove and set on the child
/// Trunk process so native LLVM instrumentation never leaks into WASM builds.
pub(crate) fn frontend_env_plan<'a>(
    existing_keys: impl Iterator<Item = &'a str>,
    is_windows_arm64: bool,
) -> (Vec<String>, Vec<(&'static str, &'static str)>) {
    let mut remove: Vec<String> = STATIC_SCRUB_KEYS.iter().map(|s| (*s).to_string()).collect();
    for key in existing_keys {
        if key.starts_with("__CARGO_LLVM_COV_") && !remove.iter().any(|k| k == key) {
            remove.push(key.to_string());
        }
    }
    let set = if is_windows_arm64 {
        vec![
            ("TRUNK_TOOLS_WASM_BINDGEN", WIN_ARM64_WASM_BINDGEN),
            ("TRUNK_TOOLS_WASM_OPT", WIN_ARM64_WASM_OPT),
        ]
    } else {
        Vec::new()
    };
    (remove, set)
}

const WIN_ARM64: bool = cfg!(all(windows, target_arch = "aarch64"));

struct FrontendBuild {
    name: &'static str,
    command: std::process::Command,
    wall: u32,
    idle: u32,
}

fn invoke(
    root: &std::path::Path,
    e2e: bool,
    keys: impl Iterator<Item = String>,
    boundary: impl FnOnce(FrontendBuild) -> Result<()>,
) -> Result<()> {
    let keys: Vec<_> = keys.collect();
    let (remove, set) = frontend_env_plan(keys.iter().map(String::as_str), WIN_ARM64);
    let mut command = std::process::Command::new("trunk");
    command
        .args(["build", "--release", "--locked"])
        .current_dir(root.join("crates/ui"));
    let name = if e2e {
        command.args(["--features", "e2e"]);
        "frontend-e2e"
    } else {
        "frontend-production"
    };
    for key in remove {
        command.env_remove(key);
    }
    command.envs(set);
    boundary(FrontendBuild {
        name,
        command,
        wall: 1200,
        idle: 180,
    })
}

pub(crate) fn run(e2e: bool) -> Result<()> {
    invoke(
        &ci_guard::root(),
        e2e,
        std::env::vars_os().filter_map(|(k, _)| k.into_string().ok()),
        |build| {
            ci_guard::run(
                Guard {
                    name: build.name,
                    wall_s: build.wall,
                    idle_s: build.idle,
                    capture: None,
                    progress: None,
                    stall: false,
                },
                build.command,
            )?
            .require_success(build.name)
        },
    )
}

pub(crate) fn contracts() -> Result<()> {
    use std::{collections::BTreeMap, ffi::OsString};
    crate::ci_guard::hosted("Frontend environment contracts run only on GitHub-hosted Actions.")?;
    let root = crate::ci_guard::state_dir(&crate::ci_guard::root())?;
    let work = tempfile::Builder::new()
        .prefix("frontend-contract-")
        .tempdir_in(root)?;
    let work_path = work.path();
    let live_before: BTreeMap<_, _> = std::env::vars_os().collect();
    for e2e in [false, true] {
        for reject in [false, true] {
            let mut before = BTreeMap::from([
                ("CARGO_ENCODED_RUSTFLAGS".to_owned(), OsString::new()),
                (
                    "RUSTC_WRAPPER".to_owned(),
                    OsString::from("owned-wrapper-sentinel"),
                ),
                (
                    "LLVM_PROFILE_FILE".to_owned(),
                    OsString::from("owned 日本 فارسی.profraw"),
                ),
                (
                    "__CARGO_LLVM_COV_FRONTEND_CONTRACT".to_owned(),
                    OsString::from("owned-coverage-sentinel"),
                ),
            ]);
            if e2e {
                before.insert(
                    "TRUNK_TOOLS_WASM_BINDGEN".to_owned(),
                    OsString::from("owned-helper-sentinel"),
                );
                before.insert("TRUNK_TOOLS_WASM_OPT".to_owned(), OsString::new());
            }
            let receipt = work_path.join("boundary-reached");
            if receipt.exists() {
                std::fs::remove_file(&receipt)?;
            }
            let expected = if e2e {
                "frontend-e2e"
            } else {
                "frontend-production"
            };
            let observed = invoke(work_path, e2e, before.keys().cloned(), |build| {
                let ui = work_path.join("crates/ui");
                let expected_args = if e2e {
                    ["build", "--release", "--locked", "--features", "e2e"].as_slice()
                } else {
                    ["build", "--release", "--locked"].as_slice()
                };
                if build.name != expected
                    || build.wall != 1200
                    || build.idle != 180
                    || build.command.get_program() != "trunk"
                    || !build.command.get_args().eq(expected_args.iter().copied())
                    || build.command.get_current_dir() != Some(ui.as_path())
                {
                    return Err("Frontend build boundary changed.".into());
                }
                let mut child = before.clone();
                for (key, value) in build.command.get_envs() {
                    let key = key.to_str().ok_or("Invalid frontend environment key.")?;
                    if let Some(value) = value {
                        child.insert(key.to_owned(), value.to_owned());
                    } else {
                        child.remove(key);
                    }
                }
                let expected_child = if WIN_ARM64 {
                    BTreeMap::from([
                        (
                            "TRUNK_TOOLS_WASM_BINDGEN".to_owned(),
                            OsString::from("0.2.129"),
                        ),
                        (
                            "TRUNK_TOOLS_WASM_OPT".to_owned(),
                            OsString::from("version_133"),
                        ),
                    ])
                } else {
                    BTreeMap::new()
                };
                if child != expected_child {
                    return Err(
                        "Frontend inherited isolated variables or changed native ARM helper pins."
                            .into(),
                    );
                }
                std::fs::write(&receipt, build.name)?;
                if reject {
                    return Err("Controlled frontend build failure.".into());
                }
                Ok(())
            });
            match (reject, observed) {
                (false, Ok(())) => (),
                (true, Err(error)) if error.to_string() == "Controlled frontend build failure." => {
                }
                (_, Err(error)) => return Err(error),
                (true, Ok(())) => {
                    return Err("Expected frontend failure was not propagated.".into());
                }
            }
            if std::fs::read_to_string(receipt)? != expected
                || std::env::vars_os().collect::<BTreeMap<_, _>>() != live_before
                || before.get("CARGO_ENCODED_RUSTFLAGS") != Some(&OsString::new())
                || before.contains_key("RUSTFLAGS")
                || before.get("LLVM_PROFILE_FILE")
                    != Some(&OsString::from("owned 日本 فارسی.profraw"))
            {
                return Err("Frontend did not preserve exact environment state or reach its guard boundary.".into());
            }
        }
    }
    work.close()?;
    println!(
        "Frontend environment contracts passed: tests=4 failed=0 skipped=0; actual runner, both profiles, absent/empty/Unicode values and failure isolation; no build executed."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_llvm_cov_and_pins_windows_arm64_helpers() {
        let keys = [
            "PATH",
            "RUSTFLAGS",
            "__CARGO_LLVM_COV_RUSTC_WRAPPER",
            "__CARGO_LLVM_COV_EXTRA",
        ];
        let (remove_non_arm, set_non_arm) = frontend_env_plan(keys.into_iter(), false);
        assert!(remove_non_arm.iter().any(|k| k == "RUSTFLAGS"));
        assert!(
            remove_non_arm
                .iter()
                .any(|k| k == "TRUNK_TOOLS_WASM_BINDGEN")
        );
        assert!(
            remove_non_arm
                .iter()
                .any(|k| k == "__CARGO_LLVM_COV_RUSTC_WRAPPER")
        );
        assert!(remove_non_arm.iter().any(|k| k == "__CARGO_LLVM_COV_EXTRA"));
        assert!(set_non_arm.is_empty());

        let (_remove_arm, set_arm) = frontend_env_plan(keys.into_iter(), true);
        assert_eq!(
            set_arm,
            vec![
                ("TRUNK_TOOLS_WASM_BINDGEN", "0.2.129"),
                ("TRUNK_TOOLS_WASM_OPT", "version_133"),
            ]
        );
    }
}
