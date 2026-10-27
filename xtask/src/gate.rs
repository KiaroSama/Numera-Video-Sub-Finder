use serde_json::Value;
use std::{collections::HashSet, error::Error, path::Path};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const TARGETS: [&str; 6] = [
    "linux-x64",
    "linux-arm64",
    "windows-x64",
    "windows-arm64",
    "macos-x64",
    "macos-arm64",
];

fn sha(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|c| c.is_ascii_hexdigit())
}
fn read(path: &Path) -> Result<Value> {
    if path.metadata()?.len() > 8 * 1024 * 1024 {
        return Err("Oversized validation report".into());
    }
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
fn passed(report: &Value, source: &str, target: &str, minimum: u64) -> Result<()> {
    if report["schema"] != 1
        || report["source_sha"].as_str() != Some(source)
        || report["target"].as_str() != Some(target)
        || report["passed"] != true
        || report["tests"].as_u64().is_none_or(|n| n < minimum)
        || report["failed"].as_u64() != Some(0)
        || report["skipped"].as_u64() != Some(0)
    {
        return Err(
            format!("Incomplete, failed, skipped or wrong-source validation: {target}").into(),
        );
    }
    Ok(())
}

pub fn validate(reports: &Path, source: &str) -> Result<()> {
    if !sha(source, 40) {
        return Err("Invalid source revision".into());
    }
    for target in TARGETS {
        let package = read(&reports.join(format!("{target}.json")))?;
        if package["schema"] != 1
            || package["source_sha"].as_str() != Some(source)
            || package["target"].as_str() != Some(target)
        {
            return Err(format!("Wrong package source/target: {target}").into());
        }
        let files = package["files"]
            .as_array()
            .ok_or("Missing package inventory")?;
        if files.len() < 2 {
            return Err("Empty package inventory".into());
        }
        let mut names = HashSet::new();
        for file in files {
            let name = file["file"].as_str().ok_or("Missing artifact filename")?;
            if name.is_empty()
                || name.starts_with('/')
                || name.contains('\\')
                || name
                    .split('/')
                    .any(|p| p.is_empty() || p == ".." || p.starts_with('.'))
                || !names.insert(name)
                || file["sha256"].as_str().is_none_or(|h| !sha(h, 64))
                || file["bytes"].as_u64().is_none()
            {
                return Err("Invalid artifact inventory".into());
            }
        }
        for (kind, minimum) in [
            ("standalone", 3),
            ("gui", 9),
            ("production", 4),
            ("suites", 1),
            ("bundles", 2),
            ("maintenance", 5),
            (
                "multilingual",
                if target.starts_with("windows-") { 6 } else { 7 },
            ),
            ("runtime-pack", 5),
            ("update-assets", 2),
            ("worker-faults", 5),
            ("specialist", 1),
            ("visual-model", 1),
        ] {
            passed(
                &read(&reports.join(format!("{target}-{kind}.json")))?,
                source,
                target,
                minimum,
            )?;
        }
    }
    passed(
        &read(&reports.join("linux-x64-media-sanitizer.json"))?,
        source,
        "linux-x64",
        6,
    )?;
    println!(
        "Six exact-source native package/runtime/GUI/suite reports passed; release qualification is a separate mandatory gate"
    );
    Ok(())
}

pub fn release(reports: &Path, source: &str) -> Result<()> {
    validate(reports, source)?;
    let record = read(&reports.join("qualification.json"))?;
    if record["source_sha"].as_str() != Some(source) || record["release_ready"] != true {
        return Err("Release qualification absent or not ready".into());
    }
    for group in [
        "physical_gpu",
        "signing",
        "provenance",
        "real_corpus_annotation",
        "coverage",
        "mutation",
        "installer_lifecycle",
    ] {
        if record["evidence"][group]["state"] != "PASS" {
            return Err(format!("Release blocked: {group}").into());
        }
    }
    let families = record["families"]
        .as_array()
        .ok_or("Missing CT execution inventory")?;
    if families.len() != 30 {
        return Err("Incomplete CT execution inventory".into());
    }
    for index in 1..=30 {
        let id = format!("CT-{index:02}");
        let rows: Vec<_> = families.iter().filter(|r| r["id"] == id).collect();
        if rows.len() != 1 {
            return Err(format!("Missing/duplicate {id}").into());
        }
        let row = rows[0];
        if index == 23 {
            if row["state"] != "NOT_APPLICABLE" || row["reason"].as_str().is_none_or(str::is_empty)
            {
                return Err("Mobile scope disposition missing".into());
            }
        } else if row["state"] != "PASS"
            || row["tests"].as_u64().is_none_or(|n| n == 0)
            || row["source_sha"].as_str() != Some(source)
        {
            return Err(format!("Missing actual CT evidence: {id}").into());
        }
    }
    for (metric, floor) in [
        ("coverage", 0.80),
        ("critical_coverage", 0.90),
        ("changed_line_coverage", 0.90),
        ("critical_mutation", 0.80),
    ] {
        if record["metrics"][metric]
            .as_f64()
            .is_none_or(|n| !n.is_finite() || n < floor || n > 1.)
        {
            return Err(format!("Qualification floor not met: {metric}").into());
        }
    }
    for (language, maximum) in [("en", 0.02), ("fa", 0.05), ("ja", 0.05)] {
        let row = &record["quality"][language];
        if row["independent_real_annotations"] != true
            || row["events"].as_u64().is_none_or(|n| n == 0)
            || row["cer"]
                .as_f64()
                .is_none_or(|n| !n.is_finite() || n < 0. || n > maximum)
            || row["recall"]
                .as_f64()
                .is_none_or(|n| !n.is_finite() || n < 0.98 || n > 1.)
            || row["boundary_pass"] != true
        {
            return Err(format!("Real-language qualification missing/failed: {language}").into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn empty_wrong_sha_failed_skipped_and_zero_test_reports_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(validate(dir.path(), "short").is_err());
        assert!(validate(dir.path(), &"g".repeat(40)).is_err());
        assert!(validate(dir.path(), &"a".repeat(40)).is_err());
        let source = "a".repeat(40);
        let good = json!({"schema":1,"source_sha":source,"target":"linux-x64","passed":true,"tests":5,"failed":0,"skipped":0});
        assert!(passed(&good, &source, "linux-x64", 5).is_ok());
        assert!(passed(&good, &source, "linux-x64", 6).is_err());
        for (field, value) in [
            ("source_sha", json!("b".repeat(40))),
            ("target", json!("linux-arm64")),
            ("tests", json!(0)),
            ("failed", json!(1)),
            ("skipped", json!(1)),
            ("passed", json!(false)),
        ] {
            let mut bad = good.clone();
            bad[field] = value;
            assert!(passed(&bad, &source, "linux-x64", 5).is_err());
        }
        assert!(release(dir.path(), &source).is_err());
    }
}
