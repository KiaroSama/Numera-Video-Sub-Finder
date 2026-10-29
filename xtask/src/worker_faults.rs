//! Hosted faults against the production supervisor, without loading media or model runtimes.
use nvsf_app::{Control, worker};
use nvsf_core::{ExtractConfig, Progress};
use serde_json::json;
use std::{
    error::Error,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const ROOT: &str = "NVSF_WORKER_FAULT_ROOT";
const ADDRESS: &str = "NVSF_WORKER_FAULT_ADDRESS";
const TOKEN: &str = "NVSF_WORKER_FAULT_TOKEN";
const LIMIT: Duration = Duration::from_secs(8);

fn hosted() -> Result<()> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
    {
        return Err("Worker faults require GitHub-hosted Actions".into());
    }
    Ok(())
}

fn connection(role: u8) -> Result<TcpStream> {
    let address = std::env::var(ADDRESS)?.parse::<std::net::SocketAddr>()?;
    if !address.ip().is_loopback() {
        return Err("Worker fault gate must be loopback".into());
    }
    let mut stream = TcpStream::connect_timeout(&address, LIMIT)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(LIMIT))?;
    stream.write_all(&[role])?;
    stream.write_all(&std::process::id().to_le_bytes())?;
    stream.write_all(&chrono::Utc::now().timestamp_millis().to_le_bytes())?;
    stream.write_all(std::env::var(TOKEN)?.as_bytes())?;
    let mut admitted = [0];
    stream.read_exact(&mut admitted)?;
    if admitted != *b"A" {
        return Err("Worker fault identity was not admitted".into());
    }
    Ok(stream)
}

/// Called before ordinary argument parsing; never accepts an unowned external executable.
pub fn dispatch_fixture() -> Result<bool> {
    let Some(root) = std::env::var_os(ROOT) else {
        return Ok(false);
    };
    hosted()?;
    // A libtest executable cannot stand in for the fixture's clean worker protocol.
    if cfg!(test) {
        return Err("Worker fault dispatch requires the built xtask executable".into());
    }
    let root = PathBuf::from(root).canonicalize()?;
    if std::env::current_exe()?.canonicalize()?.parent() != Some(root.as_path()) {
        return Err("Worker fault fixture escaped its owned directory".into());
    }
    match std::env::args().nth(1).as_deref() {
        Some("--nvsf-worker-fault-owner") => {
            let mut owner_gate = connection(b'O')?;
            let mut stall_failed = false;
            let control = Control::default();
            let cancelled = control.cancel.clone();
            let config = ExtractConfig {
                input: root.join("input.mp4").to_string_lossy().into_owned(),
                output: root.join("output.srt").to_string_lossy().into_owned(),
                data_root: Some(root.join("data").to_string_lossy().into_owned()),
                device: "cpu".into(),
                ..Default::default()
            };
            let outcome = worker::supervise(config, &control, |progress| {
                if progress.stage == "fault-cancel" {
                    cancelled.store(true, Ordering::Relaxed);
                }
                if progress.stage == "fault-owner-stall" {
                    let mut resume = [0];
                    stall_failed = owner_gate.write_all(b"B").is_err()
                        || owner_gate.read_exact(&mut resume).is_err();
                }
            });
            if stall_failed {
                return Err("Fault owner stall gate failed before forced termination".into());
            }
            let text = outcome
                .err()
                .ok_or("Fault unexpectedly completed a project")?
                .to_string();
            std::fs::write(root.join("result.txt"), text)?;
        }
        Some("worker") => {
            worker::guard_parent_lifetime()?;
            let mut gate = connection(b'W')?;
            let mut command = Command::new(std::env::current_exe()?);
            command
                .arg("--nvsf-worker-fault-descendant")
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::null());
            hidden(&mut command);
            let mut descendant = Owned(command.spawn()?);
            let mut action = [0];
            gate.read_exact(&mut action)?;
            match action[0] {
                b'X' => std::process::exit(0),
                b'M' => {
                    let mut stdout = std::io::stdout().lock();
                    stdout.write_all(b"not-json\n")?;
                    stdout.flush()?;
                }
                b'C' | b'B' => {
                    let event = worker::Event::Progress(Progress {
                        stage: if action[0] == b'C' {
                            "fault-cancel"
                        } else {
                            "fault-owner-stall"
                        }
                        .into(),
                        frames: 0,
                        position_ms: 0,
                        duration_ms: None,
                        cues: 0,
                        provider: "fixture".into(),
                        message: String::new(),
                    });
                    let mut stdout = std::io::stdout().lock();
                    let args: Vec<_> = std::env::args_os().collect();
                    let control = args
                        .windows(2)
                        .find(|pair| pair[0] == "--control")
                        .map(|pair| Path::new(&pair[1]))
                        .ok_or("Missing fixture worker control directory")?;
                    let job_id = worker::protocol::job_id(control)?;
                    stdout.write_all(&worker::protocol::encode(
                        job_id,
                        &event,
                        worker::protocol::EVENT_LIMIT - 1,
                    )?)?;
                    stdout.write_all(b"\n")?;
                    stdout.flush()?;
                }
                _ => return Err("Invalid worker fault action".into()),
            }
            // Ignore cooperative flags exactly as a blocked native call would.
            gate.read_exact(&mut action)?;
            if action == *b"X" {
                std::process::exit(0);
            }
            descendant.stop()?;
            return Err("Fault worker survived its expected termination".into());
        }
        Some("--nvsf-worker-fault-descendant") => {
            let mut gate = connection(b'D')?;
            let mut byte = [0];
            gate.read_exact(&mut byte)?;
            return Err("Fault descendant survived its expected termination".into());
        }
        _ => return Err("Unexpected worker fault fixture invocation".into()),
    }
    Ok(true)
}

