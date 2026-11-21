//! Hosted CI build-log progress observer. All filesystem work runs inside this
//! separately owned child process while the parent guard reads bounded control lines.

#![deny(unsafe_op_in_unsafe_fn)]

use std::{
    collections::HashMap,
    fs::{Metadata, ReadDir},
    io::{BufRead, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, UNIX_EPOCH},
};

#[cfg(windows)]
fn is_reparse_or_symlink(meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_type().is_symlink() || (meta.file_attributes() & 0x0400) != 0
}

#[cfg(not(windows))]
fn is_reparse_or_symlink(meta: &Metadata) -> bool {
    meta.file_type().is_symlink()
}

fn select_dir(dir: &Path) -> Result<ReadDir, ()> {
    let meta = std::fs::symlink_metadata(dir).map_err(|_| ())?;
    if is_reparse_or_symlink(&meta) || !meta.is_dir() {
        return Err(());
    }
    std::fs::read_dir(dir).map_err(|_| ())
}

struct FileProgress {
    root: PathBuf,
    clock: Instant,
    previous: HashMap<PathBuf, (u64, u128)>,
    current: HashMap<PathBuf, (u64, u128)>,
    ports: Option<ReadDir>,
    files: Option<ReadDir>,
    baseline: bool,
    directories: usize,
    last_sample_ms: f64,
}

impl FileProgress {
    fn new(root: PathBuf, checkout: &Path) -> Result<Self, ()> {
        let mut cursor: Option<&Path> = Some(root.as_path());
        while let Some(c) = cursor {
            if c == checkout
                || (cfg!(windows)
                    && c.to_string_lossy()
                        .eq_ignore_ascii_case(&checkout.to_string_lossy()))
            {
                cursor = Some(checkout);
                break;
            }
            if let Ok(meta) = std::fs::symlink_metadata(c)
                && is_reparse_or_symlink(&meta)
            {
                return Err(());
            }
            cursor = c.parent();
        }
        if cursor != Some(checkout) {
            return Err(());
        }
        std::fs::create_dir_all(&root).map_err(|_| ())?;
        let mut me = Self {
            root,
            clock: Instant::now(),
            previous: HashMap::new(),
            current: HashMap::new(),
            ports: None,
            files: None,
            baseline: true,
            directories: 0,
            last_sample_ms: -250.0,
        };
        me.begin_cycle()?;
        Ok(me)
    }

    fn begin_cycle(&mut self) -> Result<(), ()> {
        self.directories = 1;
        self.files = Some(select_dir(&self.root)?);
        self.ports = Some(select_dir(&self.root)?);
        Ok(())
    }

    fn initialized(&self) -> bool {
        !self.baseline
    }

    fn consume(&mut self) -> Result<bool, ()> {
        let now_ms = self.clock.elapsed().as_secs_f64() * 1000.0;
        if !self.baseline && now_ms - self.last_sample_ms < 250.0 {
            return Ok(false);
        }
        let page = Instant::now();
        let mut changed = false;
        for _ in 0..128 {
            if page.elapsed() >= Duration::from_millis(100) {
                break;
            }
            if let Some(rd) = self.files.as_mut() {
                match rd.next() {
                    None => {
                        self.files = None;
                        continue;
                    }
                    Some(Err(_)) => return Err(()),
                    Some(Ok(entry)) => {
                        let path = entry.path();
                        let meta = std::fs::symlink_metadata(&path).map_err(|_| ())?;
                        if is_reparse_or_symlink(&meta) {
                            return Err(());
                        }
                        if meta.is_dir() {
                            continue;
                        }
                        let is_log = entry.file_name().to_str().is_some_and(|n| {
                            n.len() >= 4 && n[n.len() - 4..].eq_ignore_ascii_case(".log")
                        });
                        if !is_log {
                            continue;
                        }
                        if !meta.is_file() || self.current.len() >= 8192 {
                            return Err(());
                        }
                        let modified = meta
                            .modified()
                            .map_err(|_| ())?
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos();
                        let value = (meta.len(), modified);
                        if !self.baseline {
                            changed |= match self.previous.get(&path) {
                                Some(&old) => old != value,
                                None => value.0 > 0,
                            };
                        }
                        self.current.insert(path, value);
                    }
                }
            } else if let Some(next_port) = self.ports.as_mut().and_then(Iterator::next) {
                let entry = next_port.map_err(|_| ())?;
                let path = entry.path();
                let meta = std::fs::symlink_metadata(&path).map_err(|_| ())?;
                if is_reparse_or_symlink(&meta) {
                    return Err(());
                }
                if !meta.is_dir() {
                    continue;
                }
                self.directories += 1;
                if self.directories > 513 {
                    return Err(());
                }
                self.files = Some(select_dir(&path)?);
            } else {
                self.ports = None;
                self.previous = std::mem::take(&mut self.current);
                self.baseline = false;
                self.begin_cycle()?;
                break;
            }
        }
        self.last_sample_ms = self.clock.elapsed().as_secs_f64() * 1000.0;
        Ok(changed)
    }
}

