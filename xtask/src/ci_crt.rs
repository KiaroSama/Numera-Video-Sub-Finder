//! Stage-two CI helper ported from `scripts/ci/crt.ps1` into Rust.
//! Validates Windows release CRT import closures and PE machine headers.

use std::{
    collections::HashSet,
    error::Error,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::Path,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const MAX_IMPORT_LISTING_BYTES: usize = 1_048_576;
const MAX_IMPORT_MEMBERS: usize = 512;
const MIN_PE_BYTES: u64 = 64;
const MAX_PE_BYTES: u64 = 16_777_216;

#[cfg(windows)]
fn is_reparse_or_symlink(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_type().is_symlink() || (meta.file_attributes() & 0x0400) != 0
}

#[cfg(not(windows))]
fn is_reparse_or_symlink(meta: &fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

fn is_import_token(line: &str) -> Option<String> {
    if !line.starts_with(char::is_whitespace) {
        return None;
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.ends_with(".dll") || lower.ends_with(".drv")) {
        return None;
    }
    let stem = &lower[..lower.len() - 4];
    if stem.is_empty()
        || !stem
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-'))
    {
        return None;
    }
    Some(lower)
}

fn is_debug_crt(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".dll") else {
        return false;
    };
    if !stem.ends_with('d') {
        return false;
    }
    ["vcruntime", "msvcp", "concrt", "vccorlib", "ucrtbase"]
        .iter()
        .any(|prefix| stem.starts_with(prefix))
}

fn is_known_release_crt(name: &str) -> bool {
    matches!(
        name,
        "concrt140.dll"
            | "msvcp140.dll"
            | "msvcp140_1.dll"
            | "msvcp140_2.dll"
            | "msvcp140_atomic_wait.dll"
            | "msvcp140_codecvt_ids.dll"
            | "vccorlib140.dll"
            | "vcruntime140.dll"
            | "vcruntime140_1.dll"
            | "vcruntime140_threads.dll"
    )
}

fn is_any_crt_prefix(name: &str) -> bool {
    ["vcruntime", "msvcp", "concrt", "vccorlib"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// Parses `dumpbin /dependents` style output and returns unique lowercase
/// redistributable release CRT DLL names in first-seen order.
pub(crate) fn parse_release_crt_imports(dependencies: &str) -> Result<Vec<String>> {
    if dependencies.len() > MAX_IMPORT_LISTING_BYTES {
        return Err("PE import listing exceeds its byte budget.".into());
    }
    let mut matches = Vec::new();
    for line in dependencies.lines() {
        if let Some(name) = is_import_token(line) {
            matches.push(name);
        }
    }
    if matches.is_empty() || matches.len() > MAX_IMPORT_MEMBERS {
        return Err("PE import listing is empty or exceeds its member budget.".into());
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for name in matches {
        if is_debug_crt(&name) {
            return Err(format!("Nonredistributable debug CRT import: {name}.").into());
        }
        if is_known_release_crt(&name) {
            if seen.insert(name.clone()) {
                out.push(name);
            }
        } else if is_any_crt_prefix(&name) {
            return Err(format!("Unknown release CRT import: {name}.").into());
        }
    }
    Ok(out)
}

/// Verifies a Windows redistributable DLL is a bounded regular PE image whose
/// COFF machine matches `x64` (`0x8664`) or `arm64` (`0xAA64`).
pub(crate) fn assert_release_crt_machine(path: &Path, architecture: &str) -> Result<()> {
    let expected: u16 = match architecture.to_ascii_lowercase().as_str() {
        "arm64" => 0xAA64,
        "x64" => 0x8664,
        _ => return Err("Unsupported native CRT architecture.".into()),
    };
    let meta = fs::symlink_metadata(path)?;
    let len = meta.len();
    if meta.is_dir()
        || is_reparse_or_symlink(&meta)
        || !(MIN_PE_BYTES..=MAX_PE_BYTES).contains(&len)
    {
        return Err("CRT input must be a bounded regular PE image.".into());
    }
    let mut file = File::open(path)?;
    let mut dos = [0_u8; 2];
    file.read_exact(&mut dos)?;
    if u16::from_le_bytes(dos) != 0x5A4D {
        return Err("Invalid redistributable DOS header.".into());
    }
    file.seek(SeekFrom::Start(0x3C))?;
    let mut off_buf = [0_u8; 4];
    file.read_exact(&mut off_buf)?;
    let offset = i32::from_le_bytes(off_buf);
    if offset < 64 || (offset as u64) > len.saturating_sub(6) {
        return Err("Invalid redistributable PE offset.".into());
    }
    file.seek(SeekFrom::Start(offset as u64))?;
    let mut sig_buf = [0_u8; 4];
    file.read_exact(&mut sig_buf)?;
    if u32::from_le_bytes(sig_buf) != 0x4550 {
        return Err("Invalid redistributable PE signature.".into());
    }
    let mut mach_buf = [0_u8; 2];
    file.read_exact(&mut mach_buf)?;
    let machine = u16::from_le_bytes(mach_buf);
    if machine != expected {
        let member = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unknown>");
        return Err(format!(
            "Redistributable DLL architecture differs: member={member} expected=0x{expected:04X} actual=0x{machine:04X}."
        )
        .into());
    }
    Ok(())
}

pub(crate) fn run(mode: &str, input: Option<&Path>, arch: Option<&str>) -> Result<()> {
    match mode {
        "imports" => {
            let text = match input {
                Some(path) => fs::read_to_string(path)?,
                None => {
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf)?;
                    buf
                }
            };
            for dll in parse_release_crt_imports(&text)? {
                println!("{dll}");
            }
            Ok(())
        }
        "machine" => {
            let path = input.ok_or("ci-crt machine mode requires --input <path>.")?;
            let architecture = arch.ok_or("ci-crt machine mode requires --arch <x64|arm64>.")?;
            assert_release_crt_machine(path, architecture)
        }
        other => Err(format!("Unsupported ci-crt mode: {other}.").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_fake_pe(dir: &Path, name: &str, machine: u16) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut bytes = vec![0_u8; 128];
        bytes[0..2].copy_from_slice(&0x5A4D_u16.to_le_bytes());
        bytes[0x3C..0x40].copy_from_slice(&64_i32.to_le_bytes());
        bytes[64..68].copy_from_slice(&0x4550_u32.to_le_bytes());
        bytes[68..70].copy_from_slice(&machine.to_le_bytes());
        let mut f = File::create(&path).unwrap();
        f.write_all(&bytes).unwrap();
        path
    }

    #[test]
    fn parses_release_crt_imports_and_deduplicates() {
        let dump = "Image has the following dependencies:\n\n    KERNEL32.dll\n    VCRUNTIME140.dll\n    vcruntime140_1.dll\n    msvcp140.dll\n    VCRUNTIME140.dll\n";
        let got = parse_release_crt_imports(dump).unwrap();
        assert_eq!(
            got,
            vec!["vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"]
        );
    }

    #[test]
    fn rejects_debug_unknown_and_empty_crt_imports() {
        let debug = "    VCRUNTIME140D.dll\n";
        assert!(
            parse_release_crt_imports(debug)
                .unwrap_err()
                .to_string()
                .contains("debug CRT")
        );
        let unknown = "    msvcp150.dll\n";
        assert!(
            parse_release_crt_imports(unknown)
                .unwrap_err()
                .to_string()
                .contains("Unknown release CRT")
        );
        assert!(parse_release_crt_imports("no imports here").is_err());
    }

    #[test]
    fn validates_pe_machine_headers_for_x64_and_arm64() {
        let tmp = tempfile::tempdir().unwrap();
        let x64 = write_fake_pe(tmp.path(), "vcruntime140.dll", 0x8664);
        let arm64 = write_fake_pe(tmp.path(), "msvcp140.dll", 0xAA64);
        assert!(assert_release_crt_machine(&x64, "x64").is_ok());
        assert!(assert_release_crt_machine(&arm64, "arm64").is_ok());
        assert!(
            assert_release_crt_machine(&x64, "arm64")
                .unwrap_err()
                .to_string()
                .contains("expected=0xAA64 actual=0x8664")
        );
        assert!(assert_release_crt_machine(&x64, "x86").is_err());
    }
}
