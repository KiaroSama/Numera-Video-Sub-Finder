use serde_json::json;
use std::{collections::BTreeMap, error::Error, path::Path};
type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn coverage(input: &Path, output: &Path) -> Result<()> {
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
    let total: usize = files.values().map(BTreeMap::len).sum();
    let covered: usize = files
        .values()
        .map(|lines| lines.values().filter(|n| **n).count())
        .sum();
    let critical: Vec<_> = files
        .iter()
        .filter(|(p, _)| {
            p.starts_with("crates/core/src/")
                || p.ends_with("/storage.rs")
                || p.ends_with("/extraction.rs")
                || p.ends_with("/resume.rs")
                || p.ends_with("/queue.rs")
        })
        .collect();
    let critical_total: usize = critical.iter().map(|(_, lines)| lines.len()).sum();
    let critical_hit: usize = critical
        .iter()
        .map(|(_, lines)| lines.values().filter(|n| **n).count())
        .sum();
    let ratio = covered as f64 / total as f64;
    let critical_ratio = if critical_total > 0 {
        critical_hit as f64 / critical_total as f64
    } else {
        0.
    };
    let native_pass = ratio >= 0.80 && critical_ratio >= 0.90;
    let record = json!({"schema":1,"source_sha":std::env::var("GITHUB_SHA")?,"scope":"native-instrumented-owned-source","lines":total,"covered":covered,"coverage":ratio,"critical_coverage":critical_ratio,"passed":native_pass,"full_product_qualified":false,"remaining":"WASM UI browser coverage and changed-line/native-platform union require separate evidence","files":files});
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
        coverage(&input, &output).unwrap();
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(report["lines"], 3);
        assert_eq!(report["covered"], 3);
        assert_eq!(report["coverage"], 1.0);
        assert_eq!(report["critical_coverage"], 1.0);
        assert_eq!(report["passed"], true);
    }

    #[test]
    fn malformed_and_empty_quality_data_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input");
        let output = dir.path().join("report");
        std::fs::write(&input, b"TN:\n").unwrap();
        assert!(coverage(&input, &output).is_err());
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
