use crate::{Result, fail};
use fs2::FileExt;
use nvsf_core::Project;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
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
    if path.exists() && !overwrite {
        return Err(fail("Output exists; explicitly enable overwrite"));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err(fail("Output directory does not exist"));
    }
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    if overwrite {
        tmp.persist(path).map_err(|e| e.error)?;
    } else {
        tmp.persist_noclobber(path).map_err(|e| e.error)?;
    }
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn save_project(project: &Project, path: &Path) -> Result<()> {
    project.validate()?;
    write_atomic(path, &serde_json::to_vec_pretty(project)?, true)
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
