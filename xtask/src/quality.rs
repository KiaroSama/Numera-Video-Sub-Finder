use serde_json::json;
use std::{collections::BTreeMap, error::Error, path::Path};
type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn coverage(input: &Path, output: &Path) -> Result<()> {
    let scope = crate::coverage_scope::Scope::load(&std::env::current_dir()?)?;
    coverage_scoped(input, output, &scope)
}

fn coverage_scoped(
    input: &Path,
    output: &Path,
    scope: &crate::coverage_scope::Scope,
) -> Result<()> {
    let text = std::fs::read_to_string(input)?;
    if text.len() > 64 * 1024 * 1024 {
        return Err("Oversized coverage report".into());
    }
    let root = std::env::current_dir()?
        .canonicalize()?
        .to_string_lossy()
        .replace('\\', "/");
    let mut files: BTreeMap<String, BTreeMap<u64, bool>> = BTreeMap::new();
    let mut current = None;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("SF:") {
            let name = name.replace('\\', "/");
            current = name
                .strip_prefix(&(root.clone() + "/"))
                .filter(|p| p.starts_with("crates/") && p.contains("/src/"))
                .map(str::to_owned);
        } else if let Some(data) = line.strip_prefix("DA:")
            && let Some(name) = &current
        {
            let mut fields = data.split(',');
            let number = fields
                .next()
                .ok_or("Missing coverage line")?
                .parse::<u64>()?;
            let hit = fields
                .next()
                .ok_or("Missing coverage count")?
                .parse::<u64>()?
                > 0;
            files
                .entry(name.clone())
                .or_default()
                .entry(number)
                .and_modify(|old| *old |= hit)
                .or_insert(hit);
        }
    }
    if files.is_empty() {
        return Err("No instrumentable owned source in coverage".into());
    }
    let full = crate::coverage_scope::counts(&files);
    let measured = crate::coverage_scope::counts(&scope.measured(&files)?);
    let ratio = measured.covered as f64 / measured.lines as f64;
    let critical_ratio = measured.critical_covered as f64 / measured.critical_lines as f64;
    let native_pass = ratio >= 0.80 && critical_ratio >= 0.90;
    let record = json!({
        "schema":1,"source_sha":std::env::var("GITHUB_SHA")?,
        "scope":"native-owned-source-with-reviewed-external-boundaries",
        "lines":measured.lines,"covered":measured.covered,"coverage":ratio,
        "critical_coverage":critical_ratio,"passed":native_pass,
        "full":full,"measured":measured,
        "excluded_lines":full.lines-measured.lines,
        "excluded_covered":full.covered-measured.covered,
        "exclusions":scope.exclusions,"full_product_qualified":false,
        "remaining":"Excluded boundary acceptance, WASM UI browser coverage and changed-line/native-platform union require separate evidence",
        "files":files
    });
    std::fs::write(output, serde_json::to_vec_pretty(&record)?)?;
    if !native_pass {
        return Err(format!(
            "Coverage below retained floors: overall={ratio:.3}, critical={critical_ratio:.3}"
        )
        .into());
    }
    Ok(())
}

