//! Hosted-only verification of the uninstrumented, packaged GUI using native window lifecycle.
use serde_json::json;
use std::{
    error::Error,
    fs::File,
    io::Read,
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
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
    fn descendants_gone(&self) -> Result<bool> {
        #[cfg(windows)]
        {
            return self.job.empty();
        }
        #[cfg(unix)]
        {
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
        if self.started.elapsed() >= WALL {
            return Err("Production GUI exceeded 60s overall wall budget".into());
        }
        if self.last_progress.elapsed() >= IDLE {
            return Err("Production GUI made no readiness/shutdown progress for 15s".into());
        }
        Ok(())
    }
}

fn no_listener() -> Result<()> {
    for address in ["127.0.0.1:4445", "[::1]:4445"] {
        let address: SocketAddr = address.parse()?;
        match TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
            Ok(_) => {
                return Err(format!(
                    "Production must expose no WebDriver listener; connection accepted on {address}"
                )
                .into());
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::AddrNotAvailable
                        | std::io::ErrorKind::NetworkUnreachable
                ) => {}
            Err(e) => {
                return Err(
                    format!("Listener absence could not be established on {address}: {e}").into(),
                );
            }
        }
    }
    Ok(())
}

fn diagnostic(logs: &Path) -> Result<(String, u64)> {
    if !logs.is_dir() {
        return Ok((String::new(), 0));
    }
    let mut text = String::new();
    let mut length = 0;
    for entry in std::fs::read_dir(logs)?.take(20) {
        let path = entry?.path();
        if !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("nvsf-desktop_"))
        {
            continue;
        }
        let file = File::open(&path)?;
        if file.metadata()?.len() > 1024 * 1024 {
            return Err("Production startup log exceeds bounded diagnostic size".into());
        }
        let mut part = String::new();
        file.take(1024 * 1024 + 1).read_to_string(&mut part)?;
        length += part.len() as u64;
        text.push_str(&part);
    }
    Ok((text, length))
}

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
        .tempdir_in(output)?;
    let state = tempfile::Builder::new()
        .prefix("production-data-")
        .tempdir_in(data)?;
    let capture = evidence.path().join("capture.log");
    let logs = state.path().join("logs");
    no_listener()?;
    let mut command = Command::new(&gui);
    command
        .current_dir(evidence.path())
        .env("NVSF_DATA_ROOT", state.path())
        .env("TAURI_WEBDRIVER_PORT", "4445")
        .env_remove("NVSF_E2E_FIXTURE_ROOT")
        .env_remove("NVSF_RUNTIME_ROOT");
    let mut process = Owned::spawn(&mut command, &capture)?;
    let pid = process.pid();
    let mut budget = Budget::new();
    let mut helper_index = 0u64;
    let result = (|| -> Result<()> {
        let window = loop {
            if let Some(status) = process.status()? {
                return Err(
                    format!("Production GUI exited before native readiness: {status}").into(),
                );
            }
            no_listener()?;
            let (log, length) = diagnostic(&logs)?;
            let window = native_window(pid, evidence.path(), &mut helper_index)?;
            budget.observe(format!("startup:{length}:{}", window.is_some()))?;
            if log.contains("[INFO] [APP] Startup version=") {
                if let Some(window) = window {
                    break window;
                }
            }
            // Native readiness is observed on every iteration; no arbitrary fixed startup sleep.
            std::thread::park_timeout(Duration::from_millis(40));
        };
        no_listener()?;
        // Ordinary ui_ready is the only production path that reveals the initially hidden window.
        budget.observe("visible-owned-window".into())?;
        native_close(pid, &window, evidence.path(), &mut helper_index)?;
        budget.observe("native-close-requested".into())?;
        let exit = loop {
            no_listener()?;
            let (log, length) = diagnostic(&logs)?;
            budget.observe(format!("shutdown:{length}"))?;
            if let Some(status) = process.status()? {
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
        if native_window(pid, evidence.path(), &mut helper_index)?.is_some() {
            return Err("Owned native window survived process shutdown".into());
        }
        no_listener()?;
        budget.observe("verification-complete".into())?;
        let (log, _) = diagnostic(&logs)?;
        std::fs::write(
            evidence.path().join("production-native.log"),
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
    if result.is_err() {
        process.cleanup();
    }
    if let Ok((log, _)) = diagnostic(&logs) {
        let _ = std::fs::write(
            evidence.path().join("production-native.log"),
            log.as_bytes(),
        );
    }
    let cleanup_deadline = Instant::now() + Duration::from_secs(3);
    while !process.descendants_gone()? && Instant::now() < cleanup_deadline {
        std::thread::park_timeout(Duration::from_millis(20));
    }
    let clean = process.descendants_gone()?;
    let evidence_path = evidence.keep();
    if !clean {
        return Err(format!(
            "Production smoke cleanup incomplete; inspect {}",
            evidence_path.display()
        )
        .into());
    }
    process.released = true;
    if let Err(error) = &result {
        std::fs::write(
            output.join("production-gui-report.json"),
            serde_json::to_vec_pretty(
                &json!({"schema":1,"source_sha":sha,"passed":false,"profile":"production","tests":0,"error":error.to_string(),"cleanup_verified":clean,"duration_ms":budget.started.elapsed().as_millis()}),
            )?,
        )?;
    }
    result
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn helper(program: &str, args: &[String], root: &Path, index: &mut u64) -> Result<(bool, String)> {
    *index += 1;
    let capture = root.join(format!("native-helper-{index}.log"));
    let mut command = Command::new(program);
    command.args(args);
    let mut owned = Owned::spawn(&mut command, &capture)?;
    let start = Instant::now();
    loop {
        if let Some(status) = owned.status()? {
            let file = File::open(&capture)?;
            if file.metadata()?.len() > 65536 {
                return Err("Native helper response exceeded64KiB".into());
            }
            let mut text = String::new();
            file.take(65537).read_to_string(&mut text)?;
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
mod windows {
    use super::*;
    use std::{ffi::c_void, os::windows::io::AsRawHandle};
    type Handle = *mut c_void;
    #[repr(C)]
    struct Limits {
        user: i64,
        total: i64,
        flags: u32,
        min: usize,
        max: usize,
        active: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }
    #[repr(C)]
    struct Io {
        read_ops: u64,
        write_ops: u64,
        other_ops: u64,
        read_bytes: u64,
        write_bytes: u64,
        other_bytes: u64,
    }
    #[repr(C)]
    struct Extended {
        limits: Limits,
        io: Io,
        process_memory: usize,
        job_memory: usize,
        peak_process: usize,
        peak_job: usize,
    }
    #[repr(C)]
    struct Accounting {
        user: i64,
        kernel: i64,
        period_user: i64,
        period_kernel: i64,
        faults: u32,
        total: u32,
        active: u32,
        terminated: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(job: Handle, class: i32, info: *const c_void, len: u32) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn QueryInformationJobObject(
            job: Handle,
            class: i32,
            info: *mut c_void,
            len: u32,
            returned: *mut u32,
        ) -> i32;
        fn TerminateJobObject(job: Handle, code: u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(
            callback: unsafe extern "system" fn(Handle, isize) -> i32,
            data: isize,
        ) -> i32;
        fn IsWindowVisible(window: Handle) -> i32;
        fn GetWindowThreadProcessId(window: Handle, pid: *mut u32) -> u32;
        fn GetWindowTextW(window: Handle, text: *mut u16, len: i32) -> i32;
        fn PostMessageW(window: Handle, message: u32, wparam: usize, lparam: isize) -> i32;
    }
    pub struct Job(Handle);
    impl Job {
        pub fn attach(child: &Child) -> Result<Self> {
            // SAFETY: null name/attributes select an unnamed private job; repr(C) structures match Win64 Win32 ABI.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(std::io::Error::last_os_error().into());
                }
                let job = Self(handle);
                let mut limits: Extended = std::mem::zeroed();
                limits.limits.flags = 0x2000;
                if SetInformationJobObject(
                    handle,
                    9,
                    &limits as *const _ as *const c_void,
                    std::mem::size_of::<Extended>() as u32,
                ) == 0
                    || AssignProcessToJobObject(handle, child.as_raw_handle()) == 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                Ok(job)
            }
        }
        pub fn empty(&self) -> Result<bool> {
            // SAFETY: job handle remains live; query writes only this initialized ABI-compatible output buffer.
            unsafe {
                let mut info: Accounting = std::mem::zeroed();
                if QueryInformationJobObject(
                    self.0,
                    1,
                    &mut info as *mut _ as *mut c_void,
                    std::mem::size_of::<Accounting>() as u32,
                    std::ptr::null_mut(),
                ) == 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                Ok(info.active == 0)
            }
        }
        pub fn terminate(&self) {
            // SAFETY: this unnamed job contains only the process attached by this smoke owner and its descendants.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }
    impl Drop for Job {
        fn drop(&mut self) {
            self.terminate();
            // SAFETY: Job exclusively owns this live handle and closes it exactly once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    struct Search {
        pid: u32,
        window: Option<Handle>,
    }
    unsafe extern "system" fn visit(window: Handle, data: isize) -> i32 {
        // SAFETY: EnumWindows is synchronous; data points to Search for the whole enumeration and Win32 writes bounded stack buffers.
        unsafe {
            let search = &mut *(data as *mut Search);
            let mut pid = 0;
            GetWindowThreadProcessId(window, &mut pid);
            if pid == search.pid && IsWindowVisible(window) != 0 {
                let mut text = [0u16; 256];
                let length = GetWindowTextW(window, text.as_mut_ptr(), text.len() as i32);
                if length > 0 && String::from_utf16_lossy(&text[..length as usize]) == TITLE {
                    search.window = Some(window);
                }
            }
        }
        1
    }
    pub fn window(pid: u32) -> Result<Option<String>> {
        let mut search = Search { pid, window: None };
        // SAFETY: synchronous callback receives a unique mutable stack Search; no callback reference escapes.
        if unsafe { EnumWindows(visit, &mut search as *mut _ as isize) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(search.window.map(|window| (window as usize).to_string()))
    }
    pub fn close(pid: u32, window: &str) -> Result<()> {
        let value: usize = window.parse()?;
        let window = value as Handle;
        // SAFETY: verify owner PID immediately before posting pointer-free WM_CLOSE to this exact owned handle, never broadcast.
        unsafe {
            let mut owner = 0;
            GetWindowThreadProcessId(window, &mut owner);
            if owner != pid {
                return Err("Window owner changed before close".into());
            }
            if PostMessageW(window, 0x0010, 0, 0) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_progress_budget_fails_instead_of_fabricating_ready() {
        let mut budget = Budget::new();
        budget.last_progress = Instant::now() - IDLE;
        assert!(budget.observe(String::new()).is_err());
        let mut budget = Budget::new();
        budget.started = Instant::now() - WALL;
        assert!(budget.observe("real-progress".into()).is_err());
    }
}
