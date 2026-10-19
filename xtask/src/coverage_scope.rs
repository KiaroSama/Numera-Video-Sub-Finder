use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    error::Error,
    io::Read,
    path::{Component, Path},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
pub type LineMap = BTreeMap<String, BTreeMap<u64, bool>>;

#[derive(Debug, PartialEq, Serialize)]
pub struct Counts {
    pub lines: usize,
    pub covered: usize,
    pub critical_lines: usize,
    pub critical_covered: usize,
}

pub fn counts(files: &LineMap) -> Counts {
    let mut result = Counts {
        lines: 0,
        covered: 0,
        critical_lines: 0,
        critical_covered: 0,
    };
    for (path, lines) in files {
        let covered = lines.values().filter(|hit| **hit).count();
        result.lines += lines.len();
        result.covered += covered;
        if path.starts_with("crates/core/src/")
            || ["/storage.rs", "/extraction.rs", "/resume.rs", "/queue.rs"]
                .iter()
                .any(|suffix| path.ends_with(suffix))
        {
            result.critical_lines += lines.len();
            result.critical_covered += covered;
        }
    }
    result
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    schema: u32,
    approval: String,
    pub exclusions: Vec<Exclusion>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Exclusion {
    path: String,
    start: u64,
    end: u64,
    source_sha256: String,
    category: String,
    reason: String,
    acceptance: String,
}

fn bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("Coverage scope input exceeds size limit".into());
    }
    Ok(bytes)
}

// Git may convert checkout newlines; identity still binds every source character.
fn digest(source: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(source.replace("\r\n", "\n").as_bytes())
    )
}

impl Scope {
    pub fn load(root: &Path) -> Result<Self> {
        Self::parse(&bounded(&root.join("ci/coverage-scope.json"), 65536)?, root)
    }

    pub(crate) fn parse(bytes: &[u8], root: &Path) -> Result<Self> {
        if bytes.len() > 65536 {
            return Err("Oversized coverage scope manifest".into());
        }
        let scope: Self = serde_json::from_slice(bytes)?;
        if scope.schema != 1 || scope.approval != "D012" || scope.exclusions.len() > 128 {
            return Err("Invalid coverage scope schema, approval or count".into());
        }
        let root = root.canonicalize()?;
        let mut ranges: BTreeMap<&str, Vec<(u64, u64)>> = BTreeMap::new();
        for entry in &scope.exclusions {
            if !["native-dialog", "installation-network"].contains(&entry.category.as_str())
                || entry.reason.trim().is_empty()
                || entry.reason.len() > 1024
                || entry.acceptance.trim().is_empty()
                || entry.acceptance.len() > 1024
                || entry.start == 0
                || entry.end < entry.start
                || entry.end - entry.start > 31
                || entry.source_sha256.len() != 64
                || !entry
                    .source_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
                || entry.path.len() > 256
                || entry.path.contains(['\\', ':'])
                || !entry.path.ends_with(".rs")
                || !(entry.path.starts_with("crates/desktop/src/")
                    || entry.path.starts_with("crates/app/src/"))
                || !Path::new(&entry.path)
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
            {
                return Err("Invalid coverage boundary record".into());
            }
            let mut path = root.clone();
            for part in Path::new(&entry.path).components() {
                path.push(part);
                if path.is_symlink() {
                    return Err("Coverage scope source must not traverse symlinks".into());
                }
            }
            if !path.canonicalize()?.starts_with(&root) {
                return Err("Coverage scope source escapes workspace".into());
            }
            let bytes = bounded(&path, 1024 * 1024)?;
            let source = std::str::from_utf8(&bytes)?;
            if digest(source) != entry.source_sha256 || entry.end > source.lines().count() as u64 {
                return Err(format!("Stale coverage boundary source: {}", entry.path).into());
            }
            let prior = ranges.entry(&entry.path).or_default();
            if prior
                .iter()
                .any(|(start, end)| entry.start <= *end && entry.end >= *start)
            {
                return Err("Overlapping coverage boundary ranges".into());
            }
            prior.push((entry.start, entry.end));
        }
        Ok(scope)
    }

    pub fn measured(&self, files: &LineMap) -> Result<LineMap> {
        let mut measured = files.clone();
        for entry in &self.exclusions {
            let lines = measured
                .get_mut(&entry.path)
                .ok_or("Coverage boundary source missing from report")?;
            if !lines
                .keys()
                .any(|line| (entry.start..=entry.end).contains(line))
            {
                return Err("Coverage boundary has no observed instrumentable lines".into());
            }
            lines.retain(|line, _| !(entry.start..=entry.end).contains(line));
        }
        let count = counts(&measured);
        if count.lines == 0 || count.critical_lines == 0 {
            return Err("Empty measured or critical coverage scope".into());
        }
        Ok(measured)
    }
}

#[cfg(test)]
#[path = "coverage_scope_tests.rs"]
mod tests;