pub fn mutation(input: &Path, output: &Path) -> Result<()> {
    let raw = std::fs::read(input)?;
    if raw.len() > 32 * 1024 * 1024 {
        return Err("Oversized mutation report".into());
    }
    let report: serde_json::Value = serde_json::from_slice(&raw)?;
    let caught = report["caught"].as_u64().ok_or("Missing caught count")?;
    let missed = report["missed"].as_u64().ok_or("Missing missed count")?;
    let timeout = report["timeout"].as_u64().ok_or("Missing timeout count")?;
    let total = caught
        .checked_add(missed)
        .ok_or("Mutation count overflow")?;
    let score = if total > 0 {
        caught as f64 / total as f64
    } else {
        0.
    };
    let pass = total > 0 && timeout == 0 && score >= 0.80;
    std::fs::write(
        output,
        serde_json::to_vec_pretty(
            &json!({"schema":1,"source_sha":std::env::var("GITHUB_SHA")?,"caught":caught,"missed":missed,"timeout":timeout,"executable":total,"score":score,"passed":pass,"critical_reviewed_qualified":false,"note":"Generated executable mutation score; equivalent/critical candidate independent review still required. Timeouts/unviable never counted as kills."}),
        )?,
    )?;
    if !pass {
        return Err("Mutation floor not met or execution incomplete".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coverage_unions_profile_hits_without_duplicating_source_lines() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("profiles.lcov");
        let output = dir.path().join("coverage.json");
        let root = std::env::current_dir().unwrap().canonicalize().unwrap();
        let source = root.join("crates/core/src/time.rs");
        std::fs::write(
            &input,
            format!(
                "SF:{}\nDA:10,1\nDA:11,0\nend_of_record\nSF:{}\nDA:10,0\nDA:11,3\nDA:12,1\nend_of_record\n",
                source.display(), source.display()
            ),
        ).unwrap();
        let scope = crate::coverage_scope::Scope::parse(
            br#"{"schema":1,"approval":"D012","exclusions":[]}"#,
            &root,
        )
        .unwrap();
        coverage_scoped(&input, &output, &scope).unwrap();
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(report["lines"], 3);
        assert_eq!(report["covered"], 3);
        assert_eq!(report["coverage"], 1.0);
        assert_eq!(report["critical_coverage"], 1.0);
        assert_eq!(report["passed"], true);
    }

    #[test]
    fn scoped_report_retains_full_hits_and_fails_below_unchanged_floors() {
        use sha2::{Digest, Sha256};
        let dir = tempfile::tempdir().unwrap();
        let path = "crates/desktop/src/files.rs";
        let source = "fn picker() {\n    dialog();\n    validate();\n}\n";
        std::fs::create_dir_all(dir.path().join("crates/desktop/src")).unwrap();
        std::fs::write(dir.path().join(path), source).unwrap();
        let manifest = json!({"schema":1,"approval":"D012","exclusions":[{
            "path":path,"start":2,"end":2,
            "source_sha256":format!("{:x}", Sha256::digest(source.as_bytes())),
            "category":"native-dialog","reason":"Native picker invocation",
            "acceptance":"OPEN: native selection and cancellation"
        }]});
        let scope = crate::coverage_scope::Scope::parse(
            &serde_json::to_vec(&manifest).unwrap(),
            dir.path(),
        )
        .unwrap();
        let root = std::env::current_dir().unwrap().canonicalize().unwrap();
        let input = dir.path().join("profiles.lcov");
        let output = dir.path().join("coverage.json");
        std::fs::write(
            &input,
            format!(
                "SF:{}\nDA:2,0\nDA:3,0\nend_of_record\nSF:{}\nDA:1,1\nend_of_record\n",
                root.join(path).display(),
                root.join("crates/core/src/time.rs").display(),
            ),
        )
        .unwrap();
        let error = coverage_scoped(&input, &output, &scope).unwrap_err();
        assert!(error.to_string().contains("Coverage below retained floors"));
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(report["full"]["lines"], 3);
        assert_eq!(report["full"]["covered"], 1);
        assert_eq!(report["measured"]["lines"], 2);
        assert_eq!(report["measured"]["covered"], 1);
        assert_eq!(report["excluded_lines"], 1);
        assert_eq!(report["excluded_covered"], 0);
        assert_eq!(report["coverage"], 0.5);
        assert_eq!(report["critical_coverage"], 1.0);
        assert_eq!(report["passed"], false);
        assert_eq!(report["full_product_qualified"], false);
        assert_eq!(report["files"][path]["2"], false);
        assert_eq!(report["files"][path]["3"], false);
        assert_eq!(report["exclusions"], manifest["exclusions"]);
    }

    #[test]
    fn malformed_and_empty_quality_data_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input");
        let output = dir.path().join("report");
        std::fs::write(&input, b"TN:\n").unwrap();
        let scope = crate::coverage_scope::Scope::parse(
            br#"{"schema":1,"approval":"D012","exclusions":[]}"#,
            dir.path(),
        )
        .unwrap();
        assert!(coverage_scoped(&input, &output, &scope).is_err());
        for data in [
            "{}",
            "{\"caught\":0,\"missed\":0,\"timeout\":0}",
            "{\"caught\":9,\"missed\":1,\"timeout\":1}",
        ] {
            std::fs::write(&input, data).unwrap();
            assert!(mutation(&input, &output).is_err());
        }
    }
}
