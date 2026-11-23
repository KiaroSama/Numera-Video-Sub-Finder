use crate::{
    ci_guard::{self, Guard, Result, unlinked},
    ci_tasks::sha256_hex,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_paths_reject_escape_and_windows_aliases() {
        for bad in [
            "../a",
            "a//b",
            "a/./b",
            "a\\b",
            "C:/a",
            "a/.hidden",
            "a/secrets.md",
            "a/name. ",
            "a/NUL.txt",
        ] {
            assert!(!safe_relative(bad), "accepted {bad}");
        }
        assert!(safe_relative("licenses/native/合法.txt"));
    }
}

#[derive(Deserialize)]
pub(super) struct Ort {
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
    pub native: String,
}

#[derive(Deserialize)]
pub(super) struct Target {
    pub id: String,
    pub os: String,
    pub arch: String,
    #[serde(rename = "target")]
    pub triple: String,
    pub triplet: String,
    pub package_formats: Vec<String>,
    pub ort: Ort,
}

pub(super) fn admission(id: &str, diagnostic: &str) -> Result<(PathBuf, Target, String)> {
    ci_guard::hosted(diagnostic)?;
    let root = ci_guard::root();
    let encoded = fs::read(root.join("ci/targets.json"))?;
    #[derive(Deserialize)]
    struct Matrix {
        schema: u8,
        scope: String,
        targets: Vec<Target>,
    }
    let matrix: Matrix = serde_json::from_slice(&encoded)?;
    let mut targets = matrix.targets.into_iter().filter(|target| target.id == id);
    let target = targets.next().ok_or("Unknown required desktop target.")?;
    if matrix.schema != 1 || matrix.scope != "desktop-only" || targets.next().is_some() {
        return Err("Unknown or ambiguous required desktop target.".into());
    }
    let arch = match env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return Err("Unsupported runner architecture.".into()),
    };
    if !matches!(env::consts::OS, "windows" | "linux")
        || target.os != env::consts::OS
        || target.arch != arch
        || id != format!("{}-{arch}", target.os)
    {
        return Err(
            "Desktop target does not match the hosted runner process OS/architecture.".into(),
        );
    }
    let (triple, triplet, formats): (&str, &str, &[&str]) = match id {
        "linux-x64" => (
            "x86_64-unknown-linux-gnu",
            "x64-linux",
            &["appimage", "deb", "rpm", "tar.gz"],
        ),
        "linux-arm64" => (
            "aarch64-unknown-linux-gnu",
            "arm64-linux",
            &["appimage", "deb", "rpm", "tar.gz"],
        ),
        "windows-x64" => ("x86_64-pc-windows-msvc", "x64-windows", &["msi", "zip"]),
        "windows-arm64" => ("aarch64-pc-windows-msvc", "arm64-windows", &["msi", "zip"]),
        _ => return Err("Unsupported desktop target.".into()),
    };
    let mut actual = target
        .package_formats
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    actual.sort_unstable();
    if target.triple != triple || target.triplet != triplet || actual.as_slice() != formats {
        return Err("Desktop target triple, overlay, or installer formats differ from the supported inventory.".into());
    }
    let sha = env::var("GITHUB_SHA")?;
    if !lower_hex(&sha, 40) {
        return Err("An exact Actions source SHA is required.".into());
    }
    Ok((root, target, sha))
}

pub(super) struct Session {
    root: PathBuf,
    scratch: tempfile::TempDir,
    logger: File,
    component: &'static str,
    prefix: &'static str,
    sequence: u32,
    env: BTreeMap<String, OsString>,
}

