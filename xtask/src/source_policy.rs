//! Validates tracked source publication boundaries, strict UTF-8, Rust 800-line ceiling,
//! heuristic secret absence, and CI workflow claims for every Cargo manifest.

use std::{collections::HashSet, error::Error, path::Path, process::Command};

const FORBIDDEN_DIRS: &[&str] = &[
    ".ai",
    ".specify",
    "specs",
    ".claude",
    ".kiro",
    ".codex",
    ".agents",
    ".cursor",
    ".cline",
    "graphify-out",
    ".codebase-memory",
    "plans",
    ".ignoreme",
];

const FORBIDDEN_FILES: &[&str] = &[
    "secrets.md",
    "explain-AI.md",
    "CLAUDE.md",
    "AGENTS.md",
    "reference.md",
];

const TEXT_EXTS: &[&str] = &[
    "rs", "toml", "lock", "json", "yml", "yaml", "ps1", "md", "html", "css", "cmake",
];

const TEXT_NAMES: &[&str] = &["LICENSE", ".gitignore", ".graphifyignore", ".gitattributes"];

fn is_excluded_path(file: &str) -> bool {
    for seg in file.split('/') {
        if FORBIDDEN_DIRS.contains(&seg) {
            return true;
        }
    }
    let name = file.rsplit('/').next().unwrap_or(file);
    FORBIDDEN_FILES.contains(&name) || name == ".env" || name.starts_with(".env.")
}

fn is_text_file(file: &str) -> bool {
    if TEXT_NAMES.contains(&file) {
        return true;
    }
    file.rsplit_once('.')
        .is_some_and(|(_, ext)| TEXT_EXTS.contains(&ext))
}

fn has_secret_pattern(text: &str) -> bool {
    // Assemble PEM markers at runtime so this detector never embeds a literal
    // private-key header in tracked source (which trips both itself and Gitleaks).
    const PEM_PREFIX: &str = "-----BEG";
    for kind in ["", "RSA ", "EC ", "OPENSSH "] {
        let header = format!("{PEM_PREFIX}IN {kind}PRIVATE KEY-----");
        if text.contains(&header) {
            return true;
        }
    }
    for prefix in ["ghp_", "github_pat_"] {
        let mut rest = text;
        while let Some(idx) = rest.find(prefix) {
            let after = &rest[idx + prefix.len()..];
            let count = after
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count();
            if count >= 30 {
                return true;
            }
            rest = &after[count..];
        }
    }
    false
}

fn extract_job_body<'a>(workflow: &'a str, job_name: &str) -> Option<&'a str> {
    let header = format!("  {job_name}:");
    let lines: Vec<&str> = workflow.lines().collect();
    let start_line = lines
        .iter()
        .position(|l| l.trim_end_matches('\r') == header)?;
    let byte_start: usize = workflow
        .split_inclusive('\n')
        .take(start_line + 1)
        .map(str::len)
        .sum();
    let tail = &workflow[byte_start..];
    let mut byte_end = tail.len();
    let mut offset = 0;
    for raw in tail.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\r', '\n']);
        if let Some(rest) = line.strip_prefix("  ")
            && !rest.is_empty()
            && !rest.starts_with(' ')
            && let Some(name) = rest.strip_suffix(':')
            && !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            byte_end = offset;
            break;
        }
        offset += raw.len();
    }
    Some(&tail[..byte_end])
}

