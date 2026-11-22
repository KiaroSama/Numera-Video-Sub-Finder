//! Stage-two CI helper ported from `scripts/ci/coverage.ps1` into Rust.
//! Parses `cargo llvm-cov show-env --pwsh`, unions bounded LCOV profiles,
//! and executes the `report` phase in hosted GitHub Actions.

use crate::{ci_tasks, quality};
use std::{
    error::Error,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const MAX_COMBINED_LCOV_BYTES: u64 = 67_108_864;

#[cfg_attr(not(test), allow(dead_code))]
fn is_allowed_coverage_key(key: &str) -> bool {
    matches!(
        key,
        "RUSTFLAGS"
            | "CARGO_ENCODED_RUSTFLAGS"
            | "RUSTC_WRAPPER"
            | "RUSTC_WORKSPACE_WRAPPER"
            | "LLVM_PROFILE_FILE"
            | "CARGO_LLVM_COV"
            | "__CARGO_LLVM_COV_RUSTC_WRAPPER"
            | "__CARGO_LLVM_COV_RUSTC_WRAPPER_RUSTFLAGS"
            | "__CARGO_LLVM_COV_RUSTC_WRAPPER_CRATE_NAMES"
            | "__CARGO_LLVM_COV_RUSTC_WRAPPER_PRE_EXISTING"
            | "__CARGO_LLVM_COV_RUSTC_WRAPPER_COVERAGE_TARGET"
            | "__CARGO_LLVM_COV_RUSTC_WRAPPER_HOST"
    ) || key.strip_prefix("CARGO_LLVM_COV_").is_some_and(|rest| {
        !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
    })
}

#[cfg_attr(not(test), allow(dead_code))]
fn decode_pwsh_unicode_escapes(encoded: &str) -> Result<String> {
    let mut out = String::new();
    let mut rest = encoded;
    while !rest.is_empty() {
        let Some(after_prefix) = rest.strip_prefix("`u{") else {
            return Err("Unexpected coverage environment output".into());
        };
        let Some(end) = after_prefix.find('}') else {
            return Err("Unexpected coverage environment output".into());
        };
        let hex = &after_prefix[..end];
        if hex.is_empty()
            || !hex
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("Unexpected coverage environment output".into());
        }
        let code =
            u32::from_str_radix(hex, 16).map_err(|_| "Unexpected coverage environment output")?;
        let ch = char::from_u32(code).ok_or("Unexpected coverage environment output")?;
        out.push(ch);
        rest = &after_prefix[end + 1..];
    }
    Ok(out)
}

/// Parses `cargo llvm-cov show-env --pwsh` lines against the strict variable allowlist.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn parse_llvm_cov_pwsh_env(text: &str) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_end_matches('\r');
        if trimmed.is_empty() {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("$env:") else {
            return Err("Unexpected coverage environment output".into());
        };
        let Some((key, quoted)) = rest.split_once('=') else {
            return Err("Unexpected coverage environment output".into());
        };
        if key.is_empty()
            || !key
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err("Unexpected coverage environment output".into());
        }
        let Some(inner) = quoted.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
            return Err("Unexpected coverage environment output".into());
        };
        let value = decode_pwsh_unicode_escapes(inner)?;
        if !is_allowed_coverage_key(key) {
            return Err(format!("Unreviewed coverage variable: {key}").into());
        }
        out.push((key.to_string(), value));
    }
    Ok(out)
}

/// Unions non-empty LCOV profiles with an exact combined byte ceiling.
pub(crate) fn combine_lcov_files(inputs: &[PathBuf], output: &Path, max_bytes: u64) -> Result<u64> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut dest = File::create(output)?;
    let mut written: u64 = 0;
    for path in inputs {
        let mut src = File::open(path)?;
        let len = src.metadata()?.len();
        if len == 0 || written.saturating_add(len).saturating_add(1) > max_bytes {
            return Err("Coverage profile union exceeds its input bounds.".into());
        }
        let copied = io::copy(&mut src, &mut dest)?;
        if copied != len {
            return Err("Coverage profile copy length mismatch.".into());
        }
        dest.write_all(b"\n")?;
        written = written.saturating_add(copied).saturating_add(1);
    }
    dest.flush()?;
    Ok(written)
}

pub(crate) fn run(phase: &str, target: Option<&str>) -> Result<()> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return Err("Coverage only runs in GitHub Actions".into());
    }
    if phase != "report" {
        return Err(format!("xtask ci-coverage supports --phase report (got {phase})").into());
    }
    let target = target
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or("Coverage reporting requires the declared native target.")?;
    if !target
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err("Invalid coverage target identifier.".into());
    }
    let root = std::env::current_dir()?;
    let mut exports = Vec::with_capacity(2);
    for profile in ["debug", "release"] {
        let path = root.join(format!(".ci-work/coverage-{profile}.lcov"));
        let path_str = path.to_string_lossy().to_string();
        let mut args = vec![
            "llvm-cov",
            "report",
            "-v",
            "--lcov",
            "--output-path",
            &path_str,
        ];
        if profile == "release" {
            args.push("--release");
            args.push("--target");
            args.push(target);
        }
        let guard_name = format!("llvm-cov-{profile}");
        ci_tasks::invoke_guard_in_dir(&guard_name, 180, 60, "cargo", &args, None, None)
            .map_err(|_| format!("Coverage {profile} export failed"))?;
        exports.push(path);
    }
    let combined = root.join(".ci-work/coverage.lcov");
    combine_lcov_files(&exports, &combined, MAX_COMBINED_LCOV_BYTES)?;
    quality::coverage(
        &combined,
        &root.join(".ci-work/packages/reports/coverage.json"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_pwsh_coverage_env_and_rejects_unreviewed_keys() {
        let sample = "$env:RUSTFLAGS=\"`u{2d}`u{43}\"\n$env:CARGO_LLVM_COV=\"`u{31}\"\n";
        let pairs = parse_llvm_cov_pwsh_env(sample).unwrap();
        assert_eq!(
            pairs,
            vec![
                ("RUSTFLAGS".to_string(), "-C".to_string()),
                ("CARGO_LLVM_COV".to_string(), "1".to_string()),
            ]
        );

        let bad = "$env:PATH=\"`u{2f}\"\n";
        assert!(
            parse_llvm_cov_pwsh_env(bad)
                .unwrap_err()
                .to_string()
                .contains("Unreviewed coverage variable")
        );
    }

    #[test]
    fn combines_lcov_profiles_and_enforces_bounds() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.lcov");
        let b = tmp.path().join("b.lcov");
        let out = tmp.path().join("combined.lcov");
        fs::write(&a, b"SF:a.rs\nend_of_record").unwrap();
        fs::write(&b, b"SF:b.rs\nend_of_record").unwrap();

        let bytes = combine_lcov_files(&[a.clone(), b.clone()], &out, 1024).unwrap();
        assert_eq!(bytes as usize, fs::read(&out).unwrap().len());
        assert!(fs::read_to_string(&out).unwrap().contains("SF:b.rs"));

        // Rejects cap overflow and empty input files.
        assert!(combine_lcov_files(&[a.clone(), b], &out, 10).is_err());
        let empty = tmp.path().join("empty.lcov");
        fs::write(&empty, b"").unwrap();
        assert!(combine_lcov_files(&[a, empty], &out, 1024).is_err());
    }
}
