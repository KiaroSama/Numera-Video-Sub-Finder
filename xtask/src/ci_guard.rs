#[path = "ci_guard_observer.rs"]
mod observer;
#[path = "ci_guard_process.rs"]
mod ownership;
#[path = "ci_guard_tests.rs"]
mod tests;

use chrono::Utc;
use observer::Observer;
use ownership::Owned;
use std::{
    env, fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    process::{Child, Command},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub(crate) type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const POLL: Duration = Duration::from_millis(20);
const MAX_CAPTURE: u64 = 8 * 1024 * 1024;
const MAX_LOG: u64 = 64 * 1024 * 1024;

pub(crate) struct Guard<'a> {
    pub name: &'a str,
    pub wall_s: u32,
    pub idle_s: u32,
    pub capture: Option<&'a Path>,
    pub progress: Option<&'a Path>,
    pub stall: bool,
}

pub(crate) struct Outcome {
    pub code: i32,
    pub log: PathBuf,
}

#[derive(Debug)]
pub(crate) struct Failure {
    pub code: i32,
    name: String,
    log: PathBuf,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Guarded command failed: name={} exit_code={} log={}",
            self.name,
            self.code,
            self.log.display()
        )
    }
}

impl std::error::Error for Failure {}

impl Outcome {
    pub(crate) fn require_success(self, name: &str) -> Result<()> {
        if self.code == 0 {
            return Ok(());
        }
        Err(Failure {
            code: self.code,
            name: name.to_owned(),
            log: self.log,
        }
        .into())
    }
}

pub(crate) fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(PathBuf::new, Path::to_path_buf)
}

pub(crate) fn hosted(message: &str) -> Result<()> {
    if env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
        && env::var("RUNNER_ENVIRONMENT").as_deref() == Ok("github-hosted")
    {
        Ok(())
    } else {
        Err(message.to_owned().into())
    }
}

pub(crate) fn unlinked(path: &Path) -> io::Result<fs::Metadata> {
    let info = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    let linked = {
        use std::os::windows::fs::MetadataExt;
        info.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let linked = info.is_symlink();
    if linked {
        return Err(io::Error::other(format!(
            "Path must not be linked: {}",
            path.display()
        )));
    }
    Ok(info)
}

pub(crate) fn state_dir(root: &Path) -> Result<PathBuf> {
    directory(root.join(".ci-work"))
}

fn directory(path: PathBuf) -> Result<PathBuf> {
    match fs::create_dir(&path) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error.into()),
    }
    if !unlinked(&path)?.is_dir() {
        return Err(format!("Expected regular directory: {}", path.display()).into());
    }
    Ok(path)
}

pub(crate) fn inside(root: &Path, path: &Path, error: &str) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        if component == Component::ParentDir {
            normalized.pop();
        } else {
            normalized.push(component);
        }
    }
    if normalized == root || !normalized.starts_with(root) {
        return Err(error.to_owned().into());
    }
    Ok(normalized)
}

