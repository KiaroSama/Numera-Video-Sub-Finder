//! Stage-two CI helper ported from `scripts/ci/notices.ps1` into Rust.
//! Copies bounded Visual Studio distribution notices with link, dotfile,
//! budget, and SHA-256 verification.

use sha2::{Digest, Sha256};
use std::{
    collections::{HashSet, VecDeque},
    error::Error,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const MAX_ENTRIES: usize = 4096;
const MAX_DEPTH: usize = 16;
const MAX_REL_LEN: usize = 512;
const MAX_TOTAL_BYTES: u64 = 33_554_432;
const BUDGET_DURATION: Duration = Duration::from_secs(120);

#[cfg(windows)]
fn is_reparse_or_symlink(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_type().is_symlink() || (meta.file_attributes() & 0x0400) != 0
}

#[cfg(not(windows))]
fn is_reparse_or_symlink(meta: &fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

fn hash_file_hex(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 65_536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{b:02X}");
    }
    Ok(hex)
}

fn is_allowed_extension(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "txt" | "rtf" | "html" | "htm" | "pdf"
    )
}

fn is_forbidden_relative_path(relative: &str) -> bool {
    if relative.contains('\\') || relative.contains(':') {
        return true;
    }
    for segment in relative.split('/') {
        if segment.starts_with('.') {
            return true;
        }
        if matches!(
            segment.to_ascii_lowercase().as_str(),
            "secrets.md" | "explain-ai.md" | "claude.md" | "agents.md" | "reference.md"
        ) {
            return true;
        }
    }
    false
}

struct NoticeFile {
    source: PathBuf,
    relative: String,
    bytes: u64,
    hash: String,
}

/// Discovers and copies bounded Visual Studio license notices from `source` to
/// `destination`, returning sorted relative paths matching `scripts/ci/notices.ps1`.
pub(crate) fn copy_visual_studio_notices(source: &Path, destination: &Path) -> Result<Vec<String>> {
    if !source.is_dir() {
        return Err("Visual Studio distribution license terms are absent.".into());
    }
    let root = fs::canonicalize(source)?;
    let start = Instant::now();
    let mut pending = VecDeque::new();
    pending.push_back(root.clone());

    let mut files: Vec<NoticeFile> = Vec::new();
    let mut names_ci: HashSet<String> = HashSet::new();
    let mut total_bytes: u64 = 0;
    let mut entry_count: usize = 0;

    while let Some(dir) = pending.pop_front() {
        let dir_meta = fs::symlink_metadata(&dir)?;
        if is_reparse_or_symlink(&dir_meta) {
            return Err("Visual Studio notices may not contain links.".into());
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            entry_count += 1;
            if entry_count > MAX_ENTRIES {
                return Err("Visual Studio notice discovery exceeds its entry budget.".into());
            }
            if start.elapsed() >= BUDGET_DURATION {
                return Err("Visual Studio notice discovery exceeds its time budget.".into());
            }
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)?;
            if is_reparse_or_symlink(&meta) {
                return Err("Visual Studio notices may not contain links.".into());
            }
            let rel_path = path
                .strip_prefix(&root)
                .map_err(|_| "Unexpected Visual Studio notice path.")?;
            let mut relative = rel_path
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let depth = relative.split('/').count();
            if depth > MAX_DEPTH || relative.len() > MAX_REL_LEN {
                return Err("Unexpected Visual Studio notice path.".into());
            }
            if meta.is_dir() {
                pending.push_back(path);
                continue;
            }
            if !is_allowed_extension(&path) {
                continue;
            }
            if entry.file_name().to_str() == Some(".NET_Framework_EULA.rtf") {
                let prefix_len = relative.len() - ".NET_Framework_EULA.rtf".len();
                relative = format!("{}DOTNET_Framework_EULA.rtf", &relative[..prefix_len]);
            }
            if is_forbidden_relative_path(&relative)
                || !names_ci.insert(relative.to_ascii_lowercase())
            {
                return Err("Unexpected Visual Studio notice path.".into());
            }
            let size = meta.len();
            total_bytes = total_bytes
                .checked_add(size)
                .ok_or("Visual Studio notice bytes exceed their budget.")?;
            if total_bytes > MAX_TOTAL_BYTES {
                return Err("Visual Studio notice bytes exceed their budget.".into());
            }
            let hash = hash_file_hex(&path)?;
            files.push(NoticeFile {
                source: path,
                relative,
                bytes: size,
                hash,
            });
        }
    }

    if files.is_empty() {
        return Err("Visual Studio distribution license terms are absent.".into());
    }
    files.sort_by(|a, b| {
        a.relative
            .to_ascii_lowercase()
            .cmp(&b.relative.to_ascii_lowercase())
            .then_with(|| a.relative.cmp(&b.relative))
    });

    let mut copied = Vec::with_capacity(files.len());
    for file in files {
        if start.elapsed() >= BUDGET_DURATION {
            return Err("Visual Studio notice copying exceeds its budget.".into());
        }
        let out = destination.join(&file.relative);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Ok(prior) = fs::symlink_metadata(&out) {
            if prior.is_dir()
                || is_reparse_or_symlink(&prior)
                || prior.len() != file.bytes
                || hash_file_hex(&out)? != file.hash
            {
                return Err("Visual Studio notice destination bytes collide.".into());
            }
        } else {
            fs::copy(&file.source, &out)?;
        }
        let out_meta = fs::metadata(&out)?;
        if out_meta.len() != file.bytes || hash_file_hex(&out)? != file.hash {
            return Err("Copied Visual Studio notice bytes differ.".into());
        }
        copied.push(file.relative);
    }
    Ok(copied)
}

