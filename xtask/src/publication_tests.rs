use super::*;
use serde_json::json;

const SOURCE: &str = "0123456789abcdef0123456789abcdef01234567";
const REPO: &str = "KiaroSama/Numera-Video-Sub-Finder";

fn verified_run() -> Value {
    json!({
        "id": 42,
        "head_sha": SOURCE,
        "repository": {"full_name": REPO},
        "head_repository": {"full_name": REPO},
        "path": ".github/workflows/verify.yml",
        "event": "workflow_dispatch",
        "status": "completed",
        "conclusion": "success"
    })
}

fn reports() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("reports directory");
    for target in ["linux-x64", "linux-arm64", "windows-x64", "windows-arm64"] {
        let package = json!({
            "schema": 1, "source_sha": SOURCE, "target": target,
            "files": [
                {"file": "gui", "sha256": "a".repeat(64), "bytes": 1},
                {"file": "cli", "sha256": "b".repeat(64), "bytes": 1}
            ]
        });
        std::fs::write(
            root.path().join(format!("{target}.json")),
            serde_json::to_vec(&package).expect("package JSON"),
        )
        .expect("write package");
        for kind in [
            "standalone",
            "gui",
            "production",
            "suites",
            "bundles",
            "maintenance",
            "multilingual",
            "runtime-pack",
            "update-assets",
            "worker-faults",
            "specialist",
            "visual-model",
            "media-sanitizer",
        ] {
            let row = json!({
                "schema": 1, "source_sha": SOURCE, "target": target,
                "passed": true, "tests": 10, "failed": 0, "skipped": 0
            });
            std::fs::write(
                root.path().join(format!("{target}-{kind}.json")),
                serde_json::to_vec(&row).expect("report JSON"),
            )
            .expect("write report");
        }
    }
    std::fs::write(
        root.path().join("run.json"),
        serde_json::to_vec(&verified_run()).expect("run JSON"),
    )
    .expect("write run");
    root
}

#[test]
fn stable_publication_and_qualification_only_require_full_evidence() {
    let root = reports();
    let evidence = root.path().join("run.json");
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    assert!(verify(root.path(), SOURCE, &evidence, REPO, true, true, &tag).is_ok());
    for (publish, prerelease) in [(true, false), (false, false), (false, true)] {
        let error = verify(
            root.path(),
            SOURCE,
            &evidence,
            REPO,
            publish,
            prerelease,
            &tag,
        )
        .expect_err("missing full qualification must block stable or qualification-only runs");
        assert!(
            error.downcast_ref::<std::io::Error>().is_some()
                || error.to_string().contains("qualification"),
            "the explicit development path passed, so this must reach the full qualification gate"
        );
    }
    assert!(verify(root.path(), SOURCE, &evidence, REPO, true, true, "v999.0.0").is_err());
}

#[test]
fn failed_missing_stale_and_foreign_verification_runs_are_rejected() {
    let good = verified_run();
    assert!(verify_run(&good, SOURCE, REPO).is_ok());
    assert!(verify_run(&Value::Null, SOURCE, REPO).is_err());
    for (field, value) in [
        ("head_sha", json!("f".repeat(40))),
        ("repository", json!({"full_name": "other/repository"})),
        ("head_repository", json!({"full_name": "other/repository"})),
        ("path", json!(".github/workflows/gitleaks.yml")),
        ("event", json!("pull_request")),
        ("status", json!("in_progress")),
        ("conclusion", json!("failure")),
        ("conclusion", Value::Null),
        ("id", json!(0)),
    ] {
        let mut bad = good.clone();
        bad[field] = value;
        assert!(verify_run(&bad, SOURCE, REPO).is_err(), "accepted {field}");
    }
}

#[test]
fn development_prerelease_still_requires_every_native_report() {
    let root = reports();
    let evidence = root.path().join("run.json");
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    std::fs::remove_file(root.path().join("windows-arm64-bundles.json")).expect("remove fixture");
    assert!(verify(root.path(), SOURCE, &evidence, REPO, true, true, &tag).is_err());
}