impl Session {
    pub fn new(root: &Path, prefix: &'static str, component: &'static str) -> Result<Self> {
        let logs = ci_guard::state_dir(root)?.join("logs");
        fs::create_dir_all(&logs)?;
        if !unlinked(&logs)?.is_dir() {
            return Err("Invalid CI log directory.".into());
        }
        let stamp = chrono::Utc::now().format("%Y-%m-%d_%H-%M-%S");
        let logger = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(logs.join(format!("{prefix}_{stamp}_UTC.log")));
        let logger = match logger {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                tempfile::Builder::new()
                    .prefix(&format!("{prefix}_{stamp}_UTC_"))
                    .suffix(".log")
                    .tempfile_in(&logs)?
                    .keep()?
                    .0
            }
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            root: root.to_owned(),
            scratch: tempfile::Builder::new().prefix(prefix).tempdir_in(&logs)?,
            logger,
            component,
            prefix,
            sequence: 0,
            env: BTreeMap::new(),
        })
    }

    pub fn log(&mut self, level: &str, message: &str) -> Result<()> {
        let line = format!(
            "[{}] [{level}] [{}] {message}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
            self.component
        );
        writeln!(self.logger, "{line}")?;
        self.logger.flush()?;
        eprintln!("{line}");
        Ok(())
    }

    pub fn set(&mut self, key: &str, value: impl AsRef<OsStr>) {
        self.env.insert(key.to_owned(), value.as_ref().to_owned());
    }

    pub fn get(&self, key: &str) -> Option<OsString> {
        self.env.get(key).cloned().or_else(|| env::var_os(key))
    }

    pub fn export(&mut self, key: &str, value: impl AsRef<OsStr>) -> Result<()> {
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err("Invalid CI environment variable name.".into());
        }
        let value = value.as_ref();
        let text = value.to_str().ok_or("CI environment value is not UTF-8.")?;
        append_runner("GITHUB_ENV", &format!("{key}={text}"))?;
        self.set(key, value);
        Ok(())
    }

    pub fn find(&self, name: &str) -> Option<PathBuf> {
        env::split_paths(&self.get("PATH")?)
            .map(|dir| dir.join(name))
            .find(|path| path.is_file())
    }

    pub fn run(
        &mut self,
        name: &str,
        exe: impl AsRef<OsStr>,
        args: impl IntoIterator<Item = impl AsRef<OsStr>>,
        limits: (u32, u32),
    ) -> Result<()> {
        let mut command = Command::new(exe);
        command.args(args);
        self.command(name, command, limits, false, None).map(|_| ())
    }

    pub fn capture(
        &mut self,
        name: &str,
        exe: impl AsRef<OsStr>,
        args: impl IntoIterator<Item = impl AsRef<OsStr>>,
        limits: (u32, u32),
    ) -> Result<String> {
        let mut command = Command::new(exe);
        command.args(args);
        self.command(name, command, limits, true, None)
    }

    pub fn command(
        &mut self,
        name: &str,
        mut command: Command,
        (wall_s, idle_s): (u32, u32),
        capture: bool,
        progress: Option<&Path>,
    ) -> Result<String> {
        self.sequence += 1;
        let name = format!("{}-{name}-{}", self.prefix, self.sequence);
        self.log(
            "INFO",
            &format!("Command stage={name} wall_s={wall_s} idle_s={idle_s}"),
        )?;
        let path = self
            .scratch
            .path()
            .join(format!("{}.stdout", self.sequence));
        if command.get_current_dir().is_none() {
            command.current_dir(&self.root);
        }
        command.envs(&self.env);
        ci_guard::run(
            Guard {
                name: &name,
                wall_s,
                idle_s,
                capture: capture.then_some(path.as_path()),
                progress,
                stall: false,
            },
            command,
        )?
        .require_success(&name)?;
        if capture {
            let text = fs::read_to_string(&path)?;
            fs::remove_file(path)?;
            Ok(text.trim().to_owned())
        } else {
            Ok(String::new())
        }
    }
}

pub(super) fn append_runner(key: &str, line: &str) -> Result<()> {
    if line.contains(['\r', '\n']) {
        return Err("Runner environment export contains a newline.".into());
    }
    writeln!(
        OpenOptions::new()
            .append(true)
            .open(env::var_os(key).ok_or("Missing Actions environment file.")?)?,
        "{line}"
    )?;
    Ok(())
}

pub(super) fn path_text(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| "CI path is not UTF-8.".into())
}
pub(super) fn lower_hex(text: &str, bytes: usize) -> bool {
    text.len() == bytes
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn sha(path: &Path) -> Result<String> {
    Ok(sha256_hex(path)?.1)
}
pub(super) fn json(path: &Path) -> Result<serde_json::Value> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

pub(super) fn safe_relative(text: &str) -> bool {
    !text.contains(['\\', ':'])
        && text.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && !part.ends_with(['.', ' '])
                && !part.chars().any(char::is_control)
                && !matches!(
                    part.to_ascii_lowercase().as_str(),
                    "secrets.md" | "explain-ai.md"
                )
                && !matches!(
                    part.split('.')
                        .next()
                        .unwrap_or_default()
                        .to_ascii_uppercase()
                        .as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                )
        })
}

