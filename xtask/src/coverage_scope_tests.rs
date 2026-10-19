use super::*;
use serde_json::json;

fn fixture() -> (tempfile::TempDir, serde_json::Value, LineMap) {
    let dir = tempfile::tempdir().unwrap();
    let path = "crates/desktop/src/files.rs";
    std::fs::create_dir_all(dir.path().join("crates/desktop/src")).unwrap();
    let source = "fn picker() {\n    dialog();\n    validate();\n}\n";
    std::fs::write(dir.path().join(path), source).unwrap();
    let manifest = json!({"schema":1,"approval":"D012","exclusions":[{
        "path":path,"start":2,"end":2,"source_sha256":digest(source),
        "category":"native-dialog","reason":"Native file picker invocation",
        "acceptance":"Observe picker selection and cancellation on each supported desktop"
    }]});
    let files = BTreeMap::from([
        (
            path.into(),
            BTreeMap::from([(1, true), (2, false), (3, false)]),
        ),
        (
            "crates/core/src/time.rs".into(),
            BTreeMap::from([(1, true)]),
        ),
    ]);
    (dir, manifest, files)
}

#[test]
fn scope_preserves_raw_hits_and_reconciles_full_and_measured_counts() {
    let (dir, manifest, files) = fixture();
    let original = files.clone();
    let scope = Scope::parse(&serde_json::to_vec(&manifest).unwrap(), dir.path()).unwrap();
    let measured = scope.measured(&files).unwrap();
    assert_eq!(files, original);
    assert_eq!(
        counts(&files),
        Counts {
            lines: 4,
            covered: 2,
            critical_lines: 1,
            critical_covered: 1
        }
    );
    assert_eq!(
        counts(&measured),
        Counts {
            lines: 3,
            covered: 2,
            critical_lines: 1,
            critical_covered: 1
        }
    );
    assert_eq!(measured["crates/desktop/src/files.rs"].get(&2), None);
    assert_eq!(
        measured["crates/desktop/src/files.rs"].get(&3),
        Some(&false)
    );
}

#[test]
fn invalid_and_stale_scope_records_fail_closed() {
    let (dir, base, _) = fixture();
    for (field, value) in [
        ("path", json!("../outside.rs")),
        ("path", json!("crates/desktop/src/missing.rs")),
        ("source_sha256", json!("a".repeat(64))),
        ("start", json!(0)),
        ("end", json!(100)),
        ("category", json!("low-coverage")),
        ("reason", json!("")),
        ("acceptance", json!("")),
    ] {
        let mut manifest = base.clone();
        manifest["exclusions"][0][field] = value;
        assert!(
            Scope::parse(&serde_json::to_vec(&manifest).unwrap(), dir.path()).is_err(),
            "{field}"
        );
    }
    let mut overlap = base.clone();
    overlap["exclusions"]
        .as_array_mut()
        .unwrap()
        .push(base["exclusions"][0].clone());
    assert!(Scope::parse(&serde_json::to_vec(&overlap).unwrap(), dir.path()).is_err());
    assert!(Scope::parse(b"{}", dir.path()).is_err());
    std::fs::write(dir.path().join("crates/desktop/src/files.rs"), "changed\n").unwrap();
    assert!(Scope::parse(&serde_json::to_vec(&base).unwrap(), dir.path()).is_err());
}

#[test]
fn no_observed_boundary_or_empty_critical_scope_cannot_pass() {
    let (dir, manifest, mut files) = fixture();
    let scope = Scope::parse(&serde_json::to_vec(&manifest).unwrap(), dir.path()).unwrap();
    files.remove("crates/desktop/src/files.rs");
    assert!(scope.measured(&files).is_err());
    let (_, mut manifest, _) = fixture();
    manifest["exclusions"][0]["path"] = json!("crates/core/src/time.rs");
    std::fs::create_dir_all(dir.path().join("crates/core/src")).unwrap();
    std::fs::write(dir.path().join("crates/core/src/time.rs"), "critical();\n").unwrap();
    manifest["exclusions"][0]["start"] = json!(1);
    manifest["exclusions"][0]["end"] = json!(1);
    manifest["exclusions"][0]["source_sha256"] = json!(digest("critical();\n"));
    // Core policy is not an external dialog/network adapter and cannot be scoped out.
    assert!(Scope::parse(&serde_json::to_vec(&manifest).unwrap(), dir.path()).is_err());
    let empty = Scope::parse(
        br#"{"schema":1,"approval":"D012","exclusions":[]}"#,
        dir.path(),
    )
    .unwrap();
    assert!(empty.measured(&LineMap::new()).is_err());
    let no_critical = BTreeMap::from([(
        "crates/app/src/paths.rs".into(),
        BTreeMap::from([(1, true)]),
    )]);
    assert!(empty.measured(&no_critical).is_err());
}

#[test]
fn source_identity_ignores_checkout_line_endings_not_content() {
    assert_eq!(
        digest("hello world"),
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
    );
    assert_eq!(digest("first\r\nsecond\r\n"), digest("first\nsecond\n"));
    assert_ne!(digest("first\nsecond\n"), digest("first\nchanged\n"));
}
