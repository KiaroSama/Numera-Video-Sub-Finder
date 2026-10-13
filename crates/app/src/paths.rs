use crate::{Result, fail};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
    pub models: PathBuf,
    pub projects: PathBuf,
    pub logs: PathBuf,
    pub runtimes: PathBuf,
    pub cache: PathBuf,
}

impl Paths {
    pub fn new(data_root: Option<&Path>) -> Result<Self> {
        let exe = std::env::current_exe()?;
        let dir = exe
            .parent()
            .ok_or_else(|| fail("Executable has no resource directory"))?;
        let root = if let Some(root) = data_root {
            root.to_path_buf()
        } else if let Some(root) = std::env::var_os("NVSF_DATA_ROOT") {
            PathBuf::from(root)
        } else if dir.join("portable.flag").is_file() {
            dir.join("data")
        } else {
            dirs::data_local_dir()
                .ok_or_else(|| fail("No writable application data location; choose --data-root"))?
                .join("Numera Video Sub Finder")
        };
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        let runtimes = std::env::var_os("NVSF_RUNTIME_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let sibling = dir.join("runtimes");
                if sibling.is_dir() {
                    return sibling;
                }
                #[cfg(target_os = "macos")]
                if dir.file_name().is_some_and(|n| n == "MacOS") {
                    if let Some(contents) = dir.parent() {
                        return contents.join("Resources/runtimes");
                    }
                }
                #[cfg(target_os = "linux")]
                if let Some(prefix) = dir.parent() {
                    let installed = prefix.join("lib/Numera Video Sub Finder/runtimes");
                    if installed.is_dir() {
                        return installed;
                    }
                }
                sibling
            });
        let paths = Self {
            models: root.join("models"),
            projects: root.join("projects"),
            logs: root.join("logs"),
            cache: root.join("cache"),
            root,
            runtimes,
        };
        for d in [&paths.models, &paths.projects, &paths.logs, &paths.cache] {
            std::fs::create_dir_all(d)?;
        }
        crate::models::seed_bundled(&paths)?;
        Ok(paths)
    }

    pub fn runtime(&self, name: &str) -> Result<PathBuf> {
        if name.contains(['/', '\\']) || name.contains("..") {
            return Err(fail("Invalid runtime resource name"));
        }
        let file = self.runtimes.join(name);
        if !file.is_file() {
            return Err(fail(format!(
                "Missing packaged runtime {name}; reinstall the matching native package"
            )));
        }
        let file = file.canonicalize()?;
        if !file.starts_with(self.runtimes.canonicalize()?) {
            return Err(fail("Runtime resource escapes package"));
        }
        Ok(file)
    }
}