#[test]
fn full_matching_qualification_allows_stable_publication() {
    let root = reports();
    let evidence = root.path().join("run.json");
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    let families: Vec<_> = (1..=30)
        .map(|index| {
            if index == 23 {
                json!({"id": "CT-23", "state": "NOT_APPLICABLE", "reason": "Desktop-only scope"})
            } else {
                json!({"id": format!("CT-{index:02}"), "state": "PASS", "tests": 1, "source_sha": SOURCE})
            }
        })
        .collect();
    let qualification = json!({
        "source_sha": SOURCE,
        "release_ready": true,
        "evidence": {
            "physical_gpu": {"state": "PASS"}, "signing": {"state": "PASS"},
            "provenance": {"state": "PASS"}, "real_corpus_annotation": {"state": "PASS"},
            "coverage": {"state": "PASS"}, "mutation": {"state": "PASS"},
            "installer_lifecycle": {"state": "PASS"}
        },
        "families": families,
        "metrics": {"coverage": 0.95, "critical_coverage": 0.95, "changed_line_coverage": 0.95, "critical_mutation": 0.95},
        "quality": {
            "en": {"independent_real_annotations": true, "events": 10, "cer": 0.01, "recall": 0.99, "boundary_pass": true},
            "fa": {"independent_real_annotations": true, "events": 10, "cer": 0.04, "recall": 0.99, "boundary_pass": true},
            "ja": {"independent_real_annotations": true, "events": 10, "cer": 0.04, "recall": 0.99, "boundary_pass": true}
        }
    });
    std::fs::write(
        root.path().join("qualification.json"),
        serde_json::to_vec(&qualification).expect("qualification JSON"),
    )
    .expect("write qualification");
    assert!(verify(root.path(), SOURCE, &evidence, REPO, true, false, &tag).is_ok());
    assert!(verify(root.path(), SOURCE, &evidence, REPO, false, true, "").is_ok());
}

#[cfg(unix)]
#[test]
fn existing_stable_assets_cannot_be_replaced_by_development_publication() {
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};

    // Execute the actual workflow shell body with a recording GitHub CLI substitute.
    // A failure after upload is too late: unqualified assets would already be public.
    let workflow = include_str!("../../.github/workflows/release.yml");
    let step = workflow
        .split_once("      - name: Publish GitHub Release\n")
        .expect("publication step")
        .1;
    let script = step
        .split_once("        run: |\n")
        .expect("publication script")
        .1
        .lines()
        .map(|line| line.strip_prefix("          ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");
    let root = tempfile::tempdir().expect("publication fixture");
    let bin = root.path().join("bin");
    fs::create_dir(&bin).expect("stub directory");
    let gh = bin.join("gh");
    fs::write(
        &gh,
        r#"#!/usr/bin/env bash
set -euo pipefail
log() { printf '[%s] [%s] [RELEASE-TEST] %s\n' "$(date -u '+%Y-%m-%d %H:%M:%S UTC')" "$1" "$2" >&2; }
log DEBUG 'Recording the requested release operation'
printf '%s\n' "$*" >> "$GH_CALLS"
case "$1 $2" in
  'release view')
    if [[ " $* " == *' --json '* ]]; then printf '%s\n' "$EXISTING_PRERELEASE"; fi
    ;;
  'release upload'|'release edit') log INFO 'Recorded a release write' ;;
  *) log ERROR 'Unexpected release operation'; exit 41 ;;
esac
"#,
    )
    .expect("write GitHub CLI substitute");
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).expect("executable substitute");
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").expect("runner PATH"),
    )))
    .expect("fixture PATH");
    for (requested, existing, accepted) in [
        ("true", "false", false),
        ("true", "null", false),
        ("true", "true", true),
        ("false", "false", true),
    ] {
        let calls = root.path().join("calls.log");
        fs::write(&calls, "").expect("clear call recording");
        let result = Command::new("bash")
            .args(["-c", &script])
            .current_dir(root.path())
            .env("PATH", &path)
            .env("GH_CALLS", &calls)
            .env("EXISTING_PRERELEASE", existing)
            .env("IS_PRERELEASE", requested)
            .env("TAG", "v0.4.0")
            .env("SOURCE_SHA", SOURCE)
            .env("GITHUB_REPOSITORY", REPO)
            .env("EVIDENCE_RUN", "42")
            .output()
            .expect("execute actual publication body");
        assert_eq!(
            result.status.success(),
            accepted,
            "requested={requested} existing={existing}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let recorded = fs::read_to_string(calls).expect("read operation recording");
        assert_eq!(recorded.contains("release upload"), accepted, "{recorded}");
        assert_eq!(recorded.contains("release edit"), accepted, "{recorded}");
    }
}