fn capture_file(root: &Path, path: &Path) -> Result<fs::File> {
    let path = inside(
        root,
        path,
        "Output capture must remain inside this project.",
    )?;
    let relative = path.strip_prefix(root)?;
    let mut current = root.to_path_buf();
    for segment in relative.iter() {
        current.push(segment);
        match unlinked(&current) {
            Ok(metadata)
                if if current == path {
                    !metadata.is_file()
                } else {
                    !metadata.is_dir()
                } =>
            {
                return Err(
                    "Output capture path traverses a non-directory or names a non-file.".into(),
                );
            }
            Ok(_) => (),
            Err(error) if current == path && error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(fs::File::create(path)?)
}

struct Log {
    file: fs::File,
    path: PathBuf,
    size: u64,
    redactions: Vec<String>,
}

impl Log {
    fn new(state: &Path, name: &str, command: &Command) -> Result<Self> {
        let directory = directory(state.join("logs"))?;
        let stamp = Utc::now().format("%Y-%m-%d_%H-%M-%S");
        let base = format!("guard-{name}_{stamp}_UTC");
        let mut sequence = 0_u32;
        let (file, path) = loop {
            let path = if sequence == 0 {
                directory.join(format!("{base}.log"))
            } else {
                directory.join(format!("{base}_{}_{sequence}.log", std::process::id()))
            };
            match fs::File::create_new(&path) {
                Ok(file) => break (file, path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => sequence += 1,
                Err(error) => {
                    eprintln!("File logging initialization failed; using console diagnostics.");
                    return Err(error.into());
                }
            }
        };
        let mut redactions: Vec<_> =
            env::vars_os()
                .chain(command.get_envs().filter_map(|(key, value)| {
                    value.map(|value| (key.to_owned(), value.to_owned()))
                }))
                .filter_map(|(key, value)| {
                    let key = key.to_string_lossy().to_ascii_uppercase();
                    let value = value.into_string().ok()?;
                    (value.chars().count() >= 8
                        && [
                            "TOKEN",
                            "PASSWORD",
                            "SECRET",
                            "API_KEY",
                            "ACCESS_KEY",
                            "CONNECTION_STRING",
                            "CODESIGN_PFX",
                        ]
                        .iter()
                        .any(|suffix| key.ends_with(suffix)))
                    .then_some(value)
                })
                .collect();
        redactions.sort_unstable_by_key(|value| std::cmp::Reverse(value.len()));
        redactions.dedup();
        Ok(Self {
            file,
            path,
            size: 0,
            redactions,
        })
    }

    fn redact(&self, text: &str) -> String {
        self.redactions
            .iter()
            .fold(text.to_owned(), |text, secret| {
                text.replace(secret, "[REDACTED]")
            })
    }

    fn write(&mut self, level: &str, text: &str) -> Result<()> {
        let text = redact_headers(&self.redact(text));
        let line = format!(
            "[{}] [{level}] [CI-GUARD] {text}",
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        );
        self.size += line.len() as u64 + 1;
        if self.size > MAX_LOG {
            return Err("Guard diagnostic log exceeded 64 MiB; command must stop.".into());
        }
        writeln!(self.file, "{line}")?;
        eprintln!("{line}");
        Ok(())
    }
}

fn redact_headers(text: &str) -> String {
    let lowercase = text.to_ascii_lowercase();
    let bytes = lowercase.as_bytes();
    let mut out = String::new();
    let mut last = 0;
    for (index, _) in text.char_indices() {
        if index < last {
            continue;
        }
        let remaining = &lowercase[index..];
        let mut start = if remaining.starts_with("authorization") {
            let mut start = index + "authorization".len();
            while bytes.get(start).is_some_and(u8::is_ascii_whitespace) {
                start += 1;
            }
            if !matches!(bytes.get(start), Some(b':' | b'=')) {
                continue;
            }
            start + 1
        } else if remaining.starts_with("bearer")
            && bytes.get(index + 6).is_some_and(u8::is_ascii_whitespace)
        {
            index + 6
        } else {
            continue;
        };
        while bytes.get(start).is_some_and(u8::is_ascii_whitespace) {
            start += 1;
        }
        let value = &lowercase[start..];
        if value.starts_with("bearer") && bytes.get(start + 6).is_some_and(u8::is_ascii_whitespace)
        {
            start += 6;
            while bytes.get(start).is_some_and(u8::is_ascii_whitespace) {
                start += 1;
            }
        }
        let end = text[start..]
            .find(char::is_whitespace)
            .map_or(text.len(), |offset| start + offset);
        if end > start {
            out.push_str(&text[last..start]);
            out.push_str("[REDACTED]");
            last = end;
        }
    }
    out.push_str(&text[last..]);
    out
}

fn read_pipe(mut reader: impl Read + Send + 'static) -> mpsc::Receiver<io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::sync_channel(8);
    thread::spawn(move || {
        let mut buf = [0_u8; 4096];
        loop {
            let item = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(count) => Ok(buf[..count].to_vec()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => Err(error),
            };
            let done = item.is_err();
            if sender.send(item).is_err() || done {
                break;
            }
        }
    });
    receiver
}

type Pipe = mpsc::Receiver<io::Result<Vec<u8>>>;

struct Pipes {
    streams: [Option<Pipe>; 2],
    pending: [Vec<u8>; 2],
    capture: Option<fs::File>,
    captured: u64,
}

impl Pipes {
    fn new(child: &mut Child) -> Result<Self> {
        let stdout = child.stdout.take().ok_or("Missing guarded stdout pipe.")?;
        let stderr = child.stderr.take().ok_or("Missing guarded stderr pipe.")?;
        Ok(Self {
            streams: [Some(read_pipe(stdout)), Some(read_pipe(stderr))],
            pending: [Vec::new(), Vec::new()],
            capture: None,
            captured: 0,
        })
    }

    fn emit(&mut self, stream: usize, bytes: &[u8], log: &mut Log) -> Result<()> {
        let text = String::from_utf8_lossy(bytes);
        if stream == 0
            && let Some(capture) = &mut self.capture
        {
            let value = log.redact(&text);
            self.captured += value.len() as u64;
            if self.captured > MAX_CAPTURE {
                return Err("Structured output capture exceeded 8 MiB.".into());
            }
            capture.write_all(value.as_bytes())?;
        }
        log.write(
            if stream == 0 { "INFO" } else { "WARNING" },
            text.trim_end(),
        )
    }

    fn poll(&mut self, log: &mut Log) -> Result<bool> {
        let mut progress = false;
        for stream in 0..2 {
            for _ in 0..8 {
                match self.streams[stream].as_ref().map(mpsc::Receiver::try_recv) {
                    Some(Ok(bytes)) => {
                        self.pending[stream].extend(bytes?);
                        progress = true;
                    }
                    Some(Err(mpsc::TryRecvError::Disconnected)) => {
                        self.streams[stream] = None;
                        break;
                    }
                    _ => break,
                }
            }
            if self.pending[stream].len() as u64
                > if stream == 0 && self.capture.is_some() {
                    MAX_CAPTURE
                } else {
                    MAX_LOG
                }
            {
                return Err("Guard output line exceeded its diagnostic or capture limit.".into());
            }
            let end = if self.streams[stream].is_none() {
                self.pending[stream].len()
            } else {
                self.pending[stream]
                    .iter()
                    .rposition(|byte| *byte == b'\n')
                    .map_or(0, |index| index + 1)
            };
            if end != 0 {
                // Complete lines preserve UTF-8 and prevent secrets split across pipe reads from leaking.
                let complete: Vec<_> = self.pending[stream].drain(..end).collect();
                self.emit(stream, &complete, log)?;
            }
        }
        Ok(progress)
    }

    fn closed(&self) -> bool {
        self.streams.iter().all(Option::is_none)
    }
}

fn supervise(
    options: &Guard<'_>,
    started: Instant,
    child: &mut Owned,
    pipes: &mut Pipes,
    observer: &mut Option<Observer>,
    log: &mut Log,
) -> Result<i32> {
    let wall = Duration::from_secs(u64::from(options.wall_s));
    let idle = Duration::from_secs(u64::from(options.idle_s));
    let mut last_progress = Instant::now();
    let mut last_notice = started;
    let mut exit = None;
    loop {
        if let Some(observer) = observer
            && observer.poll(log)?
        {
            last_progress = Instant::now();
            if last_notice.elapsed() >= Duration::from_secs(30) {
                log.write("INFO", "Owned native build log changed; file content remains in the build diagnostics.")?;
                last_notice = last_progress;
            }
        }
        if pipes.poll(log)? {
            last_progress = Instant::now();
        }
        if exit.is_none()
            && let Some(status) = child.child.try_wait()?
        {
            exit = Some((status.code().unwrap_or(1), Instant::now()));
        }
        if let Some((code, observed)) = exit {
            let deadline = (observed + Duration::from_secs(2))
                .min(started + wall)
                .min(last_progress + idle);
            let descendants = child.active(deadline)?;
            if descendants.is_empty() && pipes.closed() {
                return Ok(code);
            }
            if observed.elapsed() >= Duration::from_secs(2) {
                return Err(format!("Owned root exited without bounded stream/tree completion; rejecting leaked process tree. root_exit_code={code} live_tree={} stdout_pending={} stderr_pending={} live_processes={descendants:?}", descendants.len(), pipes.streams[0].is_some(), pipes.streams[1].is_some()).into());
            }
        }
        let reason = if started.elapsed() >= wall {
            "Wall timeout"
        } else if last_progress.elapsed() >= idle {
            "Idle timeout"
        } else {
            thread::sleep(POLL);
            continue;
        };
        log.write(
            "ERROR",
            &format!(
                "{reason}: Command exceeded wall/idle deadline: name={} pid={}.",
                options.name,
                child.child.id()
            ),
        )?;
        return Ok(124);
    }
}

pub(crate) fn run(options: Guard<'_>, command: Command) -> Result<Outcome> {
    if options.name.is_empty()
        || !options
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        || !(1..=10_800).contains(&options.wall_s)
        || !(1..=3_600).contains(&options.idle_s)
        || command.get_program().is_empty()
        || (options.stall && options.progress.is_none())
    {
        return Err(
            "Invalid guarded command name, executable, progress contract or deadline.".into(),
        );
    }
    hosted("Command execution is restricted to GitHub-hosted Actions runners.")?;
    let root = root();
    let state = state_dir(&root)?;
    let mut log = Log::new(&state, options.name, &command)?;
    let assembly = options
        .progress
        .map(|directory| {
            Observer::prepare(
                &root,
                &state,
                &mut log,
                directory,
                options.name,
                options.stall,
            )
        })
        .transpose()?;
    let capture = options
        .capture
        .map(|path| capture_file(&root, path))
        .transpose()?;
    let mut observer = None;
    let mut child = None;
    let mut pipes = None;
    let started = Instant::now();
    let result = (|| {
        if let Some((binary, directory)) = assembly {
            let monitor =
                observer.insert(Observer::spawn(&root, &binary, &directory, options.stall)?);
            monitor.start(
                &mut log,
                Duration::from_secs(u64::from(20.min(options.wall_s).min(options.idle_s))),
                options.stall,
            )?;
        }
        let cwd = command
            .get_current_dir()
            .map_or_else(env::current_dir, std::path::absolute)?;
        if !cwd.is_dir() {
            return Err("Working directory does not exist.".into());
        }
        let exe = Path::new(command.get_program())
            .file_name()
            .unwrap_or(command.get_program());
        log.write(
            "INFO",
            &format!(
                "Starting name={} executable={} cwd={} wall_s={} idle_s={} workers=2",
                options.name,
                exe.to_string_lossy(),
                cwd.display(),
                options.wall_s,
                options.idle_s
            ),
        )?;
        let owned = child.insert(Owned::spawn(command)?);
        log.write(
            "INFO",
            &format!(
                "Owned process pid={} start_utc={}",
                owned.child.id(),
                Utc::now().to_rfc3339()
            ),
        )?;
        owned.child.stdin.take();
        let streams = pipes.insert(Pipes::new(&mut owned.child)?);
        streams.capture = capture;
        supervise(&options, started, owned, streams, &mut observer, &mut log)
    })();
    let mut code = match result {
        Ok(code) => code,
        Err(error) => {
            let _ = log.write("ERROR", &error.to_string());
            1
        }
    };
    let marker = |code| {
        format!(
            "Command finished name={} exit_code={code} elapsed_s={:.2}",
            options.name,
            started.elapsed().as_secs_f64()
        )
    };
    if log
        .write(if code == 0 { "INFO" } else { "ERROR" }, &marker(code))
        .is_err()
    {
        code = 1;
    }
    if let Some(mut child) = child {
        match child.cleanup(Duration::from_secs(10)) {
            Ok(()) => {
                if log
                    .write(
                        "INFO",
                        &format!("Cleanup verified pid={}", child.child.id()),
                    )
                    .is_err()
                {
                    code = 1;
                }
                if let Some(pipes) = &mut pipes
                    && pipes.poll(&mut log).is_err()
                {
                    code = 1;
                }
            }
            Err(error) => {
                code = 1;
                eprintln!("Cleanup failure: {error}");
            }
        }
    }
    if let Some(mut observer) = observer
        && let Err(error) = observer.stop(&mut log)
    {
        code = 1;
        let _ = log.write("ERROR", &format!("Observer cleanup failed: {error}"));
    }
    Ok(Outcome {
        code,
        log: log.path,
    })
}

pub(crate) fn contracts() -> Result<()> {
    tests::contracts()
}

pub(crate) fn dispatch() -> Result<Option<i32>> {
    let mut args = env::args_os().skip(1);
    match args.next().as_deref().and_then(std::ffi::OsStr::to_str) {
        Some("__ci-guard-fixture") => {
            let case = args
                .next()
                .and_then(|s| s.into_string().ok())
                .ok_or("Missing guard contract fixture.")?;
            tests::fixture(&case).map(Some)
        }
        #[cfg(windows)]
        Some("__ci-guard-owned") => ownership::launch(args).map(Some),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod unit {
    use super::redact_headers;

    #[test]
    fn masks_headers_without_consuming_unrelated_fields() {
        assert_eq!(
            redact_headers("prefix AUTHORIZATION : Bearer abc user=public"),
            "prefix AUTHORIZATION : Bearer [REDACTED] user=public"
        );
        assert_eq!(
            redact_headers("bearer token authorization=hidden"),
            "bearer [REDACTED] authorization=[REDACTED]"
        );
    }
}