fn emit(line: &str) -> Result<(), ()> {
    let mut out = std::io::stdout().lock();
    out.write_all(line.as_bytes()).map_err(|_| ())?;
    out.write_all(b"\n").map_err(|_| ())?;
    out.flush().map_err(|_| ())
}

fn parse_args() -> Result<(PathBuf, PathBuf, String), ()> {
    let mut args = std::env::args().skip(1);
    let mut dir = None;
    let mut checkout = None;
    let mut contract = String::from("none");
    while let Some(flag) = args.next() {
        let val = args.next().ok_or(())?;
        match flag.as_str() {
            "--directory" => dir = Some(PathBuf::from(val)),
            "--checkout" => checkout = Some(PathBuf::from(val)),
            "--contract" => contract = val,
            _ => return Err(()),
        }
    }
    let dir = dir.ok_or(())?;
    let checkout = checkout.ok_or(())?;
    if contract != "none" && contract != "stall" {
        return Err(());
    }
    Ok((dir, checkout, contract))
}

fn run_loop(dir: PathBuf, checkout: PathBuf, contract: String) -> Result<(), ()> {
    if contract != "none" {
        let prefix = checkout.join(".ci-work").join("guard-contract-");
        let prefix_str = prefix.to_str().ok_or(())?;
        let dir_str = dir.to_str().ok_or(())?;
        let ok = if cfg!(windows) {
            dir_str.len() >= prefix_str.len()
                && dir_str[..prefix_str.len()].eq_ignore_ascii_case(prefix_str)
        } else {
            dir_str.starts_with(prefix_str)
        };
        if !ok {
            return Err(());
        }
    }
    let phase = Instant::now();
    emit(&format!("PHASE loaded_ms={}", phase.elapsed().as_millis()))?;
    if contract == "stall" {
        std::thread::sleep(Duration::from_secs(60));
        std::process::exit(3);
    }
    let mut sampler = FileProgress::new(dir, &checkout)?;
    let mut initialized = false;
    loop {
        let changed = sampler.consume()?;
        if !initialized && sampler.initialized() {
            emit(&format!(
                "PHASE baseline_ms={}",
                phase.elapsed().as_millis()
            ))?;
            emit("BASELINE")?;
            initialized = true;
        }
        if changed {
            emit("CHANGE")?;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
    {
        let _ = emit("ERROR");
        std::process::exit(1);
    }
    let parsed = match parse_args() {
        Ok(v) => v,
        Err(()) => {
            let _ = emit("ERROR");
            std::process::exit(1);
        }
    };
    if emit("READY").is_err() {
        std::process::exit(1);
    }
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err()
        || line.trim_end_matches(['\r', '\n']) != "GO"
    {
        std::process::exit(2);
    }
    if run_loop(parsed.0, parsed.1, parsed.2).is_err() {
        let _ = emit("ERROR");
        std::process::exit(1);
    }
}