pub fn verify() -> Result<(), Box<dyn Error>> {
    let out = Command::new("git").arg("ls-files").output()?;
    if !out.status.success() {
        return Err("No tracked source inventory".into());
    }
    let listing = String::from_utf8(out.stdout)?;
    let files: Vec<&str> = listing.lines().filter(|s| !s.is_empty()).collect();
    if files.is_empty() {
        return Err("No tracked source inventory".into());
    }
    let file_set: HashSet<&str> = files.iter().copied().collect();

    let vendored = ["patches/glib-0.18.5"];
    for root in vendored {
        let patched = format!("{root}/PATCHED.md");
        let lic = format!("{root}/LICENSE");
        if !file_set.contains(patched.as_str()) || !file_set.contains(lic.as_str()) {
            return Err(format!("Vendored patch lacks provenance or license: {root}").into());
        }
    }

    for &file in &files {
        if is_excluded_path(file) {
            return Err("Excluded local/private content is tracked".into());
        }
        if is_text_file(file) {
            let bytes = std::fs::read(Path::new(file))?;
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| format!("Invalid UTF-8 in tracked text file: {file}"))?;
            let is_vendored = vendored
                .iter()
                .any(|root| file.starts_with(&format!("{root}/")));
            if file.ends_with(".rs") && !is_vendored && text.split('\n').count() > 800 {
                return Err(format!("Owned source exceeds file ceiling: {file}").into());
            }
            if has_secret_pattern(text) {
                return Err("Potential real secret in tracked content; publication stops".into());
            }
        }
    }

    let claims: &[(&str, &str, &str)] = &[
        ("crates/core", ".github/workflows/ci.yml", "native"),
        ("crates/app", ".github/workflows/ci.yml", "native"),
        ("crates/media-native", ".github/workflows/ci.yml", "native"),
        ("crates/cli", ".github/workflows/ci.yml", "native"),
        ("crates/ui", ".github/workflows/ci.yml", "native"),
        ("crates/desktop", ".github/workflows/ci.yml", "native"),
        ("xtask", ".github/workflows/ci.yml", "native"),
        ("fuzz", ".github/workflows/quality.yml", "fuzz"),
        ("ci/loom", ".github/workflows/quality.yml", "loom"),
        ("patches/glib-0.18.5", ".github/workflows/ci.yml", "native"),
    ];

    for &manifest in &files {
        let Some(pkg) = manifest.strip_suffix("/Cargo.toml") else {
            continue;
        };
        let Some(&(_, wf_path, job_name)) = claims.iter().find(|&&(p, _, _)| p == pkg) else {
            return Err(format!("Cargo package has no CI suite/build claim: {pkg}").into());
        };
        if !file_set.contains(wf_path) {
            return Err("Cargo package CI claim references an untracked workflow".into());
        }
        let wf = std::fs::read_to_string(wf_path)?;
        let Some(body) = extract_job_body(&wf, job_name) else {
            return Err("Cargo package CI claim references a missing job".into());
        };
        if pkg == "fuzz"
            && (!file_set.contains("xtask/src/ci_fuzz.rs") || !body.contains("ci-fuzz"))
        {
            return Err("Standalone fuzz package requires its actual portable runner".into());
        }
        if pkg == "ci/loom"
            && (!file_set.contains("scripts/ci/loom_runner.rs")
                || !body.contains("scripts/ci/loom_runner.rs"))
        {
            return Err("Standalone loom package requires its actual portable runner".into());
        }
    }

    for &wf_path in &files {
        if !wf_path.starts_with(".github/workflows/")
            || !(wf_path.ends_with(".yml") || wf_path.ends_with(".yaml"))
        {
            continue;
        }
        let wf = std::fs::read_to_string(wf_path)?;
        validate_workflow_policy(wf_path, &wf)?;
    }

    println!(
        "Tracked text/publication boundaries, workflow policy lint, and package CI inventory validated; heuristic secret check is not an exhaustive credential scan."
    );
    Ok(())
}

fn validate_workflow_policy(path: &str, text: &str) -> Result<(), Box<dyn Error>> {
    if !text.contains("permissions:") {
        return Err(format!("Workflow lacks explicit least-privilege permissions: {path}").into());
    }
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix("- uses:")
            .or_else(|| trimmed.strip_prefix("uses:"))
        {
            let target = rest.trim().split('#').next().unwrap_or("").trim();
            if target.starts_with("./") {
                continue;
            }
            let Some((_, reference)) = target.rsplit_once('@') else {
                return Err(
                    format!("Unpinned workflow action at {path}:{}: {target}", idx + 1).into(),
                );
            };
            let is_sha = reference.len() == 40 && reference.bytes().all(|b| b.is_ascii_hexdigit());
            if !is_sha {
                return Err(format!(
                    "Workflow action must be pinned to a 40-hex commit SHA at {path}:{}: {target}",
                    idx + 1
                )
                .into());
            }
        }
        if trimmed.contains("macos-") {
            return Err(format!(
                "Unrequested macOS runner forbidden by D-018 at {path}:{}",
                idx + 1
            )
            .into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_forbidden_paths_and_extracts_workflow_jobs() {
        assert!(is_excluded_path(".ai/memory.md"));
        assert!(is_excluded_path("nested/.env.production"));
        assert!(is_excluded_path("secrets.md"));
        assert!(!is_excluded_path("crates/core/src/lib.rs"));
        let sample = "name: Sample\njobs:\n  loom:\n    runs-on: ubuntu-24.04\n    steps:\n      - run: echo ok\n  fuzz:\n    runs-on: ubuntu-24.04\n";
        let body = extract_job_body(sample, "loom").expect("loom job");
        assert!(body.contains("echo ok"));
        assert!(!body.contains("fuzz:"));

        let good_wf = "name: Ok\npermissions:\n  contents: read\njobs:\n  a:\n    runs-on: ubuntu-24.04\n    steps:\n      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1\n      - uses: ./.github/workflows/ci.yml\n";
        assert!(validate_workflow_policy(".github/workflows/ok.yml", good_wf).is_ok());
        assert!(
            validate_workflow_policy(".github/workflows/no-perm.yml", "name: Bad\njobs: {}\n")
                .is_err()
        );
        assert!(
            validate_workflow_policy(
                ".github/workflows/unpinned.yml",
                "permissions:\n  contents: read\nsteps:\n  - uses: actions/checkout@v4\n"
            )
            .is_err()
        );
        assert!(
            validate_workflow_policy(
                ".github/workflows/mac.yml",
                "permissions:\n  contents: read\nruns-on: macos-15\n"
            )
            .is_err()
        );
    }
}
