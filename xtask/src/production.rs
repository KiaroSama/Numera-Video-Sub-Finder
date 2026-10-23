//! Hosted-only verification of the uninstrumented, packaged GUI using native window lifecycle.
use serde_json::json;
use std::{
    error::Error,
    fs::File,
    io::Read,
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
#[cfg(any(target_os = "linux", test))]
#[path = "production_linux.rs"]
mod linux;
#[path = "production_listener.rs"]
mod listener;

const TITLE: &str = "Numera Video Sub Finder";
const WALL: Duration = Duration::from_secs(60);
const IDLE: Duration = Duration::from_secs(15);

struct Owned {
    child: Child,
    released: bool,
    #[cfg(windows)]
    job: windows::Job,
}
impl Owned {
    fn spawn(command: &mut Command, capture: &Path) -> Result<Self> {
        let file = File::create(capture)?;
        command
            .stdin(Stdio::null())
            .stdout(file.try_clone()?)
            .stderr(file);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        #[cfg(windows)]
        let job = match windows::Job::attach(&child) {
            Ok(job) => job,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        #[cfg(not(windows))]
        let _ = &mut child;
        Ok(Self {
            child,
            released: false,
            #[cfg(windows)]
            job,
        })
    }
    fn pid(&self) -> u32 {
        self.child.id()
    }
    fn status(&mut self) -> Result<Option<ExitStatus>> {
        Ok(self.child.try_wait()?)
    }
    fn descendants_gone(&mut self) -> Result<bool> {
        #[cfg(windows)]
        {
            return self.job.empty();
        }
        #[cfg(unix)]
        {
            #[cfg(target_os = "linux")]
            if self.child.try_wait()?.is_some() {
                crate::production_supervisor::reap_group(self.pid())?;
            }
            return Ok(unix_group_empty(self.pid()));
        }
        #[allow(unreachable_code)]
        Err("Unsupported native process ownership platform".into())
    }
    fn cleanup(&mut self) {
        if self.released {
            return;
        }
        #[cfg(windows)]
        {
            self.job.terminate();
        }
        #[cfg(unix)]
        {
            // SAFETY: process_group(0) created a dedicated owned group; no user/shared group is targeted.
            unsafe {
                kill(-(self.pid() as i32), 9);
            }
        }
        let _ = self.child.kill();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            if Instant::now() >= deadline {
                eprintln!(
                    "[ERROR] [CI] Owned production/helper process did not reap within3s pid={}",
                    self.pid()
                );
                break;
            }
            std::thread::park_timeout(Duration::from_millis(20));
        }
        #[cfg(target_os = "linux")]
        {
            while Instant::now() < deadline {
                match self.descendants_gone() {
                    Ok(true) => break,
                    Ok(false) => std::thread::park_timeout(Duration::from_millis(10)),
                    Err(error) => {
                        eprintln!("[ERROR] [CI] Owned adopted-child cleanup failed: {error}");
                        break;
                    }
                }
            }
        }
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}
#[cfg(unix)]
fn unix_group_empty(pid: u32) -> bool {
    // SAFETY: signal0 probes the exact owned process group and performs no mutation.
    if unsafe { kill(-(pid as i32), 0) } == 0 {
        return false;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(3)
}

struct Budget {
    started: Instant,
    last_progress: Instant,
    signature: String,
}
impl Budget {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            last_progress: Instant::now(),
            signature: String::new(),
        }
    }
    fn observe(&mut self, signature: String) -> Result<()> {
        if signature != self.signature {
            self.signature = signature;
            self.last_progress = Instant::now();
        }
        let reason = if self.started.elapsed() >= WALL {
            Some("exceeded 60s overall wall budget")
        } else if self.last_progress.elapsed() >= IDLE {
            Some("made no readiness/shutdown progress for 15s")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(format!(
                "Production GUI {reason}; last={} elapsed_ms={} idle_ms={}",
                self.signature,
                self.started.elapsed().as_millis(),
                self.last_progress.elapsed().as_millis()
            )
            .into());
        }
        Ok(())
    }
}

#[path = "production_readiness.rs"]
mod readiness;
use readiness::diagnostic;

/// `gui` must be the production executable from the staged native package, never the e2e binary.
/// Run on hosted desktop runners/virtual displays only; this function deliberately refuses local execution.
pub fn smoke(gui: &Path, data: &Path, output: &Path) -> Result<()> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return Err(
            "Production GUI verification is hosted-only; local execution is prohibited".into(),
        );
    }
    let sha = std::env::var("GITHUB_SHA")?;
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Production report requires an exact GitHub source SHA".into());
    }
    let gui = gui.canonicalize()?;
    if !gui.is_file() {
        return Err("Packaged production GUI executable missing".into());
    }
    std::fs::create_dir_all(data)?;
    std::fs::create_dir_all(output)?;
    let evidence = tempfile::Builder::new()
        .prefix("production-lifecycle-")
        .tempdir_in(output)?
        .keep();
    let state = tempfile::Builder::new()
        .prefix("production-data-")
        .tempdir_in(data)?;
    #[cfg(not(target_os = "linux"))]
    let capture = evidence.as_path().join("capture.log");
    let logs = state.path().join("logs");
    listener::absent()?;
    let mut command = Command::new(&gui);
    crate::fixture::clean_runtime_environment(&mut command);
    command
        .current_dir(evidence.as_path())
        .env("NVSF_DATA_ROOT", state.path())
        .env("TAURI_WEBDRIVER_PORT", "4445")
        .env_remove("NVSF_E2E_FIXTURE_ROOT")
        .env_remove("NVSF_RUNTIME_ROOT");
    #[cfg(target_os = "linux")]
    let mut command = crate::production_supervisor::command(&command, evidence.as_path())?;
    #[cfg(target_os = "linux")]
    let capture = evidence.as_path().join("parent-capture.log");
    #[cfg(target_os = "linux")]
    let _adoption = crate::production_supervisor::Adoption::begin()?;
    let mut process = Owned::spawn(&mut command, &capture)?;
    let mut budget = Budget::new();
    let mut helper_index = 0u64;
    #[cfg(not(target_os = "linux"))]
    let pid = process.pid();
    #[cfg(target_os = "linux")]
    let mut pid = 0;
    let result = (|| -> Result<()> {
        #[cfg(target_os = "linux")]
        loop {
            let identity = evidence.as_path().join("parent-pid.json");
            if identity.exists() {
                pid = crate::production_supervisor::identity(
                    &crate::production_supervisor::read(&identity)?,
                    process.pid(),
                )?;
                break;
            }
            if let Some(status) = process.status()? {
                return Err(
                    format!("Production parent exited before GUI admission: {status}").into(),
                );
            }
            budget.observe("awaiting-owned-gui-identity".into())?;
            std::thread::park_timeout(Duration::from_millis(10));
        }
        let window = loop {
            if let Some(status) = gui_status(&mut process, evidence.as_path())? {
                return Err(
                    format!("Production GUI exited before native readiness: {status}").into(),
                );
            }
            listener::absent()?;
            let (log, length) = diagnostic(&logs)?;
            // Do not send synchronous native automation to an uninitialized hidden window.
            if !readiness::shown(&log, pid) {
                budget.observe(format!("startup:{length}:false"))?;
                std::thread::park_timeout(Duration::from_millis(40));
                continue;
            }
            let observation = native_window(pid, evidence.as_path(), &mut helper_index);
            #[cfg(target_os = "linux")]
            if observation
                .as_ref()
                .is_err_and(|error| linux::interrupted_window_query(&error.to_string()))
            {
                // A vanished X11 child is an interrupted query, not proof of absence.
                budget.observe(format!("startup:{length}:false"))?;
                std::thread::park_timeout(Duration::from_millis(40));
                continue;
            }
            let window = observation?;
            budget.observe(format!("startup:{length}:{}", window.is_some()))?;
            if log.contains("[INFO] [APP] Startup version=")
                && let Some(window) = window
            {
                break window;
            }
            // Native readiness is observed on every iteration; no arbitrary fixed startup sleep.
            std::thread::park_timeout(Duration::from_millis(40));
        };
        listener::absent()?;
        // Ordinary ui_ready is the only production path that reveals the initially hidden window.
        budget.observe("visible-owned-window".into())?;
        native_close(pid, &window, evidence.as_path(), &mut helper_index)?;
        budget.observe("native-close-requested".into())?;
        let exit = loop {
            listener::absent()?;
            let (log, length) = diagnostic(&logs)?;
            budget.observe(format!("shutdown:{length}"))?;
            if let Some(status) = gui_status(&mut process, evidence.as_path())? {
                if !status.success() {
                    return Err(format!("Production GUI failed native close: {status}").into());
                }
                if !log.contains("[INFO] [DESKTOP] Native event loop exited") {
                    return Err(
                        "Process exited without the actual Tauri event-loop shutdown diagnostic"
                            .into(),
                    );
                }
                break status;
            }
            std::thread::park_timeout(Duration::from_millis(40));
        };
        budget.observe("event-loop-exited".into())?;
        while !process.descendants_gone()? {
            budget.observe("waiting-owned-descendant-exit".into())?;
            std::thread::park_timeout(Duration::from_millis(40));
        }
        if native_window(pid, evidence.as_path(), &mut helper_index)?.is_some() {
            return Err("Owned native window survived process shutdown".into());
        }
        listener::absent()?;
        budget.observe("verification-complete".into())?;
        let (log, _) = diagnostic(&logs)?;
        std::fs::write(
            evidence.as_path().join("production-native.log"),
            log.as_bytes(),
        )?;
        std::fs::write(
            output.join("production-gui-report.json"),
            serde_json::to_vec_pretty(&json!({
                "schema":1,"source_sha":sha,"target":std::env::var("NVSF_TARGET_ID")?,"passed":true,"profile":"production","tests":5,"failed":0,"skipped":0,
                "pid":pid,"executable":gui.file_name().map(|n|n.to_string_lossy()),"duration_ms":budget.started.elapsed().as_millis(),
                "exit_code":exit.code(),"proof":["fresh native application log","visible PID-owned originally hidden window after frontend ui_ready","no localhost4445 listener before/during/after","ordinary native window close and clean Tauri event-loop exit","owned descendants and window gone"],
                "not_proved":["OCR quality","physical IME composition","GPU placement","all arbitrary network listeners","platform installation lifecycle"]
            }))?,
        )?;
        Ok(())
    })();
    let status_at_failure = gui_status(&mut process, evidence.as_path());
    #[cfg(target_os = "linux")]
    let alive_at_failure = pid != 0 && matches!(&status_at_failure, Ok(None));
    let status_at_failure = status_at_failure.map(|status| format!("{status:?}"));
    let failure_context = json!({
        "pid":pid,"target":std::env::var("NVSF_TARGET_ID").ok(),
        "last_signature":budget.signature,"elapsed_ms":budget.started.elapsed().as_millis(),
        "idle_ms":budget.last_progress.elapsed().as_millis(),"helper_count":helper_index,
        "child_status":status_at_failure.map_err(|error| error.to_string()),
        "evidence_directory":evidence.as_path().file_name().map(|name|name.to_string_lossy())
    });
    if let Err(error) = &result {
        eprintln!("[ERROR] [CI] {error}; context={failure_context}");
        #[cfg(target_os = "linux")]
        if alive_at_failure {
            if let Err(error) = linux::capture(
                pid,
                gui.parent().ok_or("Production package parent missing")?,
                evidence.as_path(),
            ) {
                eprintln!("[WARNING] [CI] Native wait/library evidence inconclusive: {error}");
            }
            let diagnostic = capture_stack(&mut process, evidence.as_path());
            eprintln!(
                "[INFO] [CI] Failure-latched native stack capture={}; readiness result remains failed",
                match diagnostic {
                    Ok(true) => "completed; inspect native stack capture".to_owned(),
                    Ok(false) => "inconclusive; debugger returned failure".to_owned(),
                    Err(error) => format!("inconclusive: {error}"),
                }
            );
        }
        process.cleanup();
    }
    if let Ok((log, _)) = diagnostic(&logs) {
        let _ = std::fs::write(
            evidence.as_path().join("production-native.log"),
            log.as_bytes(),
        );
    }
    let cleanup_deadline = Instant::now() + Duration::from_secs(3);
    while !process.descendants_gone()? && Instant::now() < cleanup_deadline {
        std::thread::park_timeout(Duration::from_millis(20));
    }
    let clean = process.descendants_gone()?;
    let evidence_path = &evidence;
    if result.is_err() || !clean {
        std::fs::write(
            output.join("production-gui-report.json"),
            serde_json::to_vec_pretty(&json!({
                "schema":1,"source_sha":sha,"passed":false,"profile":"production","tests":0,
                "error":result.as_ref().err().map(|error|error.to_string()),
                "context":failure_context,"cleanup_verified":clean,"duration_ms":budget.started.elapsed().as_millis()
            }))?,
        )?;
    }
    if !clean {
        return Err(format!(
            "Production smoke cleanup incomplete; inspect {}",
            evidence_path.display()
        )
        .into());
    }
    process.released = true;
    result
}