fn hidden(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    #[cfg(not(windows))]
    let _ = command;
}

struct Owned(Child);
impl Owned {
    fn wait(&mut self, deadline: Instant) -> Result<ExitStatus> {
        loop {
            if let Some(status) = self.0.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err("Worker fault owner exceeded deadline".into());
            }
            std::thread::park_timeout(Duration::from_millis(5));
        }
    }
    fn stop(&mut self) -> Result<()> {
        if self.0.try_wait()?.is_none() {
            self.0.kill()?;
        }
        self.wait(Instant::now() + Duration::from_secs(3))?;
        Ok(())
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            eprintln!("[ERROR] [WORKER-FAULT] Owner cleanup failed: {error}");
        }
    }
}

struct Actor {
    pid: u32,
    ready_utc_ms: i64,
    gate: TcpStream,
    #[cfg(windows)]
    process: windows::Process,
}
impl Actor {
    fn alive(&self) -> Result<bool> {
        #[cfg(windows)]
        return self.process.alive();
        #[cfg(unix)]
        {
            // SAFETY: signal0 only observes the exact handshake PID, never sends a signal.
            if unsafe { kill(self.pid as i32, 0) } == 0 {
                return Ok(true);
            }
            match std::io::Error::last_os_error().raw_os_error() {
                Some(3) => Ok(false), // ESRCH on Linux and macOS.
                _ => Err(std::io::Error::last_os_error().into()),
            }
        }
    }
    fn connected(&self) -> Result<bool> {
        let mut byte = [0];
        match self.gate.peek(&mut byte) {
            Ok(0) => Ok(false),
            Ok(_) => Err("Unexpected bytes in worker fault identity gate".into()),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
    fn stop(&self) -> Result<()> {
        #[cfg(windows)]
        return self.process.stop();
        #[cfg(unix)]
        {
            // A held fixture connection plus matching group prevents signaling a recycled PID.
            if self.connected()? && self.alive()? {
                // SAFETY: this is our connected, still-live fixture identity.
                if unsafe { kill(self.pid as i32, 9) } != 0
                    && std::io::Error::last_os_error().raw_os_error() != Some(3)
                {
                    return Err(std::io::Error::last_os_error().into());
                }
            }
            Ok(())
        }
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
    fn getpgid(pid: i32) -> i32;
}

struct Tree {
    owner: Owned,
    actors: Vec<(u8, Actor)>,
}
impl Tree {
    fn actor(&self, role: u8) -> Result<&Actor> {
        self.actors
            .iter()
            .find(|(r, _)| *r == role)
            .map(|(_, a)| a)
            .ok_or_else(|| "Worker fault readiness identity missing".into())
    }
    fn sweep(&mut self) -> Result<()> {
        #[cfg(unix)]
        if let Ok(worker) = self.actor(b'W')
            && worker.connected()? && worker.alive()?
            // SAFETY: connected fixture PID is still the live leader of this exact group.
            && unsafe { getpgid(worker.pid as i32) } == worker.pid as i32
        {
            // SAFETY: catches even a descendant whose readiness message was not accepted yet.
            if unsafe { kill(-(worker.pid as i32), 9) } != 0
                && std::io::Error::last_os_error().raw_os_error() != Some(3)
            {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        // Parent death closes the real Windows job, including pre-handshake descendants.
        self.owner.stop()?;
        for (role, actor) in self.actors.iter().rev() {
            if *role != b'O' {
                actor.stop()?;
            }
        }
        self.absent(Instant::now() + Duration::from_secs(3))
    }
    fn absent(&self, deadline: Instant) -> Result<()> {
        loop {
            #[cfg(target_os = "linux")]
            if let Ok(worker) = self.actor(b'W') {
                crate::production_supervisor::reap_group(worker.pid)?;
            }
            let mut alive = false;
            for (role, actor) in &self.actors {
                if *role != b'O' {
                    alive |= actor.alive()? || actor.connected()?;
                }
            }
            if !alive {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("Worker fault left a live or unreaped process identity".into());
            }
            std::thread::park_timeout(Duration::from_millis(5));
        }
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        if let Err(error) = self.sweep() {
            eprintln!("[ERROR] [WORKER-FAULT] Tree cleanup failed: {error}");
        }
    }
}

fn ready(listener: &TcpListener, tree: &mut Tree, token: &[u8]) -> Result<()> {
    let deadline = Instant::now() + LIMIT;
    while tree.actors.len() < 3 {
        if tree.owner.0.try_wait()?.is_some() {
            return Err("Worker fault owner exited before readiness".into());
        }
        match listener.accept() {
            Ok((mut gate, address)) => {
                if !address.ip().is_loopback() {
                    return Err("Unexpected nonlocal worker fault connection".into());
                }
                gate.set_nonblocking(true)?;
                let mut identity = vec![0; 13 + token.len()];
                let mut filled = 0;
                while filled < identity.len() {
                    if Instant::now() >= deadline {
                        return Err("Worker fault identity exceeded readiness deadline".into());
                    }
                    match gate.read(&mut identity[filled..]) {
                        Ok(0) => return Err("Worker fault identity ended before admission".into()),
                        Ok(count) => filled += count,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::park_timeout(Duration::from_millis(5));
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                let role = identity[0];
                let pid = u32::from_le_bytes(identity[1..5].try_into()?);
                let ready_utc_ms = i64::from_le_bytes(identity[5..13].try_into()?);
                if !b"OWD".contains(&role)
                    || identity[13..] != *token
                    || ready_utc_ms <= 0
                    || pid <= 1
                    || pid > i32::MAX as u32
                    || tree.actors.iter().any(|(r, a)| *r == role || a.pid == pid)
                    || (role == b'O' && pid != tree.owner.0.id())
                {
                    return Err("Invalid or duplicated worker fault identity".into());
                }
                let mut actor = Actor {
                    pid,
                    ready_utc_ms,
                    gate,
                    #[cfg(windows)]
                    process: windows::Process::open(pid)?,
                };
                actor.gate.set_nonblocking(false)?;
                actor.gate.set_write_timeout(Some(LIMIT))?;
                actor.gate.write_all(b"A")?;
                actor.gate.set_nonblocking(true)?;
                tree.actors.push((role, actor));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("Worker fault readiness exceeded8s".into());
                }
                std::thread::park_timeout(Duration::from_millis(5));
            }
            Err(error) => return Err(error.into()),
        }
    }
    #[cfg(unix)]
    {
        let worker = tree.actor(b'W')?;
        let descendant = tree.actor(b'D')?;
        // SAFETY: both PIDs were received over their still-open fixture identity connections.
        if unsafe { getpgid(worker.pid as i32) } != worker.pid as i32
            || unsafe { getpgid(descendant.pid as i32) } != worker.pid as i32
        {
            return Err("Worker fault descendant is outside the production-owned group".into());
        }
    }
    for (_, actor) in &tree.actors {
        if !actor.connected()? || !actor.alive()? {
            return Err("Worker fault was not live at admission".into());
        }
    }
    Ok(())
}

/// Serial cases reuse one built executable and do not require inference/media resources.
pub fn verify(output: &Path) -> Result<()> {
    hosted()?;
    let source = std::env::var("GITHUB_SHA")?;
    let target = std::env::var("NVSF_TARGET_ID")?;
    let expected_target = format!(
        "{}-{}",
        std::env::consts::OS,
        match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            _ => return Err("Unsupported worker fault architecture".into()),
        }
    );
    if source.len() != 40
        || !source.bytes().all(|byte| byte.is_ascii_hexdigit())
        || nvsf_core::BUILD_SOURCE_SHA != Some(source.as_str())
        || target != expected_target
    {
        return Err("Worker fault source/target identity mismatch".into());
    }
    std::fs::create_dir_all(output)?;
    #[cfg(target_os = "linux")]
    let _adoption = crate::production_supervisor::Adoption::begin()?;
    let scratch = tempfile::Builder::new()
        .prefix("worker fault 日本 ")
        .tempdir_in(output)?;
    let root = scratch.path().canonicalize()?;
    let exe = root.join(if cfg!(windows) { "nvsf.exe" } else { "nvsf" });
    std::fs::copy(std::env::current_exe()?, &exe)?;
    let mut evidence = Vec::new();
    for (name, action, expected) in [
        ("supervisor-parent-death", None, ""),
        ("root-exit-then-supervisor-death", Some(b'B'), ""),
        (
            "inherited-output-root-exit",
            Some(b'X'),
            "Worker output remained open",
        ),
        (
            "malformed-output-live-tree",
            Some(b'M'),
            "Invalid project or metadata:",
        ),
        ("cancel-uncooperative-tree", Some(b'C'), "cancel"),
    ] {
        let result_path = root.join("result.txt");
        if result_path.exists() {
            std::fs::remove_file(&result_path)?;
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let token = format!("{}-{name}", std::process::id());
        let mut command = Command::new(&exe);
        command
            .arg("--nvsf-worker-fault-owner")
            .env(ROOT, &root)
            .env(ADDRESS, listener.local_addr()?.to_string())
            .env(TOKEN, &token)
            .env("NVSF_RUNTIME_ROOT", root.join("empty-runtimes"))
            .current_dir(&root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hidden(&mut command);
        let mut tree = Tree {
            owner: Owned(command.spawn()?),
            actors: Vec::new(),
        };
        let outcome = (|| -> Result<_> {
            ready(&listener, &mut tree, token.as_bytes())?;
            let started = Instant::now();
            let ids: Vec<_> = tree
                .actors
                .iter()
                .map(|(role, a)| json!({"role":char::from(*role),"pid":a.pid,"ready_utc_ms":a.ready_utc_ms,"identity_binding":if cfg!(windows) {"retained noninherited OS handle before admission"} else {"held ready connection and verified owned PGID"}}))
                .collect();
            if let Some(action) = action {
                let mut gate = &tree.actor(b'W')?.gate;
                gate.set_nonblocking(false)?;
                gate.set_write_timeout(Some(LIMIT))?;
                gate.write_all(&[action])?;
                gate.set_nonblocking(true)?;
                if action == b'B' {
                    let mut gate = &tree.actor(b'O')?.gate;
                    let mut blocked = [0];
                    loop {
                        if Instant::now() >= started + LIMIT {
                            return Err("Supervisor did not enter the combined-fault gate".into());
                        }
                        match gate.read(&mut blocked) {
                            Ok(1) if blocked == *b"B" => break,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                std::thread::park_timeout(Duration::from_millis(5));
                            }
                            _ => return Err("Supervisor combined-fault gate ended early".into()),
                        }
                    }
                    let mut gate = &tree.actor(b'W')?.gate;
                    gate.set_nonblocking(false)?;
                    gate.write_all(b"X")?;
                    gate.set_nonblocking(true)?;
                    while tree.actor(b'W')?.connected()? {
                        if Instant::now() >= started + LIMIT {
                            return Err("Root did not exit before supervisor death".into());
                        }
                        std::thread::park_timeout(Duration::from_millis(5));
                    }
                    if !tree.actor(b'D')?.connected()? || !tree.actor(b'D')?.alive()? {
                        return Err(
                            "Combined fault lost its live descendant before owner death".into()
                        );
                    }
                    tree.owner.0.kill()?;
                    tree.owner.wait(started + LIMIT)?;
                } else {
                    let status = tree.owner.wait(started + LIMIT)?;
                    if !status.success() {
                        return Err(format!("Worker fault owner failed: {status}").into());
                    }
                    let result = std::fs::read_to_string(root.join("result.txt"))?;
                    if !result.to_lowercase().contains(&expected.to_lowercase())
                        && !(action == b'X'
                            && result.contains("Worker ended without a complete project"))
                    {
                        return Err(format!("Worker fault returned wrong result: {result}").into());
                    }
                }
            } else {
                tree.owner.0.kill()?; // Deliberately bypass all Rust supervisor destructors.
                tree.owner.wait(started + LIMIT)?;
            }
            tree.absent(started + LIMIT)?;
            if root.join("output.srt").exists() {
                return Err("Worker fault unexpectedly published output".into());
            }
            Ok(
                json!({"case":name,"state":"PASS","elapsed_ms":started.elapsed().as_millis(),"identities":ids,"cleanup":"all recorded process identities absent"}),
            )
        })();
        let cleanup = tree.sweep();
        match (outcome, cleanup) {
            (Ok(record), Ok(())) => evidence.push(record),
            (Err(error), Ok(())) => return Err(format!("{name}: {error}; cleanup verified").into()),
            (outcome, Err(cleanup)) => {
                return Err(format!(
                    "{name}: fault={:?}; cleanup failed: {cleanup}",
                    outcome.err().map(|e| e.to_string())
                )
                .into());
            }
        }
    }
    scratch.close()?;
    nvsf_app::storage::write_atomic(
        &output.join("worker-fault-report.json"),
        &serde_json::to_vec_pretty(
            &json!({"schema":1,"source_sha":source,"target":target,"passed":true,"tests":5,"failed":0,"skipped":0,"cases":evidence,"worker_ceiling":2,"unix_guardian_helpers":if cfg!(unix) {1} else {0},"native_inference_hang_qualified":false}),
        )?,
        true,
    )?;
    Ok(())
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        fn TerminateProcess(handle: *mut c_void, code: u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    pub(super) struct Process(*mut c_void);
    impl Process {
        pub(super) fn open(pid: u32) -> Result<Self> {
            // SAFETY: noninherited handle binds this fixture's handshake PID for its lifetime.
            let handle = unsafe { OpenProcess(0x0010_0001, 0, pid) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(Self(handle))
        }
        pub(super) fn alive(&self) -> Result<bool> {
            // SAFETY: retained process handle; zero wait never blocks.
            match unsafe { WaitForSingleObject(self.0, 0) } {
                0 => Ok(false),
                258 => Ok(true),
                _ => Err(std::io::Error::last_os_error().into()),
            }
        }
        pub(super) fn stop(&self) -> Result<()> {
            // SAFETY: exact retained owned fixture process, not a reusable numeric PID.
            if self.alive()? && unsafe { TerminateProcess(self.0, 71) } == 0 && self.alive()? {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(())
        }
    }
    impl Drop for Process {
        fn drop(&mut self) {
            // SAFETY: close this sole owned observer handle; it does not kill unrelated processes.
            unsafe { CloseHandle(self.0) };
        }
    }
}
