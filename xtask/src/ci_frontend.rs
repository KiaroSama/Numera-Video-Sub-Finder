//! Stage-two CI helper ported from `scripts/ci/frontend.ps1` into Rust.
//! Scrubs native LLVM coverage and helper overrides from the child Trunk
//! build without mutating the parent process environment.

use crate::ci_tasks;
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

pub(crate) fn run(e2e: bool) -> Result<()> {
    let root = std::env::current_dir()?;
    let ui_dir = root.join("crates/ui");
    let current_keys: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
    let is_windows_arm64 = cfg!(all(windows, target_arch = "aarch64"));
    let (remove_keys, set_pairs) =
        frontend_env_plan(current_keys.iter().map(String::as_str), is_windows_arm64);

    let mut args = vec!["build", "--release", "--locked"];
    if e2e {
        args.push("--features");
        args.push("e2e");
    }
    let name = if e2e {
        "frontend-e2e"
    } else {
        "frontend-production"
    };
    ci_tasks::invoke_guard_with_env(
        name,
        1200,
        180,
        "trunk",
        &args,
        None,
        Some(&ui_dir),
        &remove_keys,
        &set_pairs,
    )
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