fn gui_status(process: &mut Owned, evidence: &Path) -> Result<Option<ExitStatus>> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::ExitStatusExt;
        let Some(parent_status) = process.status()? else {
            return Ok(None);
        };
        let path = evidence.join("parent-exit.json");
        if !path.exists() {
            return Err(
                format!("Production parent exited without GUI status: {parent_status}").into(),
            );
        }
        let raw =
            crate::production_supervisor::exit_status(&crate::production_supervisor::read(&path)?)?;
        Ok(Some(ExitStatus::from_raw(raw)))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = evidence;
        process.status()
    }
}

#[cfg(target_os = "linux")]
fn capture_stack(process: &mut Owned, evidence: &Path) -> Result<bool> {
    nvsf_app::storage::write_atomic(
        &evidence.join("capture-request.json"),
        b"{\"capture\":true}",
        false,
    )?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let capture = evidence.join("native-stack.log");
    loop {
        if capture.exists() && std::fs::metadata(&capture)?.len() > 65536 {
            return Err("Native debugger response exceeded64KiB".into());
        }
        if let Some(status) = process.status()? {
            if !capture.is_file() || std::fs::metadata(&capture)?.len() > 65536 {
                return Err("Native debugger capture missing or oversized".into());
            }
            return Ok(status.success());
        }
        if Instant::now() >= deadline {
            return Err("Failure-only parent debugger exceeded3s".into());
        }
        std::thread::park_timeout(Duration::from_millis(10));
    }
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn stack_arguments(pid: u32) -> Vec<String> {
    [
        "-nx",
        "-batch",
        "-iex",
        "set auto-load off",
        "-iex",
        "set debuginfod enabled off",
        "-iex",
        "set print frame-arguments none",
        "-iex",
        "set print entry-values no",
        "-iex",
        "set print frame-info location",
        "-ex",
        "thread apply all backtrace 24",
        "-ex",
        "kill",
        "-p",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain(std::iter::once(pid.to_string()))
    .collect()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn helper(program: &str, args: &[String], root: &Path, index: &mut u64) -> Result<(bool, String)> {
    *index += 1;
    let capture = root.join(format!("native-helper-{index}.log"));
    let mut command = Command::new(program);
    command.args(args);
    if program == "gdb" {
        command.env("DEBUGINFOD_URLS", "");
    }
    let mut owned = Owned::spawn(&mut command, &capture)?;
    let start = Instant::now();
    loop {
        if std::fs::metadata(&capture)?.len() > 65536 {
            return Err("Native helper response exceeded64KiB".into());
        }
        if let Some(status) = owned.status()? {
            let file = File::open(&capture)?;
            if file.metadata()?.len() > 65536 {
                return Err("Native helper response exceeded64KiB".into());
            }
            let mut text = String::new();
            file.take(65537).read_to_string(&mut text)?;
            if text.len() > 65536 {
                return Err("Native helper response exceeded64KiB".into());
            }
            if !owned.descendants_gone()? {
                return Err("Native window helper leaked an owned descendant".into());
            }
            owned.released = true;
            return Ok((status.success(), text));
        }
        if start.elapsed() >= Duration::from_secs(3) {
            owned.cleanup();
            return Err(format!("Native window helper {program} exceeded3s").into());
        }
        std::thread::park_timeout(Duration::from_millis(10));
    }
}

#[cfg(target_os = "linux")]
fn native_window(pid: u32, root: &Path, index: &mut u64) -> Result<Option<String>> {
    let (success, text) = helper(
        "xdotool",
        &[
            "search".into(),
            "--all".into(),
            "--onlyvisible".into(),
            "--limit".into(),
            "1".into(),
            "--pid".into(),
            pid.to_string(),
            "--name".into(),
            format!("^{TITLE}$"),
        ],
        root,
        index,
    )?;
    let value = text.trim();
    if success && value.bytes().all(|b| b.is_ascii_digit()) && !value.is_empty() {
        return Ok(Some(value.into()));
    }
    if !success && value.is_empty() {
        return Ok(None);
    }
    Err(
        format!("X11 native window query failed (xdotool and virtual display required): {value}")
            .into(),
    )
}
#[cfg(target_os = "linux")]
fn native_close(pid: u32, window: &str, root: &Path, index: &mut u64) -> Result<()> {
    let (success, owner) = helper(
        "xdotool",
        &["getwindowpid".into(), window.into()],
        root,
        index,
    )?;
    if !success || owner.trim() != pid.to_string() {
        return Err("Native window owner changed before close".into());
    }
    let (success, text) = helper("wmctrl", &["-ic".into(), window.into()], root, index)?;
    if success {
        Ok(())
    } else {
        Err(format!("Native EWMH close failed; window manager required: {text}").into())
    }
}

#[cfg(target_os = "macos")]
fn native_window(pid: u32, root: &Path, index: &mut u64) -> Result<Option<String>> {
    let script = format!(
        "tell application \"System Events\"\nset owned to application processes whose unix id is {pid}\nif (count of owned) is 0 then return \"\"\ntell item 1 of owned\nif visible is false then return \"\"\nrepeat with w in windows\nif name of w is \"{TITLE}\" then return name of w\nend repeat\nend tell\nreturn \"\"\nend tell"
    );
    let (success, text) = helper("osascript", &["-e".into(), script], root, index)?;
    if !success {
        return Err(format!("macOS native UI verification blocked; existing Accessibility authorization required: {text}").into());
    }
    Ok((text.trim() == TITLE).then(|| TITLE.into()))
}
#[cfg(target_os = "macos")]
fn native_close(pid: u32, _window: &str, root: &Path, index: &mut u64) -> Result<()> {
    let script = format!(
        "tell application \"System Events\"\nset owned to application processes whose unix id is {pid}\nif (count of owned) is not 1 then error \"Owned process unavailable\"\ntell item 1 of owned\nset w to first window whose name is \"{TITLE}\"\nclick (first button of w whose subrole is \"AXCloseButton\")\nend tell\nend tell"
    );
    let (success, text) = helper("osascript", &["-e".into(), script], root, index)?;
    if success {
        Ok(())
    } else {
        Err(format!("macOS owned-window close blocked: {text}").into())
    }
}

#[cfg(windows)]
fn native_window(pid: u32, _root: &Path, _index: &mut u64) -> Result<Option<String>> {
    windows::window(pid)
}
#[cfg(windows)]
fn native_close(pid: u32, window: &str, _root: &Path, _index: &mut u64) -> Result<()> {
    windows::close(pid, window)
}

#[cfg(windows)]
#[path = "production_windows.rs"]
mod windows;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_stack_capture_disables_evaluation_and_argument_values_before_attach() {
        let args = stack_arguments(12345);
        let attach = args.iter().position(|arg| arg == "-p").unwrap();
        assert_eq!(args[attach + 1], "12345");
        assert_eq!(&args[..2], ["-nx", "-batch"]);
        for setting in [
            "set auto-load off",
            "set debuginfod enabled off",
            "set print frame-arguments none",
            "set print entry-values no",
            "set print frame-info location",
        ] {
            let index = args.iter().position(|arg| arg == setting).unwrap();
            assert!(index < attach);
            assert_eq!(args[index - 1], "-iex");
        }
        assert!(
            args.iter()
                .any(|arg| arg == "thread apply all backtrace 24")
        );
        assert!(args.iter().any(|arg| arg == "kill"));
        assert!(!args.iter().any(|arg| arg == "detach"));
        assert!(
            !args
                .iter()
                .any(|arg| arg.contains(" full") || arg.contains("locals"))
        );
    }

    #[test]
    fn no_progress_budget_fails_instead_of_fabricating_ready() {
        let mut budget = Budget::new();
        budget.signature = "startup:0:false".into();
        budget.last_progress = Instant::now() - IDLE;
        let error = budget
            .observe("startup:0:false".into())
            .unwrap_err()
            .to_string();
        assert!(error.contains("startup:0:false"));
        assert!(error.contains("idle_ms="));
        let mut budget = Budget::new();
        budget.started = Instant::now() - WALL;
        assert!(budget.observe("real-progress".into()).is_err());
    }
}