pub(crate) fn run(source: &Path, destination: &Path) -> Result<()> {
    for rel in copy_visual_studio_notices(source, destination)? {
        println!("{rel}");
    }
    Ok(())
}

pub(crate) fn contracts() -> Result<()> {
    crate::ci_guard::hosted("Native notice contracts run only on GitHub-hosted Actions.")?;
    let state = crate::ci_guard::state_dir(&crate::ci_guard::root())?;
    let work = tempfile::Builder::new()
        .prefix("notice-contract-")
        .tempdir_in(state)?;
    let source = work.path().join("source");
    let locale = source.join("1033");
    let destination = work.path().join("destination");
    fs::create_dir_all(&locale)?;
    let bytes = b"{\\rtf1 \xe9}";
    let original = locale.join(".NET_Framework_EULA.rtf");
    let copied = destination.join("1033/DOTNET_Framework_EULA.rtf");
    fs::write(&original, bytes)?;
    fs::write(locale.join("ThirdPartyNotices.txt"), bytes)?;
    let files = copy_visual_studio_notices(&source, &destination)?;
    if files
        != [
            "1033/DOTNET_Framework_EULA.rtf",
            "1033/ThirdPartyNotices.txt",
        ]
    {
        return Err("Native notice destination inventory differs from its contract.".into());
    }
    if fs::read(original)? != bytes || fs::read(&copied)? != bytes {
        return Err("Notice bytes changed.".into());
    }
    if copy_visual_studio_notices(&source, &destination)? != files {
        return Err("Identical notice copying is not idempotent.".into());
    }
    let reject = |message| match copy_visual_studio_notices(&source, &destination) {
        Err(error) if error.to_string() == message => Ok(()),
        Err(error) => Err(error),
        Ok(_) => Err(format!("Expected rejection: {message}").into()),
    };
    fs::write(&copied, [1])?;
    reject("Visual Studio notice destination bytes collide.")?;
    fs::write(copied, bytes)?;
    fs::write(locale.join(".private-fixture.txt"), bytes)?;
    reject("Unexpected Visual Studio notice path.")?;
    if destination.join("1033/.private-fixture.txt").exists() {
        return Err("Unexpected hidden notice was copied.".into());
    }
    crate::ci_crt::contract(work.path())?;
    work.close()?;
    println!(
        "Native notice/CRT contracts passed: tests=7 failed=0 skipped=0; exact bytes, collision, hidden paths, import selection and strict PE admission."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_dotnet_eula_preserves_bytes_and_is_idempotent() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let nested = src.path().join("1033");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join(".NET_Framework_EULA.rtf"), b"rtf-payload").unwrap();
        fs::write(nested.join("license.txt"), b"txt-payload").unwrap();
        fs::write(nested.join("ignored.bin"), b"skip-me").unwrap();

        let first = copy_visual_studio_notices(src.path(), dst.path()).unwrap();
        assert_eq!(
            first,
            vec!["1033/DOTNET_Framework_EULA.rtf", "1033/license.txt"]
        );
        assert_eq!(
            fs::read(dst.path().join("1033/DOTNET_Framework_EULA.rtf")).unwrap(),
            b"rtf-payload"
        );
        // Second identical copy succeeds idempotently.
        let second = copy_visual_studio_notices(src.path(), dst.path()).unwrap();
        assert_eq!(second, first);
    }

    #[test]
    fn rejects_unexpected_dotfile_collision_and_empty_source() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        assert!(copy_visual_studio_notices(src.path(), dst.path()).is_err());

        fs::write(src.path().join(".hidden.txt"), b"nope").unwrap();
        assert!(
            copy_visual_studio_notices(src.path(), dst.path())
                .unwrap_err()
                .to_string()
                .contains("Unexpected Visual Studio notice path")
        );
        fs::remove_file(src.path().join(".hidden.txt")).unwrap();

        fs::write(src.path().join("eula.txt"), b"original").unwrap();
        fs::write(dst.path().join("eula.txt"), b"different").unwrap();
        assert!(
            copy_visual_studio_notices(src.path(), dst.path())
                .unwrap_err()
                .to_string()
                .contains("bytes collide")
        );
    }
}
