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
