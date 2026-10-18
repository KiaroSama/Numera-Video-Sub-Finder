//! Hosted Linux parent ownership and failure-only debugger handoff for one production GUI.
use serde_json::Value;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn record(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > 256 {
        return Err("Production parent record exceeds256bytes".into());
    }
    Ok(serde_json::from_slice(bytes)?)
}

pub fn identity(bytes: &[u8], parent: u32) -> Result<u32> {
    let value = record(bytes)?;
    if value["parent"].as_u64() != Some(u64::from(parent)) {
        return Err("Production parent identity mismatch".into());
    }
    value["child"]
        .as_u64()
        .and_then(|id| u32::try_from(id).ok())
        .filter(|id| *id != parent && *id > 0 && *id <= i32::MAX as u32)
        .ok_or_else(|| "Production GUI identity missing or invalid".into())
}

pub fn exit_status(bytes: &[u8]) -> Result<i32> {
    record(bytes)?["raw"]
        .as_i64()
        .and_then(|raw| i32::try_from(raw).ok())
        .filter(|raw| *raw >= 0)
        .ok_or_else(|| "Production GUI exit status missing or invalid".into())
}

#[cfg(target_os = "linux")]
mod native {
    use super::*;
    use std::{
        fs::File,
        io::Read,
        os::unix::process::{CommandExt, ExitStatusExt},
        path::Path,
        process::{Child, Command, Stdio},
        time::{Duration, Instant},
    };

    unsafe extern "C" {
        fn prctl(option: i32, ...) -> i32;
        fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    }

    pub struct Adoption(i32);
    impl Adoption {
        pub fn begin() -> Result<Self> {
            let mut prior = 0i32;
            // SAFETY: Linux PR_GET_CHILD_SUBREAPER writes one initialized int.
            if unsafe { prctl(37, &mut prior as *mut i32, 0usize, 0usize, 0usize) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            // SAFETY: this process-local ownership setting changes no ptrace/privilege policy.
            if prior == 0 && unsafe { prctl(36, 1usize, 0usize, 0usize, 0usize) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(Self(prior))
        }
    }
    impl Drop for Adoption {
        fn drop(&mut self) {
            // SAFETY: restore only this process's previously observed subreaper state.
            if self.0 == 0 && unsafe { prctl(36, 0usize, 0usize, 0usize, 0usize) } != 0 {
                eprintln!("[ERROR] [CI] Unable to restore prior process adoption state");
            }
        }
    }

    pub fn reap_group(group: u32) -> Result<()> {
        let group = i32::try_from(group)
            .ok()
            .filter(|g| *g > 1)
            .ok_or("Invalid owned process group")?;
        // Limit each sweep even if a defective child continuously forks; the caller owns time.
        for _ in 0..256 {
            let mut status = 0;
            // SAFETY: negative exact owned PGID, WNOHANG, initialized status; never waitpid(-1).
            let result = unsafe { waitpid(-group, &mut status, 1) };
            if result > 0 {
                continue;
            }
            if result == 0 {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(10) => Ok(()),  // ECHILD: no adopted waitable child in this group.
                Some(4) => continue, // EINTR: retry only inside this bounded sweep.
                _ => Err(error.into()),
            };
        }
        Err("Owned adopted-child sweep exceeded256records".into())
    }

    pub fn read(path: &Path) -> Result<Vec<u8>> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.len() > 256 {
            return Err("Production parent record must be a bounded regular file".into());
        }
        let mut bytes = Vec::new();
        File::open(path)?.take(257).read_to_end(&mut bytes)?;
        record(&bytes)?;
        Ok(bytes)
    }

    pub fn command(original: &Command, state: &Path) -> Result<Command> {
        if original.get_args().next().is_some() {
            return Err(
                "Production GUI parent accepts only the existing no-argument launch".into(),
            );
        }
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("production-parent")
            .arg("--gui")
            .arg(original.get_program())
            .arg("--state")
            .arg(state.canonicalize()?);
        if let Some(cwd) = original.get_current_dir() {
            command.current_dir(cwd);
        }
        for (key, value) in original.get_envs() {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
        Ok(command)
    }

    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let deadline = Instant::now() + Duration::from_secs(3);
            while !matches!(self.0.try_wait(), Ok(Some(_))) && Instant::now() < deadline {
                std::thread::park_timeout(Duration::from_millis(10));
            }
        }
    }