pub(super) fn entries(path: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort_unstable();
    Ok(paths)
}

pub(super) fn files(path: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for child in entries(path)? {
        let meta = unlinked(&child)?;
        if meta.is_dir() {
            found.extend(files(&child)?);
        } else if meta.is_file() {
            found.push(child);
        } else {
            return Err("Unexpected special file in CI inventory.".into());
        }
    }
    Ok(found)
}

pub(super) fn basename(path: &Path) -> Result<&str> {
    path.file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| "Missing or non-UTF-8 filename.".into())
}

#[derive(Deserialize, Serialize)]
pub(super) struct Member {
    pub file: String,
    pub bytes: u64,
    pub sha256: String,
}

impl Member {
    pub fn read(path: &Path, root: &Path) -> Result<Self> {
        let file = path_text(path.strip_prefix(root)?)?.replace('\\', "/");
        let (bytes, sha256) = sha256_hex(path)?;
        Ok(Self {
            file,
            bytes,
            sha256,
        })
    }
    pub fn verify(&self, path: &Path) -> Result<()> {
        if !lower_hex(&self.sha256, 64)
            || !unlinked(path)?.is_file()
            || sha256_hex(path)? != (self.bytes, self.sha256.clone())
        {
            return Err(format!("CI payload bytes/SHA256 mismatch: {}", self.file).into());
        }
        Ok(())
    }
}

pub(super) fn header(path: &Path, offset: u64, expected: &[u8]) -> Result<()> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut actual = vec![0; expected.len()];
    if file.read_exact(&mut actual).is_err() || actual != expected {
        return Err("Generated artifact has an invalid package header.".into());
    }
    Ok(())
}

pub(super) fn elf(path: &Path, arch: &str, shared: bool) -> Result<()> {
    header(path, 0, b"\x7fELF\x02\x01")?;
    if shared {
        header(path, 16, &[3, 0])?;
    }
    header(path, 18, &[if arch == "arm64" { 183 } else { 62 }, 0])
}

#[cfg(windows)]
pub(super) fn pe_machine(path: &Path) -> Result<u16> {
    let mut file = File::open(path)?;
    let mut head = [0; 64];
    file.read_exact(&mut head)?;
    if &head[..2] != b"MZ" {
        return Err("Invalid native DOS header.".into());
    }
    let offset = u32::from_le_bytes(head[60..64].try_into()?);
    if u64::from(offset) + 6 > file.metadata()?.len() {
        return Err("Invalid native PE offset.".into());
    }
    file.seek(SeekFrom::Start(u64::from(offset)))?;
    let mut hdr = [0; 6];
    file.read_exact(&mut hdr)?;
    if &hdr[..4] != b"PE\0\0" {
        return Err("Invalid native PE header.".into());
    }
    Ok(u16::from_le_bytes([hdr[4], hdr[5]]))
}

#[cfg(windows)]
pub(super) fn crt_directory(vs: &Path, arch: &str) -> Result<(String, PathBuf)> {
    let mut versions = Vec::new();
    for dir in entries(&vs.join("VC/Redist/MSVC"))? {
        let name = basename(&dir)?;
        let components = name
            .split('.')
            .map(str::parse::<u32>)
            .collect::<std::result::Result<Vec<_>, _>>();
        if dir.is_dir()
            && let Ok(components) = components
            && components.len() == 3
        {
            versions.push((components, dir));
        }
    }
    let (_, latest) = versions
        .into_iter()
        .max()
        .ok_or("Visual C++ release redistributable is missing.")?;
    let mut dirs = entries(&latest.join(arch))?.into_iter().filter(|dir| {
        dir.is_dir()
            && basename(dir)
                .is_ok_and(|name| name.starts_with("Microsoft.VC") && name.ends_with(".CRT"))
    });
    let dir = dirs
        .next()
        .ok_or("Visual C++ release CRT directory is missing.")?;
    if dirs.next().is_some() {
        return Err("Visual C++ release CRT directory is ambiguous.".into());
    }
    Ok((basename(&latest)?.to_owned(), dir))
}
