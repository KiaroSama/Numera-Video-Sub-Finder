use super::{Log, POLL, Result, inside, ownership::Owned, unlinked};
use crate::ci_tasks::sha256_hex;
use std::{
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

struct Assembly {
    source: String,
    runtime: String,
    binary: PathBuf,
    digest: String,
}

static ASSEMBLY: Mutex<Option<Assembly>> = Mutex::new(None);

fn hash(path: &Path) -> Result<String> {
    Ok(sha256_hex(path)?.1.to_ascii_uppercase())
}

pub(super) fn assembly_identity() -> Result<(PathBuf, String)> {
    let cached = ASSEMBLY
        .lock()
        .map_err(|_| "Observer assembly lock poisoned.")?;
    let cached = cached.as_ref().ok_or("Missing observer assembly.")?;
    Ok((cached.binary.clone(), hash(&cached.binary)?))
}

fn assembly(root: &Path, state: &Path, log: &mut Log) -> Result<PathBuf> {
    let source = root.join("scripts/ci/build_log_observer.rs");
    let source_hash = hash(&source)?;
    let rustc = Command::new("rustc").arg("--version").output()?;
    let version = std::str::from_utf8(&rustc.stdout)?.trim();
    if !rustc.status.success() || version.is_empty() {
        return Err("rustc unavailable for observer compilation.".into());
    }
    let runtime = format!("{version}|{}", std::env::consts::ARCH);
    let mut cached = ASSEMBLY
        .lock()
        .map_err(|_| "Observer assembly lock poisoned.")?;
    if let Some(record) = cached.as_ref() {
        let parent = record
            .binary
            .parent()
            .ok_or("Missing observer binary directory.")?;
        if record.source != source_hash
            || record.runtime != runtime
            || parent.parent() != Some(state)
            || !unlinked(parent)?.is_dir()
            || !unlinked(&record.binary)?.is_file()
            || hash(&record.binary)? != record.digest
        {
            return Err(
                "Cached observer binary identity changed; start a fresh guard session.".into(),
            );
        }
        return Ok(record.binary.clone());
    }
    let directory = tempfile::Builder::new()
        .prefix("observer-assembly-")
        .tempdir_in(state)?;
    let binary = directory.path().join(format!(
        "build-log-observer{}",
        std::env::consts::EXE_SUFFIX
    ));
    let started = Instant::now();
    let mut compiler = Command::new("rustc");
    compiler
        .args([
            "--edition",
            "2024",
            "-O",
            "--crate-name",
            "build_log_observer",
            "-o",
        ])
        .arg(&binary)
        .arg(&source)
        .current_dir(root);
    let mut compiler = Owned::spawn(compiler)?;
    // rustc emits nothing on success; drain both pipes so a diagnostic cannot block its exit.
    let pipes = super::Pipes::new(&mut compiler.child)?;
    compiler.child.stdin.take();
    let status = loop {
        for event in pipes.streams.iter().flatten() {
            for _ in 0..8 {
                if event.try_recv().is_err() {
                    break;
                }
            }
        }
        if let Some(status) = compiler.child.try_wait()? {
            break status;
        }
        if started.elapsed() >= Duration::from_secs(120) {
            compiler.cleanup(Duration::from_secs(10))?;
            return Err("Observer Rust compilation exceeded its deadline.".into());
        }
        thread::sleep(POLL);
    };
    compiler.cleanup(Duration::from_secs(10))?;
    if !status.success() || !unlinked(&binary)?.is_file() {
        return Err("Observer Rust compilation failed.".into());
    }
    if hash(&source)? != source_hash {
        return Err("Observer source changed during compilation.".into());
    }
    let digest = hash(&binary)?;
    log.write(
        "INFO",
        &format!(
            "Observer assembly initialized elapsed_ms={} source_sha256={source_hash}",
            started.elapsed().as_millis()
        ),
    )?;
    directory.keep();
    *cached = Some(Assembly {
        source: source_hash,
        runtime,
        binary: binary.clone(),
        digest,
    });
    Ok(binary)
}

pub(super) struct Observer {
    process: Owned,
    pub(super) pid: u32,
    events: mpsc::Receiver<io::Result<String>>,
    diagnostics: mpsc::Receiver<io::Result<Vec<u8>>>,
    baseline: bool,
    ready: bool,
    phase: String,
}

impl Observer {
    pub(super) fn prepare(
        root: &Path,
        state: &Path,
        log: &mut Log,
        directory: &Path,
        name: &str,
        stall: bool,
    ) -> Result<(PathBuf, PathBuf)> {
        let directory = inside(
            root,
            &root.join(directory),
            "Build progress directory must remain inside this project.",
        )?;
        if stall
            && (!name.starts_with("contract-")
                || !directory
                    .strip_prefix(state)
                    .ok()
                    .and_then(|p| p.components().next())
                    .is_some_and(|c| {
                        c.as_os_str()
                            .to_string_lossy()
                            .starts_with("guard-contract-")
                    }))
        {
            return Err("Observer fault injection requires an owned hosted contract.".into());
        }
        Ok((assembly(root, state, log)?, directory))
    }

    pub(super) fn spawn(root: &Path, binary: &Path, directory: &Path, stall: bool) -> Result<Self> {
        let mut command = Command::new(binary);
        command
            .arg("--directory")
            .arg(directory)
            .arg("--checkout")
            .arg(root)
            .args(["--contract", if stall { "stall" } else { "none" }])
            .current_dir(root);
        let mut process = Owned::spawn(command)?;
        let pid = process.child.id();
        let stdout = process
            .child
            .stdout
            .take()
            .ok_or("Missing observer control pipe.")?;
        let stderr = process
            .child
            .stderr
            .take()
            .ok_or("Missing observer diagnostic pipe.")?;
        let (sender, events) = mpsc::sync_channel(8);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                use std::io::Read;
                let mut bytes = Vec::new();
                let event = reader
                    .by_ref()
                    .take(128)
                    .read_until(b'\n', &mut bytes)
                    .and_then(|n| {
                        if n == 0 || !bytes.ends_with(b"\n") {
                            return Err(io::Error::other("Invalid observer control frame."));
                        }
                        bytes.pop();
                        String::from_utf8(bytes).map_err(io::Error::other)
                    });
                let done = event.is_err();
                if sender.send(event).is_err() || done {
                    break;
                }
            }
        });
        Ok(Self {
            process,
            pid,
            events,
            diagnostics: super::read_pipe(stderr),
            ready: false,
            baseline: false,
            phase: "starting".to_owned(),
        })
    }

    pub(super) fn start(&mut self, log: &mut Log, budget: Duration, stall: bool) -> Result<()> {
        let startup = Instant::now();
        loop {
            match self.events.try_recv() {
                Ok(Ok(line)) if line == "READY" => break,
                Err(mpsc::TryRecvError::Empty)
                    if startup.elapsed() < budget && self.process.child.try_wait()?.is_none() =>
                {
                    thread::sleep(POLL)
                }
                Err(mpsc::TryRecvError::Empty) => {
                    return Err("Observer ownership readiness failed.".into());
                }
                _ => return Err("Invalid observer ownership readiness.".into()),
            }
        }
        self.phase = format!("ready_ms={}", startup.elapsed().as_millis());
        log.write(
            "INFO",
            &format!("Owned build log observer pid={} {}", self.pid, self.phase),
        )?;
        let mut control = self
            .process
            .child
            .stdin
            .take()
            .ok_or("Missing observer input pipe.")?;
        control.write_all(b"GO\n")?;
        drop(control);
        self.ready = true;
        while !stall && !self.baseline {
            self.poll(log)?;
            if startup.elapsed() >= budget {
                return Err(format!(
                    "Observer baseline exceeded startup deadline elapsed_ms={} last_phase={}.",
                    startup.elapsed().as_millis(),
                    self.phase
                )
                .into());
            }
            if !self.baseline {
                thread::sleep(POLL);
            }
        }
        Ok(())
    }

    pub(super) fn poll(&mut self, log: &mut Log) -> Result<bool> {
        if self.diagnostics.try_recv().is_ok() {
            return Err("Build log observer emitted an unexpected diagnostic.".into());
        }
        let mut changed = false;
        for _ in 0..8 {
            let event = match self.events.try_recv() {
                Ok(Ok(line)) => line,
                Err(mpsc::TryRecvError::Empty) => break,
                _ => return Err("Build log observer failed or its control pipe closed.".into()),
            };
            let phase = event.strip_prefix("PHASE ").filter(|p| {
                p.split_once('=').is_some_and(|(k, v)| {
                    matches!(k, "loaded_ms" | "baseline_ms")
                        && (1..=10).contains(&v.len())
                        && v.bytes().all(|b| b.is_ascii_digit())
                })
            });
            match (event.as_str(), self.baseline, phase) {
                (_, false, Some(phase)) => {
                    self.phase = phase.to_owned();
                    log.write("INFO", &format!("Observer phase {phase}"))?;
                }
                ("BASELINE", false, _) => self.baseline = true,
                ("CHANGE", true, _) => changed = true,
                _ => return Err("Build log observer failed or its control pipe closed.".into()),
            }
        }
        if self.process.child.try_wait()?.is_some() {
            return Err("Build log observer exited before its owner.".into());
        }
        Ok(changed)
    }

    pub(super) fn stop(&mut self, log: &mut Log) -> Result<()> {
        let status = if self.ready {
            self.poll(log).map(|_| ())
        } else {
            Ok(())
        };
        self.process.cleanup(Duration::from_secs(3))?;
        log.write(
            "INFO",
            &format!("Observer cleanup verified pid={}", self.pid),
        )?;
        status
    }
}