    pub fn run(gui: &Path, state: &Path) -> Result<()> {
        if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
            return Err("Production GUI parent is hosted Linux only".into());
        }
        let state = state.canonicalize()?;
        let gui = gui.canonicalize()?;
        if !state.is_dir() || !gui.is_file() {
            return Err("Production GUI or owned evidence directory missing".into());
        }
        let capture = File::options()
            .create_new(true)
            .write(true)
            .open(state.join("capture.log"))?;
        // Inherit the integrator's dedicated group; never detach the GUI from its owner.
        let mut child = ChildGuard(
            Command::new(gui)
                .stdin(Stdio::null())
                .stdout(capture.try_clone()?)
                .stderr(capture)
                .spawn()?,
        );
        nvsf_app::storage::write_atomic(
            &state.join("parent-pid.json"),
            &serde_json::to_vec(
                &serde_json::json!({"parent":std::process::id(),"child":child.0.id()}),
            )?,
            false,
        )?;
        let deadline = Instant::now() + Duration::from_secs(66);
        loop {
            if let Some(status) = child.0.try_wait()? {
                nvsf_app::storage::write_atomic(
                    &state.join("parent-exit.json"),
                    &serde_json::to_vec(&serde_json::json!({"raw":status.into_raw()}))?,
                    false,
                )?;
                return Ok(());
            }
            if state.join("capture-request.json").exists() {
                let request = read(&state.join("capture-request.json"))?;
                if record(&request)? != serde_json::json!({"capture":true}) {
                    return Err("Production diagnostic request invalid".into());
                }
                let log = File::options()
                    .create_new(true)
                    .write(true)
                    .open(state.join("native-stack.log"))?;
                let mut debugger = Command::new("gdb");
                debugger
                    .args(crate::production::stack_arguments(child.0.id()))
                    .env("DEBUGINFOD_URLS", "")
                    .stdin(Stdio::null())
                    .stdout(log.try_clone()?)
                    .stderr(log);
                // exec preserves this PID: GDB becomes the actual parent, not a sibling.
                return Err(debugger.exec().into());
            }
            if Instant::now() >= deadline {
                return Err("Production parent exceeded66s ownership deadline".into());
            }
            // One file-based request, bounded by both this owner and the original integrator.
            std::thread::park_timeout(Duration::from_millis(10));
        }
    }
}

#[cfg(target_os = "linux")]
pub use native::{Adoption, command, read, reap_group, run};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supervisor_records_bind_the_actual_parent_and_preserve_native_exit_status() {
        assert_eq!(identity(br#"{"parent":41,"child":42}"#, 41).unwrap(), 42);
        for bytes in [
            br#"{"parent":40,"child":42}"#.as_slice(),
            br#"{"parent":41,"child":41}"#.as_slice(),
            br#"{"parent":41,"child":0}"#.as_slice(),
            br#"{"parent":41,"child":4294967296}"#.as_slice(),
            b"not json".as_slice(),
        ] {
            assert!(identity(bytes, 41).is_err());
        }
        assert!(identity(&vec![b' '; 257], 41).is_err());
        assert_eq!(exit_status(br#"{"raw":0}"#).unwrap(), 0);
        assert_eq!(exit_status(br#"{"raw":2816}"#).unwrap(), 2816);
        assert_eq!(exit_status(br#"{"raw":10}"#).unwrap(), 10);
        assert!(exit_status(br#"{"raw":-1}"#).is_err());
        assert!(exit_status(br#"{"raw":4294967296}"#).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parent_launch_keeps_the_exact_environment_and_absolute_evidence_identity() {
        use std::{ffi::OsStr, process::Command};
        let directory = tempfile::tempdir().unwrap();
        let mut original = Command::new("/owned/package/GUI 日本");
        original
            .current_dir(directory.path())
            .env("NVSF_DATA_ROOT", directory.path())
            .env_remove("LD_LIBRARY_PATH")
            .env_remove("NVSF_RUNTIME_ROOT");
        let parent = command(&original, directory.path()).unwrap();
        let args: Vec<_> = parent.get_args().collect();
        assert_eq!(args[0], "production-parent");
        assert_eq!(args[2], original.get_program());
        assert_eq!(
            args[4],
            directory.path().canonicalize().unwrap().as_os_str()
        );
        assert_eq!(parent.get_current_dir(), original.get_current_dir());
        let environment: std::collections::BTreeMap<_, _> = parent.get_envs().collect();
        assert_eq!(environment.get(OsStr::new("LD_LIBRARY_PATH")), Some(&None));
        assert_eq!(
            environment.get(OsStr::new("NVSF_RUNTIME_ROOT")),
            Some(&None)
        );
        assert_eq!(
            environment.get(OsStr::new("NVSF_DATA_ROOT")),
            Some(&Some(directory.path().as_os_str()))
        );
        original.arg("unrequested-argument");
        assert!(command(&original, directory.path()).is_err());
    }
}
