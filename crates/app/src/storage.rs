use crate::{Result, fail};
use fs2::FileExt;
use nvsf_core::Project;
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(target_os = "linux")]
#[path = "storage_linux.rs"]
mod native;
#[cfg(windows)]
#[path = "storage_windows.rs"]
mod native;

pub fn hash_file(path: &Path) -> Result<String> {
    hash_reader(File::open(path)?)
}

fn hash_reader(mut file: File) -> Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub fn write_atomic(path: &Path, bytes: &[u8], overwrite: bool) -> Result<()> {
    AtomicOutput::new(path, overwrite)?.write(bytes)
}

pub fn save_project(project: &Project, path: &Path) -> Result<()> {
    project.validate()?;
    let output = AtomicOutput::new(path, true)?;
    output.protect_project_source(project)?;
    output.write(&serde_json::to_vec_pretty(project)?)
}

/// Holds the parent directory throughout admission, temporary creation, and publication.
/// Renaming or replacing an ancestor cannot redirect any later operation to another directory.
pub struct AtomicOutput {
    entry: BoundEntry,
    overwrite: bool,
}

struct BoundEntry {
    directory: File,
    name: OsString,
}

impl BoundEntry {
    fn open(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .ok_or_else(|| fail("Missing destination filename"))?
            .to_os_string();
        native::validate_name(&name)?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        Ok(Self {
            directory: native::open_directory(parent)?,
            name,
        })
    }

    fn resolved(path: &Path) -> Result<Self> {
        match path.canonicalize() {
            Ok(path) => Self::open(&path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::open(path),
            Err(error) => Err(error.into()),
        }
    }

    fn existing(&self) -> Result<Option<File>> {
        match native::open_file(&self.directory, &self.name) {
            Ok(file) => {
                if !file.metadata()?.is_file() {
                    return Err(fail("Output destination must be a regular file"));
                }
                Ok(Some(file))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn conflicts(&self, other: &Self) -> Result<bool> {
        if !native::same_directory(&self.directory, &other.directory)? {
            return Ok(false);
        }
        if native::names_equal(&self.name, &other.name)? {
            return Ok(true);
        }
        // Resolve filesystem aliases such as a Windows short name without treating
        // distinct hard links as the same directory entry.
        match (self.existing()?, other.existing()?) {
            (Some(left), Some(right)) => Ok(native::same_entry_name(&left, &right)?),
            _ => Ok(false),
        }
    }
}

impl AtomicOutput {
    pub fn new(path: &Path, overwrite: bool) -> Result<Self> {
        let entry = BoundEntry::open(path)?;
        Self::admit(entry, overwrite)
    }

    fn admit(entry: BoundEntry, overwrite: bool) -> Result<Self> {
        if entry.existing()?.is_some() && !overwrite {
            return Err(fail("Output exists; explicitly enable overwrite"));
        }
        Ok(Self { entry, overwrite })
    }

    pub(crate) fn sibling(&self, name: &OsStr) -> Result<Self> {
        native::validate_name(name)?;
        Self::admit(
            BoundEntry {
                directory: self.entry.directory.try_clone()?,
                name: name.to_os_string(),
            },
            self.overwrite,
        )
    }

    pub fn conflicts_with(&self, path: &Path) -> Result<bool> {
        self.entry.conflicts(&BoundEntry::resolved(path)?)
    }

    /// Protects the named source, including aliases, while allowing a distinct hard link.
    pub fn protect_source(&self, source: &Path) -> Result<()> {
        let source = match BoundEntry::resolved(source) {
            Ok(source) => source,
            Err(crate::AppError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if self.entry.conflicts(&source)? {
            return Err(fail(
                "Destination resolves to source media; no output changed",
            ));
        }
        Ok(())
    }

    pub fn protect_project_source(&self, project: &Project) -> Result<()> {
        let source = match BoundEntry::resolved(Path::new(&project.config.input)) {
            Ok(source) => Some(source),
            Err(crate::AppError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if let Some(source) = &source
            && self.entry.conflicts(source)?
        {
            return Err(fail(
                "Destination resolves to source media; no output changed",
            ));
        }
        let Some(destination) = self.entry.existing()? else {
            return Ok(());
        };
        // Ordinary subtitle/checkpoint replacements need only hash their small existing
        // output. Recheck the video only when this destination has the recorded content.
        if hash_reader(destination)? != project.source_hash {
            return Ok(());
        }
        if let Some(source) = source
            && let Some(file) = source.existing()?
            && hash_reader(file)? == project.source_hash
        {
            return Ok(());
        }
        Err(fail(
            "Destination matches source media content; no output changed",
        ))
    }

    pub fn write(self, bytes: &[u8]) -> Result<()> {
        let mut temporary = Temporary::create(&self.entry.directory)?;
        temporary.file.write_all(bytes)?;
        temporary.file.sync_all()?;
        native::publish(
            &self.entry.directory,
            &temporary.file,
            &temporary.name,
            &self.entry.name,
            self.overwrite,
        )?;
        temporary.published = true;
        native::sync_directory(&self.entry.directory)?;
        Ok(())
    }
}

struct Temporary<'a> {
    directory: &'a File,
    file: File,
    name: OsString,
    published: bool,
}

impl<'a> Temporary<'a> {
    fn create(directory: &'a File) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| fail("System clock predates the Unix epoch"))?
            .as_nanos();
        for _ in 0..64 {
            let name: OsString = format!(
                ".nvsf-{:x}-{time:x}-{:x}.tmp",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )
            .into();
            match native::create_file(directory, &name) {
                Ok(file) => {
                    return Ok(Self {
                        directory,
                        file,
                        name,
                        published: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(fail("Unable to reserve an atomic output temporary file"))
    }
}

impl Drop for Temporary<'_> {
    fn drop(&mut self) {
        if !self.published {
            let _ = native::remove_file(self.directory, &self.file, &self.name);
        }
    }
}

// Admission-only check for lock files, which are opened without replacing their contents.
// Writers must retain one AtomicOutput through both protection and publication.
pub(crate) fn protect_source(source: &Path, destination: &Path) -> Result<()> {
    AtomicOutput::new(destination, true)?.protect_source(source)
}

pub fn load_project(path: &Path) -> Result<Project> {
    let file = File::open(path)?;
    if file.metadata()?.len() > 64 * 1024 * 1024 {
        return Err(fail("Project exceeds 64MiB limit"));
    }
    let project: Project = serde_json::from_reader(file)?;
    project.validate()?;
    Ok(project)
}

pub struct JobLock(File);
impl JobLock {
    pub fn acquire(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        file.try_lock_exclusive()
            .map_err(|_| fail("Job already open in another process"))?;
        Ok(Self(file))
    }
}
impl Drop for JobLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_output_survives_refused_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("日本 fa.srt");
        write_atomic(&p, b"original", false).unwrap();
        assert!(write_atomic(&p, b"replacement", false).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), b"original");
        write_atomic(&p, "سلام".as_bytes(), true).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "سلام");
    }
}
